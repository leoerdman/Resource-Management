//! macOS: `libproc`, `mach`, `sysctl` напрямую через `libc`.
//!
//! Чужие и root-процессы читаются частично: чего не дали права — `None` или пусто.
#![allow(deprecated)] // mach_host_self / mach_timebase_info помечены в libc как «берите mach2».

use std::collections::HashMap;
use std::ffi::CStr;
use std::mem::{MaybeUninit, size_of};

use crate::error::{Error, Result};
use crate::group::app_group;
use crate::model::{Pressure, ProcState, ProcessInfo, SystemMemory, VmCounters};
use crate::platform::{Platform, ProcessSample, Signal};

/// Платформа macOS.
pub struct MacOsPlatform {
    page_size: u64,
    total: u64,
    /// Множитель mach-тиков в наносекунды (на Apple Silicon 125/3).
    timebase: (u32, u32),
    host: libc::mach_port_t,
    /// Неизменное за жизнь процесса (имя, путь, аргументы, группа) по (pid, started_at):
    /// `KERN_PROCARGS2` и `proc_pidpath` на 800 процессах каждые 2 с — основная цена снимка.
    cache: HashMap<(i32, u64), Static>,
}

#[derive(Clone)]
struct Static {
    name: String,
    exe: Option<String>,
    args: Vec<String>,
    app_group: String,
}

impl MacOsPlatform {
    /// Читает постоянные величины машины.
    pub fn new() -> Result<Self> {
        let page_size = sysctl_u64("hw.pagesize")?;
        let total = sysctl_u64("hw.memsize")?;
        let mut tb = libc::mach_timebase_info { numer: 0, denom: 0 };
        // SAFETY: передаём указатель на живую структуру нужного типа.
        unsafe { libc::mach_timebase_info(&mut tb) };
        let timebase = if tb.denom == 0 { (1, 1) } else { (tb.numer, tb.denom) };
        // SAFETY: без аргументов, возвращает порт хоста текущей задачи.
        let host = unsafe { libc::mach_host_self() };
        Ok(Self { page_size, total, timebase, host, cache: HashMap::new() })
    }

    fn ticks_to_ns(&self, ticks: u64) -> u64 {
        (u128::from(ticks) * u128::from(self.timebase.0) / u128::from(self.timebase.1)) as u64
    }

    fn read_process(&mut self, pid: i32) -> Option<ProcessSample> {
        let id = identity(pid)?;
        let key = (pid, id.started_at);
        let st = match self.cache.get(&key) {
            // Пустые аргументы могли быть гонкой с exec — перечитываем.
            Some(st) if !st.args.is_empty() || st.exe.is_none() => st.clone(),
            _ => {
                let name = proc_name(pid).unwrap_or_else(|| id.name.clone());
                let exe = proc_path(pid);
                let args = proc_args(pid);
                let app_group = app_group(&name, exe.as_deref());
                let st = Static { name, exe, args, app_group };
                self.cache.insert(key, st.clone());
                st
            }
        };
        let Static { name, exe, args, app_group } = st;
        let rusage = rusage(pid);
        let task = task_info(pid);

        let (footprint, rss, cpu_ticks) = match &rusage {
            Some(r) => (Some(r.ri_phys_footprint), r.ri_resident_size, Some(r.ri_user_time + r.ri_system_time)),
            None => (None, task.as_ref().map_or(0, |t| t.pti_resident_size), None),
        };
        // И rusage, и taskinfo отдают время в mach-тиках.
        let cpu_time_ns = cpu_ticks
            .or_else(|| task.as_ref().map(|t| t.pti_total_user + t.pti_total_system))
            .map_or(0, |t| self.ticks_to_ns(t));
        let threads = task.as_ref().map_or(0, |t| t.pti_threadnum.max(0) as u32);

        let state = match id.status {
            libc::SRUN => ProcState::Running,
            1 | 3 => ProcState::Sleeping, // SIDL, SSLEEP
            libc::SSTOP => ProcState::Stopped,
            libc::SZOMB => ProcState::Zombie,
            _ => ProcState::Unknown,
        };
        Some(ProcessSample {
            info: ProcessInfo {
                pid,
                ppid: id.ppid,
                name,
                exe,
                args,
                uid: id.uid,
                footprint,
                rss,
                cpu_pct: 0.0,
                threads,
                state,
                started_at: id.started_at,
                app_group,
            },
            cpu_time_ns,
        })
    }
}

impl Platform for MacOsPlatform {
    fn system_memory(&self) -> Result<SystemMemory> {
        let mut vm = MaybeUninit::<libc::vm_statistics64>::zeroed();
        let mut count = (size_of::<libc::vm_statistics64>() / size_of::<libc::integer_t>()) as libc::mach_msg_type_number_t;
        // SAFETY: буфер размера vm_statistics64, count — его длина в integer_t; ядро пишет не больше count.
        let kr = unsafe { libc::host_statistics64(self.host, libc::HOST_VM_INFO64, vm.as_mut_ptr().cast(), &mut count) };
        if kr != libc::KERN_SUCCESS {
            return Err(Error::Invalid { what: "host_statistics64", detail: format!("kern_return {kr}") });
        }
        // SAFETY: структура обнулена заранее и частично заполнена ядром — любые байты допустимы для целых.
        let vm = unsafe { vm.assume_init() };
        let page = self.page_size;
        let internal = u64::from(vm.internal_page_count);
        let purgeable = u64::from(vm.purgeable_count);
        let app = internal.saturating_sub(purgeable) * page;
        let wired = u64::from(vm.wire_count) * page;
        let compressed = u64::from(vm.compressor_page_count) * page;
        let cached_files = (u64::from(vm.external_page_count) + purgeable) * page;
        let free = u64::from(vm.free_count).saturating_sub(u64::from(vm.speculative_count)) * page;

        let swap = sysctl_struct::<libc::xsw_usage>("vm.swapusage").ok();
        let pressure = sysctl_i32("kern.memorystatus_vm_pressure_level").map(Pressure::from_level).unwrap_or_default();
        let available_pct = sysctl_i32("kern.memorystatus_level").unwrap_or(0).clamp(0, 100) as u8;

        Ok(SystemMemory {
            total: self.total,
            used: app + wired + compressed,
            app,
            wired,
            compressed,
            cached_files,
            free,
            swap_total: swap.map_or(0, |s| s.xsu_total),
            swap_used: swap.map_or(0, |s| s.xsu_used),
            pressure,
            available_pct,
            counters: VmCounters {
                pageins: vm.pageins,
                pageouts: vm.pageouts,
                swapins: vm.swapins,
                swapouts: vm.swapouts,
                compressions: vm.compressions,
                decompressions: vm.decompressions,
            },
        })
    }

    fn processes(&mut self) -> Result<Vec<ProcessSample>> {
        let pids = list_pids()?;
        let out: Vec<ProcessSample> = pids.into_iter().filter(|&p| p > 0).filter_map(|p| self.read_process(p)).collect();
        // Кэш — только живые процессы.
        let alive: std::collections::HashSet<(i32, u64)> = out.iter().map(|s| (s.info.pid, s.info.started_at)).collect();
        self.cache.retain(|k, _| alive.contains(k));
        Ok(out)
    }

    fn process(&mut self, pid: i32) -> Result<Option<ProcessInfo>> {
        // Для сверки личности перед снятием — всегда свежее чтение, мимо кэша.
        if let Some(id) = identity(pid) {
            self.cache.remove(&(pid, id.started_at));
        }
        Ok(self.read_process(pid).map(|s| s.info))
    }

    fn signal(&mut self, pid: i32, signal: Signal) -> Result<()> {
        let sig = match signal {
            Signal::Term => libc::SIGTERM,
            Signal::Kill => libc::SIGKILL,
        };
        // SAFETY: kill без указателей; pid проверен вызывающим слоем.
        if unsafe { libc::kill(pid, sig) } == 0 {
            return Ok(());
        }
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::ESRCH) => Err(Error::ProcessGone { pid }),
            Some(libc::EPERM) => Err(Error::Permission { pid, source: err }),
            _ => Err(Error::Syscall { call: "kill", source: err }),
        }
    }

    fn env(&mut self, pid: i32, keys: &[&str]) -> Result<Vec<(String, String)>> {
        let Some((_, env)) = procargs(pid) else { return Ok(Vec::new()) };
        Ok(env
            .into_iter()
            .filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
            .filter(|(k, _)| keys.contains(&k.as_str()))
            .collect())
    }

    fn cwd(&mut self, pid: i32) -> Result<Option<String>> {
        let Some(info) = pidinfo::<libc::proc_vnodepathinfo>(pid, libc::PROC_PIDVNODEPATHINFO) else {
            return Ok(None);
        };
        Ok(cstr(info.pvi_cdir.vip_path.as_flattened()))
    }

    fn alive(&mut self, pid: i32) -> Result<bool> {
        // SAFETY: сигнал 0 ничего не посылает, только проверяет существование.
        if unsafe { libc::kill(pid, 0) } == 0 {
            // Зомби ещё «жив» для kill(0), но память уже отдал.
            return Ok(!matches!(identity(pid).map(|i| i.status), Some(libc::SZOMB)));
        }
        Ok(std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
    }
}

fn list_pids() -> Result<Vec<i32>> {
    // SAFETY: нулевой буфер — запрос числа процессов.
    let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if n <= 0 {
        return Err(Error::syscall("proc_listallpids"));
    }
    let mut buf = vec![0i32; n as usize + 64];
    let bytes = (buf.len() * size_of::<i32>()) as libc::c_int;
    // SAFETY: буфер на bytes байт; функция возвращает число записанных pid.
    let got = unsafe { libc::proc_listallpids(buf.as_mut_ptr().cast(), bytes) };
    if got <= 0 {
        return Err(Error::syscall("proc_listallpids"));
    }
    buf.truncate(got as usize);
    Ok(buf)
}

fn pidinfo<T>(pid: i32, flavor: libc::c_int) -> Option<T> {
    let mut v = MaybeUninit::<T>::zeroed();
    let size = size_of::<T>() as libc::c_int;
    // SAFETY: буфер ровно size байт; успех — только если ядро записало его целиком.
    let n = unsafe { libc::proc_pidinfo(pid, flavor, 0, v.as_mut_ptr().cast(), size) };
    // SAFETY: n == size — структура заполнена; это POD из целых.
    (n == size).then(|| unsafe { v.assume_init() })
}

/// Личность процесса: родитель, владелец, состояние, время запуска, короткое имя.
struct Identity {
    ppid: i32,
    uid: u32,
    status: u32,
    started_at: u64,
    name: String,
}

/// Свой процесс — `PROC_PIDTBSDINFO`. Чужой (root, `_windowserver`) его не отдаёт, но
/// `PROC_PIDT_SHORTBSDINFO` и `sysctl KERN_PROC_PID` доступны без прав — так процессы
/// других пользователей остаются в снимке (память у них `None`, как в `ps` без root).
fn identity(pid: i32) -> Option<Identity> {
    if let Some(b) = bsd_info(pid) {
        return Some(Identity {
            ppid: b.pbi_ppid as i32,
            uid: b.pbi_uid,
            status: b.pbi_status,
            started_at: b.pbi_start_tvsec,
            name: cstr(&b.pbi_name).or_else(|| cstr(&b.pbi_comm)).unwrap_or_default(),
        });
    }
    let short: libc::proc_bsdshortinfo = pidinfo(pid, PROC_PIDT_SHORTBSDINFO)?;
    Some(Identity {
        ppid: short.pbsi_ppid as i32,
        uid: short.pbsi_uid,
        status: short.pbsi_status,
        started_at: start_time_sysctl(pid).unwrap_or(0),
        name: cstr(&short.pbsi_comm).unwrap_or_default(),
    })
}

/// `PROC_PIDT_SHORTBSDINFO` из `<sys/proc_info.h>` (в libc нет константы).
const PROC_PIDT_SHORTBSDINFO: libc::c_int = 13;
/// `sizeof(struct kinfo_proc)` на 64-битной macOS.
const KINFO_PROC_SIZE: usize = 648;

/// Время запуска из `kinfo_proc`: `kp_proc.p_un.__p_starttime` — первое поле структуры.
fn start_time_sysctl(pid: i32) -> Option<u64> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PID, pid];
    let mut buf = vec![0u64; KINFO_PROC_SIZE / 8];
    let mut size = KINFO_PROC_SIZE;
    // SAFETY: буфер KINFO_PROC_SIZE байт, выровнен по 8; ядро пишет не больше size.
    let rc = unsafe { libc::sysctl(mib.as_mut_ptr(), 4, buf.as_mut_ptr().cast(), &mut size, std::ptr::null_mut(), 0) };
    if rc != 0 || size < 16 {
        return None;
    }
    // timeval { tv_sec: i64, tv_usec: i32 } — tv_sec в первых 8 байтах.
    u64::try_from(buf[0] as i64).ok().filter(|&s| s > 0)
}

fn bsd_info(pid: i32) -> Option<libc::proc_bsdinfo> {
    pidinfo(pid, libc::PROC_PIDTBSDINFO)
}

fn task_info(pid: i32) -> Option<libc::proc_taskinfo> {
    pidinfo(pid, libc::PROC_PIDTASKINFO)
}

fn rusage(pid: i32) -> Option<libc::rusage_info_v4> {
    let mut v = MaybeUninit::<libc::rusage_info_v4>::zeroed();
    // SAFETY: буфер под rusage_info_v4 для флейвора RUSAGE_INFO_V4.
    let rc = unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V4, v.as_mut_ptr().cast()) };
    // SAFETY: rc == 0 — ядро заполнило структуру.
    (rc == 0).then(|| unsafe { v.assume_init() })
}

fn proc_path(pid: i32) -> Option<String> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: буфер указанной длины; возвращается длина пути.
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}

fn proc_name(pid: i32) -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: буфер указанной длины; возвращается длина имени.
    let n = unsafe { libc::proc_name(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}

/// `KERN_PROCARGS2`: argc, путь запуска, выравнивание нулями, argv[], окружение.
fn proc_args(pid: i32) -> Vec<String> {
    procargs(pid).map(|(args, _)| args).unwrap_or_default()
}

/// Аргументы и окружение процесса (окружение — только своих процессов: так решает ядро).
fn procargs(pid: i32) -> Option<(Vec<String>, Vec<String>)> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut argmax: libc::c_int = 0;
    let mut amsize = size_of::<libc::c_int>();
    let mut am_mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    // SAFETY: читаем int в живую переменную.
    if unsafe { libc::sysctl(am_mib.as_mut_ptr(), 2, (&mut argmax as *mut libc::c_int).cast(), &mut amsize, std::ptr::null_mut(), 0) } != 0 {
        return None;
    }
    let mut size = argmax.max(0) as usize;
    if size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size];
    // SAFETY: буфер size байт; ядро пишет не больше и обновляет size.
    if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr().cast(), &mut size, std::ptr::null_mut(), 0) } != 0 || size < 4 {
        return None;
    }
    buf.truncate(size);
    Some(parse_procargs(&buf))
}

/// Разбор буфера `KERN_PROCARGS2` (отдельно — для теста без системы).
fn parse_procargs(buf: &[u8]) -> (Vec<String>, Vec<String>) {
    if buf.len() < 4 {
        return (Vec::new(), Vec::new());
    }
    let argc = i32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]).max(0) as usize;
    let mut rest = &buf[4..];
    // Путь запуска, затем нули выравнивания.
    let Some(end) = rest.iter().position(|&b| b == 0) else { return (Vec::new(), Vec::new()) };
    rest = &rest[end..];
    let start = rest.iter().position(|&b| b != 0).unwrap_or(rest.len());
    rest = &rest[start..];
    let mut parts = rest.split(|&b| b == 0);
    let args = parts.by_ref().take(argc).map(|s| String::from_utf8_lossy(s).into_owned()).collect();
    let env = parts.take_while(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect();
    (args, env)
}

fn cstr(chars: &[libc::c_char]) -> Option<String> {
    // SAFETY: c_char и u8 одного размера; читаем внутри среза.
    let bytes = unsafe { std::slice::from_raw_parts(chars.as_ptr().cast::<u8>(), chars.len()) };
    let s = CStr::from_bytes_until_nul(bytes).ok()?.to_string_lossy().into_owned();
    (!s.is_empty()).then_some(s)
}

fn sysctl_struct<T>(name: &str) -> Result<T> {
    let cname = std::ffi::CString::new(name).expect("имя sysctl без нулей");
    let mut v = MaybeUninit::<T>::zeroed();
    let mut size = size_of::<T>();
    // SAFETY: буфер size байт под T; ядро пишет не больше size.
    let rc = unsafe { libc::sysctlbyname(cname.as_ptr(), v.as_mut_ptr().cast(), &mut size, std::ptr::null_mut(), 0) };
    if rc != 0 {
        return Err(Error::syscall("sysctlbyname"));
    }
    // SAFETY: обнулено и заполнено ядром; T здесь — только целые и POD.
    Ok(unsafe { v.assume_init() })
}

fn sysctl_u64(name: &str) -> Result<u64> {
    sysctl_struct::<u64>(name)
}

fn sysctl_i32(name: &str) -> Result<i32> {
    sysctl_struct::<i32>(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_procargs_with_env() {
        let mut buf = 2i32.to_ne_bytes().to_vec();
        buf.extend_from_slice(b"/bin/x\0\0\0x\0-v\0HOME=/h\0SECRET=1\0\0junk");
        let (args, env) = parse_procargs(&buf);
        assert_eq!(args, ["x", "-v"]);
        assert_eq!(env, ["HOME=/h", "SECRET=1"]);
    }

    #[test]
    fn reads_only_requested_env_keys() {
        let mut p = MacOsPlatform::new().unwrap();
        let env = p.env(std::process::id() as i32, &["HOME"]).unwrap();
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].0, "HOME");
    }

    #[test]
    fn sees_foreign_processes() {
        let mut p = MacOsPlatform::new().unwrap();
        let launchd = p.process(1).unwrap().expect("launchd виден без прав");
        assert_eq!(launchd.uid, 0);
        assert!(launchd.started_at > 0);
        let me = p.process(std::process::id() as i32).unwrap().unwrap();
        assert!(me.footprint.is_some());
        let start = start_time_sysctl(std::process::id() as i32).unwrap();
        assert_eq!(start, me.started_at, "оба пути дают одно время запуска");
    }
}

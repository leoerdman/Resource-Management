//! Linux: всё из `/proc` (и `/sys/block/zram*`), сигналы — `libc::kill`.
//!
//! Разборщики текстов `/proc` — чистые функции без `cfg`: их тесты идут на любой ОС.
//! Чего не дали права (чужие процессы: `exe`, `cwd`) — `None` или пусто, без паники.
//!
//! # Как цифры Linux ложатся на модель (она списана с Activity Monitor)
//!
//! - `used = MemTotal − MemAvailable` — как «used» у `free` (procps-ng 4) и GNOME System Monitor.
//! - `wired = Unevictable + SUnreclaim + KernelStack + PageTables` — то, что ядро не отдаст:
//!   неэвиктируемые страницы (`Mlocked` — их подмножество, отдельно не прибавляется), не
//!   возвращаемый slab, стеки ядра, таблицы страниц.
//! - `compressed` — сжатые страницы в ОЗУ: пул `Zswap` из meminfo плюс `mem_used_total`
//!   всех `/sys/block/zram*/mm_stat` (сколько ОЗУ реально занимает zram с накладными).
//! - `app = used − wired − compressed` — чтобы сохранялся инвариант модели
//!   `used = app + wired + compressed`; по смыслу это ≈ `AnonPages + Shmem` (+ неотдаваемая часть кэша).
//! - `cached_files = Cached + Buffers − Shmem` — Shmem (tmpfs, общая память) числится в Cached,
//!   но вытеснить её как файловый кэш нельзя.
//! - `free = MemFree`; своп — `SwapTotal`, `SwapTotal − SwapFree` (zram-своп тоже сюда).
//! - `available_pct = MemAvailable / MemTotal`.
//! - Давление — PSI `/proc/pressure/memory` и доля доступной памяти, берётся худшее
//!   (см. [`pressure_from`]).
//! - Счётчики `/proc/vmstat`: `pgpgin/pgpgout` (в КиБ → страницы), `pswpin/pswpout`,
//!   `zswpout/zswpin` как сжатия/распаковки (если zswap есть).
//!
//! Процессы:
//! - `footprint = RssAnon + RssShmem + VmSwap` из `/proc/<pid>/status` — ближайший аналог
//!   `phys_footprint` macOS (своя грязная анонимная память, её общая память и выгруженное в своп);
//!   та же анонимная часть, что ядро считает в `oom_badness`. `status` читается для процессов
//!   всех пользователей, поэтому `footprint` есть почти всегда. `smaps_rollup` (PSS) точнее для
//!   форкнутых воркеров, но это обход таблиц страниц: на 800 процессах раз в 2 с — дорого.
//! - `rss = VmRSS`; `uid` — реальный uid из `status` (по нему ядро решает, можно ли послать сигнал).
//! - `started_at = btime + starttime / CLK_TCK`, CPU — `(utime + stime) / CLK_TCK` в нс.
//! - Потоки ядра (флаг `PF_KTHREAD`) в список не попадают: своей памяти у них нет, сигналы
//!   они игнорируют.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::collections::HashMap;

use crate::model::{Pressure, ProcState, SystemMemory, VmCounters};

/// Байты из строк вида `MemTotal:  16314388 kB` (`/proc/meminfo`).
pub(crate) fn parse_meminfo(text: &str) -> HashMap<&str, u64> {
    text.lines()
        .filter_map(|line| {
            let (key, rest) = line.split_once(':')?;
            let mut it = rest.split_whitespace();
            let n: u64 = it.next()?.parse().ok()?;
            let mult = match it.next() {
                Some("kB") => 1024,
                _ => 1,
            };
            Some((key.trim(), n.saturating_mul(mult)))
        })
        .collect()
}

/// Строка `some avg10=…` / `full avg10=…` из `/proc/pressure/memory`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct Psi {
    /// Доля времени (%), когда хотя бы одна задача ждала память, за 10 с.
    pub some_avg10: f64,
    /// Доля времени (%), когда ждали все незанятые задачи; нет на старых ядрах.
    pub full_avg10: Option<f64>,
}

pub(crate) fn parse_psi(text: &str) -> Option<Psi> {
    let avg10 = |line: &str| {
        line.split_whitespace()
            .find_map(|kv| kv.strip_prefix("avg10="))
            .and_then(|v| v.parse::<f64>().ok())
    };
    let mut psi = None::<Psi>;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("some ") {
            psi.get_or_insert_with(Psi::default).some_avg10 = avg10(rest)?;
        } else if let Some(rest) = line.strip_prefix("full ") {
            psi.get_or_insert_with(Psi::default).full_avg10 = avg10(rest);
        }
    }
    psi
}

/// Давление памяти: худшее из PSI и доли доступной памяти.
///
/// Пороги PSI близки к Android `lmkd` и `low-memory-monitor` GNOME:
/// - `Critical` — `full avg10 ≥ 10 %` или `some avg10 ≥ 40 %` (система почти встала на памяти);
/// - `Warning` — `some avg10 ≥ 10 %` (задачи заметно ждут подкачку/реклейм).
///
/// По доступной памяти (`MemAvailable / MemTotal`) — тот же смысл, что у `kern.memorystatus_level`:
/// `< 5 %` — `Critical`, `< 15 %` — `Warning`. Без PSI (ядро без `CONFIG_PSI`) решает только она.
pub(crate) fn pressure_from(psi: Option<Psi>, available_pct: u8) -> Pressure {
    let by_psi = match psi {
        Some(p) if p.full_avg10.unwrap_or(0.0) >= 10.0 || p.some_avg10 >= 40.0 => Pressure::Critical,
        Some(p) if p.some_avg10 >= 10.0 => Pressure::Warning,
        _ => Pressure::Normal,
    };
    let by_avail = match available_pct {
        0..5 => Pressure::Critical,
        5..15 => Pressure::Warning,
        _ => Pressure::Normal,
    };
    worst(by_psi, by_avail)
}

fn worst(a: Pressure, b: Pressure) -> Pressure {
    let rank = |p: Pressure| match p {
        Pressure::Normal => 0,
        Pressure::Warning => 1,
        Pressure::Critical => 2,
    };
    if rank(a) >= rank(b) { a } else { b }
}

/// Накопительные счётчики `/proc/vmstat`. `pgpgin/pgpgout` ядро считает в КиБ — переводим в страницы.
pub(crate) fn parse_vmstat(text: &str, page_size: u64) -> VmCounters {
    let mut c = VmCounters::default();
    let kib_per_page = (page_size / 1024).max(1);
    for line in text.lines() {
        let Some((key, value)) = line.split_once(' ') else { continue };
        let Ok(v) = value.trim().parse::<u64>() else { continue };
        match key {
            "pgpgin" => c.pageins = v / kib_per_page,
            "pgpgout" => c.pageouts = v / kib_per_page,
            "pswpin" => c.swapins = v,
            "pswpout" => c.swapouts = v,
            "zswpout" => c.compressions = v,
            "zswpin" => c.decompressions = v,
            _ => {}
        }
    }
    c
}

/// Сколько ОЗУ занимает zram: третье поле `mm_stat` (`mem_used_total`, байты).
pub(crate) fn parse_zram_mm_stat(text: &str) -> Option<u64> {
    text.split_whitespace().nth(2)?.parse().ok()
}

/// Память системы из уже прочитанных текстов — чистая функция, чтобы проверять её тестами.
pub(crate) fn system_memory_from(meminfo: &str, zram_bytes: u64, psi: Option<Psi>, vmstat: &str, page_size: u64) -> Option<SystemMemory> {
    let m = parse_meminfo(meminfo);
    let get = |k: &str| m.get(k).copied().unwrap_or(0);
    let total = *m.get("MemTotal")?;
    let free = get("MemFree");
    let shmem = get("Shmem");
    let cached_files = (get("Cached") + get("Buffers")).saturating_sub(shmem);
    // Ядра до 3.14 без MemAvailable: оценка как у старого `free` (free + buffers/cache).
    let available = m.get("MemAvailable").copied().unwrap_or(free + cached_files).min(total);
    let used = total - available;
    let wired = (get("Unevictable") + get("SUnreclaim") + get("KernelStack") + get("PageTables")).min(used);
    let compressed = (get("Zswap") + zram_bytes).min(used - wired);
    let app = used - wired - compressed;
    let swap_total = get("SwapTotal");
    let swap_used = swap_total.saturating_sub(get("SwapFree"));
    let available_pct = available.saturating_mul(100).checked_div(total).map_or(0, |v| v.min(100) as u8);
    Some(SystemMemory {
        total,
        used,
        app,
        wired,
        compressed,
        cached_files,
        free,
        swap_total,
        swap_used,
        pressure: pressure_from(psi, available_pct),
        available_pct,
        counters: parse_vmstat(vmstat, page_size),
    })
}

/// Поля `/proc/<pid>/stat`, которые нужны.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stat {
    pub comm: String,
    pub state: char,
    pub ppid: i32,
    pub flags: u64,
    /// Тики `CLK_TCK`.
    pub utime: u64,
    pub stime: u64,
    pub threads: u32,
    /// Тики `CLK_TCK` от загрузки.
    pub starttime: u64,
}

/// `PF_KTHREAD` из `include/linux/sched.h`.
pub(crate) const PF_KTHREAD: u64 = 0x0020_0000;

/// Разбор `pid (comm) state ppid …`. `comm` может содержать пробелы и скобки — ищем последнюю `)`.
pub(crate) fn parse_stat(text: &str) -> Option<Stat> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let comm = text.get(open + 1..close)?.to_string();
    // Поля после `)`: [0] state (поле 3 в man proc), [1] ppid (4), … [6] flags (9),
    // [11] utime (14), [12] stime (15), [17] num_threads (20), [19] starttime (22).
    let f: Vec<&str> = text.get(close + 1..)?.split_whitespace().collect();
    if f.len() < 20 {
        return None;
    }
    Some(Stat {
        comm,
        state: f[0].chars().next()?,
        ppid: f[1].parse().ok()?,
        flags: f[6].parse().ok()?,
        utime: f[11].parse().ok()?,
        stime: f[12].parse().ok()?,
        threads: f[17].parse().ok()?,
        starttime: f[19].parse().ok()?,
    })
}

/// Поля `/proc/<pid>/status`, байты.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Status {
    /// Реальный uid.
    pub uid: Option<u32>,
    pub vm_rss: u64,
    pub rss_anon: u64,
    pub rss_shmem: u64,
    pub vm_swap: u64,
    /// Были ли поля памяти вообще (у зомби и потоков ядра их нет).
    pub has_memory: bool,
}

impl Status {
    /// Аналог `phys_footprint`: своя анонимная, своя общая и выгруженная память.
    pub fn footprint(&self) -> Option<u64> {
        self.has_memory.then_some(self.rss_anon + self.rss_shmem + self.vm_swap)
    }
}

pub(crate) fn parse_status(text: &str) -> Status {
    let mut s = Status::default();
    let kib = |rest: &str| rest.split_whitespace().next().and_then(|v| v.parse::<u64>().ok()).map_or(0, |v| v * 1024);
    for line in text.lines() {
        let Some((key, rest)) = line.split_once(':') else { continue };
        match key {
            "Uid" => s.uid = rest.split_whitespace().next().and_then(|v| v.parse().ok()),
            "VmRSS" => {
                s.vm_rss = kib(rest);
                s.has_memory = true;
            }
            "RssAnon" => s.rss_anon = kib(rest),
            "RssShmem" => s.rss_shmem = kib(rest),
            "VmSwap" => s.vm_swap = kib(rest),
            _ => {}
        }
    }
    s
}

/// `/proc/<pid>/cmdline`: аргументы через NUL, в конце NUL.
pub(crate) fn parse_cmdline(bytes: &[u8]) -> Vec<String> {
    let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
    if bytes.is_empty() {
        return Vec::new();
    }
    bytes.split(|&b| b == 0).map(|s| String::from_utf8_lossy(s).into_owned()).collect()
}

/// Буква состояния из `stat`.
pub(crate) fn proc_state(c: char) -> ProcState {
    match c {
        'R' => ProcState::Running,
        'S' | 'D' | 'I' | 'W' | 'P' | 'K' => ProcState::Sleeping,
        'T' | 't' => ProcState::Stopped,
        'Z' | 'X' | 'x' => ProcState::Zombie,
        _ => ProcState::Unknown,
    }
}

/// `btime` из `/proc/stat` — время загрузки, unix-секунды.
pub(crate) fn parse_btime(text: &str) -> Option<u64> {
    text.lines().find_map(|l| l.strip_prefix("btime ")).and_then(|v| v.trim().parse().ok())
}

/// Имя процесса: `comm` урезан ядром до 15 байт — если имя файла его продолжает, берём полное.
pub(crate) fn process_name(comm: &str, exe: Option<&str>) -> String {
    if comm.len() >= 15
        && let Some(base) = exe.and_then(|e| e.rsplit('/').next())
        && base.starts_with(comm)
    {
        return base.to_string();
    }
    comm.to_string()
}

#[cfg(target_os = "linux")]
pub use imp::LinuxPlatform;

#[cfg(target_os = "linux")]
mod imp {
    use std::collections::{HashMap, HashSet};
    use std::fs;

    use super::{PF_KTHREAD, Stat, parse_btime, parse_cmdline, parse_psi, parse_stat, parse_status, parse_zram_mm_stat, proc_state, process_name, system_memory_from};
    use crate::error::{Error, Result};
    use crate::group::{app_group, flatpak_app_id};
    use crate::model::{ProcState, ProcessInfo, SystemMemory};
    use crate::platform::{Platform, ProcessSample, Signal};

    /// Платформа Linux.
    pub struct LinuxPlatform {
        page_size: u64,
        clk_tck: u64,
        /// Время загрузки читается один раз: `btime` в `/proc/stat` может «дрожать» на секунду
        /// при подстройке часов, а от него зависит `started_at` — половина личности процесса.
        btime: u64,
        /// Неизменное за жизнь процесса по (pid, started_at) — как на macOS.
        cache: HashMap<(i32, u64), Static>,
    }

    #[derive(Clone)]
    struct Static {
        comm: String,
        name: String,
        exe: Option<String>,
        args: Vec<String>,
        app_group: String,
    }

    impl LinuxPlatform {
        /// Читает постоянные величины машины.
        pub fn new() -> Result<Self> {
            // SAFETY: sysconf без указателей, только читает константу libc.
            let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
            // SAFETY: то же.
            let clk_tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
            let stat = fs::read_to_string("/proc/stat").map_err(|e| Error::Syscall { call: "read /proc/stat", source: e })?;
            let btime = parse_btime(&stat).ok_or_else(|| Error::Invalid { what: "/proc/stat", detail: "нет btime".into() })?;
            Ok(Self {
                page_size: if page_size > 0 { page_size as u64 } else { 4096 },
                clk_tck: if clk_tck > 0 { clk_tck as u64 } else { 100 },
                btime,
                cache: HashMap::new(),
            })
        }

        fn started_at(&self, st: &Stat) -> u64 {
            self.btime + st.starttime / self.clk_tck
        }

        fn read_process(&mut self, pid: i32) -> Option<ProcessSample> {
            let st = parse_stat(&fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)?;
            if st.flags & PF_KTHREAD != 0 {
                return None;
            }
            let started_at = self.started_at(&st);
            let key = (pid, started_at);
            let cached = match self.cache.get(&key) {
                // Пустые аргументы могли быть гонкой с exec, смена comm — exec на месте: перечитываем.
                Some(s) if s.comm == st.comm && (!s.args.is_empty() || s.exe.is_none()) => s.clone(),
                _ => {
                    let exe = fs::read_link(format!("/proc/{pid}/exe"))
                        .ok()
                        .map(|p| p.to_string_lossy().trim_end_matches(" (deleted)").to_string());
                    let args = fs::read(format!("/proc/{pid}/cmdline")).map(|b| parse_cmdline(&b)).unwrap_or_default();
                    let name = process_name(&st.comm, exe.as_deref());
                    let cgroup = fs::read_to_string(format!("/proc/{pid}/cgroup")).unwrap_or_default();
                    let app_group = match flatpak_app_id(&cgroup) {
                        Some(id) => id.to_string(),
                        None => app_group(&name, exe.as_deref()),
                    };
                    let s = Static { comm: st.comm.clone(), name, exe, args, app_group };
                    self.cache.insert(key, s.clone());
                    s
                }
            };
            let status = fs::read_to_string(format!("/proc/{pid}/status")).map(|t| parse_status(&t)).unwrap_or_default();
            let cpu_ticks = st.utime + st.stime;
            let cpu_time_ns = (u128::from(cpu_ticks) * 1_000_000_000 / u128::from(self.clk_tck)) as u64;
            let Static { name, exe, args, app_group, .. } = cached;
            Some(ProcessSample {
                info: ProcessInfo {
                    pid,
                    ppid: st.ppid,
                    name,
                    exe,
                    args,
                    uid: status.uid.unwrap_or(u32::MAX),
                    footprint: status.footprint(),
                    rss: status.vm_rss,
                    cpu_pct: 0.0,
                    threads: st.threads,
                    state: proc_state(st.state),
                    started_at,
                    app_group,
                },
                cpu_time_ns,
            })
        }
    }

    fn list_pids() -> Result<Vec<i32>> {
        let dir = fs::read_dir("/proc").map_err(|e| Error::Syscall { call: "read_dir /proc", source: e })?;
        Ok(dir
            .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<i32>().ok())
            .filter(|&p| p > 0)
            .collect())
    }

    fn zram_bytes() -> u64 {
        let Ok(dir) = fs::read_dir("/sys/block") else { return 0 };
        dir.filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("zram"))
            .filter_map(|e| fs::read_to_string(e.path().join("mm_stat")).ok())
            .filter_map(|t| parse_zram_mm_stat(&t))
            .sum()
    }

    impl Platform for LinuxPlatform {
        fn system_memory(&self) -> Result<SystemMemory> {
            let meminfo = fs::read_to_string("/proc/meminfo").map_err(|e| Error::Syscall { call: "read /proc/meminfo", source: e })?;
            let psi = fs::read_to_string("/proc/pressure/memory").ok().and_then(|t| parse_psi(&t));
            let vmstat = fs::read_to_string("/proc/vmstat").unwrap_or_default();
            system_memory_from(&meminfo, zram_bytes(), psi, &vmstat, self.page_size)
                .ok_or_else(|| Error::Invalid { what: "/proc/meminfo", detail: "нет MemTotal".into() })
        }

        fn processes(&mut self) -> Result<Vec<ProcessSample>> {
            let out: Vec<ProcessSample> = list_pids()?.into_iter().filter_map(|p| self.read_process(p)).collect();
            let alive: HashSet<(i32, u64)> = out.iter().map(|s| (s.info.pid, s.info.started_at)).collect();
            self.cache.retain(|k, _| alive.contains(k));
            Ok(out)
        }

        fn process(&mut self, pid: i32) -> Result<Option<ProcessInfo>> {
            // Для сверки личности перед снятием — всегда свежее чтение, мимо кэша.
            self.cache.retain(|k, _| k.0 != pid);
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

        fn alive(&mut self, pid: i32) -> Result<bool> {
            // SAFETY: сигнал 0 ничего не посылает, только проверяет существование.
            if unsafe { libc::kill(pid, 0) } == 0 {
                // Зомби ещё «жив» для kill(0), но память уже отдал.
                let state = fs::read_to_string(format!("/proc/{pid}/stat")).ok().and_then(|t| parse_stat(&t)).map(|s| proc_state(s.state));
                return Ok(state != Some(ProcState::Zombie));
            }
            Ok(std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
        }

        fn cwd(&mut self, pid: i32) -> Result<Option<String>> {
            Ok(fs::read_link(format!("/proc/{pid}/cwd")).ok().map(|p| p.to_string_lossy().into_owned()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEMINFO: &str = "\
MemTotal:       16000000 kB
MemFree:         2000000 kB
MemAvailable:    6000000 kB
Buffers:          300000 kB
Cached:          4500000 kB
SwapCached:        10000 kB
Unevictable:      100000 kB
Mlocked:           50000 kB
SwapTotal:       8000000 kB
SwapFree:        7000000 kB
Zswap:            200000 kB
Zswapped:         600000 kB
Shmem:            800000 kB
SReclaimable:     400000 kB
SUnreclaim:       300000 kB
KernelStack:       20000 kB
PageTables:        80000 kB
HugePages_Total:       0
";

    #[test]
    fn meminfo_maps_to_model() {
        let m = parse_meminfo(MEMINFO);
        assert_eq!(m["MemTotal"], 16_000_000 * 1024);
        assert_eq!(m["HugePages_Total"], 0, "без единиц — как есть");
        let s = system_memory_from(MEMINFO, 100 * 1024, None, "", 4096).unwrap();
        let k = 1024;
        assert_eq!(s.total, 16_000_000 * k);
        assert_eq!(s.used, 10_000_000 * k, "total − available");
        assert_eq!(s.wired, (100_000 + 300_000 + 20_000 + 80_000) * k, "Mlocked не удваивается");
        assert_eq!(s.compressed, 200_000 * k + 100 * k, "zswap + zram");
        assert_eq!(s.app + s.wired + s.compressed, s.used, "инвариант модели");
        assert_eq!(s.cached_files, (4_500_000 + 300_000 - 800_000) * k);
        assert_eq!(s.free, 2_000_000 * k);
        assert_eq!(s.swap_total, 8_000_000 * k);
        assert_eq!(s.swap_used, 1_000_000 * k);
        assert_eq!(s.available_pct, 37);
        assert_eq!(s.pressure, Pressure::Normal);
    }

    #[test]
    fn meminfo_without_available_and_garbage() {
        let s = system_memory_from("MemTotal: 1000 kB\nMemFree: 100 kB\nCached: 200 kB\nbroken line\n", 0, None, "", 4096).unwrap();
        assert_eq!(s.used, 700 * 1024);
        assert_eq!(s.available_pct, 30);
        assert!(system_memory_from("MemFree: 1 kB", 0, None, "", 4096).is_none());
    }

    #[test]
    fn psi_and_pressure() {
        let text = "some avg10=12.50 avg60=3.00 avg300=1.00 total=123456\nfull avg10=0.40 avg60=0.10 avg300=0.00 total=2345\n";
        let psi = parse_psi(text).unwrap();
        assert_eq!(psi, Psi { some_avg10: 12.5, full_avg10: Some(0.4) });
        assert_eq!(pressure_from(Some(psi), 50), Pressure::Warning);
        assert_eq!(pressure_from(Some(Psi { some_avg10: 1.0, full_avg10: Some(15.0) }), 50), Pressure::Critical);
        assert_eq!(pressure_from(Some(Psi { some_avg10: 45.0, full_avg10: None }), 50), Pressure::Critical);
        assert_eq!(pressure_from(Some(Psi { some_avg10: 0.0, full_avg10: Some(0.0) }), 50), Pressure::Normal);
        assert_eq!(pressure_from(None, 4), Pressure::Critical, "без PSI — по доступной памяти");
        assert_eq!(pressure_from(None, 10), Pressure::Warning);
        assert_eq!(pressure_from(Some(Psi::default()), 3), Pressure::Critical, "худшее из двух");
        assert_eq!(parse_psi("some avg10=0.00 avg60=0.00 avg300=0.00 total=0\n").unwrap().full_avg10, None);
        assert_eq!(parse_psi(""), None);
    }

    #[test]
    fn vmstat_counters() {
        let text = "nr_free_pages 12345\npgpgin 4000\npgpgout 8000\npswpin 7\npswpout 9\nzswpin 3\nzswpout 5\npgfault 1\n";
        let c = parse_vmstat(text, 4096);
        assert_eq!(c, VmCounters { pageins: 1000, pageouts: 2000, swapins: 7, swapouts: 9, compressions: 5, decompressions: 3 });
        assert_eq!(parse_vmstat("pgpgin 4000\n", 16384).pageins, 250, "страницы 16 КиБ (arm64)");
    }

    #[test]
    fn zram_mm_stat() {
        assert_eq!(parse_zram_mm_stat("  1048576   262144   300000        0   400000      10       0       0       0\n"), Some(300_000));
        assert_eq!(parse_zram_mm_stat("1 2"), None);
    }

    #[test]
    fn stat_with_tricky_comm() {
        let text = "4242 (Web Content (x) ) S 1000 4242 4242 0 -1 4194560 100 0 0 0 250 50 0 0 20 0 31 0 987654 1000000 2000 18446744073709551615 0 0\n";
        let st = parse_stat(text).unwrap();
        assert_eq!(st.comm, "Web Content (x) ");
        assert_eq!(st.state, 'S');
        assert_eq!(st.ppid, 1000);
        assert_eq!((st.utime, st.stime), (250, 50));
        assert_eq!(st.threads, 31);
        assert_eq!(st.starttime, 987654);
        assert_eq!(st.flags & PF_KTHREAD, 0);
        let kthread = "2 (kthreadd) S 0 0 0 0 -1 2129984 0 0 0 0 0 0 0 0 20 0 1 0 0 0 0 18446744073709551615 0 0\n";
        assert_ne!(parse_stat(kthread).unwrap().flags & PF_KTHREAD, 0);
        assert!(parse_stat("1 (x) S 0").is_none());
        assert!(parse_stat("garbage").is_none());
    }

    #[test]
    fn status_footprint() {
        let text = "Name:\tnode\nState:\tS (sleeping)\nUid:\t1000\t1000\t1000\t1000\nVmRSS:\t  500000 kB\nRssAnon:\t  400000 kB\nRssFile:\t   90000 kB\nRssShmem:\t   10000 kB\nVmSwap:\t   50000 kB\nThreads:\t12\n";
        let s = parse_status(text);
        assert_eq!(s.uid, Some(1000));
        assert_eq!(s.vm_rss, 500_000 * 1024);
        assert_eq!(s.footprint(), Some((400_000 + 10_000 + 50_000) * 1024));
        let zombie = parse_status("Name:\tx\nState:\tZ (zombie)\nUid:\t0\t0\t0\t0\n");
        assert_eq!(zombie.uid, Some(0));
        assert_eq!(zombie.footprint(), None);
    }

    #[test]
    fn cmdline_and_names() {
        assert_eq!(parse_cmdline(b"node\0server.js\0--port\x003000\0"), vec!["node", "server.js", "--port", "3000"]);
        assert_eq!(parse_cmdline(b"chrome --type=renderer"), vec!["chrome --type=renderer"]);
        assert!(parse_cmdline(b"").is_empty());
        assert_eq!(parse_cmdline(b"a\0\0b\0"), vec!["a", "", "b"]);
        assert_eq!(process_name("telegram-deskto", Some("/usr/bin/telegram-desktop")), "telegram-desktop");
        assert_eq!(process_name("Isolated Web Co", Some("/usr/lib/firefox/firefox")), "Isolated Web Co");
        assert_eq!(process_name("node", Some("/usr/bin/nodejs")), "node");
        assert_eq!(parse_btime("cpu 1 2 3\nbtime 1700000000\nprocesses 5\n"), Some(1_700_000_000));
        assert_eq!(proc_state('D'), ProcState::Sleeping);
        assert_eq!(proc_state('Z'), ProcState::Zombie);
        assert_eq!(proc_state('t'), ProcState::Stopped);
        assert_eq!(proc_state('?'), ProcState::Unknown);
    }
}

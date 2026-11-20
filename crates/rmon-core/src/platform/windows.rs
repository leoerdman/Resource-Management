//! Windows: Win32 через `windows-sys` (Toolhelp, PSAPI, токены) и `NtQueryInformationProcess`.
//!
//! Чистые части (разбор командной строки, перевод FILETIME, раскладка памяти) — без `cfg`:
//! их тесты идут на любой ОС. Чего не дали права — `None` или пусто, без паники.
//!
//! # Как цифры Windows ложатся на модель
//!
//! - `total = ullTotalPhys`, `used = total − ullAvailPhys` («Используется» в диспетчере задач).
//! - `cached_files = SystemCache` (standby-список + системный рабочий набор из
//!   `GetPerformanceInfo`), но не больше доступного; `free = доступно − cached_files`.
//! - `wired = KernelNonpaged` — невыгружаемый пул ядра.
//! - `compressed` — рабочий набор процесса «Memory Compression» (хранилище сжатых страниц),
//!   если его удалось открыть; иначе 0.
//! - `app = used − wired − compressed` — инвариант модели `used = app + wired + compressed`.
//! - Своп — файл подкачки: `swap_total = лимит выделения − ОЗУ`,
//!   `swap_used = выделено − занято в ОЗУ` (часть выделенного, которой нет в ОЗУ), в пределах total.
//! - `available_pct = ullAvailPhys / ullTotalPhys`.
//! - Давление — худшее из загрузки памяти (`dwMemoryLoad`) и доли выделенного от лимита
//!   (при упоре в лимит Windows отказывает в выделении): см. [`pressure_from`].
//! - Накопительных счётчиков подкачки Win32 без PDH не даёт — `VmCounters` нулевые.
//!
//! Процессы:
//! - список, родитель, потоки, имя — `CreateToolhelp32Snapshot`;
//! - путь — `QueryFullProcessImageNameW`, аргументы —
//!   `NtQueryInformationProcess(ProcessCommandLineInformation)` (хватает
//!   `PROCESS_QUERY_LIMITED_INFORMATION`, Windows 8.1+), разбор — правила `CommandLineToArgvW`;
//! - `footprint = PrivateWorkingSetSize` (столбец «Память» диспетчера задач), `rss = WorkingSetSize`;
//! - запуск и CPU — `GetProcessTimes`;
//! - `uid`: SID владельца процесса совпал с нашим → [`crate::platform::WINDOWS_OWN_UID`] (= `current_uid()`),
//!   чужой или непрочитанный → `u32::MAX`, и политика защищает его как процесс другого пользователя.
//!
//! Сигналы: `Term` — `WM_CLOSE` видимым окнам верхнего уровня процесса (вежливо, как кнопка
//! «закрыть»); окон нет — `TerminateProcess`. `Kill` — `TerminateProcess`.
#![cfg_attr(not(windows), allow(dead_code))]

use crate::model::{Pressure, SystemMemory, VmCounters};

/// Разбор командной строки по правилам `CommandLineToArgvW` / MSVC CRT.
///
/// argv[0] — до пробела или в кавычках, без обработки `\`. Дальше: `2n` обратных слэшей перед `"`
/// дают `n` слэшей и кавычка переключает режим, `2n+1` — `n` слэшей и буквальную `"`;
/// `""` внутри кавычек — буквальная `"`. Прочие `\` — как есть.
pub(crate) fn split_command_line(cmd: &str) -> Vec<String> {
    let chars: Vec<char> = cmd.chars().collect();
    let n = chars.len();
    let ws = |c: char| c == ' ' || c == '\t';
    let mut i = 0;
    let mut out = Vec::new();
    while i < n && ws(chars[i]) {
        i += 1;
    }
    if i >= n {
        return out;
    }
    let mut first = String::new();
    if chars[i] == '"' {
        i += 1;
        while i < n && chars[i] != '"' {
            first.push(chars[i]);
            i += 1;
        }
        i += 1;
    } else {
        while i < n && !ws(chars[i]) {
            first.push(chars[i]);
            i += 1;
        }
    }
    out.push(first);
    loop {
        while i < n && ws(chars[i]) {
            i += 1;
        }
        if i >= n {
            break;
        }
        let mut arg = String::new();
        let mut quoted = false;
        while i < n {
            let c = chars[i];
            if c == '\\' {
                let start = i;
                while i < n && chars[i] == '\\' {
                    i += 1;
                }
                let count = i - start;
                if i < n && chars[i] == '"' {
                    arg.extend(std::iter::repeat_n('\\', count / 2));
                    if count % 2 == 1 {
                        arg.push('"');
                        i += 1;
                    }
                } else {
                    arg.extend(std::iter::repeat_n('\\', count));
                }
            } else if c == '"' {
                if quoted && i + 1 < n && chars[i + 1] == '"' {
                    arg.push('"');
                    i += 2;
                } else {
                    quoted = !quoted;
                    i += 1;
                }
            } else if ws(c) && !quoted {
                break;
            } else {
                arg.push(c);
                i += 1;
            }
        }
        out.push(arg);
    }
    out
}

/// Разница между эпохами FILETIME (1601) и unix (1970) в 100-нс интервалах.
const FILETIME_UNIX_OFFSET: u64 = 116_444_736_000_000_000;

/// FILETIME (100 нс с 1601 г.) → unix-секунды; до 1970 — 0.
pub(crate) fn filetime_to_unix_secs(ft: u64) -> u64 {
    ft.saturating_sub(FILETIME_UNIX_OFFSET) / 10_000_000
}

/// Давление памяти: худшее из загрузки физической памяти и выделения от лимита.
///
/// - `Critical` — загрузка ≥ 92 % или выделено ≥ 95 % лимита (рядом отказ в выделении памяти);
/// - `Warning` — загрузка ≥ 80 % или выделено ≥ 85 % лимита.
pub(crate) fn pressure_from(load_pct: u32, commit_pct: u32) -> Pressure {
    if load_pct >= 92 || commit_pct >= 95 {
        Pressure::Critical
    } else if load_pct >= 80 || commit_pct >= 85 {
        Pressure::Warning
    } else {
        Pressure::Normal
    }
}

/// `MEMORYSTATUSEX` в байтах.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MemStatus {
    pub load_pct: u32,
    pub total_phys: u64,
    pub avail_phys: u64,
    /// Лимит выделения (ОЗУ + файлы подкачки).
    pub commit_limit: u64,
    /// Сколько ещё можно выделить.
    pub commit_avail: u64,
}

/// Нужное из `PERFORMANCE_INFORMATION`, в байтах.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct PerfInfo {
    pub system_cache: u64,
    pub kernel_nonpaged: u64,
}

/// Раскладка памяти — чистая функция, чтобы проверять её тестами.
pub(crate) fn system_memory_from(ms: MemStatus, perf: PerfInfo, compressed: u64) -> SystemMemory {
    let total = ms.total_phys;
    let avail = ms.avail_phys.min(total);
    let used = total - avail;
    let cached_files = perf.system_cache.min(avail);
    let free = avail - cached_files;
    let wired = perf.kernel_nonpaged.min(used);
    let compressed = compressed.min(used - wired);
    let app = used - wired - compressed;
    let commit_used = ms.commit_limit.saturating_sub(ms.commit_avail);
    let commit_pct = commit_used.saturating_mul(100).checked_div(ms.commit_limit).map_or(0, |v| v.min(100) as u32);
    let swap_total = ms.commit_limit.saturating_sub(total);
    let swap_used = commit_used.saturating_sub(used).min(swap_total);
    let available_pct = avail.saturating_mul(100).checked_div(total).map_or(0, |v| v.min(100) as u8);
    SystemMemory {
        total,
        used,
        app,
        wired,
        compressed,
        cached_files,
        free,
        swap_total,
        swap_used,
        pressure: pressure_from(ms.load_pct, commit_pct),
        available_pct,
        counters: VmCounters::default(),
    }
}

/// Строка из UTF-16 до первого NUL.
pub(crate) fn wide_to_string(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

#[cfg(windows)]
pub use imp::WindowsPlatform;

#[cfg(windows)]
mod imp {
    use std::collections::{HashMap, HashSet};
    use std::mem::size_of;
    use std::sync::atomic::{AtomicU64, Ordering};

    use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessCommandLineInformation};
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, FILETIME, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM, STATUS_BUFFER_OVERFLOW,
        STATUS_BUFFER_TOO_SMALL, STATUS_INFO_LENGTH_MISMATCH, STILL_ACTIVE, UNICODE_STRING,
    };
    use windows_sys::Win32::Security::{GetLengthSid, GetTokenInformation, IsValidSid, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::System::ProcessStatus::{GetPerformanceInfo, GetProcessMemoryInfo, PERFORMANCE_INFORMATION, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX2};
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetExitCodeProcess, GetProcessTimes, OpenProcess, OpenProcessToken, PROCESS_ACCESS_RIGHTS, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, PROCESS_VM_READ, QueryFullProcessImageNameW, TerminateProcess,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GW_OWNER, GetWindow, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_CLOSE};
    use windows_sys::core::BOOL;

    use super::{MemStatus, PerfInfo, filetime_to_unix_secs, split_command_line, system_memory_from, wide_to_string};
    use crate::error::{Error, Result};
    use crate::group::app_group;
    use crate::model::{ProcState, ProcessInfo, SystemMemory};
    use crate::platform::{Platform, ProcessSample, Signal, WINDOWS_OWN_UID};

    /// Владеющая обёртка над HANDLE: закрывается при выходе из области видимости.
    struct Handle(HANDLE);

    impl Handle {
        fn process(pid: u32, access: PROCESS_ACCESS_RIGHTS) -> Option<Self> {
            // SAFETY: OpenProcess без указателей; NULL при отказе проверяется ниже.
            let h = unsafe { OpenProcess(access, 0, pid) };
            (!h.is_null()).then_some(Self(h))
        }
    }

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: хэндл получен от ОС, принадлежит только этой обёртке и закрывается один раз.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// Строка Toolhelp.
    struct Entry {
        pid: u32,
        ppid: u32,
        threads: u32,
        name: String,
    }

    #[derive(Clone)]
    struct Static {
        name: String,
        exe: Option<String>,
        args: Vec<String>,
        uid: u32,
        app_group: String,
    }

    /// Платформа Windows.
    pub struct WindowsPlatform {
        /// SID текущего пользователя (байты): свой процесс — тот, у кого SID владельца такой же.
        own_sid: Option<Vec<u8>>,
        /// Неизменное за жизнь процесса по (pid, started_at) — как на macOS.
        cache: HashMap<(i32, u64), Static>,
        /// Рабочий набор «Memory Compression» из последнего списка процессов.
        compressed: AtomicU64,
    }

    impl WindowsPlatform {
        /// Запоминает SID текущего пользователя.
        pub fn new() -> Result<Self> {
            // SAFETY: псевдохэндл текущего процесса, закрывать не нужно и нельзя.
            let me = unsafe { GetCurrentProcess() };
            Ok(Self { own_sid: token_sid(me), cache: HashMap::new(), compressed: AtomicU64::new(0) })
        }

        fn read_process(&mut self, e: &Entry) -> Option<ProcessSample> {
            let pid = i32::try_from(e.pid).ok().filter(|&p| p > 0)?;
            let handle = Handle::process(e.pid, PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ)
                .or_else(|| Handle::process(e.pid, PROCESS_QUERY_LIMITED_INFORMATION));
            let times = handle.as_ref().and_then(|h| process_times(h.0));
            let started_at = times.map_or(0, |t| t.0);
            let key = (pid, started_at);
            let st = match self.cache.get(&key) {
                Some(s) if s.name == e.name && (!s.args.is_empty() || s.exe.is_none()) => s.clone(),
                _ => {
                    let exe = handle.as_ref().and_then(|h| image_path(h.0));
                    let args = handle.as_ref().map(|h| command_line(h.0)).unwrap_or_default();
                    let sid = handle.as_ref().and_then(|h| token_sid(h.0));
                    let uid = match (&sid, &self.own_sid) {
                        (Some(a), Some(b)) if a == b => WINDOWS_OWN_UID,
                        _ => u32::MAX,
                    };
                    let app_group = app_group(&e.name, exe.as_deref());
                    let s = Static { name: e.name.clone(), exe, args, uid, app_group };
                    self.cache.insert(key, s.clone());
                    s
                }
            };
            let (footprint, rss) = handle.as_ref().and_then(|h| memory(h.0)).unwrap_or((None, 0));
            let Static { name, exe, args, uid, app_group } = st;
            Some(ProcessSample {
                info: ProcessInfo {
                    pid,
                    ppid: i32::try_from(e.ppid).unwrap_or(0),
                    name,
                    exe,
                    args,
                    uid,
                    footprint,
                    rss,
                    cpu_pct: 0.0,
                    threads: e.threads,
                    // Toolhelp не отдаёт состояние; живой процесс в списке — «работает».
                    state: if handle.is_some() { ProcState::Running } else { ProcState::Unknown },
                    started_at,
                    app_group,
                },
                cpu_time_ns: times.map_or(0, |t| t.1),
            })
        }
    }

    fn filetime(ft: FILETIME) -> u64 {
        (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
    }

    /// (запуск в unix-секундах, user + kernel время в нс).
    fn process_times(h: HANDLE) -> Option<(u64, u64)> {
        let (mut created, mut exited, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        // SAFETY: четыре указателя на живые FILETIME; хэндл открыт с QUERY_LIMITED_INFORMATION.
        let ok = unsafe { GetProcessTimes(h, &mut created, &mut exited, &mut kernel, &mut user) };
        (ok != 0).then(|| (filetime_to_unix_secs(filetime(created)), (filetime(kernel) + filetime(user)).saturating_mul(100)))
    }

    /// (`PrivateWorkingSetSize`, `WorkingSetSize`). До Windows 10 20H2 нет EX2 — тогда только rss.
    fn memory(h: HANDLE) -> Option<(Option<u64>, u64)> {
        let mut ex2 = PROCESS_MEMORY_COUNTERS_EX2 { cb: size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32, ..Default::default() };
        // SAFETY: буфер размера cb; EX2 начинается с полей PROCESS_MEMORY_COUNTERS, функция смотрит на cb.
        if unsafe { GetProcessMemoryInfo(h, (&mut ex2 as *mut PROCESS_MEMORY_COUNTERS_EX2).cast(), ex2.cb) } != 0 {
            return Some((Some(ex2.PrivateWorkingSetSize as u64), ex2.WorkingSetSize as u64));
        }
        let mut basic = PROCESS_MEMORY_COUNTERS { cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32, ..Default::default() };
        // SAFETY: то же для базовой структуры.
        (unsafe { GetProcessMemoryInfo(h, &mut basic, basic.cb) } != 0).then_some((None, basic.WorkingSetSize as u64))
    }

    fn image_path(h: HANDLE) -> Option<String> {
        let mut buf = vec![0u16; 4096];
        let mut len = buf.len() as u32;
        // SAFETY: буфер на len UTF-16 символов; функция пишет не больше и возвращает длину в len.
        let ok = unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) };
        (ok != 0).then(|| String::from_utf16_lossy(&buf[..(len as usize).min(buf.len())]))
    }

    /// Аргументы процесса через `ProcessCommandLineInformation`: ответ — UNICODE_STRING и сразу за
    /// ним сам текст в том же буфере.
    fn command_line(h: HANDLE) -> Vec<String> {
        // u64 — выравнивание под UNICODE_STRING (в нём указатель).
        let mut buf: Vec<u64> = vec![0; 512];
        for _ in 0..3 {
            let bytes = (buf.len() * size_of::<u64>()) as u32;
            let mut need = 0u32;
            // SAFETY: буфер на bytes байт, need — живая переменная; хэндл открыт с QUERY_LIMITED_INFORMATION.
            let status = unsafe { NtQueryInformationProcess(h, ProcessCommandLineInformation, buf.as_mut_ptr().cast(), bytes, &mut need) };
            if status >= 0 {
                // SAFETY: успех — в начале буфера лежит UNICODE_STRING; буфер выровнен под u64.
                let us = unsafe { &*(buf.as_ptr() as *const UNICODE_STRING) };
                let start = buf.as_ptr() as usize;
                let end = start + bytes as usize;
                let text = us.Buffer as usize;
                let len = usize::from(us.Length);
                // Текст должен лежать внутри нашего буфера — иначе не читаем.
                if us.Buffer.is_null() || text < start || text + len > end || text % 2 != 0 {
                    return Vec::new();
                }
                // SAFETY: проверено выше: [text, text + len) внутри живого буфера, выравнивание u16.
                let wide = unsafe { std::slice::from_raw_parts(us.Buffer, len / 2) };
                return split_command_line(&String::from_utf16_lossy(wide));
            }
            if status != STATUS_INFO_LENGTH_MISMATCH && status != STATUS_BUFFER_TOO_SMALL && status != STATUS_BUFFER_OVERFLOW {
                return Vec::new();
            }
            let want = (need as usize).div_ceil(size_of::<u64>()).max(buf.len() * 2);
            if want > 1 << 20 {
                return Vec::new();
            }
            buf.resize(want, 0);
        }
        Vec::new()
    }

    /// Байты SID владельца процесса (из его токена); `None`, если токен не открыть.
    fn token_sid(process: HANDLE) -> Option<Vec<u8>> {
        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: указатель на живую переменную под хэндл токена.
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 || token.is_null() {
            return None;
        }
        let token = Handle(token);
        // u64 — выравнивание под TOKEN_USER (в нём указатель); 512 байт хватает на любой SID.
        let mut buf = [0u64; 64];
        let mut len = 0u32;
        // SAFETY: буфер указанного размера; ядро пишет TOKEN_USER и SID за ним.
        let ok = unsafe { GetTokenInformation(token.0, TokenUser, buf.as_mut_ptr().cast(), size_of_val(&buf) as u32, &mut len) };
        if ok == 0 {
            return None;
        }
        // SAFETY: успех — в начале буфера TOKEN_USER; буфер выровнен под u64.
        let user = unsafe { &*(buf.as_ptr() as *const TOKEN_USER) };
        let sid = user.User.Sid;
        // SAFETY: sid указывает внутрь buf (живой), IsValidSid только читает.
        if sid.is_null() || unsafe { IsValidSid(sid) } == 0 {
            return None;
        }
        // SAFETY: SID валиден — GetLengthSid читает его заголовок.
        let n = unsafe { GetLengthSid(sid) } as usize;
        // SAFETY: SID длины n лежит в живом buf (ядро записало его туда целиком).
        Some(unsafe { std::slice::from_raw_parts(sid as *const u8, n) }.to_vec())
    }

    fn entries() -> Result<Vec<Entry>> {
        // SAFETY: без указателей; INVALID_HANDLE_VALUE при отказе проверяется ниже.
        let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snap == INVALID_HANDLE_VALUE || snap.is_null() {
            return Err(Error::syscall("CreateToolhelp32Snapshot"));
        }
        let snap = Handle(snap);
        let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut out = Vec::new();
        // SAFETY: e — живая структура с выставленным dwSize, снимок открыт.
        let mut ok = unsafe { Process32FirstW(snap.0, &mut e) };
        while ok != 0 {
            out.push(Entry { pid: e.th32ProcessID, ppid: e.th32ParentProcessID, threads: e.cntThreads, name: wide_to_string(&e.szExeFile) });
            // SAFETY: то же.
            ok = unsafe { Process32NextW(snap.0, &mut e) };
        }
        Ok(out)
    }

    /// Ошибка открытия процесса по коду Win32.
    fn open_error(pid: i32, call: &'static str) -> Error {
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            Some(c) if c == ERROR_INVALID_PARAMETER as i32 => Error::ProcessGone { pid },
            Some(c) if c == ERROR_ACCESS_DENIED as i32 => Error::Permission { pid, source: err },
            _ => Error::Syscall { call, source: err },
        }
    }

    fn terminate(pid: i32, pid_u: u32) -> Result<()> {
        let h = Handle::process(pid_u, PROCESS_TERMINATE).ok_or_else(|| open_error(pid, "OpenProcess"))?;
        // SAFETY: хэндл открыт с PROCESS_TERMINATE.
        if unsafe { TerminateProcess(h.0, 1) } == 0 {
            return Err(open_error(pid, "TerminateProcess"));
        }
        Ok(())
    }

    struct CloseCtx {
        pid: u32,
        posted: u32,
    }

    unsafe extern "system" fn post_close(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: lparam — адрес CloseCtx, живого на всё время EnumWindows (см. close_windows).
        let ctx = unsafe { &mut *(lparam as *mut CloseCtx) };
        let mut owner = 0u32;
        // SAFETY: hwnd пришёл от EnumWindows, owner — живая переменная.
        unsafe { GetWindowThreadProcessId(hwnd, &mut owner) };
        // SAFETY: функции только читают свойства окна, переданного EnumWindows.
        let main_window = owner == ctx.pid && unsafe { IsWindowVisible(hwnd) } != 0 && unsafe { GetWindow(hwnd, GW_OWNER) }.is_null();
        // SAFETY: асинхронная отправка сообщения окну; указателей нет.
        if main_window && unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) } != 0 {
            ctx.posted += 1;
        }
        1
    }

    /// Послать WM_CLOSE главным окнам процесса; сколько окон получили.
    fn close_windows(pid: u32) -> u32 {
        let mut ctx = CloseCtx { pid, posted: 0 };
        // SAFETY: колбэк живёт всю программу, ctx — до конца вызова (EnumWindows синхронна).
        unsafe { EnumWindows(Some(post_close), &mut ctx as *mut CloseCtx as LPARAM) };
        ctx.posted
    }

    impl Platform for WindowsPlatform {
        fn system_memory(&self) -> Result<SystemMemory> {
            let mut ms = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
            // SAFETY: живая структура с выставленным dwLength.
            if unsafe { GlobalMemoryStatusEx(&mut ms) } == 0 {
                return Err(Error::syscall("GlobalMemoryStatusEx"));
            }
            let mut pi = PERFORMANCE_INFORMATION { cb: size_of::<PERFORMANCE_INFORMATION>() as u32, ..Default::default() };
            // SAFETY: живая структура размера cb.
            let perf = if unsafe { GetPerformanceInfo(&mut pi, pi.cb) } != 0 {
                let page = pi.PageSize as u64;
                PerfInfo { system_cache: pi.SystemCache as u64 * page, kernel_nonpaged: pi.KernelNonpaged as u64 * page }
            } else {
                PerfInfo::default()
            };
            let status = MemStatus {
                load_pct: ms.dwMemoryLoad,
                total_phys: ms.ullTotalPhys,
                avail_phys: ms.ullAvailPhys,
                commit_limit: ms.ullTotalPageFile,
                commit_avail: ms.ullAvailPageFile,
            };
            Ok(system_memory_from(status, perf, self.compressed.load(Ordering::Relaxed)))
        }

        fn processes(&mut self) -> Result<Vec<ProcessSample>> {
            let list = entries()?;
            let out: Vec<ProcessSample> = list.iter().filter_map(|e| self.read_process(e)).collect();
            let compressed = out.iter().find(|s| s.info.name == "Memory Compression").map_or(0, |s| s.info.rss);
            self.compressed.store(compressed, Ordering::Relaxed);
            let alive: HashSet<(i32, u64)> = out.iter().map(|s| (s.info.pid, s.info.started_at)).collect();
            self.cache.retain(|k, _| alive.contains(k));
            Ok(out)
        }

        fn process(&mut self, pid: i32) -> Result<Option<ProcessInfo>> {
            // Для сверки личности перед снятием — всегда свежее чтение, мимо кэша.
            self.cache.retain(|k, _| k.0 != pid);
            let Ok(pid_u) = u32::try_from(pid) else { return Ok(None) };
            Ok(entries()?.iter().find(|e| e.pid == pid_u).and_then(|e| self.read_process(e)).map(|s| s.info))
        }

        fn signal(&mut self, pid: i32, signal: Signal) -> Result<()> {
            let pid_u = u32::try_from(pid).map_err(|_| Error::ProcessGone { pid })?;
            match signal {
                Signal::Term if close_windows(pid_u) > 0 => Ok(()),
                // Окон нет (консольная программа, служба) — вежливо попросить нечем.
                Signal::Term | Signal::Kill => terminate(pid, pid_u),
            }
        }

        fn alive(&mut self, pid: i32) -> Result<bool> {
            let Ok(pid_u) = u32::try_from(pid) else { return Ok(false) };
            let Some(h) = Handle::process(pid_u, PROCESS_QUERY_LIMITED_INFORMATION) else {
                // Нет прав — процесс есть, просто не наш; иначе его нет.
                return Ok(std::io::Error::last_os_error().raw_os_error() == Some(ERROR_ACCESS_DENIED as i32));
            };
            let mut code = 0u32;
            // SAFETY: хэндл открыт с QUERY_LIMITED_INFORMATION, code — живая переменная.
            if unsafe { GetExitCodeProcess(h.0, &mut code) } == 0 {
                return Ok(true);
            }
            Ok(code == STILL_ACTIVE as u32)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_like_command_line_to_argv() {
        assert_eq!(split_command_line(r#""C:\Program Files\nodejs\node.exe" server.js --port 3000"#), vec![r"C:\Program Files\nodejs\node.exe", "server.js", "--port", "3000"]);
        assert_eq!(split_command_line(r"C:\x\a.exe"), vec![r"C:\x\a.exe"]);
        assert_eq!(split_command_line(r#"a.exe "two words" x"#), vec!["a.exe", "two words", "x"]);
        assert_eq!(split_command_line(r#"a.exe a\\\"b c\\"d e" f\g"#), vec!["a.exe", r#"a\"b"#, r"c\d e", r"f\g"]);
        assert_eq!(split_command_line(r#"a.exe "say ""hi""""#), vec!["a.exe", r#"say "hi""#]);
        assert_eq!(split_command_line(r#"a.exe """#), vec!["a.exe", ""]);
        assert_eq!(split_command_line("  a.exe\t b  "), vec!["a.exe", "b"]);
        assert!(split_command_line("   ").is_empty());
        assert_eq!(split_command_line(r#""C:\a b\x.exe""#), vec![r"C:\a b\x.exe"]);
    }

    #[test]
    fn filetime_epoch() {
        assert_eq!(filetime_to_unix_secs(FILETIME_UNIX_OFFSET), 0);
        assert_eq!(filetime_to_unix_secs(FILETIME_UNIX_OFFSET + 17_000_000_000_000_000), 1_700_000_000);
        assert_eq!(filetime_to_unix_secs(5), 0);
    }

    #[test]
    fn memory_layout() {
        let g = 1024 * 1024 * 1024;
        let ms = MemStatus { load_pct: 75, total_phys: 16 * g, avail_phys: 4 * g, commit_limit: 24 * g, commit_avail: 10 * g };
        let perf = PerfInfo { system_cache: 3 * g, kernel_nonpaged: g / 2 };
        let s = system_memory_from(ms, perf, g);
        assert_eq!(s.used, 12 * g);
        assert_eq!(s.cached_files, 3 * g);
        assert_eq!(s.free, g);
        assert_eq!(s.wired, g / 2);
        assert_eq!(s.compressed, g);
        assert_eq!(s.app + s.wired + s.compressed, s.used, "инвариант модели");
        assert_eq!(s.swap_total, 8 * g);
        assert_eq!(s.swap_used, 2 * g, "выделено 14 ГБ, в ОЗУ 12");
        assert_eq!(s.available_pct, 25);
        assert_eq!(s.pressure, Pressure::Normal);
        let tight = MemStatus { commit_avail: g / 2, ..ms };
        assert_eq!(system_memory_from(tight, perf, 0).pressure, Pressure::Critical, "упор в лимит выделения");
        let empty = system_memory_from(MemStatus::default(), PerfInfo { system_cache: g, kernel_nonpaged: g }, g);
        assert_eq!((empty.used, empty.cached_files, empty.available_pct), (0, 0, 0));
    }

    #[test]
    fn pressure_thresholds() {
        assert_eq!(pressure_from(50, 50), Pressure::Normal);
        assert_eq!(pressure_from(80, 50), Pressure::Warning);
        assert_eq!(pressure_from(50, 85), Pressure::Warning);
        assert_eq!(pressure_from(92, 0), Pressure::Critical);
        assert_eq!(pressure_from(0, 95), Pressure::Critical);
    }

    #[test]
    fn wide_strings() {
        let mut w: Vec<u16> = "chrome.exe".encode_utf16().collect();
        w.extend([0, 65, 66]);
        assert_eq!(wide_to_string(&w), "chrome.exe");
        assert_eq!(wide_to_string(&[]), "");
    }
}

//! Платформенный слой: трейт [`Platform`] и его реализации.
//!
//! Всё выше трейта платформонезависимо. Сейчас есть только macOS
//! ([`macos::MacOsPlatform`]); на других ОС крейт собирается, а [`current`]
//! возвращает [`Error::UnsupportedPlatform`] во время работы.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[cfg(not(target_os = "macos"))]
use crate::error::Error;
use crate::error::Result;
use crate::model::{ProcessInfo, SystemMemory};

#[cfg(target_os = "macos")]
pub mod macos;

/// Сигнал снятия.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum Signal {
    /// SIGTERM — вежливая просьба завершиться.
    #[serde(rename = "SIGTERM")]
    Term,
    /// SIGKILL — снятие без возможности отказаться.
    #[serde(rename = "SIGKILL")]
    Kill,
}

impl Signal {
    /// Имя для человека и журнала.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Term => "SIGTERM",
            Self::Kill => "SIGKILL",
        }
    }
}

/// Процесс вместе с накопленным процессорным временем.
///
/// `cpu_pct` считает [`crate::collect::Collector`] по разнице двух снимков, поэтому
/// платформенный слой отдаёт сырое время, а не проценты.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessSample {
    /// Всё, что видно о процессе.
    pub info: ProcessInfo,
    /// user + system время процесса, наносекунды.
    pub cpu_time_ns: u64,
}

/// Источник цифр: память системы, процессы и сигналы.
pub trait Platform: Send {
    /// Память системы целиком.
    fn system_memory(&self) -> Result<SystemMemory>;

    /// Все процессы с их процессорным временем.
    fn processes(&mut self) -> Result<Vec<ProcessSample>>;

    /// Один процесс: нужен для сверки личности перед сигналом.
    fn process(&mut self, pid: i32) -> Result<Option<ProcessInfo>>;

    /// Послать сигнал.
    fn signal(&mut self, pid: i32, signal: Signal) -> Result<()>;

    /// Жив ли процесс (EPERM — жив, просто не наш).
    fn alive(&mut self, pid: i32) -> Result<bool>;

    /// Рабочий каталог процесса; `None`, если нет прав или ОС не отдаёт.
    fn cwd(&mut self, _pid: i32) -> Result<Option<String>> {
        Ok(None)
    }
}

/// Реализация платформы, на которой мы работаем.
pub fn current() -> Result<Box<dyn Platform>> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(macos::MacOsPlatform::new()?))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(Error::UnsupportedPlatform(std::env::consts::OS))
    }
}

/// Есть ли реализация под текущую ОС.
pub fn supported() -> bool {
    cfg!(target_os = "macos")
}

/// Наибольший pid ядра и init: их не снимают никогда (launchd/systemd — 1, Windows System — 4).
pub const LAST_KERNEL_PID: i32 = if cfg!(windows) { 4 } else { 1 };

/// Идентификатор владельца текущего процесса. На Windows платформа заполняет
/// `ProcessInfo::uid` так, чтобы свои процессы совпадали с этим значением.
pub fn current_uid() -> u32 {
    #[cfg(unix)]
    // SAFETY: geteuid не имеет аргументов и не может упасть — только читает поле ядра.
    unsafe {
        libc::geteuid()
    }
    #[cfg(not(unix))]
    {
        0
    }
}

//! Платформенный слой: трейт [`Platform`] и его реализации.
//!
//! Всё выше трейта платформонезависимо. Сейчас есть только macOS
//! ([`macos::MacOsPlatform`]); на других ОС крейт собирается, а [`current`]
//! возвращает [`Error::UnsupportedPlatform`] во время работы.

use serde::{Deserialize, Serialize};

#[cfg(not(target_os = "macos"))]
use crate::error::Error;
use crate::error::Result;
use crate::model::{ProcessInfo, SystemMemory};

#[cfg(target_os = "macos")]
pub mod macos;

/// Сигнал снятия.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Signal {
    /// SIGTERM — вежливая просьба завершиться.
    Term,
    /// SIGKILL — снятие без возможности отказаться.
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

/// Идентификатор владельца текущего процесса.
pub fn current_uid() -> u32 {
    #[cfg(target_os = "macos")]
    // SAFETY: geteuid не имеет аргументов и не может упасть — только читает поле ядра.
    unsafe {
        libc::geteuid()
    }
    #[cfg(not(target_os = "macos"))]
    {
        0
    }
}

//! Ошибки ядра. Внятные человеку, с причиной отказа защитного слоя.

use std::path::PathBuf;

/// Ошибка ядра `rmon`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Платформа без реализации: на не-macOS сборка проходит, работа — нет.
    #[error("платформа пока не поддерживается: {0}")]
    UnsupportedPlatform(&'static str),

    /// Системный вызов вернул ошибку.
    #[error("системный вызов {call} не удался: {source}")]
    Syscall {
        call: &'static str,
        #[source]
        source: std::io::Error,
    },

    /// Ответ системы не разобрался (неожидаемый размер, битые данные).
    #[error("не удалось прочитать {what}: {detail}")]
    Invalid { what: &'static str, detail: String },

    /// Процесс исчез между снимком и действием.
    #[error("процесс {pid} уже завершился")]
    ProcessGone { pid: i32 },

    /// На месте снимка оказался другой процесс: pid переиспользован.
    #[error("процесс {pid} подменился: ожидался запуск {expected}, сейчас {actual} — снятие отменено")]
    IdentityMismatch { pid: i32, expected: u64, actual: u64 },

    /// Политика запрещает снятие при любом подтверждении.
    #[error("снятие {pid} ({name}) запрещено: {reason}")]
    Protected { pid: i32, name: String, reason: String },

    /// Политика требует явного подтверждения.
    #[error("снятие {pid} ({name}) требует подтверждения: {reason}; повторите с confirm")]
    ConfirmationRequired { pid: i32, name: String, reason: String },

    /// Нет прав на сигнал: чужой или root-процесс.
    #[error("нет прав на снятие {pid}: {source}")]
    Permission {
        pid: i32,
        #[source]
        source: std::io::Error,
    },

    /// Файл настроек не читается или не разбирается.
    #[error("настройки {path}: {detail}")]
    Config { path: PathBuf, detail: String },

    /// Журнал действий недоступен.
    #[error("журнал {path}: {source}")]
    Journal {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl Error {
    /// Ошибка системного вызова с именем вызова — единственный способ узнать, что именно упало.
    pub fn syscall(call: &'static str) -> Self {
        Self::Syscall { call, source: std::io::Error::last_os_error() }
    }

    /// Ошибка ввода-вывода без errno (файлы журнала, пути).
    pub fn journal(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Journal { path: path.into(), source }
    }
}

/// Результат операций ядра.
pub type Result<T> = std::result::Result<T, Error>;

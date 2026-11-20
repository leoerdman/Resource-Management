//! Настройки и пути — одно место для CLI, MCP и окна.
//!
//! Файл `config.toml` необязателен: без него действуют значения по умолчанию.
//! Где он лежит — [`config_file`]; переопределение — env `RMON_CONFIG`.
//!
//! ```toml
//! [thresholds]
//! hog_bytes = 2147483648        # 2 ГБ
//!
//! [[policy.protect]]            # добавить «никогда не трогать»
//! name = "мой сервер"
//! args = "my-server\\.js"
//!
//! [[policy.confirm]]            # добавить «только с подтверждением»
//! name = "Obsidian"
//! path = "/Obsidian\\.app/"
//!
//! [guard]                       # сторож памяти (см. rmon_core::guard)
//! enabled = true
//! dry_run = false
//! ```
//!
//! Конфиг может только ужесточить политику: сделать процесс `Free` из него нельзя.

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, RwLock};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::analyze::Thresholds;
use crate::error::{Error, Result};

/// Правило политики — та же форма, что у `never`/`allow` в `memory-guard/config.json`.
/// Все заданные поля должны совпасть (регулярные выражения); пустое правило не совпадает ни с чем.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct RuleSpec {
    /// Имя правила — оно же причина для человека.
    pub name: String,
    /// По имени процесса или имени исполняемого файла.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exe: Option<String>,
    /// По полному пути исполняемого файла.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// По строке аргументов.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
}

/// Расширение политики снятия.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PolicyConfig {
    /// Дополнительные «никогда».
    pub protect: Vec<RuleSpec>,
    /// Дополнительные «только с подтверждением».
    pub confirm: Vec<RuleSpec>,
}

/// Переопределение путей.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PathsConfig {
    /// Журнал действий rmon.
    pub actions_log: Option<PathBuf>,
    /// Журнал событий старого memory-guard (для истории во вкладке «Журнал»).
    pub legacy_guard_log: Option<PathBuf>,
}

/// Настройки rmon.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Пороги анализа.
    pub thresholds: Thresholds,
    /// Политика снятия.
    pub policy: PolicyConfig,
    /// Пути.
    pub paths: PathsConfig,
    /// Сторож памяти.
    pub guard: crate::guard::GuardConfig,
    /// Приложение: автозапуск, значок.
    pub app: crate::settings::AppConfig,
}

impl Config {
    /// Прочитать [`config_file`]; нет файла — значения по умолчанию.
    pub fn load() -> Result<Self> {
        Self::load_from(&config_file())
    }

    /// Прочитать конкретный файл; нет файла — значения по умолчанию.
    pub fn load_from(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text).map_err(|detail| Error::Config { path: path.to_path_buf(), detail }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(Error::Config { path: path.to_path_buf(), detail: e.to_string() }),
        }
    }

    /// Разобрать текст TOML.
    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }
}

/// Действующие настройки процесса и их поколение (растёт при каждой замене).
struct Current {
    config: Arc<Config>,
    generation: u64,
}

static CURRENT: LazyLock<RwLock<Current>> = LazyLock::new(|| {
    let config = Config::load().unwrap_or_else(|e| {
        eprintln!("rmon: {e}; действуют настройки по умолчанию");
        Config::default()
    });
    RwLock::new(Current { config: Arc::new(config), generation: 0 })
});

/// Действующие настройки процесса. Читаются из файла при первом обращении; ошибка файла —
/// в stderr и значения по умолчанию (показать её явно — [`Config::load`], так делает `rmon config`).
/// Меняются через [`set_global`] — тогда же пересобирается политика снятия.
pub fn global() -> Arc<Config> {
    CURRENT.read().unwrap_or_else(|e| e.into_inner()).config.clone()
}

/// Поколение настроек: кэши, построенные из них (политика), сверяют его и пересобираются.
pub fn generation() -> u64 {
    CURRENT.read().unwrap_or_else(|e| e.into_inner()).generation
}

/// Заменить действующие настройки (после сохранения из окна или перечитывания файла).
pub fn set_global(config: Config) {
    let mut cur = CURRENT.write().unwrap_or_else(|e| e.into_inner());
    cur.config = Arc::new(config);
    cur.generation += 1;
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Домашний каталог пользователя.
pub fn home_dir() -> PathBuf {
    env_path("HOME").or_else(|| env_path("USERPROFILE")).unwrap_or_else(std::env::temp_dir)
}

/// Каталог настроек: `~/.config/rmon` (macOS по SPEC и Linux с учётом `XDG_CONFIG_HOME`),
/// `%APPDATA%\rmon` на Windows.
pub fn config_dir() -> PathBuf {
    if cfg!(windows) {
        return env_path("APPDATA").unwrap_or_else(home_dir).join("rmon");
    }
    env_path("XDG_CONFIG_HOME").unwrap_or_else(|| home_dir().join(".config")).join("rmon")
}

/// Файл настроек (`RMON_CONFIG` переопределяет).
pub fn config_file() -> PathBuf {
    env_path("RMON_CONFIG").unwrap_or_else(|| config_dir().join("config.toml"))
}

/// Каталог журналов: `~/Library/Logs/rmon`, `$XDG_STATE_HOME/rmon`, `%LOCALAPPDATA%\rmon\logs`.
pub fn log_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        home_dir().join("Library/Logs/rmon")
    } else if cfg!(windows) {
        env_path("LOCALAPPDATA").unwrap_or_else(home_dir).join("rmon").join("logs")
    } else {
        env_path("XDG_STATE_HOME").unwrap_or_else(|| home_dir().join(".local/state")).join("rmon")
    }
}

/// Журнал действий: env `RMON_ACTIONS_LOG` → конфиг → [`log_dir`]`/actions.jsonl`.
pub fn actions_log() -> PathBuf {
    env_path("RMON_ACTIONS_LOG")
        .or_else(|| global().paths.actions_log.clone())
        .unwrap_or_else(|| log_dir().join("actions.jsonl"))
}

/// Журнал событий старого memory-guard: env `RMON_GUARD_LOG` → конфиг → его каталог.
/// Только для истории: сторож теперь в rmon и пишет в [`crate::guard::runner::events_log`].
pub fn legacy_guard_log() -> PathBuf {
    env_path("RMON_GUARD_LOG")
        .or_else(|| global().paths.legacy_guard_log.clone())
        .unwrap_or_else(|| home_dir().join("Programming - Main/Hooks/memory-guard/log/events.jsonl"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_partial_config() {
        let c = Config::parse(
            r#"
            [thresholds]
            hog_bytes = 2147483648
            [[policy.confirm]]
            name = "Obsidian"
            path = "/Obsidian\\.app/"
            "#,
        )
        .unwrap();
        assert_eq!(c.thresholds.hog_bytes, 2 * 1024 * 1024 * 1024);
        assert_eq!(c.thresholds.duplicate_count, Thresholds::default().duplicate_count);
        assert_eq!(c.policy.confirm[0].name, "Obsidian");
        assert!(c.guard.enabled);
        assert_eq!(c.guard.sustain_secs, 12, "сторож по умолчанию — как memory-guard");
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(Config::parse("[policy]\nfree = []").is_err(), "сделать процесс свободным конфигом нельзя");
    }
}

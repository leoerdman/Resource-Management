//! Аргументы инструментов: схемы для модели и проверка значений.
//!
//! Тип не совпал (`pid: "abc"`) — ошибку даёт разбор rmcp (`invalid_params`); значение вне
//! допустимого (`limit: 0`, пустая `reason`) — [`Validate::validate`], тоже `invalid_params`.

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::Deserialize;

/// Проверка значений после разбора.
pub trait Validate {
    /// Текст ошибки по-английски для модели (как и описания инструментов).
    fn check(&self) -> Result<(), String>;

    /// То же как ошибка протокола `invalid_params`.
    fn validate(&self) -> Result<(), ErrorData> {
        self.check().map_err(|e| ErrorData::invalid_params(e, None))
    }
}

fn check_pid(pid: i32) -> Result<(), String> {
    if pid <= 0 { Err(format!("pid must be a positive integer, got {pid}")) } else { Ok(()) }
}

fn check_range(name: &str, v: u32, min: u32, max: u32) -> Result<(), String> {
    if (min..=max).contains(&v) { Ok(()) } else { Err(format!("{name} must be in {min}..={max}, got {v}")) }
}

/// Порядок строк `list_processes`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SortKey {
    /// Memory footprint, largest first.
    #[default]
    Mem,
    /// CPU usage, highest first.
    Cpu,
    /// Memory growth rate over the history window, fastest first.
    Growth,
    /// Process id, ascending.
    Pid,
}

impl SortKey {
    /// Имя как в схеме.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mem => "mem",
            Self::Cpu => "cpu",
            Self::Growth => "growth",
            Self::Pid => "pid",
        }
    }
}

fn default_list_limit() -> u32 {
    25
}

/// `list_processes`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ListArgs {
    /// Sort order: mem (default), cpu, growth, pid.
    #[serde(default)]
    pub sort: SortKey,
    /// How many rows to return, 1..=500. Default 25.
    #[serde(default = "default_list_limit")]
    #[schemars(range(min = 1, max = 500))]
    pub limit: u32,
    /// Case-insensitive substring matched against process name, app name, executable path and
    /// command line; a number also matches the pid exactly.
    #[serde(default)]
    pub filter: Option<String>,
    /// Aggregate rows by application (all helper processes of an app summed) instead of listing processes.
    #[serde(default)]
    pub group_by_app: bool,
}

impl Default for ListArgs {
    fn default() -> Self {
        Self { sort: SortKey::default(), limit: default_list_limit(), filter: None, group_by_app: false }
    }
}

impl Validate for ListArgs {
    fn check(&self) -> Result<(), String> {
        check_range("limit", self.limit, 1, 500)
    }
}

/// `process_details`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DetailsArgs {
    /// Process id.
    pub pid: i32,
}

impl Validate for DetailsArgs {
    fn check(&self) -> Result<(), String> {
        check_pid(self.pid)
    }
}

fn default_min_score() -> u32 {
    30
}

/// `suggest_cleanup`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CleanupArgs {
    /// Minimum suspect score, 0..=100. Default 30.
    #[serde(default = "default_min_score")]
    #[schemars(range(max = 100))]
    pub min_score: u32,
}

impl Validate for CleanupArgs {
    fn check(&self) -> Result<(), String> {
        check_range("min_score", self.min_score, 0, 100)
    }
}

/// `kill_process`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct KillArgs {
    /// Process id to terminate.
    pub pid: i32,
    /// Why the process should be terminated (recorded in the action journal). Required, non-empty.
    pub reason: String,
    /// Send SIGKILL right after SIGTERM instead of waiting 5 seconds for a graceful exit.
    #[serde(default)]
    pub hard: bool,
    /// Explicit user consent. Required for Confirm-policy processes; set it to true ONLY after the
    /// user has agreed to terminate this specific process in the conversation.
    #[serde(default)]
    pub confirm: bool,
    /// Only check what would happen (policy, memory to be freed); send no signal.
    #[serde(default)]
    pub dry_run: bool,
}

impl Validate for KillArgs {
    fn check(&self) -> Result<(), String> {
        check_pid(self.pid)?;
        if self.reason.trim().is_empty() {
            return Err("reason must be a non-empty string".into());
        }
        Ok(())
    }
}

fn default_guard_limit() -> u32 {
    20
}

/// `guard_events`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GuardArgs {
    /// How many recent events to return, 1..=200. Default 20.
    #[serde(default = "default_guard_limit")]
    #[schemars(range(min = 1, max = 200))]
    pub limit: u32,
}

impl Validate for GuardArgs {
    fn check(&self) -> Result<(), String> {
        check_range("limit", self.limit, 1, 200)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse<T: serde::de::DeserializeOwned>(v: serde_json::Value) -> Result<T, String> {
        serde_json::from_value(v).map_err(|e| e.to_string())
    }

    #[test]
    fn list_defaults_and_sort_values() {
        let a: ListArgs = parse(json!({})).unwrap();
        assert_eq!((a.sort, a.limit, a.filter.is_none(), a.group_by_app), (SortKey::Mem, 25, true, false));
        assert!(a.check().is_ok());
        for (s, k) in [("mem", SortKey::Mem), ("cpu", SortKey::Cpu), ("growth", SortKey::Growth), ("pid", SortKey::Pid)]
        {
            let a: ListArgs = parse(json!({"sort": s, "limit": 3, "filter": "node", "group_by_app": true})).unwrap();
            assert_eq!(a.sort, k);
            assert_eq!(a.sort.as_str(), s);
        }
        assert!(parse::<ListArgs>(json!({"sort": "rss"})).is_err(), "неизвестный порядок — ошибка разбора");
        assert!(parse::<ListArgs>(json!({"limit": -1})).is_err());
        assert!(parse::<ListArgs>(json!({"limit": 0})).unwrap().check().is_err());
        assert!(parse::<ListArgs>(json!({"limit": 501})).unwrap().check().is_err());
        assert!(parse::<ListArgs>(json!({"limit": 500})).unwrap().check().is_ok());
    }

    #[test]
    fn kill_requires_pid_and_reason() {
        let a: KillArgs = parse(json!({"pid": 42, "reason": "leak"})).unwrap();
        assert!(!a.hard && !a.confirm && !a.dry_run);
        assert!(a.check().is_ok());
        assert!(parse::<KillArgs>(json!({"pid": "abc", "reason": "x"})).is_err(), "pid строкой — ошибка разбора");
        assert!(parse::<KillArgs>(json!({"pid": 42})).is_err(), "reason обязателен");
        assert!(parse::<KillArgs>(json!({"pid": 42, "reason": "  "})).unwrap().check().is_err());
        assert!(parse::<KillArgs>(json!({"pid": 0, "reason": "x"})).unwrap().check().is_err());
        assert!(parse::<KillArgs>(json!({"pid": -5, "reason": "x"})).unwrap().check().is_err());
        let a: KillArgs =
            parse(json!({"pid": 7, "reason": "x", "confirm": true, "hard": true, "dry_run": true})).unwrap();
        assert!(a.confirm && a.hard && a.dry_run);
        let err = parse::<KillArgs>(json!({"pid": 0, "reason": "x"})).unwrap().validate().unwrap_err();
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }

    #[test]
    fn other_tools_validate_ranges() {
        assert_eq!(parse::<CleanupArgs>(json!({})).unwrap().min_score, 30);
        assert!(parse::<CleanupArgs>(json!({"min_score": 101})).unwrap().check().is_err());
        assert!(parse::<CleanupArgs>(json!({"min_score": 0})).unwrap().check().is_ok());
        assert_eq!(parse::<GuardArgs>(json!({})).unwrap().limit, 20);
        assert!(parse::<GuardArgs>(json!({"limit": 0})).unwrap().check().is_err());
        assert!(parse::<DetailsArgs>(json!({})).is_err(), "pid обязателен");
        assert!(parse::<DetailsArgs>(json!({"pid": 1})).unwrap().check().is_ok());
    }
}

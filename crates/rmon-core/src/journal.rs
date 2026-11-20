//! Журналы: действия rmon (`~/Library/Logs/rmon/actions.jsonl`) и события memory-guard.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::config;
use crate::error::{Error, Result};

/// Путь журнала действий — см. [`config::actions_log`].
pub fn actions_path() -> PathBuf {
    config::actions_log()
}

/// Путь журнала memory-guard — см. [`config::guard_log`].
pub fn guard_path() -> PathBuf {
    config::guard_log()
}

/// Дописать строку в журнал действий.
pub fn append_action(entry: &serde_json::Value) -> Result<()> {
    let path = actions_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| Error::journal(dir, e))?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(&path).map_err(|e| Error::journal(&path, e))?;
    writeln!(f, "{entry}").map_err(|e| Error::journal(&path, e))
}

/// Откуда запись.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, rename = "JournalSource")]
pub enum Source {
    /// Действия rmon.
    #[serde(rename = "rmon")]
    Rmon,
    /// Сторож памяти.
    #[serde(rename = "memory-guard")]
    MemoryGuard,
}

/// Запись журнала для человека.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct JournalEntry {
    /// Источник.
    pub source: Source,
    /// Время, unix-секунды.
    #[ts(type = "number | null")]
    pub ts: Option<u64>,
    /// Одна строка по-русски.
    pub text: String,
    /// Исходная запись.
    #[ts(type = "unknown")]
    pub raw: serde_json::Value,
}

/// Последние `limit` записей обоих журналов, новые сверху.
pub fn read_recent(limit: usize) -> Vec<JournalEntry> {
    let mut out: Vec<JournalEntry> = tail_lines(&actions_path(), limit)
        .into_iter()
        .map(|raw| JournalEntry { source: Source::Rmon, ts: raw["ts"].as_u64(), text: action_text(&raw), raw })
        .chain(tail_lines(&guard_path(), limit).into_iter().map(|raw| JournalEntry {
            source: Source::MemoryGuard,
            ts: raw["t"].as_str().and_then(parse_iso),
            text: guard_text(&raw),
            raw,
        }))
        .collect();
    out.sort_by_key(|e| std::cmp::Reverse(e.ts));
    out.truncate(limit);
    out
}

fn tail_lines(path: &std::path::Path, limit: usize) -> Vec<serde_json::Value> {
    let Ok(text) = fs::read_to_string(path) else { return Vec::new() };
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(limit);
    lines[start..].iter().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

fn action_text(v: &serde_json::Value) -> String {
    let who = v["requested_by"].as_str().unwrap_or("?");
    let name = v["name"].as_str().unwrap_or("?");
    let pid = v["pid"].as_i64().unwrap_or(0);
    if let Some(err) = v["error"].as_str() {
        return format!("[{who}] отказ: {err}");
    }
    let what = if v["dry_run"].as_bool() == Some(true) {
        "проба снятия"
    } else if v["exited"].as_bool() == Some(true) {
        "снят"
    } else {
        "не завершился"
    };
    let freed = v["reclaimed"].as_u64().unwrap_or(0);
    format!("[{who}] {name} ({pid}) {what}, {}", crate::fmt::bytes(freed))
}

fn guard_text(v: &serde_json::Value) -> String {
    let ev = v["ev"].as_str().unwrap_or("?");
    match ev {
        "zone" => format!(
            "зона {}: {} (свободно {} %, своп {} МБ)",
            v["zone"].as_str().unwrap_or("?"),
            v["why"].as_str().unwrap_or(""),
            v["availablePct"].as_i64().unwrap_or(0),
            v["swapUsedMb"].as_i64().unwrap_or(0)
        ),
        "kill" | "killed" => format!(
            "сторож снял {} ({}) — {}",
            v["name"].as_str().or(v["exe"].as_str()).unwrap_or("?"),
            v["pid"].as_i64().unwrap_or(0),
            v["why"].as_str().unwrap_or("")
        ),
        _ => {
            let mut s = ev.to_string();
            if let Some(why) = v["why"].as_str() {
                s.push_str(": ");
                s.push_str(why);
            }
            s
        }
    }
}

/// «2026-11-20T09:48:46.625Z» → unix-секунды (только UTC с «Z»).
fn parse_iso(s: &str) -> Option<u64> {
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>());
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let time = time.trim_end_matches('Z');
    let mut t = time.split(':');
    let (hh, mm) = (t.next()?.parse::<i64>().ok()?, t.next()?.parse::<i64>().ok()?);
    let ss = t.next()?.split('.').next()?.parse::<i64>().ok()?;
    // Дни от эпохи (алгоритм Хиннанта).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + hh * 3600 + mm * 60 + ss).ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_iso() {
        assert_eq!(super::parse_iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(super::parse_iso("2026-11-20T09:48:46.625Z"), Some(1_795_168_126));
    }
}

//! Журналы: действия rmon (`actions.jsonl`) и события сторожа памяти (`guard.jsonl`).

use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::config;
use crate::error::{Error, Result};

/// Путь журнала действий — см. [`config::actions_log`].
pub fn actions_path() -> PathBuf {
    config::actions_log()
}

/// Дописать строку в журнал действий.
pub fn append_action(entry: &serde_json::Value) -> Result<()> {
    let path = actions_path();
    append_retained(&path, &entry.to_string()).map_err(|e| Error::journal(&path, e))
}

/// Как часто проверять, не пора ли выбросить старые записи (проверка — чтение первой строки).
const PRUNE_CHECK_EVERY: u64 = 10 * 60;

/// Дописать строку JSONL в журнал с хранением по времени ([`config::LogsConfig`]).
///
/// Каждая строка — объект с полем `ts` (unix-секунды). Записи старше `retention_hours`
/// выбрасываются при дозаписи (не чаще раза в 10 минут на журнал — по первой строке видно,
/// есть ли что выбрасывать); если файл перерос `max_mb`, выбрасываются самые старые записи
/// до половины предела. Перезапись атомарная (временный файл + rename).
pub fn append_retained(path: &Path, line: &str) -> std::io::Result<()> {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};
    static CHECKED: LazyLock<Mutex<HashMap<PathBuf, u64>>> = LazyLock::new(Default::default);

    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let now = crate::collect::unix_now();
    let due = {
        let mut checked = CHECKED.lock().unwrap_or_else(|e| e.into_inner());
        let last = checked.get(path).copied().unwrap_or(0);
        let due = now.saturating_sub(last) >= PRUNE_CHECK_EVERY;
        if due {
            checked.insert(path.to_path_buf(), now);
        }
        due
    };
    if due {
        let logs = config::global().logs.clone();
        prune(path, now.saturating_sub(logs.retention_hours.max(1) * 3600), logs.max_mb.max(1) * 1024 * 1024)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{line}")
}

fn line_ts(line: &str) -> Option<u64> {
    #[derive(Deserialize)]
    struct Ts {
        ts: Option<u64>,
    }
    serde_json::from_str::<Ts>(line).ok()?.ts
}

/// Выбросить записи старше `cutoff` и, если файл больше `max_bytes`, самые старые сверх половины предела.
pub fn prune(path: &Path, cutoff: u64, max_bytes: u64) -> std::io::Result<()> {
    use std::io::BufRead;
    let Ok(meta) = fs::metadata(path) else { return Ok(()) };
    let first_old = {
        let f = fs::File::open(path)?;
        let mut first = String::new();
        std::io::BufReader::new(f).read_line(&mut first)?;
        line_ts(first.trim()).is_some_and(|ts| ts < cutoff)
    };
    if !first_old && meta.len() <= max_bytes {
        return Ok(());
    }
    let text = fs::read_to_string(path)?;
    let mut keep: Vec<&str> =
        text.lines().filter(|l| !l.trim().is_empty() && line_ts(l).is_none_or(|ts| ts >= cutoff)).collect();
    let mut size: u64 = keep.iter().map(|l| l.len() as u64 + 1).sum();
    let mut drop = 0;
    while size > max_bytes / 2 && drop < keep.len() {
        size -= keep[drop].len() as u64 + 1;
        drop += 1;
    }
    keep.drain(..drop);
    let tmp = path.with_extension("jsonl.tmp");
    let mut out = String::with_capacity(size as usize);
    for l in keep {
        out.push_str(l);
        out.push('\n');
    }
    fs::write(&tmp, out)?;
    fs::rename(&tmp, path)
}

/// Откуда запись.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, rename = "JournalSource")]
pub enum Source {
    /// Действия rmon.
    #[serde(rename = "rmon")]
    Rmon,
    /// Сторож памяти rmon.
    #[serde(rename = "guard")]
    Guard,
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

/// Последние `limit` записей всех журналов (действия rmon и сторож), новые сверху.
pub fn read_recent(limit: usize) -> Vec<JournalEntry> {
    let mut out: Vec<JournalEntry> = tail_lines(&actions_path(), limit)
        .into_iter()
        .map(|raw| JournalEntry { source: Source::Rmon, ts: raw["ts"].as_u64(), text: action_text(&raw), raw })
        .chain(guard_recent(limit))
        .collect();
    out.sort_by_key(|e| std::cmp::Reverse(e.ts));
    out.truncate(limit);
    out
}

/// Последние `limit` событий сторожа памяти rmon, новые сверху.
pub fn guard_recent(limit: usize) -> Vec<JournalEntry> {
    crate::guard::runner::read_events(limit)
        .into_iter()
        .map(|ev| JournalEntry {
            source: Source::Guard,
            ts: Some(ev.ts),
            text: ev.text(),
            raw: serde_json::to_value(&ev).unwrap_or_default(),
        })
        .collect()
}

/// Последняя смена зоны сторожа (событие `ev = "zone"`) — текущая зона.
/// Зона лежит в `raw["zone"]` (`green` / `yellow` / `red`), причина — в `raw["why"]`.
/// `None` — журнала нет или среди последних событий нет смены зоны.
pub fn last_guard_zone() -> Option<JournalEntry> {
    guard_recent(ZONE_LOOKBACK).into_iter().find(|e| e.raw["ev"] == "zone")
}

/// Сколько последних событий просматривать в поисках смены зоны.
const ZONE_LOOKBACK: usize = 200;

/// Последние `limit` строк JSON файла в порядке файла. Файл читается с конца кусками,
/// а не целиком: журнал сторожа за неделю — мегабайты.
fn tail_lines(path: &Path, limit: usize) -> Vec<serde_json::Value> {
    if limit == 0 {
        return Vec::new();
    }
    let Ok(mut f) = fs::File::open(path) else { return Vec::new() };
    let Ok(len) = f.seek(SeekFrom::End(0)) else { return Vec::new() };
    let mut chunk: u64 = 64 * 1024;
    loop {
        let start = len.saturating_sub(chunk);
        let mut buf = Vec::with_capacity((len - start) as usize);
        if f.seek(SeekFrom::Start(start)).is_err() || f.read_to_end(&mut buf).is_err() {
            return Vec::new();
        }
        let text = String::from_utf8_lossy(&buf);
        // Первая строка куска, начатого не с начала файла, может быть обрезана.
        let skip = usize::from(start > 0);
        let lines: Vec<&str> = text.lines().skip(skip).filter(|l| !l.trim().is_empty()).collect();
        if lines.len() >= limit || start == 0 {
            let first = lines.len().saturating_sub(limit);
            return lines[first..].iter().filter_map(|l| serde_json::from_str(l).ok()).collect();
        }
        chunk = chunk.saturating_mul(4);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_log(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rmon-journal-test-{}-{name}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("events.jsonl");
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn reads_tail_in_file_order() {
        let mut body = String::new();
        // Больше одного куска чтения, чтобы проверить чтение с конца.
        for i in 0..3000 {
            body.push_str(&format!("{{\"ts\":{i},\"why\":\"норма {i}\",\"pad\":\"{}\"}}\n", "x".repeat(40)));
        }
        let path = temp_log("tail", &body);
        let got = tail_lines(&path, 3);
        assert_eq!(got.iter().map(|v| v["ts"].as_u64().unwrap()).collect::<Vec<_>>(), [2997, 2998, 2999]);
        assert!(tail_lines(&path, 0).is_empty());
        assert_eq!(tail_lines(&path, 10_000).len(), 3000);
        assert!(tail_lines(&path.with_file_name("нет.jsonl"), 5).is_empty(), "нет файла — пусто");
    }

    #[test]
    fn prune_keeps_last_day_and_caps_size() {
        let dir = std::env::temp_dir().join(format!("rmon-prune-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("x.jsonl");
        let now = 1_000_000u64;
        let lines: Vec<String> = (0..48).map(|h| format!("{{\"ts\":{},\"n\":{h}}}", now - (47 - h) * 3600)).collect();
        std::fs::write(&file, lines.join("\n") + "\n").unwrap();
        super::prune(&file, now - 24 * 3600, 1 << 20).unwrap();
        let kept = std::fs::read_to_string(&file).unwrap();
        assert_eq!(kept.lines().count(), 25, "последние сутки включительно");
        assert!(kept.lines().next().unwrap().contains(&format!("{}", now - 24 * 3600)));
        super::prune(&file, 0, 200).unwrap();
        assert!(std::fs::metadata(&file).unwrap().len() <= 100, "предел размера — до половины");
        std::fs::remove_dir_all(dir).ok();
    }
}

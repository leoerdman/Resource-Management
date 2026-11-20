//! Журналы: действия rmon (`~/Library/Logs/rmon/actions.jsonl`) и события memory-guard.

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

/// Путь журнала старого memory-guard — см. [`config::legacy_guard_log`].
pub fn guard_path() -> PathBuf {
    config::legacy_guard_log()
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
    /// Сторож памяти rmon.
    #[serde(rename = "guard")]
    Guard,
    /// Старый сторож memory-guard (история до переноса).
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

/// Последние `limit` записей всех журналов (действия rmon, сторож rmon, старый memory-guard), новые сверху.
pub fn read_recent(limit: usize) -> Vec<JournalEntry> {
    let mut out: Vec<JournalEntry> = tail_lines(&actions_path(), limit)
        .into_iter()
        .map(|raw| JournalEntry { source: Source::Rmon, ts: raw["ts"].as_u64(), text: action_text(&raw), raw })
        .chain(guard_recent(limit))
        .chain(guard_entries(&guard_path(), limit))
        .collect();
    out.sort_by_key(|e| std::cmp::Reverse(e.ts));
    out.truncate(limit);
    out
}

/// Последние `limit` событий сторожа памяти rmon, новые сверху.
pub fn guard_recent(limit: usize) -> Vec<JournalEntry> {
    crate::guard::runner::read_events(limit)
        .into_iter()
        .map(|ev| JournalEntry { source: Source::Guard, ts: Some(ev.ts), text: ev.text(), raw: serde_json::to_value(&ev).unwrap_or_default() })
        .collect()
}

/// Последние `limit` событий сторожа: rmon и, для истории, старого memory-guard; новые сверху.
/// Нет журналов — пустой список, не ошибка.
pub fn read_guard_recent(limit: usize) -> Vec<JournalEntry> {
    let mut out = guard_recent(limit);
    out.extend(guard_entries(&guard_path(), limit));
    out.sort_by_key(|e| std::cmp::Reverse(e.ts));
    out.truncate(limit);
    out
}

/// Последняя смена зоны memory-guard (событие `ev = "zone"`) — текущая зона сторожа.
/// Зона лежит в `raw["zone"]` (`green` / `yellow` / `red`), причина — в `raw["why"]`.
/// `None` — журнала нет или среди последних событий нет смены зоны.
pub fn last_guard_zone() -> Option<JournalEntry> {
    read_guard_recent(ZONE_LOOKBACK).into_iter().find(|e| e.raw["ev"] == "zone")
}

/// Сколько последних событий просматривать в поисках смены зоны.
const ZONE_LOOKBACK: usize = 200;

fn guard_entries(path: &Path, limit: usize) -> Vec<JournalEntry> {
    let mut out: Vec<JournalEntry> = tail_lines(path, limit)
        .into_iter()
        .map(|raw| JournalEntry {
            source: Source::MemoryGuard,
            ts: raw["t"].as_str().and_then(parse_iso),
            text: guard_text(&raw),
            raw,
        })
        .collect();
    out.reverse();
    out
}

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
        "kill" | "killed" => {
            // Сторож пишет вид прогона в `kind`, размер — в `memMb`, причину — в `zoneWhy`.
            let name = v["name"].as_str().or(v["exe"].as_str()).or(v["kind"].as_str()).unwrap_or("?");
            let pid = v["pid"].as_i64().unwrap_or(0);
            let what = match v["outcome"].as_str() {
                Some("gone") => "не застал (процесс уже завершился)",
                Some("reused") => "не тронул (pid переиспользован)",
                _ => "снял",
            };
            let mut s = format!("сторож {what} {name} ({pid})");
            if let Some(mb) = v["memMb"].as_u64() {
                s.push_str(&format!(", {}", crate::fmt::bytes(mb * crate::model::MIB)));
            }
            if let Some(why) = v["why"].as_str().or(v["zoneWhy"].as_str()) {
                s.push_str(" — ");
                s.push_str(why);
            }
            s
        }
        "limit" => format!("сторож упёрся в предел снятий: {}", v["why"].as_str().unwrap_or("")),
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
    use super::*;

    fn temp_log(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rmon-journal-test-{}-{name}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("events.jsonl");
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn reads_guard_tail_newest_first() {
        let mut body = String::new();
        // Больше одного куска чтения, чтобы проверить чтение с конца.
        for i in 0..3000 {
            body.push_str(&format!(
                "{{\"t\":\"2026-11-20T10:00:00Z\",\"ev\":\"zone\",\"zone\":\"green\",\"why\":\"норма {i}\",\"availablePct\":40,\"swapUsedMb\":100}}\n"
            ));
        }
        body.push_str(r#"{"t":"2026-11-20T10:18:59.035Z","ev":"kill","outcome":"term","pid":1402,"kind":"tsgo","memMb":2555,"zoneWhy":"давление жёлтое"}"#);
        body.push('\n');
        let path = temp_log("tail", &body);
        let got = guard_entries(&path, 3);
        assert_eq!(got.len(), 3);
        let size = crate::fmt::bytes(2555 * crate::model::MIB);
        assert_eq!(got[0].text, format!("сторож снял tsgo (1402), {size} — давление жёлтое"));
        assert!(got[1].text.contains("норма 2999"), "{}", got[1].text);
        assert!(got[2].text.contains("норма 2998"), "{}", got[2].text);
        assert_eq!(guard_entries(&path, 0).len(), 0);
        assert_eq!(guard_entries(&path, 10_000).len(), 3001);
        assert!(guard_entries(&path.with_file_name("нет.jsonl"), 5).is_empty(), "нет файла — пусто");
    }

    #[test]
    fn parses_iso() {
        assert_eq!(super::parse_iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(super::parse_iso("2026-11-20T09:48:46.625Z"), Some(1_795_168_126));
    }
}

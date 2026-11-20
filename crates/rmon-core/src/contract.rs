//! Контракт с окном помимо типов: подписи и правило форматирования.
//!
//! Типы окна генерирует ts-rs. Здесь — то, что ts-rs не умеет: словари подписей
//! (`labels.json`) и эталонные случаи форматирования размеров (`format-cases.json`).
//! Оба файла пишет тест [`tests::export_ui_contract`] в `TS_RS_EXPORT_DIR`
//! (`apps/rmon-app/src/bindings`, см. `.cargo/config.toml`); окно импортирует подписи
//! оттуда, а его тест сверяет `fmtBytes` с эталоном. CI проверяет, что файлы закоммичены.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::analyze::SuspectKind;
use crate::fmt;
use crate::model::{GIB, KIB, MIB, Pressure, ProcState};
use crate::policy::KillPolicy;

/// Подписи перечислений по их serde-именам.
#[derive(Debug, Serialize)]
pub struct Labels {
    /// Давление.
    pub pressure: BTreeMap<String, &'static str>,
    /// Состояние процесса.
    pub proc_state: BTreeMap<String, &'static str>,
    /// Вид подозреваемого.
    pub suspect_kind: BTreeMap<String, &'static str>,
    /// Политика снятия (по `kind`).
    pub policy: BTreeMap<String, &'static str>,
}

fn key<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
}

/// Словари подписей.
pub fn labels() -> Labels {
    Labels {
        pressure: Pressure::ALL.iter().map(|p| (key(p), p.as_str())).collect(),
        proc_state: ProcState::ALL.iter().map(|s| (key(s), s.as_str())).collect(),
        suspect_kind: SuspectKind::ALL.iter().map(|k| (key(k), k.label())).collect(),
        policy: [KillPolicy::Protected(String::new()), KillPolicy::Confirm(String::new()), KillPolicy::Free]
            .iter()
            .map(|p| {
                (
                    serde_json::to_value(p)
                        .ok()
                        .and_then(|v| v["kind"].as_str().map(str::to_owned))
                        .unwrap_or_default(),
                    p.label(),
                )
            })
            .collect(),
    }
}

/// Эталонные случаи `fmt::bytes`: границы единиц, округление, целые от 100.
pub fn format_cases() -> Vec<(u64, String)> {
    let samples = [
        0,
        1,
        999,
        1000,
        1023,
        1024,
        1536,
        99 * KIB + 512,
        100 * KIB,
        999 * KIB,
        1000 * KIB,
        MIB,
        MIB + MIB / 20,
        99 * MIB + MIB / 2,
        100 * MIB,
        412 * MIB + 300 * KIB,
        999 * MIB + MIB / 2,
        999 * MIB + MIB / 4,
        1003 * MIB,
        GIB,
        3_328_600_064,
        14 * GIB + 512 * MIB,
        24 * GIB,
        99 * GIB + 950 * MIB,
        150 * GIB,
        1100 * GIB,
    ];
    samples.iter().map(|&n| (n, fmt::bytes(n))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_cover_every_variant() {
        let l = labels();
        assert_eq!(l.pressure.len(), 3);
        assert_eq!(l.proc_state.len(), 5);
        assert_eq!(l.suspect_kind.len(), 6);
        assert_eq!(l.policy.keys().map(String::as_str).collect::<Vec<_>>(), ["confirm", "free", "protected"]);
    }

    /// Пишет `labels.json` и `format-cases.json` рядом с типами ts-rs.
    #[test]
    fn export_ui_contract() {
        let Some(dir) = std::env::var_os("TS_RS_EXPORT_DIR") else { return };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, v: serde_json::Value| {
            let text = serde_json::to_string_pretty(&v).unwrap() + "\n";
            std::fs::write(dir.join(name), text).unwrap();
        };
        write("labels.json", serde_json::to_value(labels()).unwrap());
        write("format-cases.json", serde_json::to_value(format_cases()).unwrap());
    }
}

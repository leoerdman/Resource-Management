//! Старый сторож `memory-guard` (Bun + LaunchAgent `com.leoerdman.memory-guard`).
//!
//! Два сторожа на одной машине сняли бы одно и то же дважды, поэтому сторож rmon
//! не включается в бою, пока жив старый. [`retire`] переносит обязанность в rmon:
//! выгружает агент и переименовывает его plist в `*.disabled-by-rmon` — обратимо
//! ([`restore`]), каталог сторожа и его журнал не трогаются.

use std::path::PathBuf;
use std::process::Command;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::config;

/// Метка LaunchAgent старого сторожа.
pub const LABEL: &str = "com.leoerdman.memory-guard";

/// Состояние старого сторожа.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, rename = "LegacyGuard")]
pub enum Legacy {
    /// Нет и не было (или не macOS).
    Absent,
    /// Агент загружен и работает.
    Running {
        /// Процесс демона.
        pid: Option<i32>,
    },
    /// plist есть, но агент не загружен.
    Installed,
    /// Перенесён в rmon: plist отключён.
    Retired,
}

fn plist() -> PathBuf {
    config::home_dir().join("Library/LaunchAgents").join(format!("{LABEL}.plist"))
}

fn disabled_plist() -> PathBuf {
    plist().with_extension("plist.disabled-by-rmon")
}

fn domain() -> String {
    format!("gui/{}", crate::platform::current_uid())
}

fn launchctl(args: &[&str]) -> Option<(bool, String)> {
    let out = Command::new("/bin/launchctl").args(args).output().ok()?;
    Some((out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned()))
}

/// Что со старым сторожем сейчас.
pub fn status() -> Legacy {
    if !cfg!(target_os = "macos") {
        return Legacy::Absent;
    }
    if let Some((true, out)) = launchctl(&["print", &format!("{}/{LABEL}", domain())]) {
        let pid = out.lines().find_map(|l| l.trim().strip_prefix("pid = ")).and_then(|v| v.trim().parse().ok());
        return Legacy::Running { pid };
    }
    if plist().exists() {
        Legacy::Installed
    } else if disabled_plist().exists() {
        Legacy::Retired
    } else {
        Legacy::Absent
    }
}

/// Перенести обязанность в rmon: выгрузить агент, отключить plist. Идемпотентно.
pub fn retire() -> Result<Legacy, String> {
    if !cfg!(target_os = "macos") {
        return Ok(Legacy::Absent);
    }
    let _ = launchctl(&["bootout", &format!("{}/{LABEL}", domain())]);
    let (from, to) = (plist(), disabled_plist());
    if from.exists() {
        std::fs::rename(&from, &to).map_err(|e| format!("{} → {}: {e}", from.display(), to.display()))?;
    }
    match status() {
        Legacy::Running { pid } => Err(format!("старый сторож всё ещё работает (pid {pid:?})")),
        s => Ok(s),
    }
}

/// Вернуть старого сторожа (откат переноса).
pub fn restore() -> Result<Legacy, String> {
    if !cfg!(target_os = "macos") {
        return Ok(Legacy::Absent);
    }
    let (from, to) = (disabled_plist(), plist());
    if from.exists() {
        std::fs::rename(&from, &to).map_err(|e| format!("{} → {}: {e}", from.display(), to.display()))?;
    }
    match launchctl(&["bootstrap", &domain(), &to.to_string_lossy()]) {
        Some((true, _)) | None => Ok(status()),
        Some((false, _)) => Ok(status()),
    }
}

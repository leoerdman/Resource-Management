//! Защитный слой снятия: политика, сверка личности, SIGTERM → ожидание → SIGKILL, журнал.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::collect::unix_now;
use crate::error::{Error, Result};
use crate::journal;
use crate::model::ProcessInfo;
use crate::platform::{self, Platform, Signal};
pub use crate::policy::{KillPolicy, policy_for};

/// Кто попросил снятие.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Requester {
    /// Командная строка.
    Cli,
    /// MCP (Claude Code).
    Mcp,
    /// Окно.
    App,
    /// Сторож памяти (автоматически, в красной зоне).
    Guard,
}

/// Просьба снять процесс.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KillRequest {
    /// Сколько ждать после SIGTERM перед SIGKILL.
    pub signal_term_grace: Duration,
    /// Подтверждение для политики Confirm.
    pub confirm: bool,
    /// Кто попросил.
    pub requested_by: Requester,
    /// Зачем.
    pub reason: String,
    /// Только проверить, ничего не посылать.
    pub dry_run: bool,
}

impl Default for KillRequest {
    fn default() -> Self {
        Self {
            signal_term_grace: Duration::from_secs(5),
            confirm: false,
            requested_by: Requester::Cli,
            reason: String::new(),
            dry_run: false,
        }
    }
}

/// Итог снятия.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KillOutcome {
    /// Процесс.
    pub pid: i32,
    /// Имя.
    pub name: String,
    /// Последний посланный сигнал.
    pub signal: Signal,
    /// Завершился ли.
    pub exited: bool,
    /// Сколько памяти освободилось (оценка по снимку).
    #[ts(type = "number")]
    pub reclaimed: u64,
    /// Пробный прогон.
    pub dry_run: bool,
    /// Итог для человека.
    pub message: String,
}

/// Снять процесс через защитный слой текущей платформы.
pub fn kill(target: &ProcessInfo, req: KillRequest) -> Result<KillOutcome> {
    let mut platform = platform::current()?;
    kill_with(platform.as_mut(), target, req)
}

/// То же поверх заданной платформы.
pub fn kill_with(platform: &mut dyn Platform, target: &ProcessInfo, req: KillRequest) -> Result<KillOutcome> {
    let result = kill_inner(platform, target, &req);
    let entry = serde_json::json!({
        "ts": unix_now(),
        "requested_by": req.requested_by,
        "pid": target.pid,
        "name": target.name,
        "started_at": target.started_at,
        "reason": req.reason,
        "dry_run": req.dry_run,
        "ok": result.is_ok(),
        "exited": result.as_ref().map(|o| o.exited).unwrap_or(false),
        "reclaimed": result.as_ref().map(|o| o.reclaimed).unwrap_or(0),
        "error": result.as_ref().err().map(|e| e.to_string()),
    });
    let _ = journal::append_action(&entry);
    result
}

fn kill_inner(platform: &mut dyn Platform, target: &ProcessInfo, req: &KillRequest) -> Result<KillOutcome> {
    let current = platform.process(target.pid)?.ok_or(Error::ProcessGone { pid: target.pid })?;
    if !current.same_identity(target) && current.started_at != target.started_at {
        return Err(Error::IdentityMismatch { pid: target.pid, expected: target.started_at, actual: current.started_at });
    }
    if current.args != target.args && !target.args.is_empty() && !current.args.is_empty() {
        return Err(Error::IdentityMismatch { pid: target.pid, expected: target.started_at, actual: current.started_at });
    }
    match policy_for(&current) {
        KillPolicy::Protected(reason) => {
            return Err(Error::Protected { pid: current.pid, name: current.name.clone(), reason });
        }
        KillPolicy::Confirm(reason) if !req.confirm => {
            return Err(Error::ConfirmationRequired { pid: current.pid, name: current.name.clone(), reason });
        }
        _ => {}
    }
    let reclaimed = current.reclaim();
    if req.dry_run {
        return Ok(KillOutcome {
            pid: current.pid,
            name: current.name.clone(),
            signal: Signal::Term,
            exited: false,
            reclaimed,
            dry_run: true,
            message: format!("снял бы {} ({}), освободилось бы {}", current.name, current.pid, crate::fmt::bytes(reclaimed)),
        });
    }

    platform.signal(current.pid, Signal::Term)?;
    let mut signal = Signal::Term;
    if !wait_exit(platform, current.pid, req.signal_term_grace)? {
        // Перед SIGKILL ещё раз сверяем: за время ожидания pid мог смениться.
        if let Some(again) = platform.process(current.pid)?
            && again.started_at == current.started_at
        {
            match platform.signal(current.pid, Signal::Kill) {
                Ok(()) | Err(Error::ProcessGone { .. }) => {}
                Err(e) => return Err(e),
            }
            signal = Signal::Kill;
            wait_exit(platform, current.pid, Duration::from_secs(2))?;
        }
    }
    let exited = !platform.alive(current.pid)? || platform.process(current.pid)?.is_none_or(|p| p.started_at != current.started_at);
    let message = if exited {
        format!("{} ({}) снят {}, освободилось ~{}", current.name, current.pid, signal.as_str(), crate::fmt::bytes(reclaimed))
    } else {
        format!("{} ({}) не завершился после {}", current.name, current.pid, signal.as_str())
    };
    Ok(KillOutcome {
        pid: current.pid,
        name: current.name,
        signal,
        exited,
        reclaimed: if exited { reclaimed } else { 0 },
        dry_run: false,
        message,
    })
}

fn wait_exit(platform: &mut dyn Platform, pid: i32, grace: Duration) -> Result<bool> {
    let until = Instant::now() + grace;
    loop {
        if !platform.alive(pid)? {
            return Ok(true);
        }
        if Instant::now() >= until {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::model::ProcState;

    fn proc(pid: i32, name: &str, exe: Option<&str>) -> ProcessInfo {
        ProcessInfo {
            pid,
            ppid: 1,
            name: name.into(),
            exe: exe.map(Into::into),
            args: vec![],
            uid: platform::current_uid(),
            footprint: Some(1),
            rss: 1,
            cpu_pct: 0.0,
            threads: 1,
            state: ProcState::Sleeping,
            started_at: 1,
            app_group: name.into(),
        }
    }

    #[test]
    fn kills_own_sleep_and_refuses_bad_identity() {
        let mut child = std::process::Command::new("/bin/sleep").arg("300").spawn().unwrap();
        let pid = child.id() as i32;
        std::thread::sleep(Duration::from_millis(200));
        let mut plat = platform::current().unwrap();
        let info = plat.process(pid).unwrap().unwrap();
        assert!(matches!(policy_for(&info), KillPolicy::Confirm(_)), "утилиты /bin — с подтверждением");
        let refused = kill_with(plat.as_mut(), &info, KillRequest::default()).unwrap_err();
        assert!(matches!(refused, Error::ConfirmationRequired { .. }), "{refused}");
        let mut fake = info.clone();
        fake.started_at += 1000;
        let err = kill_with(plat.as_mut(), &fake, KillRequest::default()).unwrap_err();
        assert!(matches!(err, Error::IdentityMismatch { .. }), "{err}");
        // pid 1 — отказ.
        let launchd = proc(1, "launchd", Some("/sbin/launchd"));
        assert!(kill_with(plat.as_mut(), &launchd, KillRequest::default()).is_err());
        // Пожинаем ребёнка в фоне, иначе он останется зомби и «живым» для kill(0).
        let reaper = std::thread::spawn(move || child.wait());
        let out = kill_with(plat.as_mut(), &info, KillRequest { reason: "тест".into(), confirm: true, ..Default::default() }).unwrap();
        assert!(out.exited, "{}", out.message);
        reaper.join().unwrap().unwrap();
    }
}

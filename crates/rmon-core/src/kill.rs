//! Защитный слой снятия: политика, сверка личности, SIGTERM → ожидание → SIGKILL, журнал.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::collect::unix_now;
use crate::error::{Error, Result};
use crate::journal;
use crate::model::ProcessInfo;
use crate::platform::{self, Platform, Signal};

/// Можно ли снимать процесс.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "reason", rename_all = "snake_case")]
pub enum KillPolicy {
    /// Никогда.
    Protected(String),
    /// Только с явным подтверждением.
    Confirm(String),
    /// Можно.
    Free,
}

impl KillPolicy {
    /// Слово для человека.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Protected(_) => "защищён",
            Self::Confirm(_) => "с подтверждением",
            Self::Free => "можно",
        }
    }

    /// Показывать ли кнопку «Снять».
    pub fn killable(&self) -> bool {
        !matches!(self, Self::Protected(_))
    }
}

/// Кто попросил снятие.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Requester {
    /// Командная строка.
    Cli,
    /// MCP (Claude Code).
    Mcp,
    /// Окно.
    App,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub reclaimed: u64,
    /// Пробный прогон.
    pub dry_run: bool,
    /// Итог для человека.
    pub message: String,
}

/// Системные службы Apple. Утилиты из `/bin` и `/usr/bin`, запущенные владельцем
/// (`sleep`, `python3`, `git`), сюда не входят: их снимают так же, как любой прогон.
const SYSTEM_PREFIXES: &[&str] = &["/System/", "/usr/libexec/", "/usr/sbin/", "/sbin/", "/Library/Apple/"];
/// Ядро и графика.
const PROTECTED_NAMES: &[&str] = &["kernel_task", "launchd", "WindowServer", "loginwindow"];
/// Сторожа владельца.
const PROTECTED_ARGS: &[(&str, &str)] = &[
    ("memory-guard/guard.ts", "сам сторож памяти"),
    ("Hooks/machine-guard/", "хуки сторожа машины"),
];
/// Приложения владельца (часть пути .app).
const CONFIRM_BUNDLES: &[(&str, &str)] = &[
    ("Claude", "Claude и его хелперы"),
    ("M Code Desktop", "M Code Desktop"),
    ("Telegram", "Telegram"),
    ("Google Chrome", "браузер"),
    ("Safari", "браузер"),
    ("Arc", "браузер"),
    ("Firefox", "браузер"),
    ("Brave Browser", "браузер"),
    ("Microsoft Edge", "браузер"),
    ("Yandex", "браузер"),
    ("Opera", "браузер"),
    ("Chromium", "браузер"),
    ("Orion", "браузер"),
    ("Zen", "браузер"),
    ("Dia", "браузер"),
    ("Comet", "браузер"),
    ("Visual Studio Code", "редактор"),
    ("Cursor", "редактор"),
    ("Windsurf", "редактор"),
    ("Zed", "редактор"),
    ("Xcode", "редактор"),
];

/// Политика для процесса. Одно место для CLI, MCP и окна.
pub fn policy_for(p: &ProcessInfo) -> KillPolicy {
    let me = std::process::id() as i32;
    if p.pid <= 1 {
        return KillPolicy::Protected("ядро и launchd".into());
    }
    if p.pid == me {
        return KillPolicy::Protected("сам rmon".into());
    }
    if PROTECTED_NAMES.contains(&p.name.as_str()) {
        return KillPolicy::Protected(format!("{} — часть системы", p.name));
    }
    if let Some(exe) = &p.exe {
        if SYSTEM_PREFIXES.iter().any(|pre| exe.starts_with(pre)) {
            return KillPolicy::Protected("системный процесс".into());
        }
        if exe.contains("/rmon-app.app/") || exe.contains("/Resource Monitor.app/") {
            return KillPolicy::Protected("сам rmon".into());
        }
    }
    if p.uid == 0 && platform::current_uid() != 0 {
        return KillPolicy::Protected("процесс root — нет прав".into());
    }
    let cmd = p.args.join(" ");
    for (needle, why) in PROTECTED_ARGS {
        if cmd.contains(needle) {
            return KillPolicy::Protected((*why).into());
        }
    }
    if let Some(exe) = &p.exe {
        for (bundle, why) in CONFIRM_BUNDLES {
            if exe_in_bundle(exe, bundle) {
                return KillPolicy::Confirm(format!("{why}: {}", p.app_group));
            }
        }
    }
    match p.name.as_str() {
        "claude" => return KillPolicy::Confirm("сессия Claude Code".into()),
        "mcode" | "mcode-engine" => return KillPolicy::Confirm("движок M Code".into()),
        _ => {}
    }
    if cmd.contains("mcode-engine") {
        return KillPolicy::Confirm("движок M Code".into());
    }
    if p.exe.as_deref().is_some_and(|e| e.contains(".app/")) {
        return KillPolicy::Confirm(format!("приложение {}", p.app_group));
    }
    KillPolicy::Free
}

fn exe_in_bundle(exe: &str, bundle: &str) -> bool {
    exe.split('/').any(|c| c.strip_suffix(".app").is_some_and(|stem| stem == bundle || stem.starts_with(&format!("{bundle} "))))
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
    fn policies() {
        assert!(matches!(policy_for(&proc(1, "launchd", Some("/sbin/launchd"))), KillPolicy::Protected(_)));
        assert!(matches!(policy_for(&proc(400, "WindowServer", None)), KillPolicy::Protected(_)));
        assert!(!policy_for(&proc(400, "WindowServer", None)).killable());
        assert!(matches!(
            policy_for(&proc(500, "Google Chrome Helper", Some("/Applications/Google Chrome.app/Contents/Frameworks/x"))),
            KillPolicy::Confirm(_)
        ));
        assert!(matches!(policy_for(&proc(600, "claude", Some("/Users/x/.local/bin/claude"))), KillPolicy::Confirm(_)));
        assert_eq!(policy_for(&proc(700, "node", Some("/opt/homebrew/bin/node"))), KillPolicy::Free);
    }

    #[test]
    fn kills_own_sleep_and_refuses_bad_identity() {
        let mut child = std::process::Command::new("/bin/sleep").arg("300").spawn().unwrap();
        let pid = child.id() as i32;
        std::thread::sleep(Duration::from_millis(200));
        let mut plat = platform::current().unwrap();
        let info = plat.process(pid).unwrap().unwrap();
        assert_eq!(policy_for(&info), KillPolicy::Free);
        let mut fake = info.clone();
        fake.started_at += 1000;
        let err = kill_with(plat.as_mut(), &fake, KillRequest::default()).unwrap_err();
        assert!(matches!(err, Error::IdentityMismatch { .. }), "{err}");
        // pid 1 — отказ.
        let launchd = proc(1, "launchd", Some("/sbin/launchd"));
        assert!(kill_with(plat.as_mut(), &launchd, KillRequest::default()).is_err());
        // Пожинаем ребёнка в фоне, иначе он останется зомби и «живым» для kill(0).
        let reaper = std::thread::spawn(move || child.wait());
        let out = kill_with(plat.as_mut(), &info, KillRequest { reason: "тест".into(), ..Default::default() }).unwrap();
        assert!(out.exited, "{}", out.message);
        reaper.join().unwrap().unwrap();
    }
}

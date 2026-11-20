//! Политика снятия: кого нельзя трогать никогда, кого — только с подтверждением.
//!
//! Правила — данные, а не код: встроенные (по ОС) ∪ `[guard] never` сторожа памяти
//! (как «с подтверждением»: сторож не трогает их автоматически, rmon — без явного согласия)
//! ∪ `[policy]` из `config.toml`. Добавить можно только строгость: «свободных» правил нет.
//!
//! Порядок решения: ядро и сам rmon → чужой процесс без прав → «никогда» → «с подтверждением»
//! → свободно.

use std::path::Path;
use std::sync::OnceLock;

use regex_lite::Regex;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::config::{self, Config, RuleSpec};
use crate::guard::never_as_policy_rules;
use crate::model::ProcessInfo;
use crate::platform;

/// Можно ли снимать процесс.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "reason", rename_all = "snake_case")]
#[ts(export)]
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
            Self::Free => "можно снять",
        }
    }

    /// Показывать ли кнопку «Снять».
    pub fn killable(&self) -> bool {
        !matches!(self, Self::Protected(_))
    }

    /// Причина (для Free — пусто).
    pub fn reason(&self) -> &str {
        match self {
            Self::Protected(r) | Self::Confirm(r) => r,
            Self::Free => "",
        }
    }
}

/// Скомпилированное правило.
#[derive(Debug, Clone)]
pub struct Rule {
    name: String,
    exe: Option<Regex>,
    path: Option<Regex>,
    args: Option<Regex>,
}

impl Rule {
    /// Из описания; ошибка — текст с именем правила и поля.
    pub fn compile(spec: &RuleSpec) -> Result<Self, String> {
        let re = |field: &str, v: &Option<String>| -> Result<Option<Regex>, String> {
            v.as_deref()
                .map(|s| Regex::new(s).map_err(|e| format!("правило «{}», поле {field}: {e}", spec.name)))
                .transpose()
        };
        Ok(Self { name: spec.name.clone(), exe: re("exe", &spec.exe)?, path: re("path", &spec.path)?, args: re("args", &spec.args)? })
    }

    /// Совпадает ли процесс: все заданные поля, хотя бы одно задано.
    pub fn matches(&self, p: &ProcessInfo) -> bool {
        if self.exe.is_none() && self.path.is_none() && self.args.is_none() {
            return false;
        }
        let exe_ok = self.exe.as_ref().is_none_or(|re| {
            re.is_match(&p.name)
                || p.exe.as_deref().and_then(|e| Path::new(e).file_name()).and_then(|n| n.to_str()).is_some_and(|n| re.is_match(n))
        });
        let path_ok = self.path.as_ref().is_none_or(|re| p.exe.as_deref().is_some_and(|e| re.is_match(e)));
        let args_ok = self.args.as_ref().is_none_or(|re| re.is_match(&p.args.join(" ")));
        exe_ok && path_ok && args_ok
    }

    /// Имя правила.
    pub fn name(&self) -> &str {
        &self.name
    }
}

fn spec(name: &str, exe: Option<&str>, path: Option<&str>, args: Option<&str>) -> RuleSpec {
    RuleSpec { name: name.into(), exe: exe.map(Into::into), path: path.map(Into::into), args: args.map(Into::into) }
}

/// Встроенные «никогда» для текущей ОС.
pub fn builtin_protect() -> Vec<RuleSpec> {
    let mut v = vec![
        spec("сам rmon", None, Some(r"/Resource Monitor\.app/|(^|[/\\])rmon-app(\.exe)?$"), None),
        spec("сам сторож памяти", None, None, Some(r"memory-guard/guard\.ts")),
        spec("хуки сторожа машины", None, None, Some(r"Hooks/machine-guard/")),
    ];
    if cfg!(target_os = "macos") {
        v.extend([
            spec("системная служба macOS", None, Some(r"^/(System|usr/libexec|usr/sbin|sbin|Library/Apple)/"), None),
            spec("ядро и графика macOS", Some(r"^(kernel_task|launchd|WindowServer|loginwindow)$"), None, None),
        ]);
    } else if cfg!(windows) {
        v.extend([
            spec("системная служба Windows", None, Some(r"(?i)^[a-z]:\\Windows\\"), None),
            spec(
                "ядро и сеанс Windows",
                Some(r"(?i)^(System|Registry|Secure System|smss|csrss|wininit|winlogon|services|lsass|svchost|dwm|fontdrvhost|MsMpEng)(\.exe)?$"),
                None,
                None,
            ),
        ]);
    } else {
        v.extend([
            spec("системная служба Linux", None, Some(r"^/(usr/lib/systemd|lib/systemd|usr/libexec|usr/sbin|sbin)/"), None),
            spec(
                "init, графика и сеанс Linux",
                Some(r"^(systemd|init|kthreadd|Xorg|Xwayland|gnome-shell|kwin_wayland|kwin_x11|plasmashell|sddm|gdm|gdm3|lightdm|dbus-daemon|dbus-broker|pipewire|wireplumber|NetworkManager)$"),
                None,
                None,
            ),
        ]);
    }
    v
}

/// Встроенные «с подтверждением», кроме «никогда» сторожа (оно добавляется отдельно):
/// любое приложение из бандла на macOS.
pub fn builtin_confirm() -> Vec<RuleSpec> {
    if cfg!(target_os = "macos") { vec![spec("приложение", None, Some(r"\.app/"), None)] } else { Vec::new() }
}

/// Политика снятия.
#[derive(Debug, Clone)]
pub struct Policy {
    protect: Vec<Rule>,
    confirm: Vec<Rule>,
    self_pid: i32,
    self_uid: u32,
}

impl Policy {
    /// Политика с настройками по умолчанию.
    pub fn builtin() -> Self {
        Self::from_config(&Config::default()).0
    }

    /// Встроенные ∪ «никогда» сторожа ∪ конфиг. Вторым — ошибки компиляции (правило пропущено).
    pub fn from_config(cfg: &Config) -> (Self, Vec<String>) {
        let mut protect = builtin_protect();
        protect.extend(cfg.policy.protect.iter().cloned());
        let mut confirm = never_as_policy_rules(&cfg.guard.never);
        confirm.extend(builtin_confirm());
        confirm.extend(cfg.policy.confirm.iter().cloned());
        Self::from_specs(&protect, &confirm)
    }

    fn from_specs(protect: &[RuleSpec], confirm: &[RuleSpec]) -> (Self, Vec<String>) {
        let mut errors = Vec::new();
        let mut compile = |specs: &[RuleSpec]| -> Vec<Rule> {
            specs
                .iter()
                .filter_map(|s| Rule::compile(s).map_err(|e| errors.push(e)).ok())
                .collect()
        };
        let protect = compile(protect);
        let confirm = compile(confirm);
        let me = Self { protect, confirm, self_pid: std::process::id() as i32, self_uid: platform::current_uid() };
        (me, errors)
    }

    /// Решение для процесса.
    pub fn classify(&self, p: &ProcessInfo) -> KillPolicy {
        if p.pid <= platform::LAST_KERNEL_PID {
            return KillPolicy::Protected("ядро системы".into());
        }
        if p.pid == self.self_pid {
            return KillPolicy::Protected("сам rmon".into());
        }
        if self.self_uid != 0 && p.uid != self.self_uid {
            return KillPolicy::Protected("процесс другого пользователя — нет прав".into());
        }
        if let Some(r) = self.protect.iter().find(|r| r.matches(p)) {
            return KillPolicy::Protected(r.name.clone());
        }
        if let Some(r) = self.confirm.iter().find(|r| r.matches(p)) {
            let redundant = p.app_group.is_empty() || p.app_group == p.name || r.name.contains(&p.app_group) || p.app_group.contains(&r.name);
            let app = if redundant { String::new() } else { format!(": {}", p.app_group) };
            return KillPolicy::Confirm(format!("{}{app}", r.name));
        }
        KillPolicy::Free
    }
}

static GLOBAL: OnceLock<Policy> = OnceLock::new();

/// Политика процесса — строится один раз из [`config::global`].
pub fn global() -> &'static Policy {
    GLOBAL.get_or_init(|| {
        let (policy, errors) = Policy::from_config(config::global());
        for e in errors {
            eprintln!("rmon: правило политики пропущено — {e}");
        }
        policy
    })
}

/// Политика для процесса. Одно место для CLI, MCP и окна.
pub fn policy_for(p: &ProcessInfo) -> KillPolicy {
    global().classify(p)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::model::ProcState;

    fn proc(pid: i32, name: &str, exe: Option<&str>, args: &[&str]) -> ProcessInfo {
        ProcessInfo {
            pid,
            ppid: 1,
            name: name.into(),
            exe: exe.map(Into::into),
            args: args.iter().map(|s| s.to_string()).collect(),
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
    fn builtin_decisions() {
        let p = Policy::builtin();
        assert!(matches!(p.classify(&proc(1, "launchd", Some("/sbin/launchd"), &[])), KillPolicy::Protected(_)));
        let ws = p.classify(&proc(400, "WindowServer", None, &[]));
        assert!(!ws.killable(), "{ws:?}");
        assert!(matches!(p.classify(&proc(401, "cfprefsd", Some("/usr/sbin/cfprefsd"), &[])), KillPolicy::Protected(_)));
        assert!(matches!(
            p.classify(&proc(500, "Google Chrome Helper", Some("/Applications/Google Chrome.app/Contents/Frameworks/x"), &[])),
            KillPolicy::Confirm(_)
        ));
        assert!(matches!(p.classify(&proc(600, "claude", Some("/Users/x/.local/bin/claude"), &[])), KillPolicy::Confirm(_)));
        assert!(matches!(p.classify(&proc(601, "sleep", Some("/bin/sleep"), &["sleep", "300"])), KillPolicy::Confirm(_)));
        assert!(matches!(p.classify(&proc(602, "bun", None, &["bun", "/x/memory-guard/guard.ts"])), KillPolicy::Protected(_)));
        assert!(matches!(
            p.classify(&proc(603, "rmon-app", Some("/Applications/Resource Monitor.app/Contents/MacOS/rmon-app"), &[])),
            KillPolicy::Protected(_)
        ));
        assert_eq!(p.classify(&proc(700, "node", Some("/opt/homebrew/bin/node"), &["node", "x.js"])), KillPolicy::Free);
        let mut foreign = proc(800, "node", Some("/opt/homebrew/bin/node"), &[]);
        foreign.uid = foreign.uid.wrapping_add(1);
        if platform::current_uid() != 0 {
            assert!(matches!(p.classify(&foreign), KillPolicy::Protected(_)));
        }
    }

    #[test]
    fn config_only_tightens() {
        let cfg = Config::parse(
            r#"
            [[policy.protect]]
            name = "мой сервер"
            args = "my-server\\.js"
            [[policy.confirm]]
            name = "Ollama"
            exe = "^ollama$"
            "#,
        )
        .unwrap();
        let (p, errors) = Policy::from_config(&cfg);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(p.classify(&proc(900, "node", Some("/opt/homebrew/bin/node"), &["node", "my-server.js"])), KillPolicy::Protected("мой сервер".into()));
        assert!(matches!(p.classify(&proc(901, "ollama", Some("/opt/homebrew/bin/ollama"), &[])), KillPolicy::Confirm(_)));
    }

    #[test]
    fn guard_never_any_field_becomes_confirm() {
        // «движок M Code» у сторожа совпадает по exe ИЛИ по args — так же и в политике.
        let p = Policy::builtin();
        assert!(matches!(p.classify(&proc(910, "bun", Some("/opt/homebrew/bin/bun"), &["bun", "/x/mcode-engine"])), KillPolicy::Confirm(_)));
        assert!(matches!(p.classify(&proc(911, "mcode-engine", Some("/opt/x/mcode-engine"), &["mcode-engine"])), KillPolicy::Confirm(_)));
    }
}

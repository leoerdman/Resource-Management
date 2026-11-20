//! Политика снятия: кого нельзя трогать никогда, кого — только с подтверждением.
//!
//! Правила — данные, а не код: встроенные (по ОС) ∪ `never` из `memory-guard/config.json`
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
    builtin_protect_for(std::env::consts::OS)
}

/// Встроенные «с подтверждением» для текущей ОС: приложения владельца и его агенты.
pub fn builtin_confirm() -> Vec<RuleSpec> {
    builtin_confirm_for(std::env::consts::OS)
}

/// Встроенные «никогда» для ОС по имени из `std::env::consts::OS` — так правила всех ОС
/// проверяются тестами на любой машине.
fn builtin_protect_for(os: &str) -> Vec<RuleSpec> {
    let mut v = vec![
        spec("сам rmon", None, Some(r"/Resource Monitor\.app/|(^|[/\\])rmon-app(\.exe)?$"), None),
        spec("сам сторож памяти", None, None, Some(r"memory-guard/guard\.ts")),
        spec("хуки сторожа машины", None, None, Some(r"Hooks/machine-guard/")),
    ];
    match os {
        "macos" => v.extend([
            spec("системная служба macOS", None, Some(r"^/(System|usr/libexec|usr/sbin|sbin|Library/Apple)/"), None),
            spec("ядро и графика macOS", Some(r"^(kernel_task|launchd|WindowServer|loginwindow)$"), None, None),
        ]),
        "windows" => v.extend([
            spec("системная служба Windows", None, Some(r"(?i)^[a-z]:\\Windows\\"), None),
            // Процессы без прочитанного пути (защищённые, чужие) узнаются по имени.
            spec(
                "ядро и сеанс Windows",
                Some(r"(?i)^(System|Registry|Secure System|Memory Compression|smss|csrss|wininit|winlogon|services|lsass|lsaiso|svchost|dwm|fontdrvhost|sihost|ctfmon|conhost|audiodg|spoolsv|MsMpEng|NisSrv|SecurityHealthService)(\.exe)?$"),
                None,
                None,
            ),
        ]),
        _ => v.extend([
            spec("системная служба Linux", None, Some(r"^/(usr/lib/systemd|lib/systemd|usr/libexec|usr/sbin|sbin|usr/lib/polkit-1|usr/lib/snapd)/"), None),
            spec(
                "init, графика и сеанс Linux",
                Some(r"^(systemd|init|kthreadd|Xorg|Xwayland|gnome-shell|gnome-session-binary|mutter|kwin_wayland|kwin_x11|plasmashell|ksmserver|kded5|kded6|sway|Hyprland|weston|cosmic-comp|sddm|gdm|gdm3|lightdm|dbus-daemon|dbus-broker|pipewire|pipewire-pulse|wireplumber|pulseaudio|polkitd|NetworkManager|snapd)$"),
                None,
                None,
            ),
        ]),
    }
    v
}

/// Встроенные «с подтверждением» для ОС по имени из `std::env::consts::OS`.
fn builtin_confirm_for(os: &str) -> Vec<RuleSpec> {
    let mut v = vec![
        spec("сессия Claude Code", Some(r"^claude(\.exe)?$"), None, None),
        spec("движок M Code", Some(r"^mcode(-engine)?(\.exe)?$"), None, None),
        spec("движок M Code", None, None, Some(r"(^|/)mcode-engine(\s|$)")),
    ];
    match os {
        "macos" => v.extend([
            spec("Claude и его хелперы", None, Some(r"/[Cc]laude\.app/"), None),
            spec("M Code Desktop", None, Some(r"/M Code Desktop[^/]*\.app/"), None),
            spec("Telegram", None, Some(r"/Telegram[^/]*\.app/"), None),
            spec(
                "браузер",
                None,
                Some(r"/(Google Chrome|Safari|Arc|Firefox|Brave Browser|Microsoft Edge|Yandex|Opera|Chromium|Orion|Zen|Dia|Comet)[^/]*\.app/"),
                None,
            ),
            spec("редактор", None, Some(r"/(Visual Studio Code|Cursor|Windsurf|Zed|Xcode)[^/]*\.app/"), None),
            spec("системная утилита", None, Some(r"^/(usr/bin|bin)/"), None),
            spec("приложение", None, Some(r"\.app/"), None),
        ]),
        "windows" => v.extend([
            spec("оболочка Windows", Some(r"(?i)^(explorer|ShellExperienceHost|StartMenuExperienceHost|SearchHost|TextInputHost)(\.exe)?$"), None, None),
            spec("терминал", Some(r"(?i)^(WindowsTerminal|OpenConsole|cmd|powershell|pwsh|wsl|wslhost|bash)(\.exe)?$"), None, None),
            spec("браузер", Some(r"(?i)^(chrome|msedge|msedgewebview2|firefox|brave|opera|vivaldi|browser|arc|zen)(\.exe)?$"), None, None),
            spec(
                "приложение владельца",
                Some(r"(?i)^(Code|Cursor|Windsurf|Zed|Telegram|Claude|M Code Desktop|Slack|Discord|ms-teams|Teams|Spotify|Obsidian|Notion|thunderbird|OUTLOOK|WINWORD|EXCEL|POWERPNT)(\.exe)?$"),
                None,
                None,
            ),
            // Приложения Microsoft Store.
            spec("приложение", None, Some(r"(?i)\\WindowsApps\\"), None),
        ]),
        _ => v.extend([
            spec(
                "браузер",
                Some(r"^(chrome|chromium|chromium-browser|google-chrome|firefox|firefox-bin|brave|opera|vivaldi-bin|msedge|zen|zen-bin)$"),
                None,
                None,
            ),
            spec(
                "приложение владельца",
                Some(r"^(code|cursor|windsurf|zed|zed-editor|telegram-desktop|Telegram|claude-desktop|slack|discord|Discord|spotify|obsidian|thunderbird|teams-for-linux)$"),
                None,
                None,
            ),
            // Оболочки и терминалы: снять — потерять всё, что в них идёт. Утилиты /usr/bin не
            // трогаем целиком, как на macOS: на Linux там же node, python и прочие dev-прогоны.
            spec(
                "оболочка и терминал",
                Some(r"^(bash|zsh|fish|sh|dash|tmux|tmux: server|screen|sshd|ssh-agent|gpg-agent|login|sudo|su|gnome-terminal-server|konsole|kitty|alacritty|wezterm-gui|foot|ptyxis)$"),
                None,
                None,
            ),
            // Snap, flatpak (`/app/` внутри песочницы), сторонние пакеты в /opt, AppImage.
            spec("приложение", None, Some(r"^/(snap|app|opt)/|^/tmp/\.mount_"), None),
        ]),
    }
    v
}


#[derive(Deserialize)]
struct GuardConfig {
    #[serde(default)]
    never: Vec<GuardRule>,
}

#[derive(Deserialize)]
struct GuardRule {
    name: String,
    exe: Option<String>,
    path: Option<String>,
    args: Option<String>,
}

/// `never` сторожа памяти, если он есть. Битые правила пропускаются с предупреждением.
pub fn memory_guard_never(path: &Path) -> Vec<RuleSpec> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    match serde_json::from_str::<GuardConfig>(&text) {
        Ok(cfg) => cfg
            .never
            .into_iter()
            .map(|r| RuleSpec { name: format!("{} (memory-guard)", r.name), exe: r.exe, path: r.path, args: r.args })
            .collect(),
        Err(e) => {
            eprintln!("rmon: {} не разобран: {e}", path.display());
            Vec::new()
        }
    }
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
    /// Только встроенные правила.
    pub fn builtin() -> Self {
        Self::from_specs(&builtin_protect(), &builtin_confirm()).0
    }

    /// Встроенные ∪ memory-guard ∪ конфиг. Вторым — ошибки компиляции правил (правило пропущено).
    pub fn from_config(cfg: &Config) -> (Self, Vec<String>) {
        let mut protect = builtin_protect();
        protect.extend(cfg.policy.protect.iter().cloned());
        let mut confirm = builtin_confirm();
        if cfg.policy.use_memory_guard {
            confirm.extend(memory_guard_never(&config::guard_config()));
        }
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
            let app = if p.app_group.is_empty() || p.app_group == p.name { String::new() } else { format!(": {}", p.app_group) };
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
            [policy]
            use_memory_guard = false
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
    fn reads_memory_guard_never() {
        let dir = std::env::temp_dir().join(format!("rmon-guard-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.json");
        std::fs::write(&file, r#"{"never":[{"name":"Obsidian","path":"/Obsidian\\.app/"},{"name":"битое","exe":"("}]}"#).unwrap();
        let specs = memory_guard_never(&file);
        assert_eq!(specs.len(), 2);
        let (p, errors) = Policy::from_specs(&[], &specs);
        assert_eq!(errors.len(), 1, "битое правило пропущено, остальные работают");
        assert!(matches!(p.classify(&proc(902, "Obsidian", Some("/Applications/Obsidian.app/Contents/MacOS/Obsidian"), &[])), KillPolicy::Confirm(_)));
        std::fs::remove_dir_all(dir).ok();
    }
}

/// Правила Linux и Windows — на любой ОС: решения зависят только от данных процесса.
#[cfg(test)]
mod os_rules {
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

    fn policy(os: &str) -> Policy {
        let (p, errors) = Policy::from_specs(&builtin_protect_for(os), &builtin_confirm_for(os));
        assert!(errors.is_empty(), "{errors:?}");
        p
    }

    #[test]
    fn all_builtin_rules_compile() {
        for os in ["macos", "linux", "windows"] {
            policy(os);
        }
    }

    #[test]
    fn linux_decisions() {
        let p = policy("linux");
        let protected = |name: &str, exe: &str| matches!(p.classify(&proc(500, name, Some(exe), &[])), KillPolicy::Protected(_));
        let confirm = |name: &str, exe: &str| matches!(p.classify(&proc(500, name, Some(exe), &[])), KillPolicy::Confirm(_));
        assert!(protected("systemd-journal", "/usr/lib/systemd/systemd-journald"));
        assert!(protected("gnome-shell", "/usr/bin/gnome-shell"));
        assert!(protected("gsd-power", "/usr/libexec/gsd-power"));
        assert!(protected("rmon-app", "/usr/bin/rmon-app"));
        assert!(confirm("chrome", "/opt/google/chrome/chrome"));
        assert!(confirm("Isolated Web Co", "/usr/lib/firefox/firefox"), "по имени файла");
        assert!(confirm("firefox", "/snap/firefox/4793/usr/lib/firefox/firefox"));
        assert!(confirm("bash", "/usr/bin/bash"));
        assert!(confirm("cursor", "/tmp/.mount_CursorAbC/usr/share/cursor/cursor"));
        assert!(confirm("claude", "/home/x/.local/bin/claude"));
        // Dev-прогоны из /usr/bin — свободны: ради них rmon и нужен.
        assert_eq!(p.classify(&proc(700, "node", Some("/usr/bin/node"), &["node", "x.js"])), KillPolicy::Free);
        assert_eq!(p.classify(&proc(701, "sleep", Some("/usr/bin/sleep"), &["sleep", "300"])), KillPolicy::Free);
        assert_eq!(p.classify(&proc(702, "python3", Some("/usr/bin/python3.12"), &[])), KillPolicy::Free);
    }

    #[test]
    fn windows_decisions() {
        let p = policy("windows");
        let protected = |name: &str, exe: Option<&str>| matches!(p.classify(&proc(500, name, exe, &[])), KillPolicy::Protected(_));
        let confirm = |name: &str, exe: &str| matches!(p.classify(&proc(500, name, Some(exe), &[])), KillPolicy::Confirm(_));
        assert!(protected("svchost.exe", None), "без пути — по имени");
        assert!(protected("Memory Compression", None));
        assert!(protected("RuntimeBroker.exe", Some(r"C:\Windows\System32\RuntimeBroker.exe")));
        assert!(protected("explorer.exe", Some(r"C:\Windows\explorer.exe")), "путь Windows сильнее «оболочки»");
        assert!(protected("rmon-app.exe", Some(r"C:\Program Files\Resource Monitor\rmon-app.exe")));
        assert!(confirm("chrome.exe", r"C:\Program Files\Google\Chrome\Application\chrome.exe"));
        assert!(confirm("Code.exe", r"C:\Users\x\AppData\Local\Programs\Microsoft VS Code\Code.exe"));
        assert!(confirm("pwsh.exe", r"C:\Program Files\PowerShell\7\pwsh.exe"));
        assert!(confirm("claude.exe", r"C:\Users\x\.local\bin\claude.exe"));
        assert!(confirm("Spotify.exe", r"C:\Program Files\WindowsApps\SpotifyAB.SpotifyMusic_1.2\Spotify.exe"));
        assert_eq!(p.classify(&proc(700, "node.exe", Some(r"C:\Program Files\nodejs\node.exe"), &["node", "x.js"])), KillPolicy::Free);
        assert!(matches!(p.classify(&proc(4, "System", None, &[])), KillPolicy::Protected(_)));
        let mut foreign = proc(800, "node.exe", Some(r"C:\Program Files\nodejs\node.exe"), &[]);
        foreign.uid = u32::MAX;
        if platform::current_uid() != 0 {
            assert!(matches!(p.classify(&foreign), KillPolicy::Protected(_)), "чужой SID → u32::MAX → защищён");
        }
    }
}

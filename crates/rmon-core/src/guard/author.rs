//! «Кто автор процесса»: цепочка родителей до сессии, рабочий каталог, проект (корень git),
//! сессия Claude Code или M Code, терминал, хук, LaunchAgent.
//!
//! Из окружения читаются только ключи [`ENV_KEYS`]: там же лежат токены и почта, им в журнал нельзя.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex_lite::Regex;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::model::{ProcessInfo, Snapshot};
use crate::platform::Platform;

/// Переменные окружения, которые читаются у жертвы и её сессии.
pub const ENV_KEYS: &[&str] = &[
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_HOST_SESSION_ID",
    "CLAUDE_CODE_SESSION_NAME",
    "CLAUDE_CODE_ENTRYPOINT",
    "MCODE_SESSION_ID",
    "TERM_PROGRAM",
    "XPC_SERVICE_NAME",
    "__CFBundleIdentifier",
];

/// Окружение процесса — только ключи [`ENV_KEYS`].
pub type Env = HashMap<String, String>;

/// Звено цепочки родителей.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, rename = "AuthorLink")]
pub struct Link {
    /// Процесс.
    pub pid: i32,
    /// Имя.
    pub name: String,
    /// Строка запуска (≤ 200 символов).
    pub args: String,
}

/// Откуда процесс.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum Origin {
    /// Из сессии Claude Code (через bash-инструмент или хук).
    ClaudeCode {
        /// `bash` или `hook`.
        via: String,
        /// Id сессии.
        session_id: Option<String>,
        /// Id сессии приложения.
        host_session_id: Option<String>,
        /// Название сессии.
        title: Option<String>,
        /// Точка входа.
        entrypoint: Option<String>,
        /// Процесс сессии.
        session_pid: i32,
    },
    /// Из сессии M Code.
    Mcode {
        /// Id сессии.
        session_id: Option<String>,
        /// Процесс сессии.
        session_pid: Option<i32>,
    },
    /// Из терминала.
    Terminal {
        /// Какого.
        app: String,
    },
    /// Из LaunchAgent.
    LaunchAgent {
        /// Метка.
        label: String,
    },
    /// Родитель умер.
    Orphan,
    /// Не опознали.
    Unknown {
        /// Верх цепочки.
        top: String,
    },
}

/// Всё про автора жертвы.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Author {
    /// Строка запуска.
    pub command: String,
    /// Рабочий каталог.
    pub cwd: Option<String>,
    /// Корень git.
    pub git_root: Option<String>,
    /// Проект.
    pub project: Option<String>,
    /// Полоса или ворктри.
    pub lane: Option<String>,
    /// Каталог сессии.
    pub session_cwd: Option<String>,
    /// Откуда.
    pub origin: Origin,
    /// Цепочка от жертвы вверх.
    pub chain: Vec<Link>,
}

fn base(p: &ProcessInfo) -> String {
    p.exe.as_deref().and_then(|e| Path::new(e).file_name()).and_then(|n| n.to_str()).unwrap_or(&p.name).to_string()
}

static CLAUDE_ARGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"claude-code/[^ ]*/claude(\s|$)").unwrap());
static MCODE_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"M Code Desktop[^/]*\.app/").unwrap());
static MCODE_ARGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(^|/)mcode-engine(\s|$)").unwrap());
static HOOKISH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(\.claude/hooks/|Programming - Main/Hooks/|/hooks?/[^ ]+\.(ts|js|sh|py)(\s|$))").unwrap()
});
static RESUME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"--resume[= ]([0-9a-f-]{36})").unwrap());
static TERMINALS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        (r"Terminal\.app/", "Terminal"),
        (r"iTerm[^/]*\.app/", "iTerm"),
        (r"Ghostty\.app/", "Ghostty"),
        (r"Warp[^/]*\.app/", "Warp"),
        (r"WezTerm\.app/", "WezTerm"),
        (r"kitty\.app/", "kitty"),
        (r"Alacritty\.app/", "Alacritty"),
        (r"(Visual Studio Code|Cursor|Zed)[^/]*\.app/", "редактор"),
        (r"(^|/)tmux$", "tmux"),
    ]
    .into_iter()
    .map(|(r, n)| (Regex::new(r).unwrap(), n))
    .collect()
});

fn is_claude_cli(p: &ProcessInfo) -> bool {
    base(p) == "claude" || CLAUDE_ARGS.is_match(&p.command_line())
}

fn is_mcode(p: &ProcessInfo) -> bool {
    let b = base(p);
    b == "mcode"
        || b == "mcode-engine"
        || p.exe.as_deref().is_some_and(|e| MCODE_PATH.is_match(e))
        || MCODE_ARGS.is_match(&p.command_line())
}

/// Цепочка от процесса вверх, до init (сам процесс — первым).
pub fn chain_of(snap: &Snapshot, pid: i32) -> Vec<&ProcessInfo> {
    let mut out = Vec::new();
    let mut cur = snap.process(pid);
    while let Some(p) = cur {
        if out.len() >= 64 {
            break;
        }
        out.push(p);
        if p.ppid <= 1 {
            break;
        }
        cur = snap.process(p.ppid);
    }
    out
}

/// Откуда процесс: по цепочке (от жертвы вверх) и окружению жертвы и её сессии.
pub fn origin_of(chain: &[&ProcessInfo], victim_env: &Env, session_env: &Env) -> Origin {
    let get = |e: &Env, k: &str| e.get(k).cloned();
    if let Some(at) = chain.iter().position(|p| is_claude_cli(p))
        && at > 0
    {
        let cli = chain[at];
        let cmd = cli.command_line();
        let resume = RESUME.captures(&cmd).map(|c| c[1].to_string());
        let hook = chain[1..at].iter().any(|p| HOOKISH.is_match(&p.command_line()));
        return Origin::ClaudeCode {
            via: if hook { "hook" } else { "bash" }.into(),
            session_id: get(victim_env, "CLAUDE_CODE_SESSION_ID").or(resume),
            host_session_id: get(victim_env, "CLAUDE_CODE_HOST_SESSION_ID")
                .or_else(|| get(session_env, "CLAUDE_CODE_HOST_SESSION_ID")),
            title: get(session_env, "CLAUDE_CODE_SESSION_NAME").or_else(|| get(victim_env, "CLAUDE_CODE_SESSION_NAME")),
            entrypoint: get(victim_env, "CLAUDE_CODE_ENTRYPOINT")
                .or_else(|| get(session_env, "CLAUDE_CODE_ENTRYPOINT")),
            session_pid: cli.pid,
        };
    }
    let mcode_at = chain.iter().position(|p| is_mcode(p)).filter(|&i| i > 0);
    if mcode_at.is_some() || victim_env.contains_key("MCODE_SESSION_ID") {
        return Origin::Mcode {
            session_id: get(victim_env, "MCODE_SESSION_ID"),
            session_pid: mcode_at.map(|i| chain[i].pid),
        };
    }
    for p in chain {
        if let Some(exe) = &p.exe
            && let Some((_, app)) = TERMINALS.iter().find(|(re, _)| re.is_match(exe))
        {
            return Origin::Terminal { app: (*app).into() };
        }
    }
    if let Some(app) = get(victim_env, "TERM_PROGRAM") {
        return Origin::Terminal { app };
    }
    if let Some(label) = get(victim_env, "XPC_SERVICE_NAME")
        && label != "0"
        && !label.starts_with("application.")
    {
        return Origin::LaunchAgent { label };
    }
    match chain.last() {
        Some(top) if top.ppid == 1 && chain.len() <= 2 && !top.exe.as_deref().unwrap_or("").contains(".app/") => {
            Origin::Orphan
        }
        Some(top) => Origin::Unknown { top: base(top) },
        None => Origin::Unknown { top: "?".into() },
    }
}

static LANE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"TEMPORARY/mcode/lanes/([^/]+)|\.claude/worktrees/([^/]+)").unwrap());
static PROJECT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Programming - Main/([^/]+)").unwrap());
static SCRATCH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-Programming---Main-([^/]+)").unwrap());

/// Проект и полоса по пути: каталог под «Programming - Main», полоса/ворктри, слаг скретчпада Claude.
pub fn project_from_path(path: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(path) = path else { return (None, None) };
    let lane = LANE.captures(path).and_then(|c| c.get(1).or_else(|| c.get(2))).map(|m| m.as_str().to_string());
    let mut project = PROJECT.captures(path).map(|c| c[1].to_string()).filter(|p| p != "TEMPORARY");
    if project.is_none() {
        project = SCRATCH.captures(path).map(|c| c[1].replace('-', " "));
    }
    (project, lane)
}

/// Корень git: ближайший предок с `.git`.
pub fn git_root(dir: &str) -> Option<String> {
    let mut cur = Some(PathBuf::from(dir));
    while let Some(d) = cur {
        if d.join(".git").exists() {
            return Some(d.to_string_lossy().into_owned());
        }
        cur = d.parent().map(Path::to_path_buf);
    }
    None
}

/// Всё про автора — только в момент выбора жертвы (несколько системных вызовов).
pub fn gather(snap: &Snapshot, pid: i32, platform: &mut dyn Platform) -> Author {
    let chain = chain_of(snap, pid);
    let victim = chain.first().copied();
    let mut env = |p: i32| platform.env(p, ENV_KEYS).unwrap_or_default().into_iter().collect::<Env>();
    let victim_env = env(pid);
    let cli_pid = chain.iter().find(|p| is_claude_cli(p)).map(|p| p.pid);
    let session_env = cli_pid.map(&mut env).unwrap_or_default();
    let origin = origin_of(&chain, &victim_env, &session_env);
    let session_pid = match &origin {
        Origin::ClaudeCode { session_pid, .. } => Some(*session_pid),
        Origin::Mcode { session_pid, .. } => *session_pid,
        _ => None,
    };
    let cwd = platform.cwd(pid).ok().flatten();
    let session_cwd = session_pid.and_then(|p| platform.cwd(p).ok().flatten());
    let root = cwd.as_deref().and_then(git_root);
    let command = victim.map(ProcessInfo::command_line).unwrap_or_default();
    let (p_cwd, l_cwd) = project_from_path(cwd.as_deref());
    let (p_args, l_args) = project_from_path(Some(&command));
    let (p_sess, _) = project_from_path(session_cwd.as_deref());
    let root_name = root.as_deref().and_then(|r| Path::new(r).file_name()).map(|n| n.to_string_lossy().into_owned());
    Author {
        command,
        cwd,
        git_root: root,
        project: p_cwd.or(p_args).or(p_sess).or(root_name),
        lane: l_cwd.or(l_args),
        session_cwd,
        origin,
        chain: chain
            .iter()
            .map(|p| Link { pid: p.pid, name: base(p), args: p.command_line().chars().take(200).collect() })
            .collect(),
    }
}

/// Одной строкой для уведомления: «проект M Code (полоса X), сессия Claude Code «…»».
pub fn describe(a: &Author) -> String {
    let mut parts = Vec::new();
    if let Some(p) = &a.project {
        parts.push(match &a.lane {
            Some(l) => format!("проект {p} ({l})"),
            None => format!("проект {p}"),
        });
    }
    parts.push(match &a.origin {
        Origin::ClaudeCode { via, session_id, host_session_id, title, session_pid, .. } => {
            let name = title
                .as_ref()
                .map(|t| format!("«{t}»"))
                .or_else(|| host_session_id.clone())
                .or_else(|| session_id.clone())
                .unwrap_or_else(|| format!("pid {session_pid}"));
            format!("{} Claude Code {name}", if via == "hook" { "хук сессии" } else { "сессия" })
        }
        Origin::Mcode { session_id, .. } => match session_id {
            Some(s) => format!("сессия M Code {s}"),
            None => "сессия M Code".into(),
        },
        Origin::Terminal { app } => format!("терминал {app}"),
        Origin::LaunchAgent { label } => format!("LaunchAgent {label}"),
        Origin::Orphan => "сирота (родитель умер)".into(),
        Origin::Unknown { top } => format!("запущен из {top}"),
    });
    parts.join(", ")
}

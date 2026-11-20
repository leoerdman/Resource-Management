//! Сторож памяти: в красной зоне сам снимает самый большой прогон из списка «можно».
//!
//! Перенос `~/Programming - Main/Hooks/memory-guard` (Bun + LaunchAgent) в ядро rmon.
//! Логика та же — и проверена теми же сценариями (см. тесты):
//! 1. **Зона** по настоящему давлению памяти, а не по проценту ОЗУ: красная — давление
//!    критическое, или доступно ≤ `min_available_pct`, или давление жёлтое и своп ≥ `swap_red_mb`.
//! 2. **Гистерезис:** действует, только если красное держится `sustain_secs` подряд.
//! 3. **Жертва** — самый большой по footprint процесс из `allow` (tsgo, tsc, vitest, jest,
//!    node и bun прогонов), не из `never`, не сервер/watch, не меньше `min_victim_mb`, и —
//!    дополнительно к старому сторожу — со свободной политикой снятия rmon.
//! 4. **Снятие** через защитный слой rmon (`kill_with`, `Requester::Guard`): сверка личности,
//!    SIGTERM → `term_grace_secs` → SIGKILL. Одна жертва за раз, потом `after_kill_secs`
//!    паузы; не больше `limits` убийств в окне.
//! 5. **Автор** жертвы ([`author`]): проект, сессия Claude Code / M Code, терминал, LaunchAgent.
//!
//! Здесь — чистая логика (зона, гистерезис, лимит, выбор жертвы, машина состояний [`Guard`]);
//! поток, блокировка «один сторож на машину», журнал и уведомления — в [`runner`].

pub mod author;
pub mod legacy;
pub mod runner;

use std::collections::HashSet;
use std::path::Path;

use regex_lite::Regex;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::config::RuleSpec;
use crate::fmt;
use crate::model::{MIB, Pressure, ProcessInfo, Snapshot, SystemMemory};
use crate::policy::{KillPolicy, policy_for};

pub use author::Author;

/// Когда зона красная.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default, deny_unknown_fields)]
#[ts(export, rename = "GuardRed")]
pub struct RedConfig {
    /// Уровень давления (1 норма, 2 жёлтое, 4 красное), который сам по себе красный.
    pub pressure_level: u8,
    /// Страховка: доступно не больше, %.
    pub min_available_pct: u8,
    /// Страховка: своп не меньше, МБ…
    pub swap_red_mb: u64,
    /// …при давлении от этого уровня.
    pub swap_red_from_level: u8,
}

impl Default for RedConfig {
    fn default() -> Self {
        Self { pressure_level: 4, min_available_pct: 5, swap_red_mb: 12 * 1024, swap_red_from_level: 2 }
    }
}

/// Предел убийств в окне.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export, rename = "GuardLimit")]
pub struct Limit {
    /// Окно, секунды.
    pub window_secs: u64,
    /// Не больше стольких убийств.
    pub max: u32,
}

/// Правило «можно»: `exe` — по имени исполняемого файла; `args` — по строке запуска;
/// `args_not` — исключение (серверы, watch); `parent_not` — по пути исполняемого файла
/// родителя (не трогать внутренности приложений и серверы под launchd).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct AllowRule {
    /// Вид прогона — попадает в уведомление («Убит tsgo 4,6 ГБ»).
    pub name: String,
    /// Имя исполняемого файла.
    pub exe: String,
    /// Строка запуска должна совпасть.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    /// Строка запуска не должна совпасть.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args_not: Option<String>,
    /// Путь родителя не должен совпасть.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_not: Option<String>,
}

/// Настройки сторожа — секция `[guard]` в `config.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default, deny_unknown_fields)]
#[ts(export)]
pub struct GuardConfig {
    /// Сторож работает.
    pub enabled: bool,
    /// Проба: только журнал и уведомление «убил бы».
    pub dry_run: bool,
    /// Уведомлять о снятии.
    pub notify: bool,
    /// Уведомлять и в пробе.
    pub notify_dry_run: bool,
    /// Сколько секунд красное должно держаться.
    #[ts(type = "number")]
    pub sustain_secs: u64,
    /// Пауза после снятия.
    #[ts(type = "number")]
    pub after_kill_secs: u64,
    /// От SIGTERM до SIGKILL.
    #[ts(type = "number")]
    pub term_grace_secs: u64,
    /// Красная зона.
    pub red: RedConfig,
    /// Жертва меньше — не трогаем: её смерть не спасёт машину.
    #[ts(type = "number")]
    pub min_victim_mb: u64,
    /// Защита от каскада.
    pub limits: Vec<Limit>,
    /// Ротация журнала сторожа.
    #[ts(type = "number")]
    pub log_max_bytes: u64,
    /// «Можно».
    pub allow: Vec<AllowRule>,
    /// «Никогда» — совпало хоть одно поле. Тот же список политика rmon считает
    /// «только с подтверждением» и для ручного снятия.
    pub never: Vec<RuleSpec>,
}

fn allow(name: &str, exe: &str, args: Option<&str>, args_not: Option<&str>, parent_not: Option<&str>) -> AllowRule {
    AllowRule {
        name: name.into(),
        exe: exe.into(),
        args: args.map(Into::into),
        args_not: args_not.map(Into::into),
        parent_not: parent_not.map(Into::into),
    }
}

fn never(name: &str, exe: Option<&str>, path: Option<&str>, args: Option<&str>) -> RuleSpec {
    RuleSpec { name: name.into(), exe: exe.map(Into::into), path: path.map(Into::into), args: args.map(Into::into) }
}

/// Серверы и watch — не прогоны.
const SERVERISH: &str = r"(^|\s)(serve|dev|start|preview)(\s|$)|--(watch|hot|inspect)\b|mcp-server|(^|/)server\.[cm]?[jt]s(\s|$)";
/// Родители, под которыми node/bun — не прогон: launchd, приложения, сессии агентов.
const APPISH_PARENT: &str = r"^/sbin/launchd$|^/Applications/|/[Cc]laude\.app/Contents/MacOS/|(^|/)(claude|mcode|mcode-engine)$";

/// Встроенное «можно» — перенесено из `memory-guard/config.json` как есть.
pub fn default_allow() -> Vec<AllowRule> {
    vec![
        allow("tsgo", "^tsgo$", None, Some(r"(^|\s)--(lsp|watch)(\s|=|$)"), None),
        allow("tsc", r"^(tsc|node\d*|bun)$", Some(r"(^|/)tsc(\.js)?(\s|$)"), Some(r"(^|\s)(--watch|-w)(\s|$)"), None),
        allow("vitest", r"^(node\d*|bun)$", Some(r"(^|/)vitest(\.m?js)?(\s|$)"), Some(r"(^|\s)--watch(\s|$)"), None),
        allow("jest", r"^(node\d*|bun)$", Some(r"(^|/)jest(\.m?js)?(\s|$)"), Some(r"(^|\s)--watch(All)?(\s|$)"), None),
        allow(
            "node",
            r"^node\d*$",
            None,
            Some(&format!(r"{SERVERISH}|(^|/)(vite|next|nuxt|astro|webpack-dev-server)(\.m?js)?(\s|$)")),
            Some(APPISH_PARENT),
        ),
        allow("bun", "^bun$", None, Some(&format!("{SERVERISH}|mcode-engine")), Some(APPISH_PARENT)),
    ]
}

/// Встроенное «никогда» по ОС: приложения владельца, его агенты и сторожа.
pub fn default_never() -> Vec<RuleSpec> {
    let mut v = vec![
        never("сессии claude CLI", Some(r"^claude(\.exe)?$"), None, None),
        never("движок M Code", Some(r"^mcode(-engine)?(\.exe)?$"), None, Some(r"(^|/)mcode-engine(\s|$)")),
        never("сам rmon", None, Some(r"/Resource Monitor\.app/|(^|[/\\])rmon(-app)?(\.exe)?$"), None),
        never("старый сторож памяти", None, None, Some(r"memory-guard/guard\.ts")),
        never("хуки сторожа машины", None, None, Some(r"Hooks/machine-guard/")),
    ];
    if cfg!(target_os = "macos") {
        v.extend([
            never("системные процессы", None, Some(r"^/(System|usr/libexec|usr/sbin|sbin|usr/bin|bin|Library/Apple)/"), None),
            never("WindowServer и ядро", Some(r"^(WindowServer|kernel_task|launchd|loginwindow)$"), None, None),
            never("Claude и его хелперы", None, Some(r"/[Cc]laude\.app/"), None),
            never("M Code Desktop", None, Some(r"/M Code Desktop[^/]*\.app/"), None),
            never("Telegram", None, Some(r"/Telegram[^/]*\.app/"), None),
            never(
                "браузеры",
                None,
                Some(r"/(Google Chrome|Safari|Arc|Firefox|Brave Browser|Microsoft Edge|Yandex|Opera|Chromium|Orion|Zen|Dia|Comet)[^/]*\.app/"),
                None,
            ),
            never("редакторы", None, Some(r"/(Visual Studio Code|Cursor|Windsurf|Zed|Xcode)[^/]*\.app/"), None),
        ]);
    } else if cfg!(windows) {
        v.extend([
            never("системные процессы", None, Some(r"(?i)^[a-z]:\\Windows\\"), None),
            never("оболочка Windows", Some(r"(?i)^explorer(\.exe)?$"), None, None),
            never(
                "приложения владельца",
                Some(r"(?i)^(chrome|msedge|firefox|brave|opera|Code|Cursor|Windsurf|Zed|Telegram|Claude)(\.exe)?$"),
                None,
                None,
            ),
        ]);
    } else {
        v.extend([
            never("системные процессы", None, Some(r"^/(usr/lib/systemd|lib/systemd|usr/libexec|usr/sbin|sbin|usr/bin|bin)/"), None),
            never(
                "приложения владельца",
                Some(r"^(chrome|chromium|google-chrome|firefox|brave|opera|code|cursor|windsurf|zed|telegram-desktop|Telegram|claude-desktop)$"),
                None,
                None,
            ),
        ]);
    }
    v
}

impl Default for GuardConfig {
    /// Значения перенесены из `memory-guard/config.json` владельца.
    fn default() -> Self {
        Self {
            enabled: true,
            dry_run: false,
            notify: true,
            notify_dry_run: true,
            sustain_secs: 12,
            after_kill_secs: 20,
            term_grace_secs: 5,
            red: RedConfig::default(),
            min_victim_mb: 1024,
            limits: vec![Limit { window_secs: 60, max: 2 }, Limit { window_secs: 600, max: 6 }],
            log_max_bytes: 2 * MIB,
            allow: default_allow(),
            never: default_never(),
        }
    }
}

/// «Никогда» как правила политики: у сторожа правило совпадает по любому полю, у политики —
/// по всем, поэтому многопольное правило раскладывается на однопольные.
pub fn never_as_policy_rules(never: &[RuleSpec]) -> Vec<RuleSpec> {
    let mut out = Vec::new();
    for r in never {
        for (exe, path, args) in [(r.exe.clone(), None, None), (None, r.path.clone(), None), (None, None, r.args.clone())] {
            if exe.is_some() || path.is_some() || args.is_some() {
                out.push(RuleSpec { name: r.name.clone(), exe, path, args });
            }
        }
    }
    out
}

// ------------------------------------------------------------------ зона

/// Зона памяти.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename = "GuardZone")]
pub enum Zone {
    /// Норма.
    Green,
    /// Давление жёлтое.
    Yellow,
    /// Пора действовать (если продержится).
    Red,
}

/// Уровень ядра для давления: 1 / 2 / 4.
pub fn pressure_level(p: Pressure) -> u8 {
    match p {
        Pressure::Normal => 1,
        Pressure::Warning => 2,
        Pressure::Critical => 4,
    }
}

/// Зона и почему — человеческими словами.
pub fn judge_zone(m: &SystemMemory, red: &RedConfig) -> (Zone, String) {
    let level = pressure_level(m.pressure);
    let swap_mb = m.swap_used / MIB;
    if level >= red.pressure_level {
        return (Zone::Red, "давление красное".into());
    }
    if m.available_pct <= red.min_available_pct {
        return (Zone::Red, format!("доступно {} %", m.available_pct));
    }
    if level >= red.swap_red_from_level && swap_mb >= red.swap_red_mb {
        return (Zone::Red, format!("давление жёлтое и своп {}", fmt::bytes(m.swap_used)));
    }
    if level >= 2 {
        return (Zone::Yellow, "давление жёлтое".into());
    }
    (Zone::Green, "норма".into())
}

/// Красная зона должна держаться `sustain` подряд; любой не-красный опрос сбрасывает отсчёт.
#[derive(Debug, Clone, Default)]
pub struct Hysteresis {
    since_ms: Option<u64>,
}

impl Hysteresis {
    /// `true` — пора действовать.
    pub fn feed(&mut self, zone: Zone, now_ms: u64, sustain_ms: u64) -> bool {
        if zone != Zone::Red {
            self.since_ms = None;
            return false;
        }
        let since = *self.since_ms.get_or_insert(now_ms);
        now_ms.saturating_sub(since) >= sustain_ms
    }

    /// После снятия — отсчёт заново.
    pub fn reset(&mut self) {
        self.since_ms = None;
    }

    /// Сколько уже красное.
    pub fn red_for_ms(&self, now_ms: u64) -> u64 {
        self.since_ms.map_or(0, |s| now_ms.saturating_sub(s))
    }
}

/// Не больше `max` убийств за окно; несколько окон сразу.
#[derive(Debug, Clone, Default)]
pub struct KillLimiter {
    stamps: Vec<u64>,
}

impl KillLimiter {
    /// `None` — можно; иначе — какое окно упёрлось.
    pub fn blocked(&self, limits: &[Limit], now_ms: u64) -> Option<String> {
        limits.iter().find_map(|l| {
            let n = self.stamps.iter().filter(|&&t| now_ms.saturating_sub(t) < l.window_secs * 1000).count();
            (n >= l.max as usize).then(|| format!("{n} убийств за {} с (предел {})", l.window_secs, l.max))
        })
    }

    /// Отметить снятие.
    pub fn record(&mut self, limits: &[Limit], now_ms: u64) {
        self.stamps.push(now_ms);
        let longest = limits.iter().map(|l| l.window_secs).max().unwrap_or(0) * 1000;
        self.stamps.retain(|&t| now_ms.saturating_sub(t) < longest);
    }
}

// ------------------------------------------------------------------ жертва

/// Кандидат в жертвы.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, rename = "GuardCandidate")]
pub struct Candidate {
    /// Процесс.
    pub pid: i32,
    /// Время запуска.
    #[ts(type = "number")]
    pub started_at: u64,
    /// Вид прогона (имя правила «можно»).
    pub kind: String,
    /// Память, байты.
    #[ts(type = "number")]
    pub mem: u64,
    /// Строка запуска.
    pub cmd: String,
}

/// Крупный процесс, которого сторож не тронет, и почему.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, rename = "GuardRejected")]
pub struct Rejected {
    /// Процесс.
    pub pid: i32,
    /// Имя.
    pub name: String,
    /// Память, байты.
    #[ts(type = "number")]
    pub mem: u64,
    /// Почему не жертва.
    pub why: String,
}

/// Скомпилированные списки сторожа.
#[derive(Debug, Clone)]
pub struct Rules {
    allow: Vec<CompiledAllow>,
    never: Vec<CompiledNever>,
    min_victim: u64,
}

#[derive(Debug, Clone)]
struct CompiledAllow {
    name: String,
    exe: Regex,
    args: Option<Regex>,
    args_not: Option<Regex>,
    parent_not: Option<Regex>,
}

#[derive(Debug, Clone)]
struct CompiledNever {
    name: String,
    exe: Option<Regex>,
    path: Option<Regex>,
    args: Option<Regex>,
}

fn re(field: &str, name: &str, v: Option<&str>) -> Result<Option<Regex>, String> {
    v.map(|s| Regex::new(s).map_err(|e| format!("сторож, правило «{name}», поле {field}: {e}"))).transpose()
}

impl Rules {
    /// Скомпилировать; битое выражение — ошибка сразу, а не в красной зоне.
    pub fn compile(cfg: &GuardConfig) -> Result<Self, String> {
        let allow = cfg
            .allow
            .iter()
            .map(|r| {
                Ok(CompiledAllow {
                    name: r.name.clone(),
                    exe: re("exe", &r.name, Some(&r.exe))?.expect("exe задан"),
                    args: re("args", &r.name, r.args.as_deref())?,
                    args_not: re("args_not", &r.name, r.args_not.as_deref())?,
                    parent_not: re("parent_not", &r.name, r.parent_not.as_deref())?,
                })
            })
            .collect::<Result<_, String>>()?;
        let never = cfg
            .never
            .iter()
            .map(|r| {
                Ok(CompiledNever {
                    name: r.name.clone(),
                    exe: re("exe", &r.name, r.exe.as_deref())?,
                    path: re("path", &r.name, r.path.as_deref())?,
                    args: re("args", &r.name, r.args.as_deref())?,
                })
            })
            .collect::<Result<_, String>>()?;
        Ok(Self { allow, never, min_victim: cfg.min_victim_mb * MIB })
    }
}

/// Имя исполняемого файла: из пути, иначе имя процесса.
fn exe_base(p: &ProcessInfo) -> &str {
    p.exe.as_deref().and_then(|e| Path::new(e).file_name()).and_then(|n| n.to_str()).unwrap_or(&p.name)
}

fn never_match<'a>(p: &ProcessInfo, cmd: &str, rules: &'a [CompiledNever]) -> Option<&'a str> {
    let base = exe_base(p);
    let path = p.exe.as_deref().unwrap_or("");
    rules
        .iter()
        .find(|r| {
            r.exe.as_ref().is_some_and(|x| x.is_match(base))
                || r.path.as_ref().is_some_and(|x| !path.is_empty() && x.is_match(path))
                || r.args.as_ref().is_some_and(|x| x.is_match(cmd))
        })
        .map(|r| r.name.as_str())
}

fn allow_match(p: &ProcessInfo, cmd: &str, parent_exe: Option<&str>, rules: &[CompiledAllow]) -> Result<String, String> {
    let base = exe_base(p);
    let mut why = None;
    for r in rules {
        if !r.exe.is_match(base) {
            continue;
        }
        if r.args.as_ref().is_some_and(|x| !x.is_match(cmd)) {
            continue;
        }
        if r.args_not.as_ref().is_some_and(|x| x.is_match(cmd)) {
            why.get_or_insert_with(|| format!("{}: похоже на сервер или watch", r.name));
            continue;
        }
        if let (Some(x), Some(parent)) = (&r.parent_not, parent_exe)
            && x.is_match(parent)
        {
            let pb = Path::new(parent).file_name().and_then(|n| n.to_str()).unwrap_or(parent);
            why.get_or_insert_with(|| format!("{}: родитель {pb} — не прогон", r.name));
            continue;
        }
        return Ok(r.name.clone());
    }
    Err(why.unwrap_or_else(|| "не в списке «можно»".into()))
}

/// Сам процесс и вся цепочка его предков — неприкосновенны.
pub fn self_and_ancestors(snap: &Snapshot, me: i32) -> HashSet<i32> {
    let mut out = HashSet::from([me]);
    out.extend(snap.ancestors(me).iter().map(|p| p.pid));
    out
}

/// Кандидаты по убыванию памяти и заметные (≥ 500 МБ) отклонённые.
pub fn rank_victims(snap: &Snapshot, rules: &Rules, exclude: &HashSet<i32>) -> (Vec<Candidate>, Vec<Rejected>) {
    rank_victims_with(snap, rules, exclude, policy_for)
}

fn rank_victims_with(
    snap: &Snapshot,
    rules: &Rules,
    exclude: &HashSet<i32>,
    policy: impl Fn(&ProcessInfo) -> KillPolicy,
) -> (Vec<Candidate>, Vec<Rejected>) {
    let mut candidates = Vec::new();
    let mut rejected = Vec::new();
    for p in &snap.processes {
        let mem = p.reclaim();
        let cmd = p.command_line();
        let mut note = |why: String| {
            if mem >= 500 * MIB {
                rejected.push(Rejected { pid: p.pid, name: exe_base(p).to_string(), mem, why });
            }
        };
        if p.pid <= 1 || exclude.contains(&p.pid) {
            note("сторож или его предок".into());
            continue;
        }
        if let Some(name) = never_match(p, &cmd, &rules.never) {
            note(format!("никогда: {name}"));
            continue;
        }
        let parent_exe = match snap.process(p.ppid) {
            // Как у старого сторожа: путь родителя, а если его не видно — имя.
            Some(parent) => parent.exe.clone().or_else(|| Some(parent.name.clone())),
            None if p.ppid == 1 => Some("/sbin/launchd".into()),
            None => None,
        };
        let kind = match allow_match(p, &cmd, parent_exe.as_deref(), &rules.allow) {
            Ok(kind) => kind,
            Err(why) => {
                if matches!(exe_base(p), "tsgo" | "tsc" | "bun") || exe_base(p).starts_with("node") {
                    note(why);
                }
                continue;
            }
        };
        if mem < rules.min_victim {
            note(format!("меньше порога {}", fmt::bytes(rules.min_victim)));
            continue;
        }
        if let pol @ (KillPolicy::Protected(_) | KillPolicy::Confirm(_)) = policy(p) {
            note(format!("политика rmon: {} ({})", pol.label(), pol.reason()));
            continue;
        }
        candidates.push(Candidate { pid: p.pid, started_at: p.started_at, kind, mem, cmd: crate::analyze::short_cmd(p) });
    }
    candidates.sort_by_key(|c| std::cmp::Reverse(c.mem));
    rejected.sort_by_key(|r| std::cmp::Reverse(r.mem));
    (candidates, rejected)
}

// ------------------------------------------------------------------ машина состояний

/// Режим сторожа для человека.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
#[ts(export)]
pub enum GuardMode {
    /// Выключен в настройках.
    Off,
    /// Боевой.
    Armed,
    /// Проба: «убил бы».
    DryRun,
    /// Включён, но ждёт: работает другой сторож (старый memory-guard или второй rmon).
    Standby(String),
    /// Настройки не разобрались — сторож стоит.
    Broken(String),
}

/// Что решил сторож на этом опросе.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Ничего.
    Wait,
    /// Снять (или в пробе — сказать «убил бы»).
    Act { victim: Candidate, why: String, red_for_secs: u64 },
    /// Красно, а снять некого.
    NoVictim { why: String, rejected: Vec<Rejected> },
    /// Упёрлись в лимит убийств.
    Limited { why: String },
}

/// Сторож без потока: кормите его снимками, он говорит, что делать.
#[derive(Debug, Default)]
pub struct Guard {
    hyst: Hysteresis,
    limiter: KillLimiter,
    zone: Option<Zone>,
    pause_until_ms: u64,
    /// В этом красном эпизоде уже сказали «некого» / «лимит».
    noted: HashSet<&'static str>,
    /// На этих pid снятие упало с отказом прав — больше не выбирать.
    denied: HashSet<i32>,
}

/// Итог опроса: смена зоны (если была) и решение.
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    /// Зона сейчас.
    pub zone: Zone,
    /// Почему.
    pub zone_why: String,
    /// Зона сменилась на этом опросе.
    pub zone_changed: bool,
    /// Решение.
    pub decision: Decision,
}

impl Guard {
    /// Один опрос.
    pub fn observe(&mut self, cfg: &GuardConfig, rules: &Rules, snap: &Snapshot, self_pid: i32, now_ms: u64) -> Observation {
        self.observe_with(cfg, rules, snap, self_pid, now_ms, policy_for)
    }

    fn observe_with(
        &mut self,
        cfg: &GuardConfig,
        rules: &Rules,
        snap: &Snapshot,
        self_pid: i32,
        now_ms: u64,
        policy: impl Fn(&ProcessInfo) -> KillPolicy,
    ) -> Observation {
        let (zone, zone_why) = judge_zone(&snap.system, &cfg.red);
        let zone_changed = self.zone != Some(zone);
        self.zone = Some(zone);
        if zone != Zone::Red {
            self.noted.clear();
        }
        let obs = |decision| Observation { zone, zone_why: zone_why.clone(), zone_changed, decision };
        if now_ms < self.pause_until_ms {
            return obs(Decision::Wait);
        }
        if !self.hyst.feed(zone, now_ms, cfg.sustain_secs * 1000) {
            return obs(Decision::Wait);
        }
        if let Some(why) = self.limiter.blocked(&cfg.limits, now_ms) {
            return obs(if self.noted.insert("limit") { Decision::Limited { why } } else { Decision::Wait });
        }
        let mut exclude = self_and_ancestors(snap, self_pid);
        exclude.extend(&self.denied);
        let (candidates, rejected) = rank_victims_with(snap, rules, &exclude, policy);
        match candidates.into_iter().next() {
            Some(victim) => obs(Decision::Act { victim, why: zone_why.clone(), red_for_secs: self.hyst.red_for_ms(now_ms) / 1000 }),
            None if self.noted.insert("none") => {
                obs(Decision::NoVictim { why: zone_why.clone(), rejected: rejected.into_iter().take(8).collect() })
            }
            None => obs(Decision::Wait),
        }
    }

    /// Снятие (или «убил бы») состоялось: лимит, пауза, отсчёт заново.
    pub fn acted(&mut self, cfg: &GuardConfig, now_ms: u64) {
        self.limiter.record(&cfg.limits, now_ms);
        self.hyst.reset();
        self.pause_until_ms = now_ms + cfg.after_kill_secs * 1000;
    }

    /// Снять не вышло: нет прав — этот pid больше не предлагать.
    pub fn denied(&mut self, pid: i32) {
        self.denied.insert(pid);
    }

    /// Текущая зона.
    pub fn zone(&self) -> Option<Zone> {
        self.zone
    }

    /// Сколько уже красное.
    pub fn red_for_secs(&self, now_ms: u64) -> u64 {
        self.hyst.red_for_ms(now_ms) / 1000
    }
}

#[cfg(test)]
mod tests;

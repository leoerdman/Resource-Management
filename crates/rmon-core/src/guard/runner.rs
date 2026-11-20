//! Исполнитель сторожа: свой поток, блокировка «один сторож на машину», перечитывание
//! настроек на лету, снятие через защитный слой, журнал `guard.jsonl` и уведомления.
//!
//! Монитор отдаёт сюда каждый снимок ([`GuardHandle::feed`], без ожидания: если сторож
//! занят снятием, снимок пропускается). Снятие идёт в потоке сторожа — окно не замирает
//! на секунды ожидания SIGTERM.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::author::{self, Author};
use super::legacy::{self, Legacy};
use super::{Candidate, Decision, Guard, GuardConfig, GuardMode, Rejected, Rules, Zone, pressure_level};
use crate::config::{self, Config};
use crate::error::Error;
use crate::fmt;
use crate::kill::{KillRequest, Requester, kill_with};
use crate::model::Snapshot;
use crate::platform::{self, Platform};

/// Как часто проверять, жив ли старый сторож (это запуск `launchctl`).
const LEGACY_CHECK: Duration = Duration::from_secs(30);
/// Сколько последних событий держать в памяти для окна.
const RECENT: usize = 50;

/// Давление в момент события.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, rename = "GuardPressure")]
pub struct PressureFields {
    /// Уровень ядра 1 / 2 / 4.
    pub level: u8,
    /// Доступно, %.
    pub available_pct: u8,
    /// Своп, байты.
    #[ts(type = "number")]
    pub swap_used: u64,
}

/// Уведомление для человека — решает сторож, показывает окно.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Notice {
    /// Заголовок.
    pub title: String,
    /// Подзаголовок.
    pub subtitle: String,
    /// Текст.
    pub body: String,
}

/// Что случилось.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "ev", rename_all = "kebab-case")]
#[ts(export, rename = "GuardEventKind")]
pub enum EventKind {
    /// Сторож заступил.
    Start {
        /// Режим.
        mode: GuardMode,
    },
    /// Сторож сменил режим (выключен, проба, ожидание).
    Mode {
        /// Режим.
        mode: GuardMode,
    },
    /// Смена зоны.
    Zone {
        /// Зона.
        zone: Zone,
        /// Почему.
        why: String,
    },
    /// Снял.
    Kill {
        /// `term`, `kill`, `alive`, `gone`, `reused`, `denied`, `refused`.
        outcome: String,
        /// Ошибка, если была.
        error: Option<String>,
        /// Жертва.
        victim: Candidate,
        /// Почему зона красная.
        why: String,
        /// Сколько держалось красное.
        #[ts(type = "number")]
        red_for_secs: u64,
        /// Чей процесс.
        author: Author,
    },
    /// Проба: снял бы.
    WouldKill {
        /// Жертва.
        victim: Candidate,
        /// Почему зона красная.
        why: String,
        /// Сколько держалось красное.
        #[ts(type = "number")]
        red_for_secs: u64,
        /// Чей процесс.
        author: Author,
    },
    /// Красно, а снять некого.
    NoVictim {
        /// Почему зона красная.
        why: String,
        /// Крупные, кого не тронул.
        rejected: Vec<Rejected>,
    },
    /// Упёрлись в лимит.
    Limit {
        /// Какой.
        why: String,
    },
    /// Настройки не разобрались — работаем на прежних.
    ConfigError {
        /// Ошибка.
        error: String,
    },
}

/// Событие сторожа — строка `guard.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GuardEvent {
    /// Время, unix-секунды.
    #[ts(type = "number")]
    pub ts: u64,
    /// Что.
    #[serde(flatten)]
    pub kind: EventKind,
    /// Давление.
    pub pressure: Option<PressureFields>,
    /// Показать уведомление.
    pub notice: Option<Notice>,
}

impl GuardEvent {
    /// Одна строка по-русски — для журнала окна и CLI.
    pub fn text(&self) -> String {
        match &self.kind {
            EventKind::Start { mode } => format!("сторож заступил: {}", mode_text(mode)),
            EventKind::Mode { mode } => format!("сторож: {}", mode_text(mode)),
            EventKind::Zone { zone, why } => format!("зона {}: {why}", zone_word(*zone)),
            EventKind::Kill { outcome, victim, author, error, .. } => {
                let what = match outcome.as_str() {
                    "term" | "kill" => "снят",
                    "alive" => "не завершился",
                    "gone" => "уже завершился",
                    "reused" => "подменился — не тронут",
                    "denied" => "нет прав",
                    _ => "отказ",
                };
                let err = error.as_ref().map(|e| format!(" ({e})")).unwrap_or_default();
                format!("{} {} {what}{err} — {}", victim.kind, fmt::bytes(victim.mem), author::describe(author))
            }
            EventKind::WouldKill { victim, author, .. } => {
                format!("проба: снял бы {} {} — {}", victim.kind, fmt::bytes(victim.mem), author::describe(author))
            }
            EventKind::NoVictim { why, rejected } => format!("красно ({why}), снять некого; крупных не тронуто: {}", rejected.len()),
            EventKind::Limit { why } => format!("лимит: {why}"),
            EventKind::ConfigError { error } => format!("настройки сторожа не разобраны: {error}"),
        }
    }
}

fn zone_word(z: Zone) -> &'static str {
    match z {
        Zone::Green => "зелёная",
        Zone::Yellow => "жёлтая",
        Zone::Red => "красная",
    }
}

/// Режим словами.
pub fn mode_text(m: &GuardMode) -> String {
    match m {
        GuardMode::Off => "выключен".into(),
        GuardMode::Armed => "боевой режим".into(),
        GuardMode::DryRun => "режим пробы (только «убил бы»)".into(),
        GuardMode::Standby(why) => format!("ждёт: {why}"),
        GuardMode::Broken(why) => format!("стоит: {why}"),
    }
}

/// Состояние сторожа для окна, трея, MCP и CLI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GuardStatus {
    /// Режим.
    pub mode: GuardMode,
    /// Зона.
    pub zone: Option<Zone>,
    /// Почему.
    pub zone_why: String,
    /// Сколько уже красное.
    #[ts(type = "number")]
    pub red_for_secs: u64,
    /// Старый сторож.
    pub legacy: Legacy,
    /// Действующие настройки.
    pub config: GuardConfig,
    /// Где настройки.
    pub config_file: String,
    /// Последние события, новые сверху.
    pub recent: Vec<GuardEvent>,
}

/// Журнал событий сторожа.
pub fn events_log() -> PathBuf {
    config::log_dir().join("guard.jsonl")
}

fn append_event(ev: &GuardEvent, max_bytes: u64) {
    let path = events_log();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > max_bytes) {
        let _ = std::fs::rename(&path, path.with_extension("old.jsonl"));
    }
    if let (Ok(mut f), Ok(line)) = (OpenOptions::new().create(true).append(true).open(&path), serde_json::to_string(ev)) {
        let _ = writeln!(f, "{line}");
    }
}

/// Последние события из журнала, новые сверху.
pub fn read_events(limit: usize) -> Vec<GuardEvent> {
    let Ok(text) = std::fs::read_to_string(events_log()) else { return Vec::new() };
    let mut out: Vec<GuardEvent> = text.lines().rev().filter_map(|l| serde_json::from_str(l).ok()).take(limit).collect();
    out.shrink_to_fit();
    out
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Записать флаги `[guard]` в `config.toml`, сохранив остальное содержимое файла.
pub fn persist_flags(enabled: Option<bool>, dry_run: Option<bool>, notify: Option<bool>) -> Result<(), String> {
    let path = config::config_file();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{}: {e}", path.display()))?;
    let guard = doc.entry("guard").or_insert_with(toml_edit::table);
    let table = guard.as_table_mut().ok_or_else(|| format!("{}: [guard] — не таблица", path.display()))?;
    for (key, value) in [("enabled", enabled), ("dry_run", dry_run), ("notify", notify)] {
        if let Some(v) = value {
            table[key] = toml_edit::value(v);
        }
    }
    crate::settings::write_validated(&path, &doc.to_string())
}

type Listener = Arc<dyn Fn(&GuardEvent) + Send + Sync>;

struct State {
    status: GuardStatus,
}

/// Ручка сторожа: кормить снимками, читать состояние, переключать.
#[derive(Clone)]
pub struct GuardHandle {
    tx: SyncSender<Msg>,
    state: Arc<Mutex<State>>,
}

enum Msg {
    Snapshot(Arc<Snapshot>),
    Reload,
}

impl GuardHandle {
    /// Отдать снимок; если сторож занят — снимок пропускается.
    pub fn feed(&self, snap: Arc<Snapshot>) {
        match self.tx.try_send(Msg::Snapshot(snap)) {
            Ok(()) | Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => eprintln!("rmon: поток сторожа остановлен"),
        }
    }

    /// Состояние.
    pub fn status(&self) -> GuardStatus {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).status.clone()
    }

    /// Включить/выключить, проба, уведомления — запись в `config.toml` и немедленное применение.
    pub fn set(&self, enabled: Option<bool>, dry_run: Option<bool>, notify: Option<bool>) -> Result<GuardStatus, String> {
        persist_flags(enabled, dry_run, notify)?;
        let _ = self.tx.send(Msg::Reload);
        // Применение — на следующем шаге потока; ждём его коротко, чтобы вернуть свежий статус.
        let until = Instant::now() + Duration::from_secs(2);
        let want = |s: &GuardStatus| {
            enabled.is_none_or(|v| s.config.enabled == v)
                && dry_run.is_none_or(|v| s.config.dry_run == v)
                && notify.is_none_or(|v| s.config.notify == v)
        };
        loop {
            let s = self.status();
            if want(&s) || Instant::now() > until {
                return Ok(s);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Перечитать настройки немедленно (после правки из окна).
    pub fn reload(&self) {
        let _ = self.tx.send(Msg::Reload);
    }

    /// Перенести обязанность старого сторожа в rmon.
    pub fn retire_legacy(&self) -> Result<GuardStatus, String> {
        legacy::retire()?;
        let _ = self.tx.send(Msg::Reload);
        std::thread::sleep(Duration::from_millis(200));
        Ok(self.status())
    }

    /// Вернуть старого сторожа.
    pub fn restore_legacy(&self) -> Result<GuardStatus, String> {
        legacy::restore()?;
        let _ = self.tx.send(Msg::Reload);
        std::thread::sleep(Duration::from_millis(200));
        Ok(self.status())
    }
}

/// Блокировка «один сторож на машину».
fn guard_lock() -> Option<File> {
    let path = config::log_dir().join("guard.lock");
    let _ = std::fs::create_dir_all(config::log_dir());
    let f = OpenOptions::new().create(true).truncate(false).write(true).open(&path).ok()?;
    f.try_lock().ok()?;
    let _ = f.set_len(0);
    let _ = writeln!(&f, "{}", std::process::id());
    Some(f)
}

struct Runner {
    cfg: Config,
    cfg_mtime: Option<SystemTime>,
    cfg_error: Option<String>,
    rules: Rules,
    guard: Guard,
    lock: Option<File>,
    legacy: Legacy,
    legacy_checked: Option<Instant>,
    mode: GuardMode,
    platform: Box<dyn Platform>,
    listener: Listener,
    state: Arc<Mutex<State>>,
    recent: VecDeque<GuardEvent>,
    started: bool,
}

impl Runner {
    fn mtime() -> Option<SystemTime> {
        std::fs::metadata(config::config_file()).and_then(|m| m.modified()).ok()
    }

    /// Перечитать настройки, если файл изменился. Битый файл — остаёмся на прежних.
    fn reload(&mut self, force: bool) {
        let mtime = Self::mtime();
        if !force && mtime == self.cfg_mtime {
            return;
        }
        self.cfg_mtime = mtime;
        match Config::load().map_err(|e| e.to_string()).and_then(|c| Rules::compile(&c.guard).map(|r| (c, r))) {
            Ok((cfg, rules)) => {
                config::set_global(cfg.clone());
                self.cfg = cfg;
                self.rules = rules;
                self.cfg_error = None;
            }
            Err(error) => {
                if self.cfg_error.as_ref() != Some(&error) {
                    self.cfg_error = Some(error.clone());
                    self.emit(EventKind::ConfigError { error }, None, None);
                }
            }
        }
    }

    fn decide_mode(&mut self) -> GuardMode {
        let g = &self.cfg.guard;
        if !g.enabled {
            self.lock = None;
            return GuardMode::Off;
        }
        if self.legacy_checked.is_none_or(|t| t.elapsed() >= LEGACY_CHECK) {
            self.legacy = legacy::status();
            self.legacy_checked = Some(Instant::now());
        }
        if let Legacy::Running { pid } = &self.legacy {
            self.lock = None;
            let pid = pid.map(|p| format!(", pid {p}")).unwrap_or_default();
            return GuardMode::Standby(format!("работает старый сторож memory-guard{pid} — перенесите его в rmon"));
        }
        if self.lock.is_none() {
            self.lock = guard_lock();
            if self.lock.is_none() {
                let who = std::fs::read_to_string(config::log_dir().join("guard.lock")).unwrap_or_default();
                return GuardMode::Standby(format!("сторож уже работает в другом процессе rmon (pid {})", who.trim()));
            }
        }
        if g.dry_run { GuardMode::DryRun } else { GuardMode::Armed }
    }

    fn emit(&mut self, kind: EventKind, pressure: Option<PressureFields>, notice: Option<Notice>) {
        let ev = GuardEvent { ts: now_secs(), kind, pressure, notice };
        append_event(&ev, self.cfg.guard.log_max_bytes);
        self.recent.push_front(ev.clone());
        self.recent.truncate(RECENT);
        (self.listener)(&ev);
    }

    fn publish(&self, zone_why: String, red_for_secs: u64) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.status = GuardStatus {
            mode: self.mode.clone(),
            zone: self.guard.zone(),
            zone_why,
            red_for_secs,
            legacy: self.legacy.clone(),
            config: self.cfg.guard.clone(),
            config_file: config::config_file().display().to_string(),
            recent: self.recent.iter().cloned().collect(),
        };
    }

    fn step(&mut self, snap: &Snapshot) {
        self.reload(false);
        let mode = self.decide_mode();
        if !self.started {
            self.started = true;
            self.mode = mode.clone();
            self.emit(EventKind::Start { mode: mode.clone() }, None, None);
        } else if mode != self.mode {
            self.mode = mode.clone();
            self.emit(EventKind::Mode { mode: mode.clone() }, None, None);
        }
        let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        let pressure = PressureFields {
            level: pressure_level(snap.system.pressure),
            available_pct: snap.system.available_pct,
            swap_used: snap.system.swap_used,
        };
        let cfg = self.cfg.guard.clone();
        let obs = self.guard.observe(&cfg, &self.rules, snap, std::process::id() as i32, now_ms);
        if obs.zone_changed {
            self.emit(EventKind::Zone { zone: obs.zone, why: obs.zone_why.clone() }, Some(pressure.clone()), None);
        }
        let acting = matches!(mode, GuardMode::Armed | GuardMode::DryRun);
        if acting {
            match obs.decision {
                Decision::Wait => {}
                Decision::Limited { why } => self.emit(EventKind::Limit { why }, Some(pressure.clone()), None),
                Decision::NoVictim { why, rejected } => self.emit(EventKind::NoVictim { why, rejected }, Some(pressure.clone()), None),
                Decision::Act { victim, why, red_for_secs } => self.act(&cfg, snap, victim, why, red_for_secs, pressure, now_ms),
            }
        }
        self.publish(obs.zone_why, self.guard.red_for_secs(now_ms));
    }

    #[allow(clippy::too_many_arguments)]
    fn act(&mut self, cfg: &GuardConfig, snap: &Snapshot, victim: Candidate, why: String, red_for_secs: u64, pressure: PressureFields, now_ms: u64) {
        let author = author::gather(snap, victim.pid, self.platform.as_mut());
        let line = format!("{} {} — {}, {why}", victim.kind, fmt::bytes(victim.mem), author::describe(&author));
        if cfg.dry_run {
            let notice = (cfg.notify && cfg.notify_dry_run).then(|| Notice {
                title: "Сторож памяти (проба)".into(),
                subtitle: "Убил бы, но включена проба".into(),
                body: line,
            });
            self.emit(EventKind::WouldKill { victim, why, red_for_secs, author }, Some(pressure), notice);
            self.guard.acted(cfg, now_ms);
            return;
        }
        let Some(target) = snap.process(victim.pid).filter(|p| p.started_at == victim.started_at).cloned() else {
            return;
        };
        let req = KillRequest {
            signal_term_grace: Duration::from_secs(cfg.term_grace_secs),
            confirm: false,
            requested_by: Requester::Guard,
            reason: format!("сторож: {why}"),
            dry_run: false,
        };
        let (outcome, error) = match kill_with(self.platform.as_mut(), &target, req) {
            Ok(o) if o.exited => (if o.signal == platform::Signal::Kill { "kill" } else { "term" }, None),
            Ok(_) => ("alive", None),
            Err(Error::ProcessGone { .. }) => ("gone", None),
            Err(Error::IdentityMismatch { .. }) => ("reused", None),
            Err(e @ Error::Permission { .. }) => ("denied", Some(e.to_string())),
            Err(e) => ("refused", Some(e.to_string())),
        };
        let killed = matches!(outcome, "term" | "kill");
        let notice = (killed && cfg.notify).then(|| Notice {
            title: "Сторож памяти".into(),
            subtitle: format!("Убит {} {}", victim.kind, fmt::bytes(victim.mem)),
            body: line,
        });
        let pid = victim.pid;
        self.emit(
            EventKind::Kill { outcome: outcome.into(), error, victim, why, red_for_secs, author },
            Some(pressure),
            notice,
        );
        match outcome {
            "term" | "kill" | "alive" => self.guard.acted(cfg, now_ms),
            "denied" | "refused" => self.guard.denied(pid),
            _ => {}
        }
    }

    fn run(mut self, rx: Receiver<Msg>) {
        while let Ok(msg) = rx.recv() {
            match msg {
                Msg::Snapshot(snap) => self.step(&snap),
                Msg::Reload => {
                    self.reload(true);
                    self.legacy_checked = None;
                    let mode = self.decide_mode();
                    if mode != self.mode {
                        self.mode = mode.clone();
                        self.emit(EventKind::Mode { mode }, None, None);
                    }
                    let (why, red) = {
                        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        (st.status.zone_why.clone(), st.status.red_for_secs)
                    };
                    self.publish(why, red);
                }
            }
        }
    }
}

/// Запустить сторожа в своём потоке. `on_event` вызывается из потока сторожа.
pub fn spawn(on_event: impl Fn(&GuardEvent) + Send + Sync + 'static) -> crate::Result<GuardHandle> {
    let (tx, rx) = sync_channel(1);
    let cfg = (*config::global()).clone();
    let rules = Rules::compile(&cfg.guard).unwrap_or_else(|e| {
        eprintln!("rmon: {e}; сторож на встроенных правилах");
        Rules::compile(&GuardConfig::default()).expect("встроенные правила компилируются")
    });
    let state = Arc::new(Mutex::new(State {
        status: GuardStatus {
            mode: GuardMode::Off,
            zone: None,
            zone_why: String::new(),
            red_for_secs: 0,
            legacy: Legacy::Absent,
            config: cfg.guard.clone(),
            config_file: config::config_file().display().to_string(),
            recent: read_events(RECENT),
        },
    }));
    let runner = Runner {
        recent: read_events(RECENT).into(),
        cfg,
        cfg_mtime: Runner::mtime(),
        cfg_error: None,
        rules,
        guard: Guard::default(),
        lock: None,
        legacy: Legacy::Absent,
        legacy_checked: None,
        mode: GuardMode::Off,
        platform: platform::current()?,
        listener: Arc::new(on_event),
        state: state.clone(),
        started: false,
    };
    std::thread::Builder::new()
        .name("rmon-guard".into())
        .spawn(move || runner.run(rx))
        .map_err(|e| Error::Syscall { call: "thread::spawn", source: e })?;
    Ok(GuardHandle { tx, state })
}

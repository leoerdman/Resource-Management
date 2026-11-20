//! Монитор: фоновый сбор → история → анализ → готовая сводка [`Dashboard`].
//!
//! Один цикл для всех долгоживущих потребителей (окно, MCP-сервер): у них одинаковые
//! история, подозреваемые, рост и политика, а снятие идёт через [`MonitorHandle::kill`]
//! с поиском цели в последнем снимке и мгновенным обновлением сводки.
//!
//! Память монитора ограничена: в истории анализа строки старых снимков выбрасываются
//! ([`History::slim`]), а в историю идёт только каждый `analyze_every`-й снимок.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::analyze::{Suspect, Thresholds, analyze, app_groups, growth_rates, is_dev, short_cmd};
use crate::collect::Collector;
use crate::config;
use crate::error::{Error, Result};
use crate::guard::runner::{self, GuardEvent, GuardHandle};
use crate::history::History;
use crate::kill::{KillOutcome, KillRequest, kill};
use crate::model::{AppGroup, Pressure, ProcState, ProcessInfo, Snapshot, SystemMemory};
use crate::policy::{KillPolicy, policy_for};

/// Точка графика.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HistoryPoint {
    /// Время, unix-секунды.
    #[ts(type = "number")]
    pub t: u64,
    /// Занято.
    #[ts(type = "number")]
    pub used: u64,
    /// Своп.
    #[ts(type = "number")]
    pub swap_used: u64,
    /// Сжато.
    #[ts(type = "number")]
    pub compressed: u64,
    /// Давление.
    pub pressure: Pressure,
}

/// Процесс в сводке: сведения снимка + рост из истории + политика снятия.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProcessRow {
    /// Идентификатор процесса.
    pub pid: i32,
    /// Родитель.
    pub ppid: i32,
    /// Имя.
    pub name: String,
    /// Путь.
    pub exe: Option<String>,
    /// Аргументы, укороченные до [`MonitorConfig::args_limit`] символов суммарно.
    pub args: Vec<String>,
    /// Владелец.
    pub uid: u32,
    /// footprint.
    #[ts(type = "number | null")]
    pub footprint: Option<u64>,
    /// Резидентная память.
    #[ts(type = "number")]
    pub rss: u64,
    /// CPU, проценты.
    pub cpu_pct: f32,
    /// Потоки.
    pub threads: u32,
    /// Состояние.
    pub state: ProcState,
    /// Время запуска.
    #[ts(type = "number")]
    pub started_at: u64,
    /// Приложение.
    pub app_group: String,
    /// Рабочий каталог — только у dev-процессов (`node`, `claude`, `bun`…), чтобы отличать
    /// десяток одинаковых сессий прямо в списке; у прочих `None` (лишние системные вызовы).
    pub cwd: Option<String>,
    /// Рост памяти, байт/мин (по истории анализа).
    pub growth_per_min: Option<f64>,
    /// Политика снятия.
    pub policy: KillPolicy,
}

impl ProcessRow {
    /// Совпадает ли строка с фильтром человека: pid целиком, либо подстрока (без регистра) в имени,
    /// приложении, строке запуска или каталоге. То же правило — в фильтре окна (`ProcessesTab.tsx`).
    pub fn matches(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        q.is_empty()
            || self.pid.to_string() == q
            || self.name.to_lowercase().contains(&q)
            || self.app_group.to_lowercase().contains(&q)
            || self.args.join(" ").to_lowercase().contains(&q)
            || self.cwd.as_deref().is_some_and(|c| c.to_lowercase().contains(&q))
    }

    /// Сколько освободится: footprint, иначе rss.
    pub fn reclaim(&self) -> u64 {
        self.footprint.unwrap_or(self.rss)
    }
}

/// Готовая сводка для окна и MCP.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Dashboard {
    /// Время снимка.
    #[ts(type = "number")]
    pub taken_at: u64,
    /// Память системы.
    pub system: SystemMemory,
    /// График за окно.
    pub history: Vec<HistoryPoint>,
    /// Приложения по убыванию памяти.
    pub groups: Vec<AppGroup>,
    /// Процессы.
    pub processes: Vec<ProcessRow>,
    /// Подозреваемые по убыванию score.
    pub suspects: Vec<Suspect>,
}

impl Dashboard {
    /// Убрать снятый процесс, не дожидаясь следующего снимка.
    pub fn forget(&mut self, pid: i32, started_at: u64, reclaimed: u64) {
        self.processes.retain(|p| !(p.pid == pid && p.started_at == started_at));
        self.suspects.retain(|s| !(s.pid == pid && s.started_at == started_at));
        for g in &mut self.groups {
            if let Some(i) = g.pids.iter().position(|&x| x == pid) {
                g.pids.remove(i);
                g.count = g.count.saturating_sub(1);
                g.footprint = g.footprint.saturating_sub(reclaimed);
            }
        }
        self.groups.retain(|g| g.count > 0);
    }
}

/// Настройки монитора.
#[derive(Debug, Clone)]
pub struct MonitorConfig {
    /// Шаг сбора.
    pub tick: Duration,
    /// В историю анализа идёт каждый N-й снимок.
    pub analyze_every: u32,
    /// Окно истории анализа.
    pub analyze_window: Duration,
    /// Окно графика.
    pub chart_window: Duration,
    /// Пороги анализа.
    pub thresholds: Thresholds,
    /// Предел суммарной длины аргументов в [`ProcessRow`].
    pub args_limit: usize,
    /// Писать ленту состояния (`timeline.jsonl`, шаг — `[logs] timeline_every_secs`): включают
    /// долгоживущие процессы, которые сторожат или показывают окно.
    pub timeline: bool,
}

impl Default for MonitorConfig {
    /// Окно: снимок раз в 2 с, анализ раз в 10 с на 10 минутах, график 15 минут.
    fn default() -> Self {
        Self {
            tick: Duration::from_secs(2),
            analyze_every: 5,
            analyze_window: Duration::from_secs(10 * 60),
            chart_window: Duration::from_secs(15 * 60),
            thresholds: config::global().thresholds.clone(),
            args_limit: 400,
            timeline: false,
        }
    }
}

impl MonitorConfig {
    /// Фоновый сервис без окна (MCP): снимок раз в 5 с, анализ на каждом, окно 15 минут.
    pub fn background() -> Self {
        Self {
            tick: Duration::from_secs(5),
            analyze_every: 1,
            analyze_window: Duration::from_secs(15 * 60),
            chart_window: Duration::from_secs(15 * 60),
            ..Self::default()
        }
    }
}

/// Итог одного шага монитора.
pub struct Tick {
    /// Сводка.
    pub dashboard: Dashboard,
    /// Полный снимок.
    pub snapshot: Snapshot,
    /// Обновлялись ли история анализа и подозреваемые.
    pub analyzed: bool,
}

/// Монитор без потока: сам вызываете [`Monitor::tick`]. Для тестов и разовых прогонов.
pub struct Monitor {
    collector: Collector,
    cfg: MonitorConfig,
    history: History,
    chart: VecDeque<HistoryPoint>,
    suspects: Vec<Suspect>,
    growth: HashMap<(i32, u64), f64>,
    /// Каталоги dev-процессов по личности: читаются один раз за жизнь процесса.
    cwds: HashMap<(i32, u64), Option<String>>,
    /// Политика по личности процесса и поколению настроек: ~30 регулярных выражений на
    /// процесс — самое дорогое в шаге, а зависит она только от неизменного (путь, аргументы).
    policies: HashMap<(i32, u64), (u64, KillPolicy)>,
    /// Пороги — общие с ручкой, чтобы менять их на лету из настроек.
    thresholds: Arc<RwLock<Thresholds>>,
    ticks: u64,
}

impl Monitor {
    /// Монитор текущей платформы.
    pub fn new(cfg: MonitorConfig) -> Result<Self> {
        Ok(Self::with_collector(Collector::new()?, cfg))
    }

    /// Монитор поверх готового сборщика.
    pub fn with_collector(collector: Collector, cfg: MonitorConfig) -> Self {
        Self {
            history: History::slim(cfg.analyze_window),
            thresholds: Arc::new(RwLock::new(cfg.thresholds.clone())),
            collector,
            cfg,
            chart: VecDeque::new(),
            suspects: Vec::new(),
            growth: HashMap::new(),
            cwds: HashMap::new(),
            policies: HashMap::new(),
            ticks: 0,
        }
    }

    /// Один шаг: снимок, график, (по расписанию) анализ, сводка.
    pub fn tick(&mut self) -> Result<Tick> {
        let snap = self.collector.snapshot()?;
        self.ticks += 1;
        self.chart.push_back(HistoryPoint {
            t: snap.taken_at,
            used: snap.system.used,
            swap_used: snap.system.swap_used,
            compressed: snap.system.compressed,
            pressure: snap.system.pressure,
        });
        let horizon = snap.taken_at.saturating_sub(self.cfg.chart_window.as_secs());
        while self.chart.front().is_some_and(|p| p.t < horizon) {
            self.chart.pop_front();
        }
        // В первом снимке нет CPU — анализ начинается со второго.
        let analyzed = self.ticks >= 2 && (self.ticks - 2).is_multiple_of(u64::from(self.cfg.analyze_every.max(1)));
        if analyzed {
            self.history.push(snap.clone());
            let thresholds = self.thresholds.read().unwrap_or_else(|e| e.into_inner()).clone();
            self.suspects = analyze(&self.history, &thresholds);
            self.growth = growth_rates(&self.history);
        }
        self.refresh_static_info(&snap);
        let dashboard = Dashboard {
            taken_at: snap.taken_at,
            system: snap.system,
            history: self.chart.iter().cloned().collect(),
            groups: app_groups(&snap),
            processes: snap.processes.iter().map(|p| self.row(p)).collect(),
            suspects: self.suspects.clone(),
        };
        Ok(Tick { dashboard, snapshot: snap, analyzed })
    }

    fn refresh_static_info(&mut self, snap: &Snapshot) {
        let alive: std::collections::HashSet<(i32, u64)> =
            snap.processes.iter().map(|p| (p.pid, p.started_at)).collect();
        self.cwds.retain(|k, _| alive.contains(k));
        for p in snap.processes.iter().filter(|p| is_dev(&p.name)) {
            let key = (p.pid, p.started_at);
            if !self.cwds.contains_key(&key) {
                let cwd = self.collector.cwd(p.pid).ok().flatten();
                self.cwds.insert(key, cwd);
            }
        }
    }

    fn row(&self, p: &ProcessInfo) -> ProcessRow {
        ProcessRow {
            pid: p.pid,
            ppid: p.ppid,
            name: p.name.clone(),
            exe: p.exe.clone(),
            args: limit_args(&p.args, self.cfg.args_limit),
            uid: p.uid,
            footprint: p.footprint,
            rss: p.rss,
            cpu_pct: p.cpu_pct,
            threads: p.threads,
            state: p.state,
            started_at: p.started_at,
            app_group: p.app_group.clone(),
            cwd: self.cwds.get(&(p.pid, p.started_at)).cloned().flatten(),
            growth_per_min: self.growth.get(&(p.pid, p.started_at)).copied(),
            policy: self.policies.get(&(p.pid, p.started_at)).map_or_else(|| policy_for(p), |(_, pol)| pol.clone()),
        }
    }

    /// Подробности процесса по итогу шага (для разовых прогонов, например `rmon why`).
    pub fn details(&mut self, tick: &Tick, pid: i32) -> Option<ProcessDetails> {
        let info = find(&tick.snapshot, pid, None)?;
        let series = self.history.series(info);
        let cwd = self.collector.cwd(info.pid).ok().flatten();
        build_details(&tick.snapshot, &tick.dashboard, info, series, cwd)
    }

    /// История анализа.
    pub fn history(&self) -> &History {
        &self.history
    }
}

fn find(snap: &Snapshot, pid: i32, started_at: Option<u64>) -> Option<&ProcessInfo> {
    snap.processes.iter().find(|p| p.pid == pid && started_at.is_none_or(|s| s == p.started_at))
}

fn build_details(
    snap: &Snapshot,
    dash: &Dashboard,
    info: &ProcessInfo,
    series: Vec<(u64, u64)>,
    cwd: Option<String>,
) -> Option<ProcessDetails> {
    let row = dash.processes.iter().find(|r| r.pid == info.pid && r.started_at == info.started_at)?.clone();
    let descendants: Vec<ProcBrief> = snap.descendants(info.pid).into_iter().map(ProcBrief::from).collect();
    Some(ProcessDetails {
        row,
        command_line: info.command_line(),
        cwd,
        ancestors: snap.ancestors(info.pid).into_iter().map(ProcBrief::from).collect(),
        descendants_total: descendants.iter().map(|d| d.reclaim).sum(),
        descendants,
        series,
    })
}

/// Аргументы, укороченные суммарно до `limit` символов (последний элемент — «…»).
pub fn limit_args(args: &[String], limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut left = limit;
    for a in args {
        if left == 0 {
            out.push("…".to_string());
            break;
        }
        let n = a.chars().count();
        if n <= left {
            out.push(a.clone());
            left -= n;
        } else {
            let mut cut: String = a.chars().take(left).collect();
            cut.push('…');
            out.push(cut);
            break;
        }
    }
    out
}

/// Процесс коротко — для цепочки родителей и списка детей.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProcBrief {
    /// Идентификатор.
    pub pid: i32,
    /// Имя.
    pub name: String,
    /// Время запуска.
    #[ts(type = "number")]
    pub started_at: u64,
    /// Память (footprint, иначе rss).
    #[ts(type = "number")]
    pub reclaim: u64,
    /// Строка запуска, укороченная.
    pub cmd: String,
}

impl From<&ProcessInfo> for ProcBrief {
    fn from(p: &ProcessInfo) -> Self {
        Self { pid: p.pid, name: p.name.clone(), started_at: p.started_at, reclaim: p.reclaim(), cmd: short_cmd(p) }
    }
}

/// Всё о процессе: «кто это, откуда, чей и что тянет за собой».
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProcessDetails {
    /// Строка как в сводке: рост, политика.
    pub row: ProcessRow,
    /// Полная строка запуска.
    pub command_line: String,
    /// Рабочий каталог — по нему видно, в каком проекте работает сессия.
    pub cwd: Option<String>,
    /// Родители: от непосредственного до init.
    pub ancestors: Vec<ProcBrief>,
    /// Потомки (дети, внуки…).
    pub descendants: Vec<ProcBrief>,
    /// Сумма памяти потомков.
    #[ts(type = "number")]
    pub descendants_total: u64,
    /// Ряд памяти из истории анализа: [unix-секунды, байты].
    #[ts(type = "Array<[number, number]>")]
    pub series: Vec<(u64, u64)>,
}

/// Лента состояния: что было с памятью и кто её занимал — для разбора задним числом.
pub fn timeline_log() -> std::path::PathBuf {
    config::log_dir().join("timeline.jsonl")
}

fn write_timeline(d: &Dashboard, snap: &Snapshot, guard: Option<&GuardHandle>) {
    let mut top: Vec<&ProcessInfo> = snap.processes.iter().collect();
    top.sort_by_key(|p| std::cmp::Reverse(p.reclaim()));
    let top: Vec<serde_json::Value> = top
        .iter()
        .take(8)
        .map(|p| serde_json::json!({ "pid": p.pid, "name": p.name, "mem": p.reclaim(), "cmd": short_cmd(p) }))
        .collect();
    let suspects: Vec<serde_json::Value> = d
        .suspects
        .iter()
        .take(5)
        .map(|s| serde_json::json!({ "pid": s.pid, "kind": s.kind, "reclaim": s.reclaim, "reason": s.reason }))
        .collect();
    let guard = guard.map(|g| {
        let st = g.status();
        serde_json::json!({ "mode": st.mode, "zone": st.zone, "why": st.zone_why, "red_for_secs": st.red_for_secs })
    });
    let m = &d.system;
    let line = serde_json::json!({
        "ts": d.taken_at,
        "pressure": m.pressure,
        "available_pct": m.available_pct,
        "used": m.used,
        "swap_used": m.swap_used,
        "compressed": m.compressed,
        "top": top,
        "suspects": suspects,
        "guard": guard,
    });
    if let Err(e) = crate::journal::append_retained(&timeline_log(), &line.to_string()) {
        eprintln!("rmon: лента состояния: {e}");
    }
}

/// Ряды памяти процессов: (pid, started_at) → [(t, bytes)].
type SeriesMap = HashMap<(i32, u64), Vec<(u64, u64)>>;

/// Последнее опубликованное состояние.
#[derive(Default)]
struct Published {
    dashboard: Option<Arc<Dashboard>>,
    snapshot: Option<Arc<Snapshot>>,
    /// Ряды памяти процессов из истории анализа: (pid, started_at) → [(t, bytes)].
    series: Arc<SeriesMap>,
}

type Listener = Box<dyn Fn(&Arc<Dashboard>) + Send + Sync>;

/// Ручка фонового монитора: читает сводку, снимает процессы. Дешёвая в клонировании.
#[derive(Clone)]
pub struct MonitorHandle {
    published: Arc<RwLock<Published>>,
    listener: Arc<Listener>,
    guard: Option<GuardHandle>,
    thresholds: Arc<RwLock<Thresholds>>,
    /// Шаг сбора, мс — меняется на лету ([`MonitorHandle::set_tick`]).
    tick_ms: Arc<std::sync::atomic::AtomicU64>,
}

impl MonitorHandle {
    /// Сменить шаг сбора на лету: окно видно — чаще, скрыто — реже (сторожу хватает 3 с).
    pub fn set_tick(&self, tick: Duration) {
        self.tick_ms.store(tick.as_millis().max(500) as u64, std::sync::atomic::Ordering::Relaxed);
    }

    /// Сменить пороги анализа на лету (действуют со следующего анализа).
    pub fn set_thresholds(&self, t: Thresholds) {
        *self.thresholds.write().unwrap_or_else(|e| e.into_inner()) = t;
    }

    /// Сторож памяти, если монитор запущен с ним ([`spawn_guarded`]).
    pub fn guard(&self) -> Option<&GuardHandle> {
        self.guard.as_ref()
    }

    /// Последняя сводка (нет — первый снимок ещё не готов).
    pub fn dashboard(&self) -> Option<Arc<Dashboard>> {
        self.published.read().unwrap_or_else(|e| e.into_inner()).dashboard.clone()
    }

    /// Последний полный снимок (с неукороченными аргументами).
    pub fn snapshot(&self) -> Option<Arc<Snapshot>> {
        self.published.read().unwrap_or_else(|e| e.into_inner()).snapshot.clone()
    }

    /// Ряд памяти процесса из истории анализа.
    pub fn series(&self, pid: i32, started_at: u64) -> Vec<(u64, u64)> {
        let series = self.published.read().unwrap_or_else(|e| e.into_inner()).series.clone();
        series.get(&(pid, started_at)).cloned().unwrap_or_default()
    }

    /// Процесс из последнего снимка по личности.
    pub fn target(&self, pid: i32, started_at: Option<u64>) -> Option<ProcessInfo> {
        let snap = self.snapshot()?;
        snap.processes.iter().find(|p| p.pid == pid && started_at.is_none_or(|s| s == p.started_at)).cloned()
    }

    /// Подробности процесса по последнему снимку. `started_at = None` — любой процесс с этим pid.
    pub fn details(&self, pid: i32, started_at: Option<u64>) -> Option<ProcessDetails> {
        let snap = self.snapshot()?;
        let dash = self.dashboard()?;
        let info = find(&snap, pid, started_at)?;
        let series = self.series(info.pid, info.started_at);
        build_details(&snap, &dash, info, series, self.cwd(info.pid))
    }

    /// Рабочий каталог процесса (свежее чтение платформы).
    pub fn cwd(&self, pid: i32) -> Option<String> {
        crate::platform::current().ok()?.cwd(pid).ok().flatten()
    }

    /// Снять процесс через защитный слой. Цель ищется в последнем снимке (и сверяется ядром
    /// заново перед сигналом); при успехе сводка сразу обновляется и уходит слушателю.
    pub fn kill(&self, pid: i32, started_at: Option<u64>, req: KillRequest) -> Result<KillOutcome> {
        let (target, outcome) = self.kill_quiet(pid, started_at, req)?;
        if outcome.exited {
            self.forget_and_notify(&[(target.pid, target.started_at, outcome.reclaimed)]);
        }
        Ok(outcome)
    }

    /// Снять несколько процессов параллельно (каждый — через защитный слой со своим ожиданием
    /// SIGTERM), сводка обновляется и уходит слушателю один раз. Итоги — в порядке `targets`.
    pub fn kill_many(&self, targets: &[(i32, u64)], req: &KillRequest) -> Vec<Result<KillOutcome>> {
        let results: Vec<Result<(ProcessInfo, KillOutcome)>> = std::thread::scope(|scope| {
            let handles: Vec<_> = targets
                .iter()
                .map(|&(pid, started_at)| scope.spawn(move || self.kill_quiet(pid, Some(started_at), req.clone())))
                .collect();
            handles
                .into_iter()
                .map(|h| {
                    h.join().unwrap_or_else(|_| {
                        Err(Error::Invalid { what: "снятие", detail: "поток упал".into() })
                    })
                })
                .collect()
        });
        let gone: Vec<(i32, u64, u64)> = results
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .filter(|(_, o)| o.exited)
            .map(|(t, o)| (t.pid, t.started_at, o.reclaimed))
            .collect();
        if !gone.is_empty() {
            self.forget_and_notify(&gone);
        }
        results.into_iter().map(|r| r.map(|(_, o)| o)).collect()
    }

    /// Найти цель (в снимке, иначе свежим чтением) и снять, не трогая сводку.
    fn kill_quiet(&self, pid: i32, started_at: Option<u64>, req: KillRequest) -> Result<(ProcessInfo, KillOutcome)> {
        // Процесс моложе последнего снимка — читаем его заново; отказ политики всё равно
        // пройдёт через защитный слой и попадёт в журнал действий.
        let target = match self.target(pid, started_at) {
            Some(t) => t,
            None => {
                let fresh = crate::platform::current()?.process(pid)?.ok_or(Error::ProcessGone { pid })?;
                if let Some(expected) = started_at
                    && expected != fresh.started_at
                {
                    return Err(Error::IdentityMismatch { pid, expected, actual: fresh.started_at });
                }
                fresh
            }
        };
        let outcome = kill(&target, req)?;
        Ok((target, outcome))
    }

    fn forget_and_notify(&self, gone: &[(i32, u64, u64)]) {
        let updated = {
            let mut published = self.published.write().unwrap_or_else(|e| e.into_inner());
            published.dashboard.as_mut().map(|d| {
                let dash = Arc::make_mut(d);
                for &(pid, started_at, reclaimed) in gone {
                    dash.forget(pid, started_at, reclaimed);
                }
                d.clone()
            })
        };
        if let Some(d) = updated {
            (self.listener)(&d);
        }
    }
}

/// Запустить монитор в фоновом потоке. `on_update` вызывается после каждого снимка
/// и после каждого успешного снятия — из фонового потока.
pub fn spawn(cfg: MonitorConfig, on_update: impl Fn(&Arc<Dashboard>) + Send + Sync + 'static) -> Result<MonitorHandle> {
    start(cfg, Arc::new(Box::new(on_update) as Listener), None)
}

/// То же вместе со сторожем памяти: каждый снимок уходит и ему. Сторож — один на машину
/// (файловая блокировка), так что запускать так можно только долгоживущий процесс,
/// который и должен сторожить: окно или `rmon guard run`.
pub fn spawn_guarded(
    cfg: MonitorConfig,
    on_update: impl Fn(&Arc<Dashboard>) + Send + Sync + 'static,
    on_guard: impl Fn(&GuardEvent) + Send + Sync + 'static,
) -> Result<MonitorHandle> {
    let guard = runner::spawn(on_guard)?;
    start(cfg, Arc::new(Box::new(on_update) as Listener), Some(guard))
}

fn start(cfg: MonitorConfig, listener: Arc<Listener>, guard: Option<GuardHandle>) -> Result<MonitorHandle> {
    let mut monitor = Monitor::new(cfg)?;
    let handle = MonitorHandle {
        published: Arc::default(),
        listener,
        guard,
        thresholds: monitor.thresholds.clone(),
        tick_ms: Arc::new(std::sync::atomic::AtomicU64::new(monitor.cfg.tick.as_millis() as u64)),
    };
    let worker = handle.clone();
    std::thread::Builder::new()
        .name("rmon-monitor".into())
        .spawn(move || {
            let mut last_timeline = 0u64;
            loop {
                let started = Instant::now();
                match monitor.tick() {
                    Ok(Tick { dashboard, snapshot, analyzed }) => {
                        let dashboard = Arc::new(dashboard);
                        let snapshot = Arc::new(snapshot);
                        let series = analyzed.then(|| Arc::new(monitor.history().all_series()));
                        {
                            let mut published = worker.published.write().unwrap_or_else(|e| e.into_inner());
                            if let Some(series) = series {
                                published.series = series;
                            }
                            published.dashboard = Some(dashboard.clone());
                            published.snapshot = Some(snapshot.clone());
                        }
                        let every = config::global().logs.timeline_every_secs;
                        if monitor.cfg.timeline
                            && every > 0
                            && dashboard.taken_at.saturating_sub(last_timeline) >= every
                        {
                            last_timeline = dashboard.taken_at;
                            write_timeline(&dashboard, &snapshot, worker.guard.as_ref());
                        }
                        if let Some(g) = &worker.guard {
                            g.feed(snapshot);
                        }
                        (worker.listener)(&dashboard);
                    }
                    Err(e) => eprintln!("rmon: снимок не удался: {e}"),
                }
                let tick = Duration::from_millis(worker.tick_ms.load(std::sync::atomic::Ordering::Relaxed));
                std::thread::sleep(tick.saturating_sub(started.elapsed()));
            }
        })
        .map_err(|e| Error::Syscall { call: "thread::spawn", source: e })?;
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_args_by_chars() {
        let a = vec!["абвгд".to_string(), "ежз".to_string(), "ик".to_string()];
        assert_eq!(limit_args(&a, 100), a);
        assert_eq!(limit_args(&a, 6), vec!["абвгд".to_string(), "е…".to_string()]);
        assert_eq!(limit_args(&a, 5), vec!["абвгд".to_string(), "…".to_string()]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn ticks_produce_dashboard() {
        let mut m = Monitor::new(MonitorConfig { analyze_every: 1, ..MonitorConfig::default() }).unwrap();
        assert!(!m.tick().unwrap().analyzed);
        let Tick { dashboard: d, snapshot: s, analyzed } = m.tick().unwrap();
        assert!(analyzed);
        assert_eq!(d.processes.len(), s.processes.len());
        assert_eq!(d.history.len(), 2);
        assert_eq!(m.history().len(), 1);
        assert!(d.groups.iter().map(|g| g.count).sum::<usize>() == d.processes.len());
        let me = std::process::id() as i32;
        assert!(!s.ancestors(me).is_empty(), "у теста есть родитель");
        let cwd = crate::platform::current().unwrap().cwd(me).unwrap();
        assert_eq!(cwd.as_deref(), std::env::current_dir().unwrap().to_str());
    }
}

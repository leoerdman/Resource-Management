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

use crate::analyze::{Suspect, Thresholds, analyze, app_groups, growth_rates};
use crate::collect::Collector;
use crate::config;
use crate::error::{Error, Result};
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
    /// Рост памяти, байт/мин (по истории анализа).
    pub growth_per_min: Option<f64>,
    /// Политика снятия.
    pub policy: KillPolicy,
}

impl ProcessRow {
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
            collector,
            cfg,
            chart: VecDeque::new(),
            suspects: Vec::new(),
            growth: HashMap::new(),
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
        let analyzed = self.ticks >= 2 && (self.ticks - 2) % u64::from(self.cfg.analyze_every.max(1)) == 0;
        if analyzed {
            self.history.push(snap.clone());
            self.suspects = analyze(&self.history, &self.cfg.thresholds);
            self.growth = growth_rates(&self.history);
        }
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
            growth_per_min: self.growth.get(&(p.pid, p.started_at)).copied(),
            policy: policy_for(p),
        }
    }

    /// История анализа.
    pub fn history(&self) -> &History {
        &self.history
    }
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

/// Процесс коротко — звено цепочки родителей или потомок.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcBrief {
    /// Идентификатор процесса.
    pub pid: i32,
    /// Имя.
    pub name: String,
    /// Время запуска.
    pub started_at: u64,
    /// Сколько освободится: footprint, иначе rss.
    pub reclaim: u64,
    /// Строка запуска, укороченная ([`crate::analyze::short_cmd`]).
    pub cmd: String,
}

impl ProcBrief {
    fn of(p: &ProcessInfo) -> Self {
        Self { pid: p.pid, name: p.name.clone(), started_at: p.started_at, reclaim: p.reclaim(), cmd: crate::analyze::short_cmd(p) }
    }
}

/// Подробности о процессе для окна и MCP: строка сводки, cwd, родня, ряд памяти.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessDetails {
    /// Как в [`Dashboard`]: рост, политика.
    pub row: ProcessRow,
    /// Рабочий каталог (свежее чтение платформы).
    pub cwd: Option<String>,
    /// От родителя к init.
    pub ancestors: Vec<ProcBrief>,
    /// Все потомки (обход в ширину).
    pub descendants: Vec<ProcBrief>,
    /// Сумма памяти потомков.
    pub descendants_total: u64,
    /// (t, bytes) из истории анализа.
    pub series: Vec<(u64, u64)>,
}

/// Последнее опубликованное состояние.
#[derive(Default)]
struct Published {
    dashboard: Option<Arc<Dashboard>>,
    snapshot: Option<Arc<Snapshot>>,
    /// Ряды памяти процессов из истории анализа: (pid, started_at) → [(t, bytes)].
    series: Arc<SeriesMap>,
}

/// (pid, started_at) → [(t, bytes)].
type SeriesMap = HashMap<(i32, u64), Vec<(u64, u64)>>;

type Listener = Box<dyn Fn(&Arc<Dashboard>) + Send + Sync>;

/// Ручка фонового монитора: читает сводку, снимает процессы. Дешёвая в клонировании.
#[derive(Clone)]
pub struct MonitorHandle {
    published: Arc<RwLock<Published>>,
    listener: Arc<Listener>,
}

impl MonitorHandle {
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

    /// Рабочий каталог процесса (свежее чтение платформы).
    pub fn cwd(&self, pid: i32) -> Option<String> {
        crate::platform::current().ok()?.cwd(pid).ok().flatten()
    }

    /// Подробности о процессе из последней сводки и снимка; `None` — его нет в последнем снимке.
    pub fn details(&self, pid: i32, started_at: Option<u64>) -> Option<ProcessDetails> {
        let (dashboard, snap) = {
            let published = self.published.read().unwrap_or_else(|e| e.into_inner());
            (published.dashboard.clone()?, published.snapshot.clone()?)
        };
        details_from(&dashboard, &snap, pid, started_at).map(|mut d| {
            d.cwd = self.cwd(pid);
            d.series = self.series(d.row.pid, d.row.started_at);
            d
        })
    }

    /// Снять процесс через защитный слой. Цель ищется в последнем снимке (и сверяется ядром
    /// заново перед сигналом); при успехе сводка сразу обновляется и уходит слушателю.
    pub fn kill(&self, pid: i32, started_at: Option<u64>, req: KillRequest) -> Result<KillOutcome> {
        let target = self.target(pid, started_at).ok_or(Error::ProcessGone { pid })?;
        let outcome = kill(&target, req)?;
        if outcome.exited {
            let updated = {
                let mut published = self.published.write().unwrap_or_else(|e| e.into_inner());
                published.dashboard.as_mut().map(|d| {
                    Arc::make_mut(d).forget(target.pid, target.started_at, outcome.reclaimed);
                    d.clone()
                })
            };
            if let Some(d) = updated {
                (self.listener)(&d);
            }
        }
        Ok(outcome)
    }
}

/// Подробности без платформы (cwd и ряд — пустые): строка сводки и родня из снимка.
fn details_from(dashboard: &Dashboard, snap: &Snapshot, pid: i32, started_at: Option<u64>) -> Option<ProcessDetails> {
    let row = dashboard.processes.iter().find(|p| p.pid == pid && started_at.is_none_or(|s| s == p.started_at))?.clone();
    let ancestors = snap.ancestors(pid).into_iter().map(ProcBrief::of).collect();
    let descendants: Vec<ProcBrief> = snap.descendants(pid).into_iter().map(ProcBrief::of).collect();
    let descendants_total = descendants.iter().map(|d| d.reclaim).sum();
    Some(ProcessDetails { row, cwd: None, ancestors, descendants, descendants_total, series: Vec::new() })
}

/// Запустить монитор в фоновом потоке. `on_update` вызывается после каждого снимка
/// и после каждого успешного снятия — из фонового потока.
pub fn spawn(cfg: MonitorConfig, on_update: impl Fn(&Arc<Dashboard>) + Send + Sync + 'static) -> Result<MonitorHandle> {
    let mut monitor = Monitor::new(cfg)?;
    let handle = MonitorHandle { published: Arc::default(), listener: Arc::new(Box::new(on_update)) };
    let worker = handle.clone();
    std::thread::Builder::new()
        .name("rmon-monitor".into())
        .spawn(move || {
            loop {
                let started = Instant::now();
                match monitor.tick() {
                    Ok(Tick { dashboard, snapshot, analyzed }) => {
                        let dashboard = Arc::new(dashboard);
                        let series = analyzed.then(|| Arc::new(monitor.history().all_series()));
                        {
                            let mut published = worker.published.write().unwrap_or_else(|e| e.into_inner());
                            if let Some(series) = series {
                                published.series = series;
                            }
                            published.dashboard = Some(dashboard.clone());
                            published.snapshot = Some(Arc::new(snapshot));
                        }
                        (worker.listener)(&dashboard);
                    }
                    Err(e) => eprintln!("rmon: снимок не удался: {e}"),
                }
                std::thread::sleep(monitor.cfg.tick.saturating_sub(started.elapsed()));
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
        let det = details_from(&d, &s, me, None).expect("тест есть в снимке");
        assert_eq!(det.row.pid, me);
        assert_eq!(det.ancestors.len(), s.ancestors(me).len());
        assert!(details_from(&d, &s, me, Some(det.row.started_at + 1)).is_none(), "другая личность — нет");
        let cwd = crate::platform::current().unwrap().cwd(me).unwrap();
        assert_eq!(cwd.as_deref(), std::env::current_dir().unwrap().to_str());
    }
}

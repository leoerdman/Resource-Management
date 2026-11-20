//! Ответы инструментов: структура для модели (`structuredContent` по схеме) и текст по-русски
//! для человека. Чистые функции над типами ядра — без платформы, тестируются на синтетике.

use std::collections::HashMap;
use std::path::Path;

use rmon_core::analyze::Suspect;
use rmon_core::error::Error;
use rmon_core::fmt::{bytes, bytes_delta, bytes_per_min, duration_human, percent};
use rmon_core::history::slope_per_min;
use rmon_core::journal::JournalEntry;
use rmon_core::kill::KillOutcome;
use rmon_core::model::{Pressure, ProcessInfo};
use rmon_core::monitor::{Dashboard, HistoryPoint, ProcBrief, ProcessDetails, ProcessRow};
use rmon_core::policy::KillPolicy;
use schemars::JsonSchema;
use serde::Serialize;

use crate::args::{ListArgs, SortKey};

/// Строка запуска в списках, символов.
const CMD_SHORT: usize = 160;
/// Топ приложений в сводке.
const TOP_APPS: usize = 10;
/// Подозреваемых в сводке.
const TOP_SUSPECTS: usize = 5;
/// Точек ряда памяти в `process_details`.
const SERIES_POINTS: usize = 30;
/// Потомков в `process_details`.
const TOP_CHILDREN: usize = 15;
/// Рост ещё не посчитан ни для одной строки.
const GROWTH_PENDING: &str =
    "Рост пока неизвестен: он считается по трём и более снимкам (≈15 с после запуска сервера).\n";

/// Имя варианта, как его сериализует serde (`normal`, `sleeping`, `hog`…).
fn tag<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
}

fn pressure_rank(p: Pressure) -> u8 {
    match p {
        Pressure::Normal => 0,
        Pressure::Warning => 1,
        Pressure::Critical => 2,
    }
}

fn cut(s: &str, limit: usize) -> String {
    if s.chars().count() <= limit {
        s.to_owned()
    } else {
        let mut out: String = s.chars().take(limit.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn ago(now: u64, ts: u64) -> String {
    format!("{} назад", duration_human(now.saturating_sub(ts)))
}

/// Политика снятия.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PolicyOut {
    /// `protected` (never), `confirm` (only with the user's explicit consent), `free`.
    pub kind: String,
    /// Why (empty for free).
    pub reason: String,
    /// Russian label for humans.
    pub label: String,
}

impl From<&KillPolicy> for PolicyOut {
    fn from(p: &KillPolicy) -> Self {
        let kind = match p {
            KillPolicy::Protected(_) => "protected",
            KillPolicy::Confirm(_) => "confirm",
            KillPolicy::Free => "free",
        };
        Self { kind: kind.into(), reason: p.reason().into(), label: p.label().into() }
    }
}

/// «можно снять», «с подтверждением: браузер», «защищён: ядро системы».
fn policy_text(p: &KillPolicy) -> String {
    if p.reason().is_empty() { p.label().to_owned() } else { format!("{}: {}", p.label(), p.reason()) }
}

/// Подозреваемый.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SuspectOut {
    /// Process id.
    pub pid: i32,
    /// Start time, unix seconds (process identity together with pid).
    pub started_at: u64,
    /// Process name.
    pub name: String,
    /// Application.
    pub app_group: String,
    /// Command line (shortened).
    pub cmd: String,
    /// hog | growing | orphan | idle | duplicate | zombie.
    pub kind: String,
    /// 0..=100, higher = better candidate.
    pub score: u8,
    /// Russian explanation.
    pub reason: String,
    /// Bytes freed if terminated.
    pub reclaim: u64,
    /// Kill policy.
    pub policy: PolicyOut,
}

impl From<&Suspect> for SuspectOut {
    fn from(s: &Suspect) -> Self {
        Self {
            pid: s.pid,
            started_at: s.started_at,
            name: s.name.clone(),
            app_group: s.app_group.clone(),
            cmd: s.cmd.clone(),
            kind: tag(&s.kind),
            score: s.score,
            reason: s.reason.clone(),
            reclaim: s.reclaim,
            policy: (&s.policy).into(),
        }
    }
}

fn suspect_line(i: usize, s: &Suspect) -> String {
    format!(
        "  {}. [{}] {} ({}) — {}; освободит {}; {}",
        i + 1,
        s.score,
        s.name,
        s.pid,
        s.reason,
        bytes(s.reclaim),
        policy_text(&s.policy)
    )
}

// ---------------------------------------------------------------- memory_overview

/// Память системы.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct MemoryOut {
    /// Physical RAM, bytes.
    pub total: u64,
    /// Used = app + wired + compressed (Activity Monitor "Memory Used").
    pub used: u64,
    /// Used, percent of total.
    pub used_pct: f64,
    /// App memory.
    pub app: u64,
    /// Wired memory.
    pub wired: u64,
    /// Compressed by the memory compressor.
    pub compressed: u64,
    /// File cache (reclaimable).
    pub cached_files: u64,
    /// Free.
    pub free: u64,
    /// Swap total.
    pub swap_total: u64,
    /// Swap used.
    pub swap_used: u64,
    /// normal | warning | critical.
    pub pressure: String,
    /// kern.memorystatus_level: percent still available.
    pub available_pct: u8,
}

/// Тренд за окно истории.
#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
pub struct TrendOut {
    /// Window covered by samples, seconds.
    pub window_secs: u64,
    /// Samples in the window.
    pub points: usize,
    /// Change of used memory over the window, bytes.
    pub used_delta: i64,
    /// Change of compressed memory, bytes.
    pub compressed_delta: i64,
    /// Change of swap used, bytes.
    pub swap_delta: i64,
    /// Swap slope, bytes/min (least squares), null with < 2 samples.
    pub swap_per_min: Option<f64>,
    /// Highest pressure in the window: normal | warning | critical.
    pub pressure_max: String,
    /// Seconds spent above normal pressure in the window.
    pub elevated_secs: u64,
}

/// Приложение.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AppOut {
    /// Application name.
    pub name: String,
    /// Sum of footprints (rss when footprint unavailable), bytes.
    pub mem: u64,
    /// Number of processes.
    pub count: usize,
}

/// Зона сторожа памяти.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct GuardZoneOut {
    /// green | yellow | red.
    pub zone: Option<String>,
    /// Why the guard is in this zone (Russian).
    pub why: Option<String>,
    /// Event time, unix seconds.
    pub ts: Option<u64>,
    /// Russian one-liner.
    pub text: String,
}

/// Ответ `memory_overview`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct OverviewOut {
    /// Snapshot time, unix seconds.
    pub taken_at: u64,
    /// System memory.
    pub memory: MemoryOut,
    /// Trend over the history window (up to 15 minutes).
    pub trend: TrendOut,
    /// Top applications by memory.
    pub top_apps: Vec<AppOut>,
    /// Top suspects (best cleanup candidates).
    pub suspects: Vec<SuspectOut>,
    /// False during the first seconds after start: suspects need two samples.
    pub analysis_ready: bool,
    /// Current memory guard zone (last zone event of its journal), null if no guard journal.
    pub guard: Option<GuardZoneOut>,
}

/// Тренд по точкам графика.
pub fn trend(h: &[HistoryPoint]) -> TrendOut {
    let (Some(first), Some(last)) = (h.first(), h.last()) else { return TrendOut::default() };
    let delta = |a: u64, b: u64| (b as i128 - a as i128).clamp(i64::MIN as i128, i64::MAX as i128) as i64;
    let swap: Vec<(u64, u64)> = h.iter().map(|p| (p.t, p.swap_used)).collect();
    let max = h.iter().map(|p| p.pressure).max_by_key(|p| pressure_rank(*p)).unwrap_or_default();
    TrendOut {
        window_secs: last.t.saturating_sub(first.t),
        points: h.len(),
        used_delta: delta(first.used, last.used),
        compressed_delta: delta(first.compressed, last.compressed),
        swap_delta: delta(first.swap_used, last.swap_used),
        swap_per_min: slope_per_min(&swap),
        pressure_max: tag(&max),
        elevated_secs: h
            .windows(2)
            .filter(|w| w[0].pressure != Pressure::Normal)
            .map(|w| w[1].t.saturating_sub(w[0].t))
            .sum(),
    }
}

/// Зона сторожа из последнего события смены зоны.
pub fn guard_zone(e: &JournalEntry) -> GuardZoneOut {
    GuardZoneOut {
        zone: e.raw["zone"].as_str().map(str::to_owned),
        why: e.raw["why"].as_str().map(str::to_owned),
        ts: e.ts,
        text: e.text.clone(),
    }
}

/// `memory_overview`.
pub fn overview(d: &Dashboard, guard: Option<&JournalEntry>, now: u64) -> (OverviewOut, String) {
    let m = &d.system;
    let tr = trend(&d.history);
    let analysis_ready = d.history.len() >= 2;
    let out = OverviewOut {
        taken_at: d.taken_at,
        memory: MemoryOut {
            total: m.total,
            used: m.used,
            used_pct: (m.used_pct() * 10.0).round() / 10.0,
            app: m.app,
            wired: m.wired,
            compressed: m.compressed,
            cached_files: m.cached_files,
            free: m.free,
            swap_total: m.swap_total,
            swap_used: m.swap_used,
            pressure: tag(&m.pressure),
            available_pct: m.available_pct,
        },
        trend: tr.clone(),
        top_apps: d
            .groups
            .iter()
            .take(TOP_APPS)
            .map(|g| AppOut { name: g.name.clone(), mem: g.footprint, count: g.count })
            .collect(),
        suspects: d.suspects.iter().take(TOP_SUSPECTS).map(SuspectOut::from).collect(),
        analysis_ready,
        guard: guard.map(guard_zone),
    };

    let mut t = String::new();
    t.push_str(&format!(
        "Память: занято {} из {} ({}), давление {}, доступно {} %.\n",
        bytes(m.used),
        bytes(m.total),
        percent(m.used_pct()),
        m.pressure.as_str(),
        m.available_pct
    ));
    t.push_str(&format!(
        "Приложения {} · wired {} · сжато {} · кэш файлов {} · свободно {}. Своп {} из {}.\n",
        bytes(m.app),
        bytes(m.wired),
        bytes(m.compressed),
        bytes(m.cached_files),
        bytes(m.free),
        bytes(m.swap_used),
        bytes(m.swap_total)
    ));
    if tr.points >= 2 {
        let max = d.history.iter().map(|p| p.pressure).max_by_key(|p| pressure_rank(*p)).unwrap_or_default();
        let mut line = format!(
            "Тренд за {}: занято {}, сжато {}, своп {}",
            duration_human(tr.window_secs),
            bytes_delta(tr.used_delta),
            bytes_delta(tr.compressed_delta),
            bytes_delta(tr.swap_delta)
        );
        if let Some(s) = tr.swap_per_min {
            line.push_str(&format!(" ({})", bytes_per_min(s)));
        }
        line.push_str(&format!("; давление: максимум {}", max.as_str()));
        if tr.elevated_secs > 0 {
            line.push_str(&format!(", выше нормы {}", duration_human(tr.elevated_secs)));
        }
        t.push_str(&line);
        t.push_str(".\n");
    } else {
        t.push_str("Тренд: история ещё копится (снимок раз в 5 с).\n");
    }
    match guard {
        Some(g) => {
            let when = g.ts.map(|ts| format!(" ({})", ago(now, ts))).unwrap_or_default();
            t.push_str(&format!("Сторож памяти: {}{when}.\n", g.text));
        }
        None => t.push_str("Сторож памяти: журнала нет.\n"),
    }
    t.push_str("\nТоп приложений:\n");
    for (i, g) in d.groups.iter().take(TOP_APPS).enumerate() {
        t.push_str(&format!("  {}. {} — {} ({} проц.)\n", i + 1, g.name, bytes(g.footprint), g.count));
    }
    t.push_str("\nПодозреваемые:\n");
    if !analysis_ready {
        t.push_str("  анализ ещё не готов — нужен второй снимок (≈5 с после запуска).\n");
    } else if d.suspects.is_empty() {
        t.push_str("  нет.\n");
    } else {
        for (i, s) in d.suspects.iter().take(TOP_SUSPECTS).enumerate() {
            t.push_str(&suspect_line(i, s));
            t.push('\n');
        }
    }
    (out, t)
}

// ---------------------------------------------------------------- list_processes

/// Процесс.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ProcessOut {
    /// Process id.
    pub pid: i32,
    /// Parent pid.
    pub ppid: i32,
    /// Start time, unix seconds.
    pub started_at: u64,
    /// Process name.
    pub name: String,
    /// Application.
    pub app_group: String,
    /// Memory to be freed: footprint, rss when footprint unavailable.
    pub mem: u64,
    /// phys_footprint (Activity Monitor "Memory"), null without permission.
    pub footprint: Option<u64>,
    /// Resident size.
    pub rss: u64,
    /// CPU percent.
    pub cpu_pct: f32,
    /// Threads.
    pub threads: u32,
    /// running | sleeping | stopped | zombie | unknown.
    pub state: String,
    /// Memory growth over the history window, bytes/min; null until enough samples.
    pub growth_per_min: Option<f64>,
    /// Kill policy.
    pub policy: PolicyOut,
    /// Command line (shortened).
    pub cmd: String,
}

fn row_cmd(r: &ProcessRow) -> String {
    let full = if r.args.is_empty() { r.exe.clone().unwrap_or_else(|| r.name.clone()) } else { r.args.join(" ") };
    cut(&full, CMD_SHORT)
}

impl From<&ProcessRow> for ProcessOut {
    fn from(r: &ProcessRow) -> Self {
        Self {
            pid: r.pid,
            ppid: r.ppid,
            started_at: r.started_at,
            name: r.name.clone(),
            app_group: r.app_group.clone(),
            mem: r.reclaim(),
            footprint: r.footprint,
            rss: r.rss,
            cpu_pct: (r.cpu_pct * 10.0).round() / 10.0,
            threads: r.threads,
            state: tag(&r.state),
            growth_per_min: r.growth_per_min,
            policy: (&r.policy).into(),
            cmd: row_cmd(r),
        }
    }
}

/// Приложение в `list_processes {group_by_app: true}`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AppRowOut {
    /// Application name.
    pub name: String,
    /// Number of matching processes.
    pub count: usize,
    /// Sum of memory, bytes.
    pub mem: u64,
    /// Sum of CPU percent.
    pub cpu_pct: f32,
    /// Sum of known growth rates, bytes/min.
    pub growth_per_min: Option<f64>,
    /// Pids of the app, largest first (at most 20).
    pub pids: Vec<i32>,
}

/// Ответ `list_processes`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ListOut {
    /// Sort order used.
    pub sort: String,
    /// Filter used.
    pub filter: Option<String>,
    /// Rows (processes or apps) matching the filter before the limit.
    pub matched: usize,
    /// Processes (empty when group_by_app).
    pub processes: Vec<ProcessOut>,
    /// Applications (only when group_by_app).
    pub apps: Vec<AppRowOut>,
}

fn matches(r: &ProcessRow, f: &str) -> bool {
    if let Ok(pid) = f.parse::<i32>()
        && r.pid == pid
    {
        return true;
    }
    let has = |s: &str| s.to_lowercase().contains(f);
    has(&r.name) || has(&r.app_group) || r.exe.as_deref().is_some_and(has) || r.args.iter().any(|a| has(a))
}

fn growth_cmp(a: Option<f64>, b: Option<f64>) -> std::cmp::Ordering {
    // По убыванию, неизвестный рост — в конце.
    b.unwrap_or(f64::NEG_INFINITY).total_cmp(&a.unwrap_or(f64::NEG_INFINITY))
}

fn growth_text(g: Option<f64>) -> String {
    g.map_or_else(|| "—".to_owned(), bytes_per_min)
}

/// `list_processes`.
pub fn list(d: &Dashboard, a: &ListArgs) -> (ListOut, String) {
    let filter = a.filter.as_deref().map(str::trim).filter(|f| !f.is_empty()).map(str::to_lowercase);
    let mut rows: Vec<&ProcessRow> =
        d.processes.iter().filter(|r| filter.as_deref().is_none_or(|f| matches(r, f))).collect();
    let limit = a.limit as usize;
    let mut t = String::new();
    let head = match &filter {
        Some(f) => format!("фильтр «{f}», "),
        None => String::new(),
    };

    if a.group_by_app {
        let mut by: HashMap<&str, AppRowOut> = HashMap::new();
        rows.sort_by_key(|r| std::cmp::Reverse(r.reclaim()));
        for r in &rows {
            let e = by.entry(&r.app_group).or_insert_with(|| AppRowOut {
                name: r.app_group.clone(),
                count: 0,
                mem: 0,
                cpu_pct: 0.0,
                growth_per_min: None,
                pids: Vec::new(),
            });
            e.count += 1;
            e.mem += r.reclaim();
            e.cpu_pct += r.cpu_pct;
            if let Some(g) = r.growth_per_min {
                e.growth_per_min = Some(e.growth_per_min.unwrap_or(0.0) + g);
            }
            if e.pids.len() < 20 {
                e.pids.push(r.pid);
            }
        }
        let mut apps: Vec<AppRowOut> = by.into_values().collect();
        match a.sort {
            SortKey::Mem => apps.sort_by(|x, y| y.mem.cmp(&x.mem).then_with(|| x.name.cmp(&y.name))),
            SortKey::Cpu => apps.sort_by(|x, y| y.cpu_pct.total_cmp(&x.cpu_pct)),
            SortKey::Growth => apps.sort_by(|x, y| growth_cmp(x.growth_per_min, y.growth_per_min)),
            SortKey::Pid => apps.sort_by_key(|x| x.pids.iter().min().copied().unwrap_or(i32::MAX)),
        }
        let matched = apps.len();
        apps.truncate(limit);
        for x in &mut apps {
            x.cpu_pct = (x.cpu_pct * 10.0).round() / 10.0;
        }
        t.push_str(&format!("Приложения ({head}порядок {}): показано {} из {matched}.\n", a.sort.as_str(), apps.len()));
        if a.sort == SortKey::Growth && apps.iter().all(|x| x.growth_per_min.is_none()) {
            t.push_str(GROWTH_PENDING);
        }
        for x in &apps {
            t.push_str(&format!(
                "  {:>9}  {:>6}  {:>12}  {} ({} проц.)\n",
                bytes(x.mem),
                percent(f64::from(x.cpu_pct)),
                growth_text(x.growth_per_min),
                x.name,
                x.count
            ));
        }
        let out = ListOut { sort: a.sort.as_str().into(), filter, matched, processes: Vec::new(), apps };
        return (out, t);
    }

    match a.sort {
        SortKey::Mem => rows.sort_by_key(|r| std::cmp::Reverse(r.reclaim())),
        SortKey::Cpu => rows.sort_by(|x, y| y.cpu_pct.total_cmp(&x.cpu_pct)),
        SortKey::Growth => rows.sort_by(|x, y| growth_cmp(x.growth_per_min, y.growth_per_min)),
        SortKey::Pid => rows.sort_by_key(|r| r.pid),
    }
    let matched = rows.len();
    rows.truncate(limit);
    t.push_str(&format!("Процессы ({head}порядок {}): показано {} из {matched}.\n", a.sort.as_str(), rows.len()));
    if a.sort == SortKey::Growth && rows.iter().all(|r| r.growth_per_min.is_none()) {
        t.push_str(GROWTH_PENDING);
    }
    t.push_str(&format!("  {:>7}  {:>9}  {:>6}  {:>12}  имя — политика\n", "pid", "память", "cpu", "рост"));
    for r in &rows {
        t.push_str(&format!(
            "  {:>7}  {:>9}  {:>6}  {:>12}  {} — {}\n",
            r.pid,
            bytes(r.reclaim()),
            percent(f64::from(r.cpu_pct)),
            growth_text(r.growth_per_min),
            r.name,
            r.policy.label()
        ));
    }
    let out = ListOut {
        sort: a.sort.as_str().into(),
        filter,
        matched,
        processes: rows.into_iter().map(ProcessOut::from).collect(),
        apps: Vec::new(),
    };
    (out, t)
}

// ---------------------------------------------------------------- process_details

/// Процесс коротко.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct BriefOut {
    /// Process id.
    pub pid: i32,
    /// Name.
    pub name: String,
    /// Start time, unix seconds.
    pub started_at: u64,
    /// Memory, bytes.
    pub mem: u64,
    /// Command line (shortened).
    pub cmd: String,
}

impl From<&ProcBrief> for BriefOut {
    fn from(b: &ProcBrief) -> Self {
        Self { pid: b.pid, name: b.name.clone(), started_at: b.started_at, mem: b.reclaim, cmd: cut(&b.cmd, CMD_SHORT) }
    }
}

/// Потомки.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ChildrenOut {
    /// All descendants (children, grandchildren…).
    pub count: usize,
    /// Their memory in total, bytes.
    pub total_mem: u64,
    /// Largest descendants (at most 15).
    pub top: Vec<BriefOut>,
}

/// Точка ряда памяти.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PointOut {
    /// Unix seconds.
    pub t: u64,
    /// Memory, bytes.
    pub bytes: u64,
}

/// Ряд памяти из истории.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SeriesOut {
    /// Seconds covered.
    pub span_secs: u64,
    /// Minimum, bytes.
    pub min: u64,
    /// Maximum, bytes.
    pub max: u64,
    /// Slope, bytes/min (least squares).
    pub slope_per_min: Option<f64>,
    /// Downsampled points (at most 30), oldest first.
    pub points: Vec<PointOut>,
}

/// Ответ `process_details`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DetailsOut {
    /// The process.
    pub process: ProcessOut,
    /// Executable path.
    pub exe: Option<String>,
    /// Full arguments (shortened to 2000 chars total).
    pub args: Vec<String>,
    /// Owner uid.
    pub uid: u32,
    /// Working directory.
    pub cwd: Option<String>,
    /// Parent chain from the direct parent up to launchd/init.
    pub ancestors: Vec<BriefOut>,
    /// Descendants.
    pub children: ChildrenOut,
    /// Memory series over the history window; null until two samples.
    pub series: Option<SeriesOut>,
    /// The process as a cleanup suspect, if it is one.
    pub suspect: Option<SuspectOut>,
}

/// Ряд, прореженный до `n` точек (первая и последняя сохраняются).
pub fn downsample(series: &[(u64, u64)], n: usize) -> Vec<(u64, u64)> {
    if series.len() <= n || n < 2 {
        return series.to_vec();
    }
    let step = (series.len() - 1) as f64 / (n - 1) as f64;
    (0..n).map(|i| series[((i as f64 * step).round() as usize).min(series.len() - 1)]).collect()
}

/// `process_details`. `full` — процесс из полного снимка (неукороченные аргументы).
pub fn details(det: &ProcessDetails, full: Option<&ProcessInfo>, suspect: Option<&Suspect>) -> (DetailsOut, String) {
    let r = &det.row;
    let mut kids: Vec<&ProcBrief> = det.descendants.iter().collect();
    kids.sort_by_key(|k| std::cmp::Reverse(k.reclaim));
    let series = (det.series.len() >= 2).then(|| {
        let (first, last) = (det.series[0].0, det.series[det.series.len() - 1].0);
        SeriesOut {
            span_secs: last.saturating_sub(first),
            min: det.series.iter().map(|p| p.1).min().unwrap_or(0),
            max: det.series.iter().map(|p| p.1).max().unwrap_or(0),
            slope_per_min: slope_per_min(&det.series),
            points: downsample(&det.series, SERIES_POINTS)
                .into_iter()
                .map(|(t, bytes)| PointOut { t, bytes })
                .collect(),
        }
    });
    let args = full.map(|p| rmon_core::monitor::limit_args(&p.args, 2000)).unwrap_or_else(|| r.args.clone());
    let out = DetailsOut {
        process: r.into(),
        exe: r.exe.clone(),
        args: args.clone(),
        uid: r.uid,
        cwd: det.cwd.clone(),
        ancestors: det.ancestors.iter().map(BriefOut::from).collect(),
        children: ChildrenOut {
            count: det.descendants.len(),
            total_mem: det.descendants_total,
            top: kids.iter().take(TOP_CHILDREN).map(|k| BriefOut::from(*k)).collect(),
        },
        series: series.clone(),
        suspect: suspect.map(SuspectOut::from),
    };

    let mut t = format!("{} (pid {}, приложение {})\n", r.name, r.pid, r.app_group);
    t.push_str(&format!(
        "Память {} (footprint {}, rss {}), CPU {}, потоков {}, {}.\n",
        bytes(r.reclaim()),
        r.footprint.map_or_else(|| "нет прав".to_owned(), bytes),
        bytes(r.rss),
        percent(f64::from(r.cpu_pct)),
        r.threads,
        r.state.as_str()
    ));
    if let Some(exe) = &r.exe {
        t.push_str(&format!("Файл: {exe}\n"));
    }
    if !args.is_empty() {
        t.push_str(&format!("Запуск: {}\n", cut(&args.join(" "), 600)));
    }
    t.push_str(&format!("Каталог: {}\n", det.cwd.as_deref().unwrap_or("неизвестен")));
    let chain: Vec<String> = det.ancestors.iter().map(|a| format!("{} ({})", a.name, a.pid)).collect();
    t.push_str(&format!("Родители: {}\n", if chain.is_empty() { "нет".to_owned() } else { chain.join(" ← ") }));
    if det.descendants.is_empty() {
        t.push_str("Потомков нет.\n");
    } else {
        t.push_str(&format!("Потомков {}, вместе {}:\n", det.descendants.len(), bytes(det.descendants_total)));
        for k in kids.iter().take(TOP_CHILDREN) {
            t.push_str(&format!("  {} ({}) — {}\n", k.name, k.pid, bytes(k.reclaim)));
        }
    }
    match &series {
        Some(s) => {
            t.push_str(&format!("Память за {}: от {} до {}", duration_human(s.span_secs), bytes(s.min), bytes(s.max)));
            if let Some(slope) = s.slope_per_min {
                t.push_str(&format!(", наклон {}", bytes_per_min(slope)));
            }
            t.push_str(".\n");
        }
        None => t.push_str("Ряда памяти пока нет (нужно два снимка).\n"),
    }
    t.push_str(&format!("Снятие: {}.\n", policy_text(&r.policy)));
    if let Some(s) = suspect {
        t.push_str(&format!("Подозреваемый [{}]: {}\n", s.score, s.reason));
    }
    (out, t)
}

// ---------------------------------------------------------------- suggest_cleanup

/// Ответ `suggest_cleanup`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CleanupOut {
    /// Score threshold used.
    pub min_score: u32,
    /// Bytes that can be freed without asking the user (policy free).
    pub free_total: u64,
    /// Bytes that can be freed only with the user's explicit consent (policy confirm).
    pub confirm_total: u64,
    /// Bytes held by protected suspects (never killed).
    pub protected_total: u64,
    /// False during the first seconds after start.
    pub analysis_ready: bool,
    /// Suspects, best candidates first.
    pub suspects: Vec<SuspectOut>,
}

/// `suggest_cleanup`.
pub fn cleanup(d: &Dashboard, min_score: u32) -> (CleanupOut, String) {
    let picked: Vec<&Suspect> = d.suspects.iter().filter(|s| u32::from(s.score) >= min_score).collect();
    let sum = |f: fn(&KillPolicy) -> bool| picked.iter().filter(|s| f(&s.policy)).map(|s| s.reclaim).sum::<u64>();
    let free_total = sum(|p| matches!(p, KillPolicy::Free));
    let confirm_total = sum(|p| matches!(p, KillPolicy::Confirm(_)));
    let protected_total = sum(|p| matches!(p, KillPolicy::Protected(_)));
    let analysis_ready = d.history.len() >= 2;
    let mut t = format!(
        "Можно освободить ≈ {} без подтверждения, ещё {} — с подтверждением.\n",
        bytes(free_total),
        bytes(confirm_total)
    );
    if !analysis_ready {
        t.push_str("Анализ ещё не готов — нужен второй снимок (≈5 с после запуска).\n");
    } else if picked.is_empty() {
        t.push_str(&format!("Подозреваемых со счётом от {min_score} нет.\n"));
    } else {
        for (i, s) in picked.iter().enumerate() {
            t.push_str(&suspect_line(i, s));
            t.push('\n');
        }
        if protected_total > 0 {
            t.push_str(&format!("Защищённые ({}) не снимаются никогда.\n", bytes(protected_total)));
        }
        t.push_str(
            "Снять: kill_process {pid, reason}; «с подтверждением» — только после согласия владельца (confirm=true).\n",
        );
    }
    let out = CleanupOut {
        min_score,
        free_total,
        confirm_total,
        protected_total,
        analysis_ready,
        suspects: picked.into_iter().map(SuspectOut::from).collect(),
    };
    (out, t)
}

// ---------------------------------------------------------------- kill_process

/// Ответ `kill_process`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct KillOut {
    /// killed | not_exited | dry_run | refused_protected | needs_confirmation | not_found |
    /// not_in_snapshot | identity_mismatch | permission_denied | error.
    pub status: String,
    /// Process id.
    pub pid: i32,
    /// Process name, if known.
    pub name: Option<String>,
    /// Kill policy, if the process was found.
    pub policy: Option<PolicyOut>,
    /// Last signal sent: SIGTERM | SIGKILL.
    pub signal: Option<String>,
    /// The process is gone.
    pub exited: bool,
    /// Bytes freed (estimate from the snapshot); for dry_run — would be freed.
    pub reclaimed: u64,
    /// Dry run: nothing was sent.
    pub dry_run: bool,
    /// Russian message for the user.
    pub message: String,
}

impl KillOut {
    /// Отказ или неудача — `isError` в ответе.
    pub fn is_error(&self) -> bool {
        !matches!(self.status.as_str(), "killed" | "dry_run")
    }
}

/// Снятие состоялось (или проба).
pub fn kill_ok(o: &KillOutcome, policy: &KillPolicy) -> KillOut {
    let status = if o.dry_run {
        "dry_run"
    } else if o.exited {
        "killed"
    } else {
        "not_exited"
    };
    let mut message = o.message.clone();
    if o.dry_run && matches!(policy, KillPolicy::Confirm(_)) {
        message.push_str(" — с подтверждением владельца");
    }
    KillOut {
        status: status.into(),
        pid: o.pid,
        name: Some(o.name.clone()),
        policy: Some(policy.into()),
        signal: (!o.dry_run).then(|| o.signal.as_str().to_owned()),
        exited: o.exited,
        reclaimed: o.reclaimed,
        dry_run: o.dry_run,
        message,
    }
}

/// Отказ защитного слоя или ошибка.
pub fn kill_err(pid: i32, name: Option<&str>, policy: Option<&KillPolicy>, e: &Error) -> KillOut {
    let (status, message) = match e {
        Error::Protected { .. } => ("refused_protected", format!("{e}. Этот процесс не снимается никогда.")),
        Error::ConfirmationRequired { .. } => (
            "needs_confirmation",
            format!(
                "Отказ: {e}. Нужно согласие владельца: спросите его, можно ли снять именно этот процесс, \
                 и только после ответа «да» повторите с confirm=true."
            ),
        ),
        Error::ProcessGone { .. } => ("not_found", e.to_string()),
        Error::IdentityMismatch { .. } => ("identity_mismatch", e.to_string()),
        Error::Permission { .. } => ("permission_denied", e.to_string()),
        _ => ("error", e.to_string()),
    };
    KillOut {
        status: status.into(),
        pid,
        name: name.map(str::to_owned),
        policy: policy.map(PolicyOut::from),
        signal: None,
        exited: false,
        reclaimed: 0,
        dry_run: false,
        message,
    }
}

/// Процесса нет в последнем снимке. `name`/`policy` — по свежему чтению платформы (`None` — не
/// читается: системный или чужой), `alive` — жив ли он сейчас.
pub fn kill_missing(pid: i32, name: Option<&str>, policy: Option<&KillPolicy>, alive: bool) -> KillOut {
    let (status, message) = match (policy, alive) {
        (Some(KillPolicy::Protected(reason)), _) => {
            let e =
                Error::Protected {
                    pid, name: name.unwrap_or("системный процесс").to_owned(), reason: reason.clone()
                };
            ("refused_protected", format!("{e}. Этот процесс не снимается никогда."))
        }
        (None, true) => (
            "refused_protected",
            format!("снятие {pid} запрещено: процесс не читается без прав root (системный или другого пользователя)"),
        ),
        (Some(_), true) => (
            "not_in_snapshot",
            format!(
                "процесс {pid} запущен позже последнего снимка (снимок раз в 5 с) — повторите через несколько секунд"
            ),
        ),
        (_, false) => ("not_found", format!("процесса {pid} нет")),
    };
    KillOut {
        status: status.into(),
        pid,
        name: name.map(str::to_owned),
        policy: policy.map(PolicyOut::from),
        signal: None,
        exited: false,
        reclaimed: 0,
        dry_run: false,
        message,
    }
}

/// `process_details` для процесса не из последнего снимка.
pub fn details_missing(pid: i32, alive: bool) -> String {
    if alive {
        format!(
            "процесса {pid} нет в последнем снимке: он запущен только что (снимок раз в 5 с) или не читается без прав root"
        )
    } else {
        format!("процесса {pid} нет")
    }
}

// ---------------------------------------------------------------- guard_events

/// Событие сторожа памяти.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct EventOut {
    /// Unix seconds.
    pub ts: Option<u64>,
    /// Event type: zone | kill | limit | start | stop | config.
    pub ev: String,
    /// Russian one-liner.
    pub text: String,
}

/// Ответ `guard_events`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct GuardOut {
    /// Journal path (`guard.jsonl` in the rmon log directory, env `RMON_LOG_DIR`).
    pub path: String,
    /// Whether the journal file exists.
    pub exists: bool,
    /// Events, newest first.
    pub events: Vec<EventOut>,
}

/// `guard_events`.
pub fn guard(entries: &[JournalEntry], path: &Path, now: u64) -> (GuardOut, String) {
    let exists = path.exists();
    let mut t = if !exists {
        format!("Журнала сторожа нет ({}).\n", path.display())
    } else if entries.is_empty() {
        "Событий сторожа нет.\n".to_owned()
    } else {
        format!("Последние события сторожа ({}):\n", entries.len())
    };
    for e in entries {
        let when = e.ts.map_or_else(|| "?".to_owned(), |ts| ago(now, ts));
        t.push_str(&format!("  {when}: {}\n", e.text));
    }
    let out = GuardOut {
        path: path.display().to_string(),
        exists,
        events: entries
            .iter()
            .map(|e| EventOut { ts: e.ts, ev: e.raw["ev"].as_str().unwrap_or("?").to_owned(), text: e.text.clone() })
            .collect(),
    };
    (out, t)
}

#[cfg(test)]
mod tests;

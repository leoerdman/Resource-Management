//! Подозреваемые по истории снимков и суммы по приложениям.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::fmt;
use crate::history::{History, slope_per_min};
use crate::kill::{KillPolicy, policy_for};
use crate::model::{AppGroup, GIB, MIB, ProcState, ProcessInfo, Snapshot};

/// Почему процесс подозрителен.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SuspectKind {
    /// Большой.
    Hog,
    /// Растёт.
    Growing,
    /// Родитель умер.
    Orphan,
    /// Большой и без дела.
    Idle,
    /// Много одинаковых.
    Duplicate,
    /// Зомби.
    Zombie,
}

/// Подозреваемый.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Suspect {
    /// Процесс.
    pub pid: i32,
    /// Имя.
    pub name: String,
    /// Приложение.
    pub app_group: String,
    /// Строка запуска, укороченная до [`CMD_LIMIT`] символов: «node 3882» человеку ничего не говорит.
    pub cmd: String,
    /// Ключ группы для подозреваемых одной природы (копии одной команды) — окно сворачивает их в одну карточку.
    pub group: Option<String>,
    /// Время запуска (личность для снятия).
    #[ts(type = "number")]
    pub started_at: u64,
    /// Вид.
    pub kind: SuspectKind,
    /// 0–100.
    pub score: u8,
    /// Человеку, по-русски.
    pub reason: String,
    /// Сколько освободится.
    #[ts(type = "number")]
    pub reclaim: u64,
    /// Политика снятия.
    pub policy: KillPolicy,
}

/// Длина строки запуска в [`Suspect::cmd`].
pub const CMD_LIMIT: usize = 200;

/// Строка запуска, укороченная по символам.
pub fn short_cmd(p: &ProcessInfo) -> String {
    let full = p.command_line();
    if full.chars().count() <= CMD_LIMIT {
        full
    } else {
        let mut s: String = full.chars().take(CMD_LIMIT - 1).collect();
        s.push('…');
        s
    }
}

/// Пороги анализа (секция `[thresholds]` в `config.toml`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Thresholds {
    /// Hog: абсолютный порог.
    pub hog_bytes: u64,
    /// Hog: доля ОЗУ, проценты.
    pub hog_pct: f64,
    /// Growing: минимальный наклон, байт/мин.
    pub growing_per_min: f64,
    /// Growing: минимальная длина ряда, секунды.
    pub growing_min_span: u64,
    /// Idle: минимальный размер.
    pub idle_bytes: u64,
    /// Idle: максимальный средний CPU, проценты.
    pub idle_cpu: f32,
    /// Idle: минимальный возраст процесса, секунды.
    pub idle_age: u64,
    /// Orphan: минимальный размер.
    pub orphan_bytes: u64,
    /// Duplicate: сколько одинаковых строк запуска.
    pub duplicate_count: usize,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            hog_bytes: GIB,
            hog_pct: 5.0,
            growing_per_min: 20.0 * MIB as f64,
            growing_min_span: 60,
            idle_bytes: 500 * MIB,
            idle_cpu: 0.5,
            idle_age: 30 * 60,
            orphan_bytes: 50 * MIB,
            duplicate_count: 3,
        }
    }
}

/// Dev-процессы, у которых сирота — почти всегда забытый прогон.
const DEV_NAMES: &[&str] = &[
    "node", "bun", "deno", "python", "python3", "tsgo", "tsc", "claude", "cargo", "rustc", "go", "java", "ruby",
    "esbuild", "vite", "npm", "pnpm", "yarn", "uv",
];

fn is_dev(name: &str) -> bool {
    DEV_NAMES.iter().any(|d| name == *d || (name.starts_with(d) && name[d.len()..].chars().all(|c| c.is_ascii_digit() || c == '.')))
}

/// Наклон памяти каждого процесса последнего снимка, байт/мин.
pub fn growth_rates(history: &History) -> HashMap<(i32, u64), f64> {
    history
        .all_series()
        .into_iter()
        .filter_map(|(k, s)| slope_per_min(&s).map(|v| (k, v)))
        .collect()
}

/// Подозреваемые по истории, по убыванию score. Один процесс — одна строка (самая весомая причина).
pub fn analyze(history: &History, th: &Thresholds) -> Vec<Suspect> {
    let Some(last) = history.latest() else { return Vec::new() };
    let total = last.system.total.max(1);
    let hog_min = th.hog_bytes.min((total as f64 * th.hog_pct / 100.0) as u64);
    let series = history.all_series();
    let now = last.taken_at;

    // Средний CPU за окно.
    let mut cpu_sum: HashMap<(i32, u64), (f32, u32)> = HashMap::new();
    for s in history.snapshots() {
        for p in &s.processes {
            let e = cpu_sum.entry((p.pid, p.started_at)).or_default();
            e.0 += p.cpu_pct;
            e.1 += 1;
        }
    }

    // Дубли: одинаковые dev-команды. Копия лишняя, если служит тому же, кому и другая
    // (тот же родитель) или никому (сирота). Копии с разными живыми родителями — это
    // каждая своя сессия (MCP-сервер на каждую сессию claude) и дублем не считаются.
    let mut dup: HashMap<String, Vec<&ProcessInfo>> = HashMap::new();
    for p in &last.processes {
        if !p.args.is_empty() && is_dev(&p.name) {
            dup.entry(p.command_line()).or_default().push(p);
        }
    }

    let mut best: HashMap<i32, Suspect> = HashMap::new();
    let mut put = |s: Suspect| {
        best.entry(s.pid).and_modify(|cur| if s.score > cur.score { *cur = s.clone() }).or_insert(s);
    };

    for p in &last.processes {
        let mem = p.reclaim();
        let key = (p.pid, p.started_at);
        let size_score = |base: f64| -> u8 { (base + 40.0 * (mem as f64 / (4.0 * GIB as f64)).min(1.0)).min(100.0) as u8 };
        let mk = |kind, score, reason: String| Suspect {
            pid: p.pid,
            name: p.name.clone(),
            app_group: p.app_group.clone(),
            cmd: short_cmd(p),
            group: None,
            started_at: p.started_at,
            kind,
            score,
            reason,
            reclaim: mem,
            policy: policy_for(p),
        };

        if p.state == ProcState::Zombie {
            put(mk(SuspectKind::Zombie, 30, format!("{} — зомби: умер, но родитель {} его не пожал", p.name, p.ppid)));
            continue;
        }
        if mem >= hog_min {
            put(mk(
                SuspectKind::Hog,
                size_score(40.0),
                format!("{} занимает {} ({} ОЗУ)", p.name, fmt::bytes(mem), fmt::percent(mem as f64 * 100.0 / total as f64)),
            ));
        }
        if let Some(s) = series.get(&key)
            && s.len() >= 3
            && s.last().unwrap().0 - s[0].0 >= th.growing_min_span
            && let Some(slope) = slope_per_min(s)
            && slope >= th.growing_per_min
        {
            let span = s.last().unwrap().0 - s[0].0;
            let rising = s.windows(2).filter(|w| w[1].1 >= w[0].1).count() as f64 / (s.len() - 1) as f64;
            if rising >= 0.6 {
                let score = (50.0 + 30.0 * (slope / (200.0 * MIB as f64)).min(1.0) + 20.0 * rising) as u8;
                put(mk(
                    SuspectKind::Growing,
                    score.min(100),
                    format!("{} {}, растёт {} уже {}", p.name, fmt::bytes(mem), fmt::bytes_per_min(slope), fmt::duration_human(span)),
                ));
            }
        }
        if p.ppid == 1 && is_dev(&p.name) && mem >= th.orphan_bytes {
            put(mk(
                SuspectKind::Orphan,
                size_score(45.0),
                format!("{} {} — сирота: родитель умер, процесс остался", p.name, fmt::bytes(mem)),
            ));
        }
        let age = now.saturating_sub(p.started_at);
        if mem >= th.idle_bytes
            && age >= th.idle_age
            && let Some((sum, n)) = cpu_sum.get(&key)
            && *n >= 3
            && sum / *n as f32 <= th.idle_cpu
        {
            put(mk(
                SuspectKind::Idle,
                size_score(25.0),
                format!("{} {} почти без CPU, живёт {}", p.name, fmt::bytes(mem), fmt::duration_human(age)),
            ));
        }
    }

    for (cmd, copies) in &dup {
        if copies.len() < th.duplicate_count {
            continue;
        }
        let mut by_parent: HashMap<i32, Vec<&ProcessInfo>> = HashMap::new();
        for p in copies {
            by_parent.entry(p.ppid).or_default().push(p);
        }
        // В каждой семье оставляем самую новую копию; у сирот (ppid 1) — ни одной.
        let mut extra: Vec<&ProcessInfo> = Vec::new();
        for (ppid, mut family) in by_parent {
            family.sort_by_key(|p| p.started_at);
            let keep = usize::from(ppid > 1);
            extra.extend(family.iter().take(family.len().saturating_sub(keep)));
        }
        if extra.len() < 2 {
            continue;
        }
        let total: u64 = extra.iter().map(|p| p.reclaim()).sum();
        let short: String = cmd.chars().take(60).collect();
        let group = format!("dup:{short}");
        for p in extra.iter().copied() {
            put(Suspect {
                pid: p.pid,
                name: p.name.clone(),
                app_group: p.app_group.clone(),
                cmd: short_cmd(p),
                group: Some(group.clone()),
                started_at: p.started_at,
                kind: SuspectKind::Duplicate,
                score: (30.0 + 40.0 * (total as f64 / (4.0 * GIB as f64)).min(1.0)) as u8,
                reason: format!(
                    "{} лишних копий «{short}» из {} (вместе {}), эта — {}",
                    extra.len(),
                    copies.len(),
                    fmt::bytes(total),
                    fmt::bytes(p.reclaim())
                ),
                reclaim: p.reclaim(),
                policy: policy_for(p),
            });
        }
    }

    let mut out: Vec<Suspect> = best.into_values().collect();
    out.sort_by(|a, b| b.score.cmp(&a.score).then(b.reclaim.cmp(&a.reclaim)));
    out
}

/// Суммы по приложениям, по убыванию памяти.
pub fn app_groups(snapshot: &Snapshot) -> Vec<AppGroup> {
    let mut map: HashMap<&str, AppGroup> = HashMap::new();
    for p in &snapshot.processes {
        let g = map.entry(p.app_group.as_str()).or_insert_with(|| AppGroup { name: p.app_group.clone(), ..Default::default() });
        g.pids.push(p.pid);
        g.footprint += p.reclaim();
        g.count += 1;
    }
    let mut out: Vec<AppGroup> = map.into_values().collect();
    out.sort_by_key(|g| std::cmp::Reverse(g.footprint));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ProcessInfo, SystemMemory};

    fn p(pid: i32, name: &str, mem: u64, ppid: i32, args: &[&str]) -> ProcessInfo {
        ProcessInfo {
            pid,
            ppid,
            name: name.into(),
            exe: Some(format!("/opt/homebrew/bin/{name}")),
            args: args.iter().map(|s| s.to_string()).collect(),
            uid: 501,
            footprint: Some(mem),
            rss: mem,
            cpu_pct: 0.0,
            threads: 4,
            state: ProcState::Sleeping,
            started_at: 1000 + pid as u64,
            app_group: name.into(),
        }
    }

    fn snap(t: u64, procs: Vec<ProcessInfo>) -> Snapshot {
        Snapshot { taken_at: t, system: SystemMemory { total: 24 * GIB, ..Default::default() }, processes: procs }
    }

    #[test]
    fn finds_hog_growing_orphan_duplicate() {
        let mut h = History::default();
        for i in 0..10u64 {
            h.push(snap(
                10_000 + i * 10,
                vec![
                    p(10, "node", 3 * GIB, 500, &["node", "big.js"]),
                    p(11, "node", 200 * MIB + i * 10 * MIB, 500, &["node", "leak.js"]),
                    p(12, "bun", 100 * MIB, 1, &["bun", "x.ts"]),
                    p(13, "node", 30 * MIB, 500, &["node", "mcp.js"]),
                    p(14, "node", 30 * MIB, 500, &["node", "mcp.js"]),
                    p(15, "node", 30 * MIB, 500, &["node", "mcp.js"]),
                ],
            ));
        }
        let s = analyze(&h, &Thresholds::default());
        let kind = |pid| s.iter().find(|x| x.pid == pid).map(|x| x.kind);
        assert_eq!(kind(10), Some(SuspectKind::Hog));
        assert_eq!(kind(11), Some(SuspectKind::Growing));
        assert_eq!(kind(12), Some(SuspectKind::Orphan));
        assert_eq!(kind(13), Some(SuspectKind::Duplicate));
        assert_eq!(kind(14), Some(SuspectKind::Duplicate));
        assert_eq!(kind(15), None, "самую новую копию оставляем");
        assert_eq!(s.iter().find(|x| x.pid == 13).unwrap().group, s.iter().find(|x| x.pid == 14).unwrap().group);
        assert!(s[0].score >= s[s.len() - 1].score);
        assert!(s.iter().find(|x| x.pid == 11).unwrap().reason.contains("растёт"));
    }

    #[test]
    fn sessions_with_own_parents_are_not_duplicates() {
        let mut h = History::default();
        // Три MCP-сервера, у каждого своя живая сессия claude — это не дубли.
        h.push(snap(1, vec![
            p(20, "node", 80 * MIB, 101, &["node", "mcp.js"]),
            p(21, "node", 80 * MIB, 102, &["node", "mcp.js"]),
            p(22, "node", 80 * MIB, 103, &["node", "mcp.js"]),
        ]));
        assert!(analyze(&h, &Thresholds::default()).iter().all(|x| x.kind != SuspectKind::Duplicate));
    }

    #[test]
    fn groups_sum() {
        let g = app_groups(&snap(1, vec![p(1, "a", 10, 1, &[]), p(2, "a", 5, 1, &[]), p(3, "b", 7, 1, &[])]));
        assert_eq!(g[0].name, "a");
        assert_eq!(g[0].footprint, 15);
        assert_eq!(g[0].count, 2);
    }
}

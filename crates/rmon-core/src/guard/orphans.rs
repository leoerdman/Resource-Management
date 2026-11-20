//! Автоочистка сирот: dev-процесс, чей родитель умер, простаивает `idle_mins` минут — сторож
//! снимает его сам, не дожидаясь красной зоны.
//!
//! «Сирота» — то же правило, что у подозреваемых ([`crate::analyze::is_orphan`]): что окно
//! показывает с меткой «Сирота», то и чистится. «Простаивает» — на каждом опросе CPU не выше
//! `idle_cpu_pct` и память не сдвинулась дальше допуска ([`mem_tolerance`]); любое движение
//! начинает отсчёт заново.
//!
//! Защита та же, что у снятия в красной зоне: «никогда», политика rmon (только «можно снять»),
//! сам сторож и его предки, сверка личности перед сигналом. Процессы LaunchAgent (их
//! перезапустит launchd) исполнитель пропускает — это видно только по окружению, см. [`runner`](super::runner).

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{Candidate, Rules, exe_base, never_match};
use crate::analyze::{Thresholds, is_orphan, short_cmd};
use crate::model::{MIB, ProcessInfo, Snapshot};
use crate::policy::KillPolicy;

/// Автоочистка сирот — `[guard.orphans]` в `config.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default, deny_unknown_fields)]
#[ts(export, rename = "GuardOrphans")]
pub struct OrphanConfig {
    /// Снимать сирот в простое.
    pub enabled: bool,
    /// Сколько минут сирота должна простоять.
    #[ts(type = "number")]
    pub idle_mins: u64,
    /// Простой — CPU не выше, %.
    pub idle_cpu_pct: f32,
}

impl Default for OrphanConfig {
    fn default() -> Self {
        Self { enabled: true, idle_mins: 5, idle_cpu_pct: 1.0 }
    }
}

/// Сирота, простоявшая положенное.
#[derive(Debug, Clone, PartialEq)]
pub struct IdleOrphan {
    /// Жертва; `kind` — имя исполняемого файла.
    pub victim: Candidate,
    /// Сколько простаивает, секунды.
    pub idle_secs: u64,
}

/// Сироты под наблюдением — для окна.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, rename = "GuardOrphanWatch")]
pub struct OrphanStatus {
    /// Сирот, которых сторож снимет, если простоят.
    pub watched: u32,
    /// Их память, байты.
    #[ts(type = "number")]
    pub mem: u64,
    /// Через сколько секунд снимется ближайшая (если так и простоит).
    #[ts(type = "number | null")]
    pub next_in_secs: Option<u64>,
}

/// Память «не сдвинулась»: в пределах 4 МБ или 2 % — что больше.
pub fn mem_tolerance(mem: u64) -> u64 {
    (4 * MIB).max(mem / 50)
}

#[derive(Debug, Clone, Copy)]
struct Mark {
    since_ms: u64,
    mem: u64,
}

/// Отсчёт простоя каждой сироты. Ключ — личность процесса `(pid, started_at)`.
#[derive(Debug, Default)]
pub struct OrphanWatch {
    marks: HashMap<(i32, u64), Mark>,
    /// Уже сняты (в пробе), LaunchAgent или отказ — больше не предлагать.
    skip: HashSet<(i32, u64)>,
    status: OrphanStatus,
}

impl OrphanWatch {
    /// Один опрос: обновить отсчёты, вернуть простоявших положенное (крупные первыми).
    #[allow(clippy::too_many_arguments)]
    pub fn observe(
        &mut self,
        cfg: &OrphanConfig,
        th: &Thresholds,
        rules: &Rules,
        snap: &Snapshot,
        exclude: &HashSet<i32>,
        now_ms: u64,
        policy: impl Fn(&ProcessInfo) -> KillPolicy,
    ) -> Vec<IdleOrphan> {
        let idle_ms = cfg.idle_mins * 60_000;
        let mut seen = HashSet::new();
        let mut due = Vec::new();
        let mut status = OrphanStatus::default();
        for p in &snap.processes {
            let key = (p.pid, p.started_at);
            if !is_orphan(p, th) || p.pid <= 1 || exclude.contains(&p.pid) || self.skip.contains(&key) {
                continue;
            }
            let cmd = p.command_line();
            if never_match(p, &cmd, &rules.never).is_some() || policy(p) != KillPolicy::Free {
                continue;
            }
            seen.insert(key);
            let mem = p.reclaim();
            let moved = |m: &Mark| p.cpu_pct > cfg.idle_cpu_pct || mem.abs_diff(m.mem) > mem_tolerance(m.mem);
            let mark = self.marks.entry(key).or_insert(Mark { since_ms: now_ms, mem });
            if moved(mark) {
                *mark = Mark { since_ms: now_ms, mem };
            }
            let idle_for = now_ms.saturating_sub(mark.since_ms);
            status.watched += 1;
            status.mem += mem;
            let left = idle_ms.saturating_sub(idle_for) / 1000;
            status.next_in_secs = Some(status.next_in_secs.map_or(left, |n| n.min(left)));
            if idle_for >= idle_ms {
                due.push(IdleOrphan {
                    victim: Candidate {
                        pid: p.pid,
                        started_at: p.started_at,
                        kind: exe_base(p).to_string(),
                        mem,
                        cmd: short_cmd(p),
                    },
                    idle_secs: idle_for / 1000,
                });
            }
        }
        // Ушедшие процессы забываем: pid может достаться новому.
        self.marks.retain(|k, _| seen.contains(k));
        let alive: HashSet<(i32, u64)> = snap.processes.iter().map(|p| (p.pid, p.started_at)).collect();
        self.skip.retain(|k| alive.contains(k));
        self.status = status;
        due.sort_by_key(|o| std::cmp::Reverse(o.victim.mem));
        due
    }

    /// Больше не предлагать этот процесс.
    pub fn skip(&mut self, pid: i32, started_at: u64) {
        self.marks.remove(&(pid, started_at));
        self.skip.insert((pid, started_at));
    }

    /// Очистка выключена — отсчёты сначала, когда её включат.
    pub fn clear(&mut self) {
        self.marks.clear();
        self.skip.clear();
        self.status = OrphanStatus::default();
    }

    /// Под наблюдением сейчас.
    pub fn status(&self) -> OrphanStatus {
        self.status.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::GuardConfig;
    use crate::model::{ProcState, SystemMemory};

    fn proc(pid: i32, ppid: i32, name: &str, mb: u64, cpu: f32) -> ProcessInfo {
        ProcessInfo {
            pid,
            ppid,
            name: name.into(),
            exe: Some(format!("/opt/homebrew/bin/{name}")),
            args: vec![name.into(), "gw.mjs".into()],
            uid: 501,
            footprint: Some(mb * MIB),
            rss: mb * MIB,
            cpu_pct: cpu,
            threads: 4,
            state: ProcState::Sleeping,
            started_at: 1_000 + pid as u64,
            app_group: name.into(),
        }
    }

    fn snap(procs: Vec<ProcessInfo>) -> Snapshot {
        Snapshot { taken_at: 0, system: SystemMemory::default(), processes: procs }
    }

    fn run(w: &mut OrphanWatch, s: &Snapshot, now_ms: u64) -> Vec<i32> {
        let cfg = OrphanConfig { idle_mins: 3, ..Default::default() };
        let rules = Rules::compile(&GuardConfig::default()).unwrap();
        w.observe(&cfg, &Thresholds::default(), &rules, s, &HashSet::new(), now_ms, |_| KillPolicy::Free)
            .into_iter()
            .map(|o| o.victim.pid)
            .collect()
    }

    const MIN: u64 = 60_000;

    #[test]
    fn idle_orphan_is_due_after_idle_minutes() {
        let mut w = OrphanWatch::default();
        let s = snap(vec![proc(10, 1, "node", 60, 0.0), proc(11, 500, "node", 60, 0.0)]);
        assert!(run(&mut w, &s, 0).is_empty());
        assert!(run(&mut w, &s, 2 * MIN).is_empty());
        assert_eq!(w.status().watched, 1, "под живым родителем — не сирота");
        assert_eq!(w.status().next_in_secs, Some(60));
        assert_eq!(run(&mut w, &s, 3 * MIN), vec![10]);
    }

    #[test]
    fn cpu_or_memory_movement_restarts_the_count() {
        let mut w = OrphanWatch::default();
        run(&mut w, &snap(vec![proc(10, 1, "node", 60, 0.0)]), 0);
        run(&mut w, &snap(vec![proc(10, 1, "node", 60, 5.0)]), 2 * MIN);
        assert!(run(&mut w, &snap(vec![proc(10, 1, "node", 60, 0.0)]), 4 * MIN).is_empty(), "CPU сбросил отсчёт");
        assert_eq!(run(&mut w, &snap(vec![proc(10, 1, "node", 61, 0.0)]), 5 * MIN), vec![10], "1 МБ — шум");
        run(&mut w, &snap(vec![proc(10, 1, "node", 90, 0.0)]), 6 * MIN);
        assert!(run(&mut w, &snap(vec![proc(10, 1, "node", 90, 0.0)]), 8 * MIN).is_empty(), "память сдвинулась");
    }

    #[test]
    fn small_protected_never_and_skipped_are_left_alone() {
        let mut w = OrphanWatch::default();
        let mut claude = proc(12, 1, "claude", 300, 0.0);
        claude.exe = Some("/Users/u/.local/bin/claude".into());
        let s = snap(vec![
            proc(10, 1, "node", 20, 0.0),    // меньше порога сироты
            claude,                          // «никогда»
            proc(13, 1, "Finder", 300, 0.0), // не dev
            proc(14, 1, "bun", 300, 0.0),
        ]);
        run(&mut w, &s, 0);
        assert_eq!(run(&mut w, &s, 3 * MIN), vec![14]);
        w.skip(14, 1_014);
        assert!(run(&mut w, &s, 10 * MIN).is_empty(), "пропущенную больше не предлагаем");

        let mut w = OrphanWatch::default();
        let cfg = OrphanConfig::default();
        let rules = Rules::compile(&GuardConfig::default()).unwrap();
        let confirm = |_: &ProcessInfo| KillPolicy::Confirm("тест".into());
        w.observe(&cfg, &Thresholds::default(), &rules, &s, &HashSet::new(), 0, confirm);
        let due = w.observe(&cfg, &Thresholds::default(), &rules, &s, &HashSet::new(), 60 * MIN, confirm);
        assert!(due.is_empty(), "только «можно снять»");
    }

    #[test]
    fn reused_pid_starts_over() {
        let mut w = OrphanWatch::default();
        run(&mut w, &snap(vec![proc(10, 1, "node", 60, 0.0)]), 0);
        let mut fresh = proc(10, 1, "node", 60, 0.0);
        fresh.started_at = 9_999;
        assert!(run(&mut w, &snap(vec![fresh.clone()]), 3 * MIN).is_empty(), "другой процесс с тем же pid");
        assert_eq!(run(&mut w, &snap(vec![fresh]), 6 * MIN), vec![10]);
    }
}

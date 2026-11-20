//! Ответы на синтетической сводке: без платформы, суммы и тексты проверяемы руками.

use super::*;
use rmon_core::analyze::SuspectKind;
use rmon_core::model::{AppGroup, GIB, MIB, ProcState, SystemMemory};
use rmon_core::platform::Signal;
use serde_json::json;

fn row(pid: i32, name: &str, app: &str, mem: u64, cpu: f32, growth: Option<f64>, policy: KillPolicy) -> ProcessRow {
    ProcessRow {
        pid,
        ppid: 1,
        name: name.into(),
        exe: Some(format!("/usr/local/bin/{name}")),
        args: vec![name.into(), "--serve".into()],
        uid: 501,
        footprint: Some(mem),
        rss: mem / 2,
        cpu_pct: cpu,
        threads: 4,
        state: ProcState::Sleeping,
        started_at: 1_000 + pid as u64,
        app_group: app.into(),
        cwd: None,
        growth_per_min: growth,
        policy,
    }
}

fn suspect(pid: i32, name: &str, score: u8, reclaim: u64, policy: KillPolicy) -> Suspect {
    Suspect {
        pid,
        name: name.into(),
        app_group: name.into(),
        cmd: format!("{name} --serve"),
        group: None,
        group_label: None,
        started_at: 1_000 + pid as u64,
        kind: SuspectKind::Hog,
        score,
        reason: format!("{name} большой"),
        reclaim,
        policy,
    }
}

fn point(t: u64, used: u64, swap: u64, pressure: Pressure) -> HistoryPoint {
    HistoryPoint { t, used, swap_used: swap, compressed: GIB, pressure }
}

fn dashboard() -> Dashboard {
    let confirm = KillPolicy::Confirm("браузер".into());
    let protected = KillPolicy::Protected("ядро системы".into());
    Dashboard {
        taken_at: 1_600,
        system: SystemMemory {
            total: 24 * GIB,
            used: 18 * GIB,
            other: 0,
            app: 9 * GIB,
            wired: 2 * GIB,
            compressed: 7 * GIB,
            cached_files: 3 * GIB,
            free: 200 * MIB,
            swap_total: 20 * GIB,
            swap_used: 10 * GIB,
            pressure: Pressure::Warning,
            available_pct: 38,
            ..SystemMemory::default()
        },
        history: vec![
            point(1_000, 17 * GIB, 8 * GIB, Pressure::Normal),
            point(1_300, 18 * GIB, 9 * GIB, Pressure::Critical),
            point(1_600, 18 * GIB, 10 * GIB, Pressure::Warning),
        ],
        groups: vec![
            AppGroup { name: "Google Chrome".into(), pids: vec![20, 21], footprint: 3 * GIB, count: 2 },
            AppGroup { name: "node".into(), pids: vec![10], footprint: 2 * GIB, count: 1 },
            AppGroup { name: "kernel".into(), pids: vec![30], footprint: GIB, count: 1 },
        ],
        processes: vec![
            row(10, "node", "node", 2 * GIB, 1.0, Some(40.0 * MIB as f64), KillPolicy::Free),
            row(20, "Google Chrome", "Google Chrome", 2 * GIB, 30.0, None, confirm.clone()),
            row(21, "Google Chrome Helper", "Google Chrome", GIB, 5.0, Some(-(MIB as f64)), confirm.clone()),
            row(30, "kernel_task", "kernel", GIB, 2.0, Some(0.0), protected.clone()),
        ],
        suspects: vec![
            suspect(10, "node", 80, 2 * GIB, KillPolicy::Free),
            suspect(20, "Google Chrome", 50, 2 * GIB, confirm),
            suspect(30, "kernel_task", 40, GIB, protected),
            suspect(40, "bun", 20, 300 * MIB, KillPolicy::Free),
        ],
    }
}

fn guard_entry() -> JournalEntry {
    let raw = json!({"t": "2026-11-20T10:18:00Z", "ev": "zone", "zone": "red", "why": "давление жёлтое"});
    JournalEntry {
        source: rmon_core::journal::Source::MemoryGuard,
        ts: Some(1_500),
        text: "зона red: давление жёлтое".into(),
        raw,
    }
}

#[test]
fn overview_formats_memory_trend_apps_and_guard() {
    let d = dashboard();
    let g = guard_entry();
    let (out, text) = overview(&d, Some(&g), 1_620);
    assert_eq!(out.memory.total, 24 * GIB);
    assert_eq!(out.memory.used_pct, 75.0);
    assert_eq!(out.memory.pressure, "warning");
    assert!(out.analysis_ready);
    assert_eq!(out.trend.points, 3);
    assert_eq!(out.trend.window_secs, 600);
    assert_eq!(out.trend.swap_delta, 2 * GIB as i64);
    assert_eq!(out.trend.used_delta, GIB as i64);
    assert_eq!(out.trend.pressure_max, "critical");
    assert_eq!(out.trend.elevated_secs, 300, "выше нормы — только отрезок после критической точки");
    let slope = out.trend.swap_per_min.unwrap();
    assert!((slope - (GIB as f64 / 5.0)).abs() < 1.0, "{slope}");
    assert_eq!(out.top_apps.len(), 3);
    assert_eq!(out.suspects.len(), 4);
    assert_eq!(out.suspects[1].policy.kind, "confirm");
    let gz = out.guard.as_ref().unwrap();
    assert_eq!((gz.zone.as_deref(), gz.ts), (Some("red"), Some(1_500)));

    assert!(
        text.starts_with(&format!(
            "Память: занято {} из {} (75 %), давление повышенное",
            bytes(18 * GIB),
            bytes(24 * GIB)
        )),
        "{text}"
    );
    assert!(text.contains(&format!("своп {}", bytes_delta(2 * GIB as i64))), "{text}");
    assert!(text.contains("максимум критическое, выше нормы 5 мин"), "{text}");
    assert!(text.contains("Сторож памяти: зона red: давление жёлтое (2 мин назад)."), "{text}");
    assert!(text.contains(&format!("1. Google Chrome — {} (2 проц.)", bytes(3 * GIB))), "{text}");
    assert!(
        text.contains(&format!(
            "1. [80] node (10) — node большой; освободит {}; {}",
            bytes(2 * GIB),
            KillPolicy::Free.label()
        )),
        "{text}"
    );

    // Сразу после запуска: одна точка — анализа и тренда ещё нет; журнала сторожа нет.
    let mut fresh = dashboard();
    fresh.history.truncate(1);
    fresh.suspects.clear();
    let (out, text) = overview(&fresh, None, 1_620);
    assert!(!out.analysis_ready && out.guard.is_none());
    assert!(
        text.contains("история ещё копится") && text.contains("анализ ещё не готов") && text.contains("журнала нет"),
        "{text}"
    );
    let schema = serde_json::to_value(schemars::schema_for!(OverviewOut)).unwrap();
    assert_eq!(schema["type"], "object");
}

#[test]
fn list_sorts_filters_and_groups() {
    let d = dashboard();
    let names = |out: &ListOut| out.processes.iter().map(|p| p.pid).collect::<Vec<_>>();
    let a = |sort, limit, filter: Option<&str>, group| ListArgs {
        sort,
        limit,
        filter: filter.map(Into::into),
        group_by_app: group,
    };

    let (out, text) = list(&d, &a(SortKey::Mem, 25, None, false));
    assert_eq!(out.matched, 4);
    assert_eq!(out.processes[0].mem, 2 * GIB);
    assert!(text.contains("показано 4 из 4"), "{text}");
    let (out, _) = list(&d, &a(SortKey::Cpu, 2, None, false));
    assert_eq!((names(&out), out.matched), (vec![20, 21], 4));
    let (out, text) = list(&d, &a(SortKey::Growth, 25, None, false));
    assert_eq!(names(&out), vec![10, 30, 21, 20], "неизвестный рост — в конце");
    assert!(text.contains(&bytes_per_min(-(MIB as f64))), "убыль со знаком: {text}");
    assert!(!text.contains("Рост пока неизвестен"), "{text}");
    let mut early = dashboard();
    early.processes.iter_mut().for_each(|r| r.growth_per_min = None);
    let (_, text) = list(&early, &a(SortKey::Growth, 25, None, false));
    assert!(text.contains("Рост пока неизвестен"), "сразу после запуска — подсказка: {text}");
    let (out, _) = list(&d, &a(SortKey::Pid, 25, None, false));
    assert_eq!(names(&out), vec![10, 20, 21, 30]);

    let (out, _) = list(&d, &a(SortKey::Mem, 25, Some("CHROME"), false));
    assert_eq!(names(&out), vec![20, 21], "фильтр без учёта регистра");
    let (out, _) = list(&d, &a(SortKey::Mem, 25, Some("30"), false));
    assert_eq!(names(&out), vec![30], "число — pid");
    let (out, _) = list(&d, &a(SortKey::Mem, 25, Some("--serve"), false));
    assert_eq!(out.matched, 4, "по строке запуска");
    let (out, _) = list(&d, &a(SortKey::Mem, 25, Some("  "), false));
    assert_eq!((out.matched, out.filter), (4, None), "пустой фильтр — без фильтра");

    let (out, text) = list(&d, &a(SortKey::Mem, 25, None, true));
    assert!(out.processes.is_empty());
    assert_eq!(out.apps[0].name, "Google Chrome");
    assert_eq!((out.apps[0].count, out.apps[0].mem, out.apps[0].cpu_pct), (2, 3 * GIB, 35.0));
    assert_eq!(out.apps[0].pids, vec![20, 21]);
    assert_eq!(out.apps[0].growth_per_min, Some(-(MIB as f64)));
    assert!(text.contains("Google Chrome (2 проц.)"), "{text}");
}

#[test]
fn cleanup_sums_by_policy() {
    let d = dashboard();
    let (out, text) = cleanup(&d, 30);
    assert_eq!(out.suspects.len(), 3, "score 20 ниже порога");
    assert_eq!((out.free_total, out.confirm_total, out.protected_total), (2 * GIB, 2 * GIB, GIB));
    let head =
        format!("Можно освободить ≈ {} без подтверждения, ещё {} — с подтверждением.", bytes(2 * GIB), bytes(2 * GIB));
    assert!(text.starts_with(&head), "{text}");
    assert!(text.contains("Защищённые"), "{text}");
    let (out, _) = cleanup(&d, 0);
    assert_eq!(out.free_total, 2 * GIB + 300 * MIB);
    let (out, text) = cleanup(&d, 100);
    assert!(out.suspects.is_empty() && text.contains("Подозреваемых со счётом от 100 нет"), "{text}");
}

#[test]
fn details_show_chain_children_and_series() {
    let d = dashboard();
    let brief = |pid: i32, name: &str, reclaim: u64| ProcBrief {
        pid,
        name: name.into(),
        started_at: 1,
        reclaim,
        cmd: name.into(),
    };
    let det = ProcessDetails {
        row: d.processes[0].clone(),
        command_line: "node --serve".into(),
        cwd: Some("/Users/me/project".into()),
        ancestors: vec![brief(5, "zsh", MIB), brief(1, "launchd", 0)],
        descendants: vec![brief(11, "esbuild", 10 * MIB), brief(12, "tsgo", 500 * MIB)],
        descendants_total: 510 * MIB,
        series: (0..100).map(|i| (1_000 + i * 5, GIB + i * MIB)).collect(),
    };
    let (out, text) = details(&det, None, d.suspects.first());
    assert_eq!(out.ancestors.len(), 2);
    assert_eq!(out.children.count, 2);
    assert_eq!(out.children.top[0].pid, 12, "потомки по убыванию памяти");
    let s = out.series.as_ref().unwrap();
    assert_eq!(s.points.len(), 30);
    assert_eq!((s.points[0].t, s.points[29].t), (1_000, 1_495));
    assert_eq!((s.min, s.max, s.span_secs), (GIB, GIB + 99 * MIB, 495));
    assert!((s.slope_per_min.unwrap() - 12.0 * MIB as f64).abs() < 1.0);
    assert_eq!(out.suspect.as_ref().map(|s| s.score), Some(80));
    assert!(text.contains("Родители: zsh (5) ← launchd (1)"), "{text}");
    assert!(text.contains("Каталог: /Users/me/project"), "{text}");
    assert!(text.contains(&format!("Потомков 2, вместе {}", bytes(510 * MIB))), "{text}");
    assert!(text.contains(&format!("Снятие: {}.", KillPolicy::Free.label())), "{text}");

    let bare = ProcessDetails { series: vec![], descendants: vec![], descendants_total: 0, cwd: None, ..det };
    let (out, text) = details(&bare, None, None);
    assert!(out.series.is_none() && text.contains("Ряда памяти пока нет") && text.contains("Потомков нет"), "{text}");
}

#[test]
fn kill_outcomes_map_to_statuses() {
    let confirm = KillPolicy::Confirm("системная утилита".into());
    let ok = KillOutcome {
        pid: 7,
        name: "sleep".into(),
        signal: Signal::Term,
        exited: true,
        reclaimed: MIB,
        dry_run: false,
        message: "sleep (7) снят".into(),
    };
    let out = kill_ok(&ok, &confirm);
    assert_eq!((out.status.as_str(), out.signal.as_deref(), out.is_error()), ("killed", Some("SIGTERM"), false));
    let dry = kill_ok(&KillOutcome { dry_run: true, exited: false, ..ok.clone() }, &confirm);
    assert_eq!((dry.status.as_str(), dry.signal.as_deref(), dry.is_error()), ("dry_run", None, false));
    assert!(dry.message.contains("с подтверждением"));
    let stuck = kill_ok(&KillOutcome { exited: false, ..ok }, &KillPolicy::Free);
    assert!(stuck.is_error() && stuck.status == "not_exited");

    let e = Error::ConfirmationRequired {
        pid: 7, name: "sleep".into(), reason: "системная утилита".into()
    };
    let out = kill_err(7, Some("sleep"), Some(&confirm), &e);
    assert_eq!(out.status, "needs_confirmation");
    assert!(out.is_error() && out.message.contains("согласие владельца"), "{}", out.message);
    let e = Error::Protected { pid: 1, name: "launchd".into(), reason: "ядро системы".into() };
    assert_eq!(kill_err(1, None, None, &e).status, "refused_protected");
    assert_eq!(kill_err(9, None, None, &Error::ProcessGone { pid: 9 }).status, "not_found");

    let launchd = KillPolicy::Protected("ядро системы".into());
    let out = kill_missing(1, None, Some(&launchd), true);
    assert_eq!(out.status, "refused_protected");
    assert!(out.message.contains("ядро системы"), "{}", out.message);
    assert_eq!(
        kill_missing(500, None, None, true).status,
        "refused_protected",
        "нечитаемый живой — чужой или системный"
    );
    assert_eq!(kill_missing(500, Some("sleep"), Some(&confirm), true).status, "not_in_snapshot");
    assert_eq!(kill_missing(500, None, None, false).status, "not_found");
}

#[test]
fn guard_events_list_and_missing_journal() {
    let g = guard_entry();
    let dir = std::env::temp_dir();
    let (out, text) = guard(std::slice::from_ref(&g), &dir, 1_560);
    assert!(out.exists);
    assert_eq!(out.events[0].ev, "zone");
    assert!(text.contains("1 мин назад: зона red"), "{text}");
    let missing = dir.join("rmon-нет-такого-журнала.jsonl");
    let (out, text) = guard(&[], &missing, 0);
    assert!(!out.exists && out.events.is_empty() && text.starts_with("Журнала memory-guard нет"), "{text}");
}

#[test]
fn downsample_keeps_ends() {
    let s: Vec<(u64, u64)> = (0..10).map(|i| (i, i)).collect();
    assert_eq!(downsample(&s, 20), s);
    let d = downsample(&s, 4);
    assert_eq!(d.len(), 4);
    assert_eq!((d[0], d[3]), ((0, 0), (9, 9)));
}

//! Сценарии перенесены из `memory-guard/guard.test.ts` — поведение сторожа не должно измениться.

use super::author::{Env, chain_of, origin_of, project_from_path};
use super::*;
use crate::model::{GIB, ProcState, SystemMemory};

const CLAUDE_CLI: &str =
    "/Users/u/Library/Application Support/Claude/claude-code/2.1.321/x/claude.app/Contents/MacOS/claude";

fn p(pid: i32, ppid: i32, exe: &str, args: &str, mb: u64) -> ProcessInfo {
    let name = Path::new(exe).file_name().and_then(|n| n.to_str()).unwrap_or(exe).to_string();
    ProcessInfo {
        pid,
        ppid,
        name: name.clone(),
        // В старых тестах exe бывал коротким именем (как его даёт ps): путь тогда неизвестен.
        exe: exe.contains('/').then(|| exe.to_string()),
        args: args.split(' ').map(str::to_string).collect(),
        uid: 501,
        footprint: None,
        rss: mb * MIB,
        cpu_pct: 0.0,
        threads: 1,
        state: ProcState::Running,
        started_at: 1_000 + pid as u64,
        app_group: name,
    }
}

/// Снимок, похожий на инцидент: tsgo 4,7 ГБ, node --test по 3,5, Telegram, Claude, bun-сервер, MCP.
fn incident() -> Vec<ProcessInfo> {
    vec![
        p(
            46610,
            1,
            "/Applications/Claude.app/Contents/MacOS/Claude",
            "/Applications/Claude.app/Contents/MacOS/Claude",
            900,
        ),
        p(
            47355,
            46610,
            "/Applications/Claude.app/Contents/Frameworks/Claude Helper (Renderer).app/Contents/MacOS/Claude Helper (Renderer)",
            "Claude Helper (Renderer) --type=renderer",
            2500,
        ),
        p(
            744,
            46610,
            "/Applications/Claude.app/Contents/Helpers/disclaimer",
            "/Applications/Claude.app/Contents/Helpers/disclaimer --pgroup -- claude",
            5,
        ),
        p(
            746,
            744,
            CLAUDE_CLI,
            &format!("{CLAUDE_CLI} --output-format stream-json --resume=6db26558-4d99-4d9f-9442-d4847f6eb3d7"),
            350,
        ),
        p(42503, 746, "/bin/zsh", "/bin/zsh -c source snapshot.sh && bun script/check_unused_declarations.ts", 3),
        p(43217, 42503, "bun", "bun script/check_unused_declarations.ts", 80),
        p(
            43341,
            43217,
            "node",
            "node /Users/u/Programming - Main/M Code/TEMPORARY/mcode/lanes/START/MCodeDesktop/node_modules/.bin/tsgo --noEmit",
            60,
        ),
        p(
            43346,
            43341,
            "/Users/u/Programming - Main/M Code/MCodeDesktop/node_modules/@typescript/native-preview-darwin-arm64/lib/tsgo",
            "/Users/u/.../lib/tsgo --noEmit --incremental",
            4700,
        ),
        p(92100, 746, "/bin/zsh", "/bin/zsh -c timeout 900 xargs -0 node --test", 3),
        p(
            92188,
            92100,
            "node",
            "node --test /Users/u/Programming - Main/Auto Reels Infinity/.claude/worktrees/drum-mask/engine/tests/a.test.mjs",
            70,
        ),
        p(92201, 92188, "/usr/local/bin/node", "/usr/local/bin/node --test-concurrency=0 a.test.mjs", 3600),
        p(93683, 92188, "/usr/local/bin/node", "/usr/local/bin/node --test-concurrency=0 b.test.mjs", 3200),
        p(38496, 92100, "bun", "bun test --timeout 30000 test/server/a.test.ts", 1600),
        p(50000, 42503, "bun", "bun run src/index.ts serve --port 4096", 3000),
        p(50001, 746, "node", "node /Users/u/.npm/_npx/x/index.js", 2000),
        p(
            50002,
            46610,
            "/Applications/Claude.app/Contents/Resources/node",
            "/Applications/Claude.app/Contents/Resources/node helper.js",
            6000,
        ),
        p(54291, 1, "/Applications/Telegram Lite.app/Contents/MacOS/Telegram Lite", "Telegram Lite", 2600),
        p(
            407,
            1,
            "/System/Library/PrivateFrameworks/SkyLight.framework/Resources/WindowServer",
            "WindowServer -daemon",
            900,
        ),
        p(87541, 1, "/opt/homebrew/bin/python3.11", "python3.11 worker.py", 1200),
        p(
            60000,
            1,
            "/Users/u/.bun/bin/bun",
            "/Users/u/.bun/bin/bun /Users/u/Programming - Main/Hooks/memory-guard/guard.ts",
            30,
        ),
        p(61000, 1, "/Applications/M Code Desktop.app/Contents/Resources/bin/mcode-engine", "mcode-engine serve", 5000),
        p(61001, 61000, "/bin/zsh", "/bin/zsh -c bun test", 3),
        p(61002, 61001, "bun", "bun test packages/engine", 2900),
        p(62000, 746, "/Users/u/node_modules/.bin/tsgo", "tsgo --lsp --stdio", 8000),
    ]
}

fn snap(procs: Vec<ProcessInfo>, mem: SystemMemory) -> Snapshot {
    Snapshot { taken_at: 1, system: mem, processes: procs }
}

fn sys(pressure: Pressure, available_pct: u8, swap_gb: f64) -> SystemMemory {
    SystemMemory {
        total: 24 * GIB,
        pressure,
        available_pct,
        swap_used: (swap_gb * GIB as f64) as u64,
        ..Default::default()
    }
}

/// Политика в тестах: всё свободно, кроме того, что решает сам сторож.
fn free(_: &ProcessInfo) -> KillPolicy {
    KillPolicy::Free
}

fn rank(procs: Vec<ProcessInfo>, cfg: &GuardConfig) -> (Vec<Candidate>, Vec<Rejected>) {
    let s = snap(procs, sys(Pressure::Critical, 4, 10.0));
    let rules = Rules::compile(cfg).unwrap();
    rank_victims_with(&s, &rules, &self_and_ancestors(&s, 60000), free)
}

#[test]
fn zone_by_real_pressure_not_percent() {
    let red = RedConfig::default();
    assert_eq!(judge_zone(&sys(Pressure::Critical, 60, 1.0), &red).0, Zone::Red);
    assert_eq!(judge_zone(&sys(Pressure::Warning, 60, 1.0), &red).0, Zone::Yellow);
    assert_eq!(judge_zone(&sys(Pressure::Normal, 60, 1.0), &red).0, Zone::Green);
}

#[test]
fn zone_safety_nets() {
    let red = RedConfig::default();
    assert_eq!(judge_zone(&sys(Pressure::Normal, 3, 1.0), &red), (Zone::Red, "доступно 3 %".into()));
    assert_eq!(judge_zone(&sys(Pressure::Warning, 60, 12.7), &red).0, Zone::Red);
    // большой своп при зелёном давлении — не повод
    assert_eq!(judge_zone(&sys(Pressure::Normal, 60, 12.7), &red).0, Zone::Green);
}

#[test]
fn hysteresis_needs_sustained_red() {
    let mut h = Hysteresis::default();
    assert!(!h.feed(Zone::Red, 0, 12_000));
    assert!(!h.feed(Zone::Red, 9_000, 12_000));
    assert!(h.feed(Zone::Red, 12_000, 12_000));

    let mut h = Hysteresis::default();
    h.feed(Zone::Red, 0, 12_000);
    h.feed(Zone::Red, 9_000, 12_000);
    assert!(!h.feed(Zone::Yellow, 10_000, 12_000), "один не-красный опрос сбрасывает отсчёт");
    assert!(!h.feed(Zone::Red, 11_000, 12_000));
    assert!(!h.feed(Zone::Red, 20_000, 12_000));
    assert!(h.feed(Zone::Red, 23_000, 12_000));

    let mut h = Hysteresis::default();
    h.feed(Zone::Red, 0, 6_000);
    assert!(h.feed(Zone::Red, 6_000, 6_000));
    h.reset();
    assert!(!h.feed(Zone::Red, 7_000, 6_000), "после снятия — снова ждать");
    assert!(h.feed(Zone::Red, 13_000, 6_000));
}

#[test]
fn kill_limiter_windows_are_independent() {
    let limits = [Limit { window_secs: 60, max: 2 }, Limit { window_secs: 600, max: 3 }];
    let mut l = KillLimiter::default();
    l.record(&limits, 0);
    assert!(l.blocked(&limits, 1_000).is_none());
    l.record(&limits, 1_000);
    assert!(l.blocked(&limits, 2_000).unwrap().contains("за 60 с"));
    assert!(l.blocked(&limits, 61_500).is_none());
    l.record(&limits, 61_500);
    assert!(l.blocked(&limits, 130_000).unwrap().contains("за 600 с"));
    assert!(l.blocked(&limits, 602_000).is_none());
}

#[test]
fn incident_victims_in_order() {
    let (c, _) = rank(incident(), &GuardConfig::default());
    let got: Vec<(i32, &str)> = c.iter().map(|c| (c.pid, c.kind.as_str())).collect();
    assert_eq!(got, [(43346, "tsgo"), (92201, "node"), (93683, "node"), (61002, "bun"), (38496, "bun")]);
}

#[test]
fn never_beats_allow() {
    let (c, rejected) = rank(incident(), &GuardConfig::default());
    let ids: Vec<i32> = c.iter().map(|c| c.pid).collect();
    for pid in [50002, 54291, 61000, 407, 47355, 746] {
        assert!(!ids.contains(&pid), "{pid} не жертва");
    }
    let why = |pid| rejected.iter().find(|r| r.pid == pid).map(|r| r.why.clone());
    assert_eq!(why(50002).as_deref(), Some("никогда: Claude и его хелперы"));
    assert_eq!(why(54291).as_deref(), Some("никогда: Telegram"));
}

#[test]
fn servers_mcp_and_lsp_are_not_runs() {
    let (c, rejected) = rank(incident(), &GuardConfig::default());
    let ids: Vec<i32> = c.iter().map(|c| c.pid).collect();
    for pid in [50000, 50001, 62000] {
        assert!(!ids.contains(&pid));
    }
    assert!(rejected.iter().find(|r| r.pid == 50001).unwrap().why.contains("родитель claude"));
    let cfg = GuardConfig { min_victim_mb: 2800, ..GuardConfig::default() };
    let ids: Vec<i32> = rank(incident(), &cfg).0.iter().map(|c| c.pid).collect();
    assert_eq!(ids, [43346, 92201, 93683, 61002]);
}

#[test]
fn guard_itself_is_untouchable() {
    let mut procs = incident();
    let me = procs.iter_mut().find(|p| p.pid == 60000).unwrap();
    me.exe = None;
    me.name = "bun".into();
    me.args = vec!["bun".into(), "guard.ts".into()];
    me.rss = 9000 * MIB;
    assert_eq!(rank(procs, &GuardConfig::default()).0[0].pid, 43346);
}

#[test]
fn footprint_beats_rss() {
    let mut procs = incident();
    procs.iter_mut().find(|p| p.pid == 93683).unwrap().footprint = Some(7000 * MIB);
    let c = rank(procs, &GuardConfig::default()).0;
    assert_eq!((c[0].pid, c[0].mem), (93683, 7000 * MIB));
}

#[test]
fn small_is_not_a_victim() {
    let cfg = GuardConfig { min_victim_mb: 5000, ..GuardConfig::default() };
    assert!(rank(incident(), &cfg).0.is_empty());
}

#[test]
fn rmon_policy_is_an_extra_fence() {
    let s = snap(incident(), sys(Pressure::Critical, 4, 10.0));
    let rules = Rules::compile(&GuardConfig::default()).unwrap();
    let confirm_tsgo =
        |p: &ProcessInfo| if p.pid == 43346 { KillPolicy::Confirm("тест".into()) } else { KillPolicy::Free };
    let (c, rejected) = rank_victims_with(&s, &rules, &HashSet::new(), confirm_tsgo);
    assert_eq!(c[0].pid, 92201);
    assert!(rejected.iter().find(|r| r.pid == 43346).unwrap().why.starts_with("политика rmon"));
}

#[test]
fn state_machine_acts_once_then_pauses() {
    let cfg = GuardConfig { sustain_secs: 6, after_kill_secs: 20, ..GuardConfig::default() };
    let rules = Rules::compile(&cfg).unwrap();
    let red = snap(incident(), sys(Pressure::Critical, 4, 10.0));
    let mut g = Guard::default();
    let o = g.observe_with(&cfg, &rules, &red, 60000, 0, free);
    assert!(o.zone_changed);
    assert_eq!(o.decision, Decision::Wait);
    let o = g.observe_with(&cfg, &rules, &red, 60000, 6_000, free);
    let Decision::Act { victim, red_for_secs, .. } = o.decision else { panic!("{o:?}") };
    assert_eq!((victim.pid, red_for_secs), (43346, 6));
    g.acted(&cfg, 6_000);
    assert_eq!(g.observe_with(&cfg, &rules, &red, 60000, 20_000, free).decision, Decision::Wait, "пауза после снятия");
    // После паузы — снова ждать sustain.
    assert_eq!(g.observe_with(&cfg, &rules, &red, 60000, 27_000, free).decision, Decision::Wait);
    assert!(matches!(g.observe_with(&cfg, &rules, &red, 60000, 33_000, free).decision, Decision::Act { .. }));
}

#[test]
fn no_victim_is_said_once_per_red_episode() {
    let cfg = GuardConfig { sustain_secs: 0, min_victim_mb: 100_000, ..GuardConfig::default() };
    let rules = Rules::compile(&cfg).unwrap();
    let red = snap(incident(), sys(Pressure::Critical, 4, 10.0));
    let green = snap(incident(), sys(Pressure::Normal, 60, 1.0));
    let mut g = Guard::default();
    assert!(matches!(g.observe_with(&cfg, &rules, &red, 60000, 0, free).decision, Decision::NoVictim { .. }));
    assert_eq!(g.observe_with(&cfg, &rules, &red, 60000, 1_000, free).decision, Decision::Wait);
    g.observe_with(&cfg, &rules, &green, 60000, 2_000, free);
    assert!(matches!(g.observe_with(&cfg, &rules, &red, 60000, 3_000, free).decision, Decision::NoVictim { .. }));
}

#[test]
fn broken_regex_is_reported_at_compile() {
    let mut cfg = GuardConfig::default();
    cfg.allow[0].exe = "(".into();
    assert!(Rules::compile(&cfg).unwrap_err().contains("tsgo"));
}

fn chain(procs: &[ProcessInfo], pid: i32) -> Snapshot {
    let _ = pid;
    snap(procs.to_vec(), SystemMemory::default())
}

#[test]
fn origin_claude_code_session() {
    let s = chain(&incident(), 43346);
    let c = chain_of(&s, 43346);
    let session: Env = [("CLAUDE_CODE_SESSION_NAME", "Старт"), ("CLAUDE_CODE_HOST_SESSION_ID", "local_1")]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let o = origin_of(&c, &Env::new(), &session);
    assert_eq!(
        o,
        author::Origin::ClaudeCode {
            via: "bash".into(),
            session_id: Some("6db26558-4d99-4d9f-9442-d4847f6eb3d7".into()),
            host_session_id: Some("local_1".into()),
            title: Some("Старт".into()),
            entrypoint: None,
            session_pid: 746,
        }
    );
}

#[test]
fn origin_mcode_launchagent_terminal_orphan() {
    let s = chain(&incident(), 61002);
    assert!(matches!(
        origin_of(&chain_of(&s, 61002), &Env::new(), &Env::new()),
        author::Origin::Mcode { session_pid: Some(61000), .. }
    ));
    let env = |k: &str, v: &str| -> Env { [(k.to_string(), v.to_string())].into_iter().collect() };
    let job = snap(
        vec![p(70001, 70000, "node", "node gate.mjs", 1), p(70000, 1, "/usr/bin/python3", "python3 mac_jobs.py", 1)],
        SystemMemory::default(),
    );
    assert_eq!(
        origin_of(
            &chain_of(&job, 70001),
            &env("XPC_SERVICE_NAME", "com.auto-reels-infinity.nightly-gate"),
            &Env::new()
        ),
        author::Origin::LaunchAgent { label: "com.auto-reels-infinity.nightly-gate".into() }
    );
    let term = snap(
        vec![
            p(3, 2, "node", "node x", 1),
            p(2, 1, "/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal", "Terminal", 1),
        ],
        SystemMemory::default(),
    );
    assert_eq!(
        origin_of(&chain_of(&term, 3), &Env::new(), &Env::new()),
        author::Origin::Terminal { app: "Terminal".into() }
    );
    let orphan = snap(vec![p(5, 1, "tsgo", "tsgo --noEmit", 1)], SystemMemory::default());
    assert_eq!(origin_of(&chain_of(&orphan, 5), &env("XPC_SERVICE_NAME", "0"), &Env::new()), author::Origin::Orphan);
}

#[test]
fn project_lane_worktree_scratchpad() {
    assert_eq!(
        project_from_path(Some("/Users/u/Programming - Main/M Code/TEMPORARY/mcode/lanes/TITLE-INLINE/MCodeDesktop")),
        (Some("M Code".into()), Some("TITLE-INLINE".into()))
    );
    assert_eq!(
        project_from_path(Some("/Users/u/Programming - Main/Auto Reels Infinity/.claude/worktrees/drum-mask")),
        (Some("Auto Reels Infinity".into()), Some("drum-mask".into()))
    );
    assert_eq!(
        project_from_path(Some("/private/tmp/claude-501/-Users-u-Programming---Main-M-Code/abc/scratchpad"))
            .0
            .as_deref(),
        Some("M Code")
    );
}

#[test]
fn never_rules_split_for_policy() {
    let rules = never_as_policy_rules(&[RuleSpec {
        name: "x".into(),
        exe: Some("a".into()),
        path: None,
        args: Some("b".into()),
    }]);
    assert_eq!(rules.len(), 2);
    assert!(
        rules.iter().all(|r| [r.exe.is_some(), r.path.is_some(), r.args.is_some()].iter().filter(|&&b| b).count() == 1)
    );
}

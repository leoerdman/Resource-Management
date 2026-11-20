//! `rmon` — кто съел память и можно ли его снять. Командная строка поверх `rmon-core`.
//!
//! Коды выхода: 0 — успех; 1 — ошибка; 2 — снятие запрещено политикой;
//! 3 — нужно подтверждение (`--confirm`); 4 — процесс не найден или уже завершился.

use std::io::IsTerminal;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use rmon_core::analyze::{Suspect, app_groups};
use rmon_core::config::{self, Config};
use rmon_core::fmt::{bytes, bytes_per_min, duration_human, percent};
use rmon_core::guard::{self, GuardMode};
use rmon_core::kill::{KillRequest, Requester};
use rmon_core::monitor::{Monitor, MonitorConfig, ProcessRow, Tick};
use rmon_core::policy::{KillPolicy, Policy};
use rmon_core::{Error, Pressure, SystemMemory};
use serde::Serialize;

#[derive(Parser)]
#[command(name = "rmon", version, about = "Кто съел память, растёт ли он и можно ли его снять")]
struct Cli {
    /// Вывод в JSON (структуры ядра как есть).
    #[arg(long, global = true)]
    json: bool,
    /// Без цвета (также env NO_COLOR).
    #[arg(long, global = true)]
    no_color: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Clone, Copy, ValueEnum)]
enum SortBy {
    Mem,
    Cpu,
    Rss,
    Growth,
    Pid,
}

#[derive(Subcommand)]
enum Cmd {
    /// Сводка: давление, разбивка, своп, топ приложений, подозреваемые (по умолчанию).
    Status,
    /// Таблица процессов.
    Ps {
        /// Сортировка.
        #[arg(long, value_enum, default_value = "mem")]
        sort: SortBy,
        /// Сколько строк (0 — все).
        #[arg(long, default_value_t = 25)]
        limit: usize,
        /// Суммы по приложениям вместо процессов.
        #[arg(long)]
        group: bool,
        /// Фильтр по имени, приложению, строке запуска или pid.
        #[arg(long)]
        filter: Option<String>,
    },
    /// Подозреваемые по истории за окно.
    Suggest {
        /// Окно наблюдения: 10s, 2m.
        #[arg(long, default_value = "10s", value_parser = parse_duration)]
        window: Duration,
        /// Шаг снимков: 2s, 500ms.
        #[arg(long, default_value = "2s", value_parser = parse_duration)]
        interval: Duration,
    },
    /// Всё о процессе: строка запуска, рабочий каталог, родители, потомки, политика.
    Why {
        /// Идентификатор процесса.
        pid: i32,
    },
    /// Снять процесс через защитный слой.
    Kill {
        /// Идентификатор процесса.
        pid: i32,
        /// Подтвердить снятие процесса с политикой «с подтверждением».
        #[arg(long)]
        confirm: bool,
        /// Сразу SIGKILL, без ожидания после SIGTERM.
        #[arg(long)]
        hard: bool,
        /// Только проверить, ничего не посылать.
        #[arg(long)]
        dry_run: bool,
        /// Причина — попадёт в журнал действий.
        #[arg(long, default_value = "")]
        reason: String,
    },
    /// Настройки: где лежат, что действует, какие правила не разобрались.
    Config,
    /// Сторож памяти: в красной зоне сам снимает самый большой прогон.
    Guard {
        #[command(subcommand)]
        action: Option<GuardCmd>,
    },
    /// MCP-сервер для Claude Code (stdio).
    Mcp,
}

#[derive(Subcommand)]
enum GuardCmd {
    /// Режим, старый сторож, кто держит сторожа, последние события (по умолчанию).
    Status,
    /// Проба: зона сейчас и кого бы снял — ничего не трогает.
    Now,
    /// Включить сторожа (боевой режим).
    On,
    /// Выключить сторожа.
    Off,
    /// Режим пробы: только «убил бы».
    Dry,
    /// Боевой режим.
    Arm,
    /// Сторожить без окна (для LaunchAgent/терминала); события — в stdout.
    Run,
    /// Перенести обязанность старого LaunchAgent memory-guard в rmon (обратимо).
    Retire,
    /// Вернуть старый memory-guard.
    Restore,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let paint = Paint::new(cli.no_color);
    match run(cli, paint) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let code = match e.downcast_ref::<Error>() {
                Some(Error::Protected { .. }) => 2,
                Some(Error::ConfirmationRequired { .. }) => 3,
                Some(Error::ProcessGone { .. } | Error::IdentityMismatch { .. }) => 4,
                _ => 1,
            };
            eprintln!("{} {e:#}", paint.red("rmon:"));
            if code == 3 {
                eprintln!("Это процесс приложения владельца. Если снять действительно нужно — повторите с --confirm.");
            }
            ExitCode::from(code)
        }
    }
}

fn run(cli: Cli, paint: Paint) -> Result<()> {
    let json = cli.json;
    match cli.cmd.unwrap_or(Cmd::Status) {
        Cmd::Status => status(json, paint),
        Cmd::Ps { sort, limit, group, filter } => ps(json, sort, limit, group, filter.as_deref(), paint),
        Cmd::Suggest { window, interval } => suggest(json, window, interval, paint),
        Cmd::Why { pid } => why(json, pid, paint),
        Cmd::Kill { pid, confirm, hard, dry_run, reason } => kill(json, pid, confirm, hard, dry_run, reason),
        Cmd::Config => show_config(json),
        Cmd::Guard { action } => guard_cmd(json, action.unwrap_or(GuardCmd::Status), paint),
        Cmd::Mcp => rmon_mcp::serve(),
    }
}

/// Два снимка с паузой: второй даёт CPU и первую точку анализа.
fn sample(pause: Duration) -> Result<(Monitor, Tick)> {
    let mut m = Monitor::new(MonitorConfig { analyze_every: 1, ..MonitorConfig::default() })?;
    m.tick()?;
    std::thread::sleep(pause);
    let t = m.tick()?;
    Ok((m, t))
}

fn print_json<T: Serialize>(v: &T) -> Result<()> {
    println!("{}", serde_json::to_string(v)?);
    Ok(())
}

fn status(json: bool, paint: Paint) -> Result<()> {
    let (_, t) = sample(Duration::from_secs(1))?;
    let d = &t.dashboard;
    let top_apps = &d.groups[..d.groups.len().min(10)];
    let top_suspects = &d.suspects[..d.suspects.len().min(5)];
    if json {
        #[derive(Serialize)]
        struct Status<'a> {
            taken_at: u64,
            system: &'a SystemMemory,
            groups: &'a [rmon_core::AppGroup],
            suspects: &'a [Suspect],
        }
        return print_json(&Status {
            taken_at: d.taken_at,
            system: &d.system,
            groups: top_apps,
            suspects: top_suspects,
        });
    }
    let m = &d.system;
    println!(
        "Давление {}  ·  доступно {}",
        paint.pressure(m.pressure, m.pressure.as_str()),
        percent(f64::from(m.available_pct))
    );
    println!(
        "Занято {} из {}  ·  своп {} из {}  ·  сжато {}",
        paint.bold(&bytes(m.used)),
        bytes(m.total),
        paint.bold(&bytes(m.swap_used)),
        bytes(m.swap_total),
        bytes(m.compressed)
    );
    println!(
        "{}",
        paint.dim(&format!(
            "Приложения {} · Wired {} · Сжато {} · Кэш {} · Свободно {}",
            bytes(m.app),
            bytes(m.wired),
            bytes(m.compressed),
            bytes(m.cached_files),
            bytes(m.free)
        ))
    );
    println!("\n{}", paint.bold("Приложения"));
    for g in top_apps {
        println!("  {:>9}  {:>4}  {}", bytes(g.footprint), g.count, g.name);
    }
    print_suspects(top_suspects, paint);
    Ok(())
}

fn print_suspects(list: &[Suspect], paint: Paint) {
    println!("\n{}", paint.bold("Подозреваемые"));
    if list.is_empty() {
        println!("  {}", paint.dim("нет — с памятью всё в порядке"));
        return;
    }
    for s in list {
        println!("  {:>3}  {:>7}  {:>9}  {}  {}", s.score, s.pid, bytes(s.reclaim), s.reason, paint.policy(&s.policy));
    }
}

fn matches(r: &ProcessRow, f: &str) -> bool {
    let f = f.to_lowercase();
    r.pid.to_string() == f
        || r.name.to_lowercase().contains(&f)
        || r.app_group.to_lowercase().contains(&f)
        || r.args.join(" ").to_lowercase().contains(&f)
}

fn ps(json: bool, sort: SortBy, limit: usize, group: bool, filter: Option<&str>, paint: Paint) -> Result<()> {
    let (_, t) = sample(Duration::from_millis(500))?;
    let take = if limit == 0 { usize::MAX } else { limit };
    if group {
        let mut snap = t.snapshot;
        if let Some(f) = filter {
            let keep: std::collections::HashSet<i32> =
                t.dashboard.processes.iter().filter(|r| matches(r, f)).map(|r| r.pid).collect();
            snap.processes.retain(|p| keep.contains(&p.pid));
        }
        let groups: Vec<_> = app_groups(&snap).into_iter().take(take).collect();
        if json {
            return print_json(&groups);
        }
        println!("{:>10} {:>5}  приложение", "память", "проц");
        for g in groups {
            println!("{:>10} {:>5}  {}", bytes(g.footprint), g.count, g.name);
        }
        return Ok(());
    }
    let mut rows: Vec<ProcessRow> = t.dashboard.processes;
    if let Some(f) = filter {
        rows.retain(|r| matches(r, f));
    }
    match sort {
        SortBy::Mem => rows.sort_by_key(|r| std::cmp::Reverse(r.reclaim())),
        SortBy::Rss => rows.sort_by_key(|r| std::cmp::Reverse(r.rss)),
        SortBy::Cpu => rows.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct)),
        SortBy::Growth => {
            // Рост считается по истории, а у разового прогона её нет — нужен `suggest`.
            rows.retain(|r| r.growth_per_min.is_some());
            if rows.is_empty() && !json {
                println!("Рост виден только по истории: `rmon suggest --window 2m` или окно Resource Monitor.");
                return Ok(());
            }
            rows.sort_by(|a, b| b.growth_per_min.unwrap_or(0.0).total_cmp(&a.growth_per_min.unwrap_or(0.0)));
        }
        SortBy::Pid => rows.sort_by_key(|r| r.pid),
    }
    rows.truncate(take);
    if json {
        return print_json(&rows);
    }
    println!("{:>7} {:>10} {:>10} {:>6} {:>4}  {:<28} политика", "pid", "память", "rss", "cpu", "пот", "имя");
    for r in rows {
        let name: String = r.name.chars().take(28).collect();
        println!(
            "{:>7} {:>10} {:>10} {:>6.1} {:>4}  {:<28} {}",
            r.pid,
            r.footprint.map_or_else(|| "—".to_string(), bytes),
            if r.rss == 0 && r.footprint.is_none() { "—".to_string() } else { bytes(r.rss) },
            r.cpu_pct,
            r.threads,
            name,
            paint.policy(&r.policy)
        );
    }
    Ok(())
}

fn suggest(json: bool, window: Duration, interval: Duration, paint: Paint) -> Result<()> {
    if interval.is_zero() {
        bail!("--interval должен быть больше нуля");
    }
    let mut m = Monitor::new(MonitorConfig { analyze_every: 1, ..MonitorConfig::default() })?;
    let steps = (window.as_millis() / interval.as_millis()).max(2) as usize + 1;
    if !json {
        eprintln!("{}", paint.dim(&format!("наблюдаю {} …", duration_human(window.as_secs().max(1)))));
    }
    let mut last = m.tick()?;
    for _ in 1..steps {
        std::thread::sleep(interval);
        last = m.tick()?;
    }
    let suspects = last.dashboard.suspects;
    if json {
        return print_json(&suspects);
    }
    let free: u64 = suspects.iter().filter(|s| s.policy == KillPolicy::Free).map(|s| s.reclaim).sum();
    let confirm: u64 = suspects.iter().filter(|s| matches!(s.policy, KillPolicy::Confirm(_))).map(|s| s.reclaim).sum();
    println!(
        "Можно освободить ≈ {} без подтверждения, ещё {} — с подтверждением.",
        paint.bold(&bytes(free)),
        bytes(confirm)
    );
    print_suspects(&suspects, paint);
    Ok(())
}

fn why(json: bool, pid: i32, paint: Paint) -> Result<()> {
    let (mut m, t) = sample(Duration::from_millis(500))?;
    let d = m.details(&t, pid).ok_or(Error::ProcessGone { pid })?;
    if json {
        return print_json(&d);
    }
    let r = &d.row;
    println!("{} {}  ·  {}", paint.bold(&r.name), r.pid, r.app_group);
    println!("  запуск   {}", d.command_line);
    println!("  каталог  {}", d.cwd.as_deref().unwrap_or("— (нет прав)"));
    println!(
        "  память   {} (rss {}), CPU {}, потоков {}, состояние {}",
        r.footprint.map_or_else(|| "—".into(), bytes),
        bytes(r.rss),
        percent(f64::from(r.cpu_pct)),
        r.threads,
        r.state.as_str()
    );
    if let Some(g) = r.growth_per_min {
        println!("  рост     {}", bytes_per_min(g));
    }
    println!("  политика {}", paint.policy(&r.policy));
    let chain: Vec<String> = d.ancestors.iter().map(|a| format!("{} ({})", a.name, a.pid)).collect();
    println!("  родители {}", if chain.is_empty() { "—".into() } else { chain.join(" ← ") });
    println!("  потомки  {} шт., {}", d.descendants.len(), bytes(d.descendants_total));
    for c in d.descendants.iter().take(15) {
        println!("    {:>7}  {:>9}  {}", c.pid, bytes(c.reclaim), c.cmd);
    }
    Ok(())
}

fn kill(json: bool, pid: i32, confirm: bool, hard: bool, dry_run: bool, reason: String) -> Result<()> {
    let mut m = Monitor::new(MonitorConfig::default())?;
    let t = m.tick()?;
    let target = t.snapshot.processes.into_iter().find(|p| p.pid == pid).ok_or(Error::ProcessGone { pid })?;
    let req = KillRequest {
        signal_term_grace: if hard { Duration::ZERO } else { Duration::from_secs(5) },
        confirm,
        requested_by: Requester::Cli,
        reason: if reason.is_empty() { "из командной строки".into() } else { reason },
        dry_run,
    };
    let outcome = rmon_core::kill::kill(&target, req)?;
    if json {
        return print_json(&outcome);
    }
    println!("{}", outcome.message);
    if !outcome.exited && !outcome.dry_run {
        bail!("процесс {pid} не завершился");
    }
    Ok(())
}

fn show_config(json: bool) -> Result<()> {
    let path = config::config_file();
    let cfg = Config::load().with_context(|| format!("настройки {}", path.display()))?;
    let (_, mut errors) = Policy::from_config(&cfg);
    if let Err(e) = guard::Rules::compile(&cfg.guard) {
        errors.push(e);
    }
    if json {
        #[derive(Serialize)]
        struct Out<'a> {
            config_file: String,
            exists: bool,
            actions_log: String,
            guard_events: String,
            legacy_guard_log: String,
            config: &'a Config,
            rule_errors: &'a [String],
        }
        return print_json(&Out {
            config_file: path.display().to_string(),
            exists: path.exists(),
            actions_log: config::actions_log().display().to_string(),
            guard_events: guard::runner::events_log().display().to_string(),
            legacy_guard_log: config::legacy_guard_log().display().to_string(),
            config: &cfg,
            rule_errors: &errors,
        });
    }
    println!(
        "файл настроек  {}{}",
        path.display(),
        if path.exists() { "" } else { " (нет — действуют значения по умолчанию)" }
    );
    println!("журнал         {}", config::actions_log().display());
    println!("сторож         {}", guard::runner::events_log().display());
    println!("\n{}", toml::to_string_pretty(&cfg)?);
    if errors.is_empty() {
        println!("правила политики и сторожа: все разобраны");
        Ok(())
    } else {
        for e in &errors {
            eprintln!("правило пропущено: {e}");
        }
        bail!("{} правил(а) не разобрано", errors.len())
    }
}

fn guard_cmd(json: bool, action: GuardCmd, paint: Paint) -> Result<()> {
    use guard::runner::{persist_flags, read_events};
    match action {
        GuardCmd::On | GuardCmd::Arm => persist_flags(Some(true), Some(false), None).map_err(anyhow::Error::msg)?,
        GuardCmd::Off => persist_flags(Some(false), None, None).map_err(anyhow::Error::msg)?,
        GuardCmd::Dry => persist_flags(Some(true), Some(true), None).map_err(anyhow::Error::msg)?,
        GuardCmd::Retire => {
            let s = guard::legacy::retire().map_err(anyhow::Error::msg)?;
            println!("старый memory-guard: {s:?}; обязанность сторожа — у rmon");
            return Ok(());
        }
        GuardCmd::Restore => {
            let s = guard::legacy::restore().map_err(anyhow::Error::msg)?;
            println!("старый memory-guard: {s:?}");
            return Ok(());
        }
        GuardCmd::Now => return guard_now(json, paint),
        GuardCmd::Run => return guard_run(json),
        GuardCmd::Status => {}
    }
    let cfg = Config::load()?;
    let legacy = guard::legacy::status();
    let holder = std::fs::read_to_string(config::log_dir().join("guard.lock")).unwrap_or_default();
    let events = read_events(10);
    if json {
        return print_json(&serde_json::json!({
            "config": cfg.guard, "legacy": legacy, "lock_holder": holder.trim(), "recent": events,
        }));
    }
    let g = &cfg.guard;
    let mode = if !g.enabled {
        "выключен"
    } else if g.dry_run {
        "проба (только «убил бы»)"
    } else {
        "боевой"
    };
    println!("Сторож памяти: {}", paint.bold(mode));
    println!(
        "  красная зона: давление ≥ {}, или доступно ≤ {} %, или давление ≥ {} и своп ≥ {}; держится {} с",
        g.red.pressure_level,
        g.red.min_available_pct,
        g.red.swap_red_from_level,
        bytes(g.red.swap_red_mb * 1024 * 1024),
        g.sustain_secs
    );
    println!(
        "  жертва ≥ {}, пауза после снятия {} с, уведомления: {}",
        bytes(g.min_victim_mb * 1024 * 1024),
        g.after_kill_secs,
        if g.notify { "да" } else { "нет" }
    );
    println!("  старый memory-guard: {legacy:?}");
    if !holder.trim().is_empty() {
        println!("  держит сторожа: pid {}", holder.trim());
    }
    println!("\n{}", paint.bold("Последние события"));
    for e in events {
        println!("  {}  {}", e.ts, e.text());
    }
    Ok(())
}

fn guard_now(json: bool, paint: Paint) -> Result<()> {
    let cfg = Config::load()?;
    let rules = guard::Rules::compile(&cfg.guard).map_err(anyhow::Error::msg)?;
    let mut m = Monitor::new(MonitorConfig::default())?;
    let t = m.tick()?;
    let (zone, why) = guard::judge_zone(&t.snapshot.system, &cfg.guard.red);
    let exclude = guard::self_and_ancestors(&t.snapshot, std::process::id() as i32);
    let (candidates, rejected) = guard::rank_victims(&t.snapshot, &rules, &exclude);
    if json {
        return print_json(
            &serde_json::json!({ "zone": zone, "why": why, "candidates": candidates, "rejected": rejected }),
        );
    }
    println!("Зона: {} ({why})", paint.bold(&format!("{zone:?}")));
    println!("\nКандидаты (порог {}):", bytes(cfg.guard.min_victim_mb * 1024 * 1024));
    if candidates.is_empty() {
        println!("  нет");
    }
    for c in candidates.iter().take(8) {
        println!("  {:>7}  {:<7} {:>9}  {}", c.pid, c.kind, bytes(c.mem), c.cmd);
    }
    println!("\nКрупные, но не тронет:");
    for r in rejected.iter().take(12) {
        println!("  {:>7}  {:<24} {:>9}  {}", r.pid, r.name.chars().take(24).collect::<String>(), bytes(r.mem), r.why);
    }
    if let Some(v) = candidates.first() {
        let mut platform = rmon_core::platform::current()?;
        let a = guard::author::gather(&t.snapshot, v.pid, platform.as_mut());
        println!("\nВ красной зоне снял бы: {} {} — {}", v.kind, bytes(v.mem), guard::author::describe(&a));
    }
    Ok(())
}

fn guard_run(json: bool) -> Result<()> {
    let cfg = MonitorConfig { tick: Duration::from_secs(3), analyze_every: 20, ..MonitorConfig::default() };
    let handle = rmon_core::monitor::spawn_guarded(
        cfg,
        |_| {},
        move |ev| {
            if json {
                if let Ok(line) = serde_json::to_string(ev) {
                    println!("{line}");
                }
            } else {
                println!("{}", ev.text());
            }
        },
    )?;
    eprintln!("сторож без окна запущен; Ctrl-C — выход");
    loop {
        std::thread::sleep(Duration::from_secs(60));
        if let Some(g) = handle.guard()
            && let GuardMode::Broken(why) = g.status().mode
        {
            bail!("сторож остановлен: {why}");
        }
    }
}

/// «10s», «2m», «500ms», «1h», «15» (секунды).
fn parse_duration(s: &str) -> std::result::Result<Duration, String> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: f64 = num.parse().map_err(|_| format!("не число: «{s}»"))?;
    let secs = match unit.trim() {
        "" | "s" | "с" => n,
        "ms" | "мс" => n / 1000.0,
        "m" | "min" | "м" | "мин" => n * 60.0,
        "h" | "ч" => n * 3600.0,
        u => return Err(format!("неизвестная единица «{u}»: ms, s, m, h")),
    };
    if !secs.is_finite() || secs < 0.0 {
        return Err(format!("плохая длительность «{s}»"));
    }
    Ok(Duration::from_secs_f64(secs))
}

/// Цвета ANSI — только в терминал и без NO_COLOR.
#[derive(Clone, Copy)]
struct Paint {
    on: bool,
}

impl Paint {
    fn new(no_color: bool) -> Self {
        Self { on: !no_color && std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal() }
    }

    fn wrap(self, code: &str, s: &str) -> String {
        if self.on { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_string() }
    }

    fn bold(self, s: &str) -> String {
        self.wrap("1", s)
    }

    fn dim(self, s: &str) -> String {
        self.wrap("2", s)
    }

    fn red(self, s: &str) -> String {
        self.wrap("31", s)
    }

    fn pressure(self, p: Pressure, s: &str) -> String {
        let code = match p {
            Pressure::Normal => "1;32",
            Pressure::Warning => "1;33",
            Pressure::Critical => "1;31",
        };
        self.wrap(code, s)
    }

    fn policy(self, p: &KillPolicy) -> String {
        match p {
            KillPolicy::Protected(r) => self.dim(&format!("[защищён: {r}]")),
            KillPolicy::Confirm(_) => self.wrap("33", "[с подтверждением]"),
            KillPolicy::Free => self.wrap("32", "[можно снять]"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("10s").unwrap(), Duration::from_secs(10));
        assert_eq!(parse_duration("2m").unwrap(), Duration::from_secs(120));
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(parse_duration("15").unwrap(), Duration::from_secs(15));
        assert!(parse_duration("abc").is_err());
        assert!(parse_duration("5y").is_err());
    }
}

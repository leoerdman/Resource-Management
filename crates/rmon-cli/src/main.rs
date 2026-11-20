//! `rmon` — монитор памяти из командной строки.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use rmon_core::analyze::{Thresholds, analyze, app_groups};
use rmon_core::collect::Collector;
use rmon_core::fmt::{bytes, percent};
use rmon_core::history::History;
use rmon_core::kill::{KillRequest, Requester, kill, policy_for};

#[derive(Parser)]
#[command(name = "rmon", about = "Кто съел память и можно ли его снять")]
struct Cli {
    /// Вывод в JSON.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Сводка: давление, разбивка, топ приложений, подозреваемые.
    Status,
    /// Таблица процессов.
    Ps {
        #[arg(long, default_value = "mem")]
        sort: String,
        #[arg(long, default_value_t = 25)]
        limit: usize,
        #[arg(long)]
        filter: Option<String>,
    },
    /// Подозреваемые по истории на окне.
    Suggest {
        #[arg(long, default_value_t = 10)]
        window: u64,
        #[arg(long, default_value_t = 2)]
        interval: u64,
    },
    /// Снять процесс через защитный слой.
    Kill {
        pid: i32,
        #[arg(long)]
        confirm: bool,
        #[arg(long)]
        hard: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long, default_value = "")]
        reason: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut c = Collector::new()?;
    match cli.cmd.unwrap_or(Cmd::Status) {
        Cmd::Status => {
            let mut h = History::default();
            h.push(c.snapshot()?);
            std::thread::sleep(Duration::from_secs(1));
            let s = c.snapshot()?;
            h.push(s.clone());
            let groups = app_groups(&s);
            let suspects = analyze(&h, &Thresholds::default());
            if cli.json {
                println!("{}", serde_json::json!({"system": s.system, "groups": &groups[..groups.len().min(10)], "suspects": &suspects[..suspects.len().min(5)]}));
                return Ok(());
            }
            let m = &s.system;
            println!("Давление: {}  ({} доступно)", m.pressure.as_str(), percent(m.available_pct as f64));
            println!("Занято {} из {}  ·  своп {}  ·  сжато {}", bytes(m.used), bytes(m.total), bytes(m.swap_used), bytes(m.compressed));
            println!("App {} · Wired {} · Сжато {} · Кэш {} · Свободно {}", bytes(m.app), bytes(m.wired), bytes(m.compressed), bytes(m.cached_files), bytes(m.free));
            println!("\nПриложения:");
            for g in groups.iter().take(10) {
                println!("  {:>9}  {:>4}  {}", bytes(g.footprint), g.count, g.name);
            }
            println!("\nПодозреваемые:");
            for x in suspects.iter().take(5) {
                println!("  {:>3}  {:>6}  {}  [{}]", x.score, x.pid, x.reason, x.policy.label());
            }
        }
        Cmd::Ps { sort, limit, filter } => {
            c.snapshot()?;
            std::thread::sleep(Duration::from_millis(500));
            let mut ps = c.snapshot()?.processes;
            if let Some(f) = filter {
                let f = f.to_lowercase();
                ps.retain(|p| p.name.to_lowercase().contains(&f) || p.command_line().to_lowercase().contains(&f) || p.pid.to_string() == f);
            }
            match sort.as_str() {
                "cpu" => ps.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct)),
                "rss" => ps.sort_by_key(|p| std::cmp::Reverse(p.rss)),
                "pid" => ps.sort_by_key(|p| p.pid),
                _ => ps.sort_by_key(|p| std::cmp::Reverse(p.reclaim())),
            }
            ps.truncate(limit);
            if cli.json {
                println!("{}", serde_json::to_string(&ps)?);
                return Ok(());
            }
            println!("{:>7} {:>10} {:>10} {:>6} {:>4}  имя", "pid", "память", "rss", "cpu", "пот");
            for p in ps {
                println!("{:>7} {:>10} {:>10} {:>6.1} {:>4}  {}  [{}]", p.pid, p.footprint.map_or("—".into(), bytes), bytes(p.rss), p.cpu_pct, p.threads, p.name, policy_for(&p).label());
            }
        }
        Cmd::Suggest { window, interval } => {
            let mut h = History::default();
            let n = (window / interval.max(1)).max(2);
            for i in 0..n {
                if i > 0 {
                    std::thread::sleep(Duration::from_secs(interval));
                }
                h.push(c.snapshot()?);
            }
            let s = analyze(&h, &Thresholds::default());
            if cli.json {
                println!("{}", serde_json::to_string(&s)?);
            } else {
                for x in s {
                    println!("{:>3}  {:>6}  {:>9}  {}  [{}]", x.score, x.pid, bytes(x.reclaim), x.reason, x.policy.label());
                }
            }
        }
        Cmd::Kill { pid, confirm, hard, dry_run, reason } => {
            let p = c.process(pid)?.with_context(|| format!("процесса {pid} нет"))?;
            let req = KillRequest {
                signal_term_grace: if hard { Duration::ZERO } else { Duration::from_secs(5) },
                confirm,
                requested_by: Requester::Cli,
                reason,
                dry_run,
            };
            match kill(&p, req) {
                Ok(o) if cli.json => println!("{}", serde_json::to_string(&o)?),
                Ok(o) => println!("{}", o.message),
                Err(e) => bail!("{e}"),
            }
        }
    }
    Ok(())
}

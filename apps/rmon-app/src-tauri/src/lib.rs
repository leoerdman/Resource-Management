//! Resource Monitor: поток сбора → состояние → окно (React) и значок в строке меню.
//!
//! Всё платформенное — в `rmon-core`; здесь только склейка с Tauri, поэтому крейт
//! собирается под macOS, Windows и Linux одинаково.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rmon_core::analyze::{Suspect, Thresholds, analyze, app_groups, growth_rates};
use rmon_core::collect::Collector;
use rmon_core::fmt;
use rmon_core::history::History;
use rmon_core::journal::{self, JournalEntry};
use rmon_core::kill::{KillOutcome, KillPolicy, KillRequest, Requester, kill, policy_for};
use rmon_core::{AppGroup, Pressure, ProcState, ProcessInfo, SystemMemory};
use serde::Serialize;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};

/// Шаг сбора.
const TICK: Duration = Duration::from_secs(2);
/// Каждый какой снимок идёт в историю анализа (10 с).
const ANALYZE_EVERY: u32 = 5;
/// Окно графика.
const CHART_WINDOW: u64 = 15 * 60;
/// Окно анализа.
const ANALYZE_WINDOW: Duration = Duration::from_secs(10 * 60);
/// Аргументы процесса в окно — не длиннее (экономим память webview и IPC).
const ARGS_LIMIT: usize = 400;

#[derive(Debug, Clone, Serialize)]
struct HistoryPoint {
    t: u64,
    used: u64,
    swap_used: u64,
    compressed: u64,
    pressure: Pressure,
}

#[derive(Debug, Clone, Serialize)]
struct ProcessRow {
    pid: i32,
    ppid: i32,
    name: String,
    exe: Option<String>,
    args: Vec<String>,
    uid: u32,
    footprint: Option<u64>,
    rss: u64,
    cpu_pct: f32,
    threads: u32,
    state: ProcState,
    started_at: u64,
    app_group: String,
    growth_per_min: Option<f64>,
    policy: KillPolicy,
}

#[derive(Debug, Clone, Serialize)]
struct State {
    taken_at: u64,
    system: SystemMemory,
    history: Vec<HistoryPoint>,
    groups: Vec<AppGroup>,
    processes: Vec<ProcessRow>,
    suspects: Vec<Suspect>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<Option<State>>,
    /// Полные сведения последнего снимка — цель для снятия.
    procs: Mutex<Vec<ProcessInfo>>,
}

type SharedState = Arc<Shared>;

fn short_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut len = 0;
    for a in args {
        if len >= ARGS_LIMIT {
            out.push("…".to_string());
            break;
        }
        let take: String = a.chars().take(ARGS_LIMIT - len).collect();
        len += take.len().max(1);
        out.push(take);
    }
    out
}

fn collector_loop(app: AppHandle, shared: SharedState) {
    let mut collector = match Collector::new() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("rmon: {e}");
            return;
        }
    };
    let mut history = History::slim(ANALYZE_WINDOW);
    let mut chart: VecDeque<HistoryPoint> = VecDeque::new();
    let mut suspects: Vec<Suspect> = Vec::new();
    let mut growth = std::collections::HashMap::new();
    let thresholds = Thresholds::default();
    let mut tick: u32 = 0;

    loop {
        let started = Instant::now();
        match collector.snapshot() {
            Ok(snap) => {
                chart.push_back(HistoryPoint {
                    t: snap.taken_at,
                    used: snap.system.used,
                    swap_used: snap.system.swap_used,
                    compressed: snap.system.compressed,
                    pressure: snap.system.pressure,
                });
                while chart.front().is_some_and(|p| p.t + CHART_WINDOW < snap.taken_at) {
                    chart.pop_front();
                }
                // Первый снимок без CPU — в анализ берём со второго.
                if tick % ANALYZE_EVERY == 1 {
                    history.push(snap.clone());
                    suspects = analyze(&history, &thresholds);
                    growth = growth_rates(&history);
                }
                let rows = snap
                    .processes
                    .iter()
                    .map(|p| ProcessRow {
                        pid: p.pid,
                        ppid: p.ppid,
                        name: p.name.clone(),
                        exe: p.exe.clone(),
                        args: short_args(&p.args),
                        uid: p.uid,
                        footprint: p.footprint,
                        rss: p.rss,
                        cpu_pct: p.cpu_pct,
                        threads: p.threads,
                        state: p.state,
                        started_at: p.started_at,
                        app_group: p.app_group.clone(),
                        growth_per_min: growth.get(&(p.pid, p.started_at)).copied(),
                        policy: policy_for(p),
                    })
                    .collect();
                let state = State {
                    taken_at: snap.taken_at,
                    system: snap.system,
                    history: chart.iter().cloned().collect(),
                    groups: app_groups(&snap),
                    processes: rows,
                    suspects: suspects.clone(),
                };
                update_tray(&app, &state);
                let visible = app.get_webview_window("main").and_then(|w| w.is_visible().ok()).unwrap_or(false);
                if visible {
                    let _ = app.emit("rmon://state", &state);
                }
                *shared.state.lock().unwrap() = Some(state);
                *shared.procs.lock().unwrap() = snap.processes;
            }
            Err(e) => eprintln!("rmon: снимок не удался: {e}"),
        }
        tick = tick.wrapping_add(1);
        std::thread::sleep(TICK.saturating_sub(started.elapsed()));
    }
}

/// Кружок цвета давления, 36×36 RGBA со сглаженным краем.
fn pressure_icon(p: Pressure) -> Image<'static> {
    let (r, g, b) = match p {
        Pressure::Normal => (52u8, 199u8, 89u8),
        Pressure::Warning => (255, 204, 0),
        Pressure::Critical => (255, 59, 48),
    };
    let n = 36u32;
    let c = (n as f32 - 1.0) / 2.0;
    let rad = n as f32 * 0.36;
    let mut px = Vec::with_capacity((n * n * 4) as usize);
    for y in 0..n {
        for x in 0..n {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            let a = (rad + 0.5 - d).clamp(0.0, 1.0);
            px.extend_from_slice(&[r, g, b, (a * 255.0) as u8]);
        }
    }
    Image::new_owned(px, n, n)
}

fn update_tray(app: &AppHandle, s: &State) {
    let Some(tray) = app.tray_by_id("main") else { return };
    let _ = tray.set_icon(Some(pressure_icon(s.system.pressure)));
    let title = fmt::bytes(s.system.used);
    #[cfg(target_os = "macos")]
    let _ = tray.set_title(Some(title.as_str()));
    let _ = tray.set_tooltip(Some(format!(
        "Давление {} · занято {} · своп {}",
        s.system.pressure.as_str(),
        title,
        fmt::bytes(s.system.swap_used)
    )));
    if let Ok(menu) = build_menu(app, &s.suspects) {
        let _ = tray.set_menu(Some(menu));
    }
}

fn build_menu(app: &AppHandle, suspects: &[Suspect]) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;
    menu.append(&MenuItem::with_id(app, "open", "Открыть", true, None::<&str>)?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    if suspects.is_empty() {
        menu.append(&MenuItem::with_id(app, "none", "Подозреваемых нет", false, None::<&str>)?)?;
    } else {
        menu.append(&MenuItem::with_id(app, "hdr", "Подозреваемые:", false, None::<&str>)?)?;
        for (i, s) in suspects.iter().take(5).enumerate() {
            let text: String = s.reason.chars().take(80).collect();
            menu.append(&MenuItem::with_id(app, format!("s{i}"), text, false, None::<&str>)?)?;
        }
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?)?;
    Ok(menu)
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        // Окно было скрыто и событий не получало — сразу отдаём свежее состояние.
        if let Some(s) = app.state::<SharedState>().state.lock().unwrap().clone() {
            let _ = app.emit("rmon://state", &s);
        }
    }
}

#[tauri::command]
fn get_state(shared: tauri::State<'_, SharedState>) -> Option<State> {
    shared.state.lock().unwrap().clone()
}

#[tauri::command]
async fn kill_process(
    app: AppHandle,
    shared: tauri::State<'_, SharedState>,
    pid: i32,
    started_at: u64,
    confirm: bool,
    reason: String,
) -> Result<KillOutcome, String> {
    let target = shared
        .procs
        .lock()
        .unwrap()
        .iter()
        .find(|p| p.pid == pid && p.started_at == started_at)
        .cloned()
        .ok_or_else(|| format!("процесса {pid} уже нет в снимке"))?;
    let req = KillRequest {
        signal_term_grace: Duration::from_secs(5),
        confirm,
        requested_by: Requester::App,
        reason: if reason.is_empty() { "из окна".into() } else { reason },
        dry_run: false,
    };
    let outcome = tauri::async_runtime::spawn_blocking(move || kill(&target, req))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    if outcome.exited {
        // Убираем снятый процесс из кэша сразу, не дожидаясь следующего снимка.
        let mut guard = shared.state.lock().unwrap();
        if let Some(s) = guard.as_mut() {
            s.processes.retain(|p| !(p.pid == pid && p.started_at == started_at));
            s.suspects.retain(|x| !(x.pid == pid && x.started_at == started_at));
            for g in &mut s.groups {
                if let Some(i) = g.pids.iter().position(|&x| x == pid) {
                    g.pids.remove(i);
                    g.count -= 1;
                    g.footprint = g.footprint.saturating_sub(outcome.reclaimed);
                }
            }
            let _ = app.emit("rmon://state", &*s);
        }
    }
    Ok(outcome)
}

#[tauri::command]
async fn read_journal() -> Vec<JournalEntry> {
    tauri::async_runtime::spawn_blocking(|| journal::read_recent(300)).await.unwrap_or_default()
}

/// Точка входа приложения.
pub fn run() {
    let shared: SharedState = Arc::new(Shared::default());
    let app = tauri::Builder::default()
        .manage(shared.clone())
        .invoke_handler(tauri::generate_handler![get_state, kill_process, read_journal])
        .setup(move |app| {
            let handle = app.handle().clone();
            TrayIconBuilder::with_id("main")
                .icon(pressure_icon(Pressure::Normal))
                .menu(&build_menu(&handle, &[])?)
                .on_menu_event(|app, ev| match ev.id().as_ref() {
                    "open" => show_main(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("rmon-collector".into())
                .spawn(move || collector_loop(handle, shared))?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Закрытие окна — прячем: значок в строке меню продолжает работать.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .build(tauri::generate_context!())
        .expect("не удалось запустить Resource Monitor");

    app.run(|app, event| {
        #[cfg(target_os = "macos")]
        if let RunEvent::Reopen { .. } = event {
            show_main(app);
        }
        let _ = (&app, &event);
    });
}

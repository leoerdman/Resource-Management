//! Resource Monitor: склейка [`rmon_core::monitor`] с Tauri — окно (React) и значок в строке меню.
//!
//! Сбор, анализ, политика и снятие — в `rmon-core`; здесь только доставка:
//! - сводка уходит в окно через [`Channel`] (большие сообщения Tauri передаёт через свой
//!   протокол, а не `eval` строки в WebView), и только пока окно видно;
//! - значок обновляется, лишь когда изменилось то, что на нём видно: пересоздавать меню
//!   и картинку каждые 2 с — верный способ накопить память в AppKit.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmon_core::analyze::Suspect;
use rmon_core::fmt;
use rmon_core::journal::{self, JournalEntry};
use rmon_core::kill::{KillOutcome, KillRequest, Requester};
use rmon_core::monitor::{self, Dashboard, MonitorConfig, MonitorHandle, ProcessDetails};
use rmon_core::Pressure;
use tauri::image::Image;
use tauri::ipc::Channel;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, RunEvent, WindowEvent};

/// Сколько подозреваемых в меню значка.
const TRAY_SUSPECTS: usize = 5;
/// Длина строки подозреваемого в меню.
const TRAY_LINE: usize = 80;

/// То, что видно на значке. Сравниваем с прошлым, чтобы не трогать AppKit зря.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct TrayView {
    pressure: Option<Pressure>,
    title: String,
    tooltip: String,
    lines: Vec<String>,
}

impl TrayView {
    fn of(d: &Dashboard) -> Self {
        let title = fmt::bytes(d.system.used);
        Self {
            pressure: Some(d.system.pressure),
            tooltip: format!("Давление {} · занято {} · своп {}", d.system.pressure.as_str(), title, fmt::bytes(d.system.swap_used)),
            title,
            lines: d.suspects.iter().take(TRAY_SUSPECTS).map(tray_line).collect(),
        }
    }
}

fn tray_line(s: &Suspect) -> String {
    let line = format!("{} — {}", fmt::bytes(s.reclaim), s.reason);
    if line.chars().count() <= TRAY_LINE {
        line
    } else {
        line.chars().take(TRAY_LINE - 1).chain(std::iter::once('…')).collect()
    }
}

/// Состояние приложения.
#[derive(Default)]
struct AppState {
    monitor: Mutex<Option<MonitorHandle>>,
    /// Подписка окна на сводку.
    channel: Mutex<Option<Channel<Arc<Dashboard>>>>,
    tray: Mutex<TrayView>,
}

impl AppState {
    fn monitor(&self) -> Result<MonitorHandle, String> {
        self.monitor.lock().unwrap_or_else(|e| e.into_inner()).clone().ok_or_else(|| "монитор ещё не запущен".to_string())
    }
}

fn deliver(app: &AppHandle, d: &Arc<Dashboard>) {
    let state = app.state::<AppState>();
    update_tray(app, &state, d);
    let visible = app.get_webview_window("main").and_then(|w| w.is_visible().ok()).unwrap_or(false);
    if !visible {
        return;
    }
    let channel = state.channel.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(ch) = channel
        && ch.send(d.clone()).is_err()
    {
        // Окно перезагрузилось — подписка умерла; новая придёт с `subscribe`.
        *state.channel.lock().unwrap_or_else(|e| e.into_inner()) = None;
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

fn update_tray(app: &AppHandle, state: &AppState, d: &Dashboard) {
    let next = TrayView::of(d);
    let mut cur = state.tray.lock().unwrap_or_else(|e| e.into_inner());
    if *cur == next {
        return;
    }
    let Some(tray) = app.tray_by_id("main") else { return };
    if cur.pressure != next.pressure
        && let Some(p) = next.pressure
    {
        let _ = tray.set_icon(Some(pressure_icon(p)));
    }
    if cur.title != next.title {
        #[cfg(target_os = "macos")]
        let _ = tray.set_title(Some(next.title.as_str()));
    }
    if cur.tooltip != next.tooltip {
        let _ = tray.set_tooltip(Some(next.tooltip.as_str()));
    }
    if cur.lines != next.lines
        && let Ok(menu) = build_menu(app, &next.lines)
    {
        let _ = tray.set_menu(Some(menu));
    }
    *cur = next;
}

fn build_menu(app: &AppHandle, lines: &[String]) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;
    menu.append(&MenuItem::with_id(app, "open", "Открыть Resource Monitor", true, None::<&str>)?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    if lines.is_empty() {
        menu.append(&MenuItem::with_id(app, "none", "Подозреваемых нет", false, None::<&str>)?)?;
    } else {
        menu.append(&MenuItem::with_id(app, "hdr", "Подозреваемые:", false, None::<&str>)?)?;
        for (i, line) in lines.iter().enumerate() {
            // Пункт открывает окно: снимать из меню нельзя — только через окно с подтверждением.
            menu.append(&MenuItem::with_id(app, format!("s{i}"), line, true, None::<&str>)?)?;
        }
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "quit", "Выйти", true, Some("CmdOrCtrl+Q"))?)?;
    Ok(menu)
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        // Пока окно было скрыто, сводки не уходили — отдаём свежую сразу.
        if let Ok(m) = app.state::<AppState>().monitor()
            && let Some(d) = m.dashboard()
        {
            deliver(app, &d);
        }
    }
}

/// Подписка окна на сводку; сразу отдаёт последнюю.
#[tauri::command]
fn subscribe(state: tauri::State<'_, AppState>, channel: Channel<Arc<Dashboard>>) -> Result<(), String> {
    if let Some(d) = state.monitor()?.dashboard() {
        channel.send(d).map_err(|e| e.to_string())?;
    }
    *state.channel.lock().unwrap_or_else(|e| e.into_inner()) = Some(channel);
    Ok(())
}

/// Подробности процесса.
#[tauri::command]
async fn process_details(state: tauri::State<'_, AppState>, pid: i32, started_at: u64) -> Result<ProcessDetails, String> {
    let m = state.monitor()?;
    tauri::async_runtime::spawn_blocking(move || m.details(pid, Some(started_at)))
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("процесса {pid} уже нет"))
}

/// Снять процесс через защитный слой ядра.
#[tauri::command]
async fn kill_process(
    state: tauri::State<'_, AppState>,
    pid: i32,
    started_at: u64,
    confirm: bool,
    reason: String,
) -> Result<KillOutcome, String> {
    let m = state.monitor()?;
    let req = KillRequest {
        signal_term_grace: Duration::from_secs(5),
        confirm,
        requested_by: Requester::App,
        reason: if reason.trim().is_empty() { "из окна".into() } else { reason },
        dry_run: false,
    };
    tauri::async_runtime::spawn_blocking(move || m.kill(pid, Some(started_at), req))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Журналы rmon и memory-guard, новые сверху.
#[tauri::command]
async fn read_journal() -> Vec<JournalEntry> {
    tauri::async_runtime::spawn_blocking(|| journal::read_recent(300)).await.unwrap_or_default()
}

/// Точка входа приложения.
pub fn run() {
    let app = tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![subscribe, process_details, kill_process, read_journal])
        .setup(|app| {
            let handle = app.handle().clone();
            TrayIconBuilder::with_id("main")
                .icon(pressure_icon(Pressure::Normal))
                .tooltip("Resource Monitor")
                .menu(&build_menu(&handle, &[])?)
                .on_menu_event(|app, ev| match ev.id().as_ref() {
                    "quit" => app.exit(0),
                    _ => show_main(app),
                })
                .build(app)?;
            let notify = handle.clone();
            let monitor = monitor::spawn(MonitorConfig::default(), move |d| deliver(&notify, d))?;
            *app.state::<AppState>().monitor.lock().unwrap_or_else(|e| e.into_inner()) = Some(monitor);
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
        #[cfg(not(target_os = "macos"))]
        let _ = (app, event);
    });
}

//! Resource Monitor: склейка [`rmon_core`] с Tauri — окно (React), значок в строке меню,
//! сторож памяти, уведомления, автозапуск при входе.
//!
//! Сбор, анализ, политика, снятие, сторож и настройки — в `rmon-core`; здесь только доставка:
//! - сводка уходит в окно через [`Channel`] и только пока окно видно;
//! - значок обновляется, лишь когда изменилось то, что на нём видно (пересоздание меню и
//!   картинки каждые 2 с копит память в AppKit);
//! - события сторожа с `notice` становятся системными уведомлениями;
//! - автозапуск следует `[app] autostart` из `config.toml` — файл остаётся источником истины.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmon_core::Pressure;
use rmon_core::analyze::Suspect;
use rmon_core::config::Config;
use rmon_core::fmt;
use rmon_core::guard::GuardMode;
use rmon_core::guard::runner::{GuardEvent, GuardStatus};
use rmon_core::journal::{self, JournalEntry};
use rmon_core::kill::{KillOutcome, KillRequest, Requester};
use rmon_core::monitor::{self, Dashboard, MonitorConfig, MonitorHandle, ProcessDetails};
use rmon_core::settings::{self, Settings};
use tauri::image::Image;
use tauri::ipc::Channel;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_notification::NotificationExt;

/// Сколько подозреваемых в меню значка.
const TRAY_SUSPECTS: usize = 5;
/// Длина строки подозреваемого в меню.
const TRAY_LINE: usize = 80;
/// Аргумент, с которым приложение запускается при входе: окно не показывать.
const AUTOSTART_ARG: &str = "--autostart";

/// То, что видно на значке. Сравниваем с прошлым, чтобы не трогать AppKit зря.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct TrayView {
    pressure: Option<Pressure>,
    title: Option<String>,
    tooltip: String,
    lines: Vec<String>,
    guard_on: bool,
    guard_dry: bool,
    guard_note: Option<String>,
}

fn tray_line(s: &Suspect) -> String {
    let line = format!("{} — {}", fmt::bytes(s.reclaim), s.group_label.as_deref().unwrap_or(&s.reason));
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

fn window_visible(app: &AppHandle) -> bool {
    app.get_webview_window("main").and_then(|w| w.is_visible().ok()).unwrap_or(false)
}

fn deliver(app: &AppHandle, d: &Arc<Dashboard>) {
    let state = app.state::<AppState>();
    update_tray(app, &state, Some(d));
    if !window_visible(app) {
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

fn on_guard_event(app: &AppHandle, ev: &GuardEvent) {
    if let Some(n) = &ev.notice {
        let _ = app.notification().builder().title(format!("{} · {}", n.title, n.subtitle)).body(&n.body).show();
    }
    let state = app.state::<AppState>();
    update_tray(app, &state, None);
    if window_visible(app) {
        let _ = app.emit("rmon://guard", ev);
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

/// Пересчитать видимое на значке. `d = None` — изменилось только состояние сторожа.
fn update_tray(app: &AppHandle, state: &AppState, d: Option<&Dashboard>) {
    let mut cur = state.tray.lock().unwrap_or_else(|e| e.into_inner());
    let mut next = cur.clone();
    let cfg = rmon_core::config::global();
    if let Some(d) = d {
        let used = fmt::bytes(d.system.used);
        next.pressure = Some(d.system.pressure);
        next.title = cfg.app.tray_title.then(|| used.clone());
        next.tooltip = format!("Давление {} · занято {} · своп {}", d.system.pressure.as_str(), used, fmt::bytes(d.system.swap_used));
        next.lines = d.suspects.iter().take(TRAY_SUSPECTS).map(tray_line).collect();
    }
    if let Ok(m) = state.monitor()
        && let Some(g) = m.guard()
    {
        let st = g.status();
        next.guard_on = st.config.enabled;
        next.guard_dry = st.config.dry_run;
        next.guard_note = match st.mode {
            GuardMode::Standby(why) | GuardMode::Broken(why) => Some(why),
            _ => None,
        };
    }
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
        let _ = tray.set_title(next.title.as_deref().or(Some("")));
    }
    if cur.tooltip != next.tooltip {
        let _ = tray.set_tooltip(Some(next.tooltip.as_str()));
    }
    let menu_changed = cur.lines != next.lines || cur.guard_on != next.guard_on || cur.guard_dry != next.guard_dry || cur.guard_note != next.guard_note;
    if menu_changed && let Ok(menu) = build_menu(app, &next) {
        let _ = tray.set_menu(Some(menu));
    }
    *cur = next;
}

fn build_menu(app: &AppHandle, v: &TrayView) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;
    menu.append(&MenuItem::with_id(app, "open", "Открыть Resource Monitor", true, None::<&str>)?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&CheckMenuItem::with_id(app, "guard", "Сторож памяти", true, v.guard_on, None::<&str>)?)?;
    menu.append(&CheckMenuItem::with_id(app, "guard_dry", "Режим пробы (только «убил бы»)", v.guard_on, v.guard_dry, None::<&str>)?)?;
    if let Some(note) = &v.guard_note {
        let note: String = note.chars().take(TRAY_LINE).collect();
        menu.append(&MenuItem::with_id(app, "guard_note", format!("⚠︎ {note}"), true, None::<&str>)?)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    if v.lines.is_empty() {
        menu.append(&MenuItem::with_id(app, "none", "Подозреваемых нет", false, None::<&str>)?)?;
    } else {
        menu.append(&MenuItem::with_id(app, "hdr", "Подозреваемые:", false, None::<&str>)?)?;
        for (i, line) in v.lines.iter().enumerate() {
            // Пункт открывает окно: снимать из меню нельзя — только через окно с подтверждением.
            menu.append(&MenuItem::with_id(app, format!("s{i}"), line, true, None::<&str>)?)?;
        }
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "settings", "Настройки…", true, Some("CmdOrCtrl+,"))?)?;
    menu.append(&MenuItem::with_id(app, "quit", "Выйти", true, Some("CmdOrCtrl+Q"))?)?;
    Ok(menu)
}

fn show_main(app: &AppHandle, tab: Option<&str>) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        if let Some(tab) = tab {
            let _ = app.emit("rmon://open-tab", tab);
        }
        // Пока окно было скрыто, сводки не уходили — отдаём свежую сразу.
        if let Ok(m) = app.state::<AppState>().monitor()
            && let Some(d) = m.dashboard()
        {
            deliver(app, &d);
        }
    }
}

fn toggle_guard(app: &AppHandle, enabled: Option<bool>, dry_run: Option<bool>) {
    let Ok(m) = app.state::<AppState>().monitor() else { return };
    if let Some(g) = m.guard()
        && let Err(e) = g.set(enabled, dry_run, None)
    {
        let _ = app.notification().builder().title("Resource Monitor").body(format!("Сторож не переключён: {e}")).show();
    }
    let state = app.state::<AppState>();
    update_tray(app, &state, None);
}

/// Применить `[app] autostart` к системе.
fn sync_autostart(app: &AppHandle, want: bool) -> Result<(), String> {
    let al = app.autolaunch();
    let have = al.is_enabled().unwrap_or(false);
    match (want, have) {
        (true, false) => al.enable().map_err(|e| format!("автозапуск не включился: {e}")),
        (false, true) => al.disable().map_err(|e| format!("автозапуск не выключился: {e}")),
        _ => Ok(()),
    }
}

// ------------------------------------------------------------------ команды окна

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

/// Журналы rmon, сторожа и старого memory-guard, новые сверху.
#[tauri::command]
async fn read_journal() -> Vec<JournalEntry> {
    tauri::async_runtime::spawn_blocking(|| journal::read_recent(300)).await.unwrap_or_default()
}

/// Состояние сторожа.
#[tauri::command]
fn guard_status(state: tauri::State<'_, AppState>) -> Result<GuardStatus, String> {
    state.monitor()?.guard().map(|g| g.status()).ok_or_else(|| "сторож не запущен".into())
}

/// Перенести обязанность старого LaunchAgent memory-guard в rmon (обратимо).
#[tauri::command]
async fn guard_retire_legacy(app: AppHandle, state: tauri::State<'_, AppState>) -> Result<GuardStatus, String> {
    let g = state.monitor()?.guard().cloned().ok_or("сторож не запущен")?;
    let s = tauri::async_runtime::spawn_blocking(move || g.retire_legacy()).await.map_err(|e| e.to_string())??;
    update_tray(&app, &app.state::<AppState>(), None);
    Ok(s)
}

/// Вернуть старый memory-guard.
#[tauri::command]
async fn guard_restore_legacy(app: AppHandle, state: tauri::State<'_, AppState>) -> Result<GuardStatus, String> {
    let g = state.monitor()?.guard().cloned().ok_or("сторож не запущен")?;
    let s = tauri::async_runtime::spawn_blocking(move || g.restore_legacy()).await.map_err(|e| e.to_string())??;
    update_tray(&app, &app.state::<AppState>(), None);
    Ok(s)
}

/// Настройки из `config.toml`; `autostart` — как в системе сейчас.
#[tauri::command]
fn get_settings(app: AppHandle) -> Result<Settings, String> {
    let mut s = Settings::current().map_err(|e| e.to_string())?;
    if let Ok(on) = app.autolaunch().is_enabled() {
        s.app.autostart = on;
    }
    Ok(s)
}

/// Сохранить настройки и применить их сразу: сторож, пороги, значок, автозапуск.
#[tauri::command]
fn save_settings(app: AppHandle, state: tauri::State<'_, AppState>, settings: Settings) -> Result<Settings, String> {
    let cfg: Config = settings::save(&settings)?;
    rmon_core::config::set_global(cfg.clone());
    let m = state.monitor()?;
    m.set_thresholds(cfg.thresholds.clone());
    if let Some(g) = m.guard() {
        g.reload();
    }
    sync_autostart(&app, cfg.app.autostart)?;
    if let Some(d) = m.dashboard() {
        // Сбросить кэш значка, чтобы заголовок появился/исчез сразу.
        state.tray.lock().unwrap_or_else(|e| e.into_inner()).title = Some(String::from("\u{0}"));
        update_tray(&app, &state, Some(&d));
    }
    get_settings(app)
}

/// Открыть `config.toml` в редакторе по умолчанию (списки правил редактируются там).
#[tauri::command]
fn open_config() -> Result<(), String> {
    let path = rmon_core::config::config_file();
    if !path.exists() {
        settings::write_validated(&path, "# Настройки Resource Monitor (rmon). Справка: docs/SPEC.md\n")?;
    }
    open_path(&path)
}

fn open_path(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("/usr/bin/open");
    #[cfg(target_os = "macos")]
    cmd.arg("-t");
    #[cfg(windows)]
    let mut cmd = std::process::Command::new("notepad");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(path).spawn().map(|_| ()).map_err(|e| format!("{}: {e}", path.display()))
}

/// Точка входа приложения.
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec![AUTOSTART_ARG])))
        .plugin(tauri_plugin_notification::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            subscribe,
            process_details,
            kill_process,
            read_journal,
            guard_status,
            guard_retire_legacy,
            guard_restore_legacy,
            get_settings,
            save_settings,
            open_config,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            TrayIconBuilder::with_id("main")
                .icon(pressure_icon(Pressure::Normal))
                .tooltip("Resource Monitor")
                .menu(&build_menu(&handle, &TrayView::default())?)
                .on_menu_event(|app, ev| match ev.id().as_ref() {
                    "quit" => app.exit(0),
                    "settings" => show_main(app, Some("settings")),
                    "guard_note" => show_main(app, Some("guard")),
                    "guard" => {
                        let on = app.state::<AppState>().tray.lock().map(|t| t.guard_on).unwrap_or(false);
                        toggle_guard(app, Some(!on), None);
                    }
                    "guard_dry" => {
                        let dry = app.state::<AppState>().tray.lock().map(|t| t.guard_dry).unwrap_or(false);
                        toggle_guard(app, None, Some(!dry));
                    }
                    id if id.starts_with('s') => show_main(app, Some("suspects")),
                    _ => show_main(app, None),
                })
                .build(app)?;

            // Автозапуск при входе — как сказано в config.toml (по умолчанию включён).
            let want = rmon_core::config::global().app.autostart;
            if let Err(e) = sync_autostart(&handle, want) {
                eprintln!("rmon: {e}");
            }
            // Запуск при входе — тихий: только значок, окно по клику.
            if std::env::args().any(|a| a == AUTOSTART_ARG)
                && let Some(w) = app.get_webview_window("main")
            {
                let _ = w.hide();
            }

            let notify = handle.clone();
            let guard_app = handle.clone();
            let monitor = monitor::spawn_guarded(
                MonitorConfig::default(),
                move |d| deliver(&notify, d),
                move |ev| on_guard_event(&guard_app, ev),
            )?;
            *app.state::<AppState>().monitor.lock().unwrap_or_else(|e| e.into_inner()) = Some(monitor);
            Ok(())
        })
        .on_window_event(|window, event| {
            // Закрытие окна — прячем: значок в строке меню и сторож продолжают работать.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .build(tauri::generate_context!())
        .expect("не удалось запустить Resource Monitor");

    app.run(|app, event| {
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = event {
            show_main(app, None);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (app, event);
    });
}

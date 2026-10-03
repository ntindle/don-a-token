#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod auth;
mod github;
mod publish;
mod runner;
mod scheduler;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

/// Stable `ext_agent_host_id` for this machine, generated once and persisted
/// under the OS app-data dir. Must exist before the first SIWC sign-in.
#[tauri::command]
fn get_host_id(app: AppHandle) -> Result<String, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    don_a_token_core::host::load_or_generate(&dir.join("host_id")).map_err(|e| e.to_string())
}

fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Open Don-a-Token", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", "Pause donations", true, None::<&str>)?;
    let skip = MenuItem::with_id(app, "skip-week", "Skip this week", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &pause, &skip, &quit])?;

    TrayIconBuilder::new()
        .menu(&menu)
        .tooltip("Don-a-Token")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "pause" => {
                // The frontend owns settings; it listens for this event.
                let _ = app.emit("tray:pause", ());
                show_main(app);
            }
            "skip-week" => {
                let _ = app.emit("tray:skip-week", ());
                show_main(app);
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .manage(auth::PendingState::default())
        .manage(runner::JobState::default())
        .manage(scheduler::SchedulerState::default())
        .invoke_handler(tauri::generate_handler![
            get_host_id,
            auth::start_sign_in,
            auth::refresh_account,
            auth::list_accounts,
            auth::sign_out,
            github::github_device_start,
            github::github_device_poll,
            runner::job_submit,
            runner::job_status,
            runner::job_cancel,
            scheduler::scheduler_sync,
            scheduler::scheduler_status
        ])
        .setup(|app| {
            build_tray(app.handle())?;
            auth::spawn_refresh_loop(app.handle().clone());
            scheduler::restore(app.handle());
            scheduler::spawn_loop(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            // Donation jobs run in the background: closing the window
            // hides to the tray instead of quitting (tray menu has Quit).
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running don-a-token");
}

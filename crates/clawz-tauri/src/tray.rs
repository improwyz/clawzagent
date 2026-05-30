//! System tray integration for ClawZ desktop app.

use tauri::tray::TrayIconEvent;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder},
    AppHandle, Manager,
};

use crate::shell;

const TRAY_ID: &str = "clawz-tray";

/// Refresh tray tooltip from current shell config (mode + connection status).
pub fn refresh_tray_tooltip(app: &AppHandle) {
    let cfg = shell::load_config();
    let mode = match cfg.mode {
        shell::ShellMode::Standalone => "standalone",
        shell::ShellMode::Gateway => "gateway",
    };
    let tip = format!(
        "ClawZ ({mode}) — {}",
        cfg.connection_status
    );
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(tip));
    }
}

/// Build the system tray for ClawZ.
/// Returns a TrayIcon that shows in the system tray when the app is running.
pub fn build_tray(app: &AppHandle) -> Result<TrayIcon, tauri::Error> {
    // Exit menu item
    let quit = MenuItem::with_id(app, "quit", "Quit ClawZ", true, None::<&str>)?;
    let show = MenuItem::with_id(app, "show", "Show Window", true, None::<&str>)?;
    let cron = MenuItem::with_id(app, "cron", "Cron jobs", true, None::<&str>)?;

    let menu = Menu::with_items(app, &[&show, &cron, &quit])?;

    let cfg = shell::load_config();
    let initial_tip = format!(
        "ClawZ ({}) — {}",
        match cfg.mode {
            shell::ShellMode::Standalone => "standalone",
            shell::ShellMode::Gateway => "gateway",
        },
        cfg.connection_status
    );

    TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip(initial_tip)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "quit" => {
                app.exit(0);
            }
            "show" | "cron" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                    if event.id.as_ref() == "cron" {
                        let _ = window.eval("window.location.assign('/cron');");
                    }
                }
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button,
                button_state,
                ..
            } = event
            {
                if button == MouseButton::Left && button_state == MouseButtonState::Up {
                    let app = tray.app_handle();
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
            }
        })
        .build(app)
}

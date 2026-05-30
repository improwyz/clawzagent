//! ClawZ Tauri application entry point.

use std::sync::Arc;

use clawz_tauri::{commands, tray};
use tauri::Manager;
use tracing_subscriber::EnvFilter;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let _ = tray::build_tray(app.handle());

            // Build the app state synchronously using a dedicated tokio runtime.
            // This keeps the setup callback sync (required by Tauri v2) while still
            // allowing async initialization of the agent runtime.
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime for setup must succeed");
            let state = Arc::new(rt.block_on(async { commands::AppState::new().await }));
            commands::set_app_state(state.clone());
            app.manage(state);
            tray::refresh_tray_tooltip(app.handle());

            tracing::info!("ClawZ Tauri shell started");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::agent_chat,
            commands::get_platform_tier,
            commands::health_check,
            commands::get_shell_config,
            commands::set_shell_config,
            commands::load_gateway_api_key,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn main() {
    run();
}

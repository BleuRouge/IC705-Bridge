//! IC705 Bridge — passerelle CI-V pour l'Icom IC-705.
//!
//! Backend Rust : cœur réseau RS-BA1 (modules [`rsba1`]), orchestrateur de
//! [`session`], commandes Tauri ([`commands`]) et API HTTP locale ([`api`]).

mod api;
mod commands;
mod error;
mod rsba1;
mod session;
mod state;
mod util;

use std::sync::Arc;

use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("ic705_bridge_lib=info,warn")),
        )
        .init();

    let app_state = Arc::new(AppState::new());

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .manage(app_state.clone())
        .setup(move |app| {
            // Mises à jour automatiques (desktop uniquement).
            #[cfg(desktop)]
            app.handle()
                .plugin(tauri_plugin_updater::Builder::new().build())?;

            // Démarrage de l'API HTTP locale (toujours active).
            let st = app_state.clone();
            tauri::async_runtime::spawn(async move {
                api::serve(st).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::connect,
            commands::disconnect,
            commands::send_civ,
            commands::get_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

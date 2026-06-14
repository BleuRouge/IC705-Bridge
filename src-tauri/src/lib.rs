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
use std::time::Duration;

use tauri::Manager;

use state::AppState;

/// Délai max accordé à la déconnexion propre lors de la fermeture de l'app.
const SHUTDOWN_DISCONNECT_TIMEOUT: Duration = Duration::from_secs(2);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("ic705_bridge_lib=info,warn")),
        )
        .init();

    let app_state = Arc::new(AppState::new());

    let builder = tauri::Builder::default();

    // Garrot mono-instance (doit être enregistré en premier) : un second
    // lancement refocalise la fenêtre existante au lieu de démarrer un processus
    // concurrent qui échouerait à binder les ports UDP 50001/50002 et l'API 8765.
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
        }
    }));

    let app = builder
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
        .build(tauri::generate_context!())
        .expect("erreur à la construction de l'application Tauri");

    app.run(|app_handle, event| {
        // Fermeture de la fenêtre : on prévient la radio avant de quitter, sinon
        // elle conserve une session pendante et refuse la prochaine connexion.
        if let tauri::RunEvent::WindowEvent {
            event: tauri::WindowEvent::CloseRequested { api, .. },
            ..
        } = &event
        {
            let state = app_handle.state::<Arc<AppState>>().inner().clone();
            // Rien à fermer ? On laisse la fenêtre se fermer normalement.
            if state.session.try_lock().map(|g| g.is_none()).unwrap_or(false) {
                return;
            }
            api.prevent_close();
            let handle = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                if let Some(session) = state.session.lock().await.take() {
                    // Borné par un timeout : la fermeture ne doit jamais rester
                    // bloquée si le réseau ne répond plus.
                    let _ =
                        tokio::time::timeout(SHUTDOWN_DISCONNECT_TIMEOUT, session.disconnect()).await;
                }
                handle.exit(0);
            });
        }
    });
}

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

use api::ApiServerManager;
use state::AppState;

/// Délai max accordé à la déconnexion propre lors de la fermeture de l'app.
const SHUTDOWN_DISCONNECT_TIMEOUT: Duration = Duration::from_secs(2);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("ic705_bridge_lib=info,warn")
            }),
        )
        .init();

    let app_state = Arc::new(AppState::new());
    let api_server = Arc::new(ApiServerManager::default());

    let builder = tauri::Builder::default();

    // Garrot mono-instance (doit être enregistré en premier) : un second
    // lancement refocalise la fenêtre existante au lieu de démarrer un processus
    // concurrent qui échouerait à binder les ports UDP 50001/50002 et l'API locale.
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
        }
    }));

    let app = builder
        .plugin(tauri_plugin_process::init())
        .manage(app_state.clone())
        .manage(api_server.clone())
        .setup(move |app| {
            // Mises à jour automatiques (desktop uniquement).
            #[cfg(desktop)]
            app.handle()
                .plugin(tauri_plugin_updater::Builder::new().build())?;

            // Démarrage de l'API HTTP locale (toujours active).
            let st = app_state.clone();
            let server = api_server.clone();
            tauri::async_runtime::spawn(async move {
                server.ensure_started(st).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::connect,
            commands::disconnect,
            commands::send_civ,
            commands::get_status,
            commands::set_api_port,
        ])
        .build(tauri::generate_context!())
        .expect("erreur à la construction de l'application Tauri");

    // Toute sortie de l'app doit prévenir la radio (deauth/disconnect), sinon
    // elle garde une session pendante et refuse la prochaine connexion. Deux
    // portes de sortie existent : la fermeture de fenêtre (CloseRequested) et
    // la sortie « application » (ExitRequested : Cmd+Q / menu Quitter sur
    // macOS, qui ne passe PAS par CloseRequested). Les deux convergent vers
    // `begin_shutdown`, idempotent.
    app.run(|app_handle, event| match &event {
        tauri::RunEvent::WindowEvent {
            event: tauri::WindowEvent::CloseRequested { api, .. },
            ..
        } => {
            if begin_shutdown(app_handle) {
                api.prevent_close();
            }
        }
        tauri::RunEvent::ExitRequested { api, code, .. } => {
            // `code.is_some()` = exit()/restart() programmatique : soit notre
            // propre `exit(0)` post-nettoyage, soit le relaunch de mise à jour
            // (le frontend se déconnecte avant d'installer) → laisser sortir.
            if code.is_some() {
                return;
            }
            if begin_shutdown(app_handle) {
                api.prevent_exit();
            }
        }
        _ => {}
    });
}

/// Lance (une seule fois) la déconnexion propre de la radio avant la sortie.
/// Renvoie `true` si la sortie doit être retardée (un `exit(0)` suivra),
/// `false` s'il n'y a rien à nettoyer (sortie immédiate autorisée).
fn begin_shutdown(app_handle: &tauri::AppHandle) -> bool {
    use std::sync::atomic::Ordering;

    let state = app_handle.state::<Arc<AppState>>().inner().clone();
    if state.shutdown_started.swap(true, Ordering::SeqCst) {
        // Nettoyage déjà en cours : retarder cette sortie-ci, l'`exit(0)` de
        // la tâche en vol conclura.
        return true;
    }
    // Pas de session active ? Rien à nettoyer : on rend le flag (une session
    // pourrait encore être créée si cette sortie est ensuite annulée).
    if state
        .session
        .try_lock()
        .map(|g| g.is_none())
        .unwrap_or(false)
    {
        state.shutdown_started.store(false, Ordering::SeqCst);
        return false;
    }
    let handle = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(session) = state.session.lock().await.take() {
            // Borné par un timeout : la sortie ne doit jamais rester bloquée
            // si le réseau ne répond plus.
            let _ = tokio::time::timeout(SHUTDOWN_DISCONNECT_TIMEOUT, session.disconnect()).await;
        }
        handle.exit(0);
    });
    true
}

//! Commandes Tauri exposées au frontend (onglets Connection & CI-V Terminal).

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, State};

use crate::api::ApiServerManager;
use crate::error::{BridgeError, Result};
use crate::session::{ConnState, Session};
use crate::state::{AppState, StatusSnapshot};
use crate::util::{parse_hex, to_hex};

/// Résultat d'un envoi CI-V (écho TX + réponse RX en hex).
#[derive(Debug, Serialize)]
pub struct CivResult {
    pub tx: String,
    pub response: String,
}

/// Paramètres de connexion saisis dans l'onglet Connection.
#[tauri::command]
pub async fn connect(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    host: String,
    username: String,
    password: String,
) -> Result<StatusSnapshot> {
    let state = state.inner().clone();

    // Normalise l'hôte : les espaces de copier-coller ou un nom ne doivent pas
    // faire échouer la résolution.
    let host = host.trim().to_string();
    if host.is_empty() {
        let e = BridgeError::Protocol("renseigner l'IP ou le nom de l'IC-705".into());
        state.set_status(Some(&app), ConnState::Error, e.to_string(), None);
        return Err(e);
    }

    // Une seule tentative de connexion à la fois : deux `connect` concurrents
    // créeraient deux sessions dont une serait écrasée SANS deauth (radio
    // « occupée »). L'UI désactive le bouton, mais pas un invoke direct.
    let Ok(_connecting) = state.connecting.try_lock() else {
        return Err(BridgeError::Protocol(
            "une connexion est déjà en cours".into(),
        ));
    };

    // Une seule session à la fois : on ferme la précédente.
    if let Some(old) = state.session.lock().await.take() {
        old.disconnect().await;
    }

    state.set_status(
        Some(&app),
        ConnState::Connecting,
        format!("Connexion à {host}…"),
        Some(host.clone()),
    );

    match Session::connect(&host, &username, &password, Some(app.clone())).await {
        Ok(session) => {
            let session = Arc::new(session);
            *state.session.lock().await = Some(session.clone());
            spawn_link_supervisor(app.clone(), state.clone(), session);
            state.set_status(
                Some(&app),
                ConnState::CivReady,
                "Tunnel CI-V prêt",
                Some(host),
            );
            Ok(state.snapshot())
        }
        Err(e) => {
            state.set_status(Some(&app), ConnState::Error, e.to_string(), Some(host));
            Err(e)
        }
    }
}

/// Surveille la perte de lien signalée par la session (radio éteinte, Wi-Fi
/// coupé…) : retire la session de l'état et passe le statut en erreur, pour que
/// l'UI et l'API cessent d'afficher un tunnel « prêt » qui ne l'est plus.
fn spawn_link_supervisor(app: AppHandle, state: Arc<AppState>, session: Arc<Session>) {
    let mut link = session.link_lost_watch();
    tokio::spawn(async move {
        // `changed()` renvoie Err si la session est détruite proprement
        // (disconnect/drop) : dans ce cas il n'y a rien à nettoyer.
        if link.wait_for(|lost| *lost).await.is_err() {
            return;
        }
        let mut guard = state.session.lock().await;
        // Ne retirer QUE la session qu'on supervise : l'utilisateur a pu déjà
        // se reconnecter (nouvelle session) pendant la détection.
        if guard.as_ref().is_some_and(|s| Arc::ptr_eq(s, &session)) {
            *guard = None;
            drop(guard);
            session.disconnect().await; // best-effort + arrêt des tâches
            state.set_status(
                Some(&app),
                ConnState::Error,
                "Connexion radio perdue (radio éteinte ou Wi-Fi coupé ?)",
                None,
            );
        }
    });
}

/// Déconnecte la session active.
#[tauri::command]
pub async fn disconnect(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<StatusSnapshot> {
    let state = state.inner().clone();
    if let Some(session) = state.session.lock().await.take() {
        session.disconnect().await;
    }
    state.set_status(Some(&app), ConnState::Disconnected, "Déconnecté", None);
    Ok(state.snapshot())
}

/// Envoie une trame CI-V (saisie hexadécimale) et renvoie la réponse.
#[tauri::command]
pub async fn send_civ(state: State<'_, Arc<AppState>>, frame: String) -> Result<CivResult> {
    let state = state.inner().clone();
    let bytes = parse_hex(&frame)?;

    // Clone de l'Arc puis relâchement du verrou global : une déconnexion ou une
    // lecture d'état ne doit pas attendre la réponse radio. `Session` sérialise
    // elle-même les transactions CI-V concurrentes.
    let session = state
        .session
        .lock()
        .await
        .as_ref()
        .cloned()
        .ok_or(BridgeError::NotConnected)?;
    let response = session.send_civ(&bytes).await?;

    Ok(CivResult {
        tx: to_hex(&bytes),
        response: to_hex(&response),
    })
}

/// Renvoie l'état courant (pour rafraîchir l'UI).
#[tauri::command]
pub fn get_status(state: State<'_, Arc<AppState>>) -> StatusSnapshot {
    state.snapshot()
}

/// Change le port TCP de l'API loopback sans redémarrer l'application.
#[tauri::command]
pub async fn set_api_port(
    state: State<'_, Arc<AppState>>,
    api_server: State<'_, Arc<ApiServerManager>>,
    port: u16,
) -> Result<StatusSnapshot> {
    let state = state.inner().clone();
    api_server.inner().set_port(state.clone(), port).await?;
    Ok(state.snapshot())
}

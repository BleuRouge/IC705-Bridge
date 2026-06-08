//! État applicatif partagé entre les commandes Tauri et l'API HTTP locale.

use std::sync::Mutex as StdMutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

use crate::session::{ConnState, Session};

/// Adresse de l'API HTTP locale exposée à Python.
pub const API_ADDR: &str = "127.0.0.1:8765";

/// Instantané d'état sérialisable (renvoyé aux commandes et à l'API).
#[derive(Debug, Clone, Serialize)]
pub struct StatusSnapshot {
    pub state: ConnState,
    pub message: String,
    pub host: Option<String>,
    pub api_running: bool,
    pub api_url: String,
}

/// État courant (protégé par un mutex synchrone, accès très bref).
pub struct StatusInfo {
    pub state: ConnState,
    pub message: String,
    pub host: Option<String>,
    pub api_running: bool,
}

impl StatusInfo {
    fn snapshot(&self) -> StatusSnapshot {
        StatusSnapshot {
            state: self.state,
            message: self.message.clone(),
            host: self.host.clone(),
            api_running: self.api_running,
            api_url: format!("http://{API_ADDR}"),
        }
    }
}

/// État global de l'application.
pub struct AppState {
    pub session: Mutex<Option<Session>>,
    pub status: StdMutex<StatusInfo>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            session: Mutex::new(None),
            status: StdMutex::new(StatusInfo {
                state: ConnState::Disconnected,
                message: "Déconnecté".into(),
                host: None,
                api_running: false,
            }),
        }
    }

    /// Lit l'instantané d'état courant.
    pub fn snapshot(&self) -> StatusSnapshot {
        self.status.lock().unwrap().snapshot()
    }

    /// Met à jour l'état et, si possible, émet l'événement `status` vers le frontend.
    pub fn set_status(
        &self,
        app: Option<&AppHandle>,
        state: ConnState,
        message: impl Into<String>,
        host: Option<String>,
    ) {
        let snap = {
            let mut s = self.status.lock().unwrap();
            s.state = state;
            s.message = message.into();
            if host.is_some() {
                s.host = host;
            }
            if state == ConnState::Disconnected {
                s.host = None;
            }
            s.snapshot()
        };
        if let Some(app) = app {
            let _ = app.emit("status", snap);
        }
    }

    /// Marque l'API HTTP comme démarrée.
    pub fn set_api_running(&self, running: bool) {
        self.status.lock().unwrap().api_running = running;
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

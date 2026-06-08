//! Orchestrateur de session : combine le stream de contrôle et le stream serial,
//! expose l'envoi/réception CI-V, et diffuse les trames reçues vers le frontend.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::error::Result;
use crate::rsba1::control::{connect_control, ControlStream};
use crate::rsba1::serial::{connect_serial, SerialStream};
use crate::util::to_hex;

/// Capacité du canal de diffusion des trames CI-V reçues.
const CIV_CHANNEL_CAP: usize = 256;
/// Délai d'attente de la première trame de réponse à un `send_civ`.
const RESP_FIRST_TIMEOUT: Duration = Duration::from_millis(1200);
/// Délai d'inactivité après lequel on considère la réponse terminée.
const RESP_IDLE_GAP: Duration = Duration::from_millis(200);

/// État de connexion exposé au frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)] // `Authenticated` est réservé à une future étape intermédiaire.
pub enum ConnState {
    Disconnected,
    Connecting,
    Authenticated,
    CivReady,
    Error,
}

/// Une session active vers l'IC-705.
pub struct Session {
    #[allow(dead_code)] // info de session, utile au debug / futurs usages
    pub host: String,
    #[allow(dead_code)]
    control: Arc<ControlStream>,
    serial: Arc<SerialStream>,
    civ_tx: broadcast::Sender<Vec<u8>>,
    handles: Vec<JoinHandle<()>>,
}

impl Session {
    /// Établit la connexion complète : control (login/auth) puis serial (CI-V).
    /// `app` permet de diffuser les trames RX vers le frontend (événement `civ-rx`).
    pub async fn connect(
        host: &str,
        username: &str,
        password: &str,
        app: Option<AppHandle>,
    ) -> Result<Self> {
        let connection = connect_control(host, username, password).await?;

        let (civ_tx, _) = broadcast::channel(CIV_CHANNEL_CAP);
        let (serial, serial_handles) = connect_serial(host, civ_tx.clone()).await?;

        let mut handles = connection.handles;
        handles.extend(serial_handles);

        // Diffusion des trames CI-V reçues vers le frontend.
        if let Some(app) = app {
            let mut rx = civ_tx.subscribe();
            handles.push(tokio::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(frame) => {
                            let _ = app.emit("civ-rx", to_hex(&frame));
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }));
        }

        Ok(Self {
            host: host.to_string(),
            control: connection.control,
            serial,
            civ_tx,
            handles,
        })
    }

    /// Envoie une trame CI-V brute et collecte la (ou les) réponse(s) reçues
    /// dans une courte fenêtre. Renvoie les octets de réponse concaténés.
    pub async fn send_civ(&self, frame: &[u8]) -> Result<Vec<u8>> {
        let mut rx = self.civ_tx.subscribe();
        self.serial.send_civ(frame).await?;

        let mut response = Vec::new();
        let first_deadline = Instant::now() + RESP_FIRST_TIMEOUT;

        // Attente de la première trame.
        loop {
            let remaining = match first_deadline.checked_duration_since(Instant::now()) {
                Some(d) => d,
                None => break,
            };
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(frame)) => {
                    response.extend_from_slice(&frame);
                    break;
                }
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                _ => return Ok(response), // timeout / fermé : pas de réponse
            }
        }

        // Collecte des trames suivantes tant qu'elles arrivent rapidement.
        loop {
            match tokio::time::timeout(RESP_IDLE_GAP, rx.recv()).await {
                Ok(Ok(frame)) => response.extend_from_slice(&frame),
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                _ => break,
            }
        }

        Ok(response)
    }

    /// Abonne un consommateur au flux des trames CI-V reçues.
    /// (Réservé au futur `stream_civ` de l'API.)
    #[allow(dead_code)]
    pub fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        self.civ_tx.subscribe()
    }

    /// Déconnexion propre : ferme le canal serial, envoie les déconnexions et
    /// arrête toutes les tâches de fond.
    pub async fn disconnect(self) {
        let _ = self.serial.send_open_close(true).await;
        let _ = self.serial.common.send_disconnect().await;
        let _ = self.control.send_auth(0x01).await; // deauth
        tokio::time::sleep(Duration::from_millis(300)).await;
        let _ = self.control.common.send_disconnect().await;
        for h in &self.handles {
            h.abort();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        for h in &self.handles {
            h.abort();
        }
    }
}

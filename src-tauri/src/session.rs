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
/// Fenêtre totale max de collecte d'une réponse : borne le cas où la radio
/// streame en continu (transceive / scope ON), sinon la collecte ne finirait jamais.
const RESP_MAX_WINDOW: Duration = Duration::from_millis(1500);

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
    /// dans une courte fenêtre. La radio renvoie d'abord l'écho de notre trame
    /// (spec §8) : il est écarté. Renvoie les octets de réponse concaténés.
    pub async fn send_civ(&self, frame: &[u8]) -> Result<Vec<u8>> {
        let mut rx = self.civ_tx.subscribe();
        self.serial.send_civ(frame).await?;

        let mut response = Vec::new();
        let start = Instant::now();
        let first_deadline = start + RESP_FIRST_TIMEOUT;

        // Attente de la première trame de réponse (hors écho).
        loop {
            let remaining = match first_deadline.checked_duration_since(Instant::now()) {
                Some(d) => d,
                None => break,
            };
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(received)) => {
                    if received == frame {
                        continue; // écho de notre propre trame
                    }
                    response.extend_from_slice(&received);
                    break;
                }
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                _ => return Ok(response), // timeout / fermé : pas de réponse
            }
        }

        // Collecte des trames suivantes tant qu'elles arrivent rapidement,
        // sans jamais dépasser la fenêtre totale (cas transceive/scope continu).
        loop {
            if start.elapsed() >= RESP_MAX_WINDOW {
                break;
            }
            match tokio::time::timeout(RESP_IDLE_GAP, rx.recv()).await {
                Ok(Ok(received)) => {
                    if received != frame {
                        response.extend_from_slice(&received);
                    }
                }
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                _ => break,
            }
        }

        Ok(response)
    }

    /// Abonne un consommateur au flux des trames CI-V reçues (endpoint `/stream`).
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

/// Tests d'intégration : une radio IC-705 simulée (UDP local) déroule le
/// protocole complet (handshake, login, auth, 0xa8/0x90, open, tunnel CI-V).
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UdpSocket;

    const RADIO_SID: u32 = 0xDEAD_BEEF;
    const CIV_RESPONSE: [u8; 11] = [0xFE, 0xFE, 0xE0, 0xA4, 0x03, 0x00, 0x00, 0x05, 0x45, 0x01, 0xFD];

    /// En-tête 16 octets côté radio : localSID = SID radio, remoteSID = SID client.
    fn header(len: u32, typ: u8, seq: u16, radio_sid: u32, client_sid: u32) -> Vec<u8> {
        let mut p = vec![0u8; 16];
        p[0..4].copy_from_slice(&len.to_le_bytes());
        p[4] = typ;
        p[6] = seq as u8;
        p[7] = (seq >> 8) as u8;
        p[8..12].copy_from_slice(&radio_sid.to_be_bytes());
        p[12..16].copy_from_slice(&client_sid.to_be_bytes());
        p
    }

    /// Paquet data serial 0xc1 contenant une trame CI-V (longueur 2 octets LE).
    fn data_packet(civ: &[u8], client_sid: u32) -> Vec<u8> {
        let mut p = header(21 + civ.len() as u32, 0x00, 0, RADIO_SID + 1, client_sid);
        let l = civ.len() as u16;
        p.extend_from_slice(&[0xc1, l as u8, (l >> 8) as u8, 0x00, 0x00]);
        p.extend_from_slice(civ);
        p
    }

    async fn mock_control(socket: UdpSocket) {
        let mut buf = [0u8; 2048];
        loop {
            let Ok((n, peer)) = socket.recv_from(&mut buf).await else { return };
            let r = &buf[..n];
            if r.len() < 16 {
                continue;
            }
            let client_sid = u32::from_be_bytes([r[8], r[9], r[10], r[11]]);
            match (r.len(), r[4]) {
                (16, 0x03) => {
                    // are-you-there -> i-am-here
                    let p = header(16, 0x04, 0, RADIO_SID, client_sid);
                    let _ = socket.send_to(&p, peer).await;
                }
                (16, 0x06) => {
                    // are-you-ready -> ready
                    let mut p = header(16, 0x06, 0, RADIO_SID, client_sid);
                    p[6] = 0x01;
                    let _ = socket.send_to(&p, peer).await;
                }
                (128, 0x00) if r[0] == 0x80 => {
                    // login -> réponse 0x60, token à 26..32
                    let mut p = header(96, 0x00, 0, RADIO_SID, client_sid);
                    p.resize(96, 0);
                    p[26..32].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
                    let _ = socket.send_to(&p, peer).await;
                }
                (64, 0x00) if r[0] == 0x40 => {
                    // auth -> ok (r[21]=0x05), puis capabilities 0xa8
                    let mut p = header(64, 0x00, 0, RADIO_SID, client_sid);
                    p.resize(64, 0);
                    p[21] = 0x05;
                    let _ = socket.send_to(&p, peer).await;
                    let mut a8 = header(168, 0x00, 0, RADIO_SID, client_sid);
                    a8.resize(168, 0);
                    a8[66..82].copy_from_slice(&[0xAA; 16]);
                    let _ = socket.send_to(&a8, peer).await;
                }
                (144, 0x00) if r[0] == 0x90 => {
                    // conninfo -> succès serial+audio (r[96]=1)
                    let mut p = header(144, 0x00, 0, RADIO_SID, client_sid);
                    p.resize(144, 0);
                    p[26..32].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
                    p[96] = 1;
                    let _ = socket.send_to(&p, peer).await;
                }
                _ => {}
            }
        }
    }

    async fn mock_serial(socket: UdpSocket) {
        let mut buf = [0u8; 2048];
        loop {
            let Ok((n, peer)) = socket.recv_from(&mut buf).await else { return };
            let r = &buf[..n];
            if r.len() < 16 {
                continue;
            }
            let client_sid = u32::from_be_bytes([r[8], r[9], r[10], r[11]]);
            if r.len() == 16 && r[4] == 0x03 {
                let p = header(16, 0x04, 0, RADIO_SID + 1, client_sid);
                let _ = socket.send_to(&p, peer).await;
            } else if r.len() == 16 && r[4] == 0x06 {
                let mut p = header(16, 0x06, 0, RADIO_SID + 1, client_sid);
                p[6] = 0x01;
                let _ = socket.send_to(&p, peer).await;
            } else if r.len() > 21 && r[16] == 0xc1 {
                // data CI-V : écho de la trame, puis réponse
                let l = u16::from_le_bytes([r[17], r[18]]) as usize;
                if 21 + l <= r.len() {
                    let civ = r[21..21 + l].to_vec();
                    let _ = socket.send_to(&data_packet(&civ, client_sid), peer).await;
                    let _ = socket.send_to(&data_packet(&CIV_RESPONSE, client_sid), peer).await;
                }
            }
        }
    }

    #[tokio::test]
    async fn full_session_over_mock_radio() {
        let control = UdpSocket::bind("127.0.0.1:50001")
            .await
            .expect("port UDP 50001 occupé (RS-BA1 / le bridge tourne ?)");
        let serial = UdpSocket::bind("127.0.0.1:50002")
            .await
            .expect("port UDP 50002 occupé (RS-BA1 / le bridge tourne ?)");
        tokio::spawn(mock_control(control));
        tokio::spawn(mock_serial(serial));

        let session = Session::connect("127.0.0.1", "user", "pass", None)
            .await
            .expect("connexion à la radio simulée");

        let tx = [0xFE, 0xFE, 0xA4, 0xE0, 0x03, 0xFD];
        let resp = session.send_civ(&tx).await.expect("envoi CI-V");
        // L'écho de notre trame doit être écarté, seule la réponse reste.
        assert_eq!(resp, CIV_RESPONSE);

        session.disconnect().await;
    }
}

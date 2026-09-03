//! Orchestrateur de session : combine le stream de contrôle et le stream serial,
//! expose l'envoi/réception CI-V, et diffuse les trames reçues vers le frontend.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::{broadcast, watch, Mutex};
use tokio::task::JoinHandle;

use crate::error::{BridgeError, Result};
use crate::rsba1::control::{connect_control, ControlStream};
use crate::rsba1::serial::{connect_serial, SerialStream};
use crate::rsba1::stream::{StreamCommon, TaskGuard};
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

/// Silence simultané des deux streams au-delà duquel le lien est déclaré perdu.
/// En fonctionnement normal la radio répond aux pings pkt7 toutes les ~3 s :
/// 10 s = plus de 3 pings sans réponse. (Raccourci en test.)
#[cfg(not(test))]
const LINK_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(test)]
const LINK_TIMEOUT: Duration = Duration::from_millis(1200);
/// Cadence de vérification du watchdog de lien.
#[cfg(not(test))]
const LINK_CHECK_INTERVAL: Duration = Duration::from_secs(1);
#[cfg(test)]
const LINK_CHECK_INTERVAL: Duration = Duration::from_millis(150);

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
    control: Arc<ControlStream>,
    serial: Arc<SerialStream>,
    civ_tx: broadcast::Sender<Vec<u8>>,
    /// Une seule transaction requête/réponse CI-V à la fois. Le flux RX reste
    /// diffusé en parallèle vers le terminal et `/stream`, mais deux clients
    /// HTTP/Tauri ne peuvent plus s'attribuer la même réponse radio.
    civ_request: Mutex<()>,
    /// Passe à `true` quand le watchdog déclare le lien radio perdu. Le canal
    /// se ferme (sans `true`) si la session est détruite proprement.
    link_lost_rx: watch::Receiver<bool>,
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

        // Garde RAII : si l'ouverture du serial (ou la suite) échoue, les tâches
        // de fond du control sont abandonnées proprement. Sinon elles
        // continueraient leurs keepalives et la radio resterait « occupée »,
        // bloquant toute reconnexion.
        let mut guard = TaskGuard::new(connection.handles);

        let (civ_tx, _) = broadcast::channel(CIV_CHANNEL_CAP);
        let (serial, serial_handles) = match connect_serial(host, civ_tx.clone()).await {
            Ok(v) => v,
            Err(e) => {
                // Best-effort : prévenir la radio qu'on se retire pour qu'elle
                // libère immédiatement le créneau (sinon attente de son timeout).
                let _ = connection.control.send_auth(0x01).await; // deauth
                let _ = connection.control.common.send_disconnect().await;
                return Err(e); // `guard` droppée ici → tâches control abandonnées.
            }
        };
        for h in serial_handles {
            guard.push(h);
        }

        // Watchdog de perte de lien : la radio émet en continu (réponses aux
        // pings pkt7, idles). Si les DEUX streams restent muets au-delà de
        // LINK_TIMEOUT, le lien est déclaré perdu (radio éteinte, Wi-Fi coupé).
        let (link_tx, link_lost_rx) = watch::channel(false);
        guard.push(spawn_link_watchdog(
            connection.control.common.clone(),
            serial.common.clone(),
            link_tx,
        ));

        // Diffusion des trames CI-V reçues vers le frontend.
        if let Some(app) = app {
            let mut rx = civ_tx.subscribe();
            guard.push(tokio::spawn(async move {
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
            civ_request: Mutex::new(()),
            link_lost_rx,
            handles: guard.disarm(),
        })
    }

    /// Envoie une trame CI-V brute et collecte la (ou les) réponse(s) reçues
    /// dans une courte fenêtre. La radio renvoie d'abord l'écho de notre trame
    /// (spec §8) : il est écarté. Les trames spontanées sans rapport avec la
    /// commande sont également ignorées. Renvoie les réponses corrélées
    /// concaténées et une erreur explicite si aucune n'arrive à temps.
    pub async fn send_civ(&self, frame: &[u8]) -> Result<Vec<u8>> {
        let _request = self.civ_request.lock().await;
        let mut rx = self.civ_tx.subscribe();
        self.serial.send_civ(frame).await?;

        let mut response = Vec::new();
        let start = Instant::now();
        let first_deadline = start + RESP_FIRST_TIMEOUT;

        // Attente de la première trame de réponse (hors écho).
        while let Some(remaining) = first_deadline.checked_duration_since(Instant::now()) {
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(received)) => {
                    if !is_response_to(frame, &received) {
                        continue;
                    }
                    response.extend_from_slice(&received);
                    break;
                }
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                _ => {
                    return Err(BridgeError::Timeout(format!(
                        "réponse CI-V à {}",
                        to_hex(frame)
                    )))
                }
            }
        }

        if response.is_empty() {
            return Err(BridgeError::Timeout(format!(
                "réponse CI-V à {}",
                to_hex(frame)
            )));
        }

        // Collecte des trames suivantes tant qu'elles arrivent rapidement,
        // sans jamais dépasser la fenêtre totale (cas transceive/scope continu).
        loop {
            if start.elapsed() >= RESP_MAX_WINDOW {
                break;
            }
            match tokio::time::timeout(RESP_IDLE_GAP, rx.recv()).await {
                Ok(Ok(received)) => {
                    if is_response_to(frame, &received) {
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

    /// Récepteur du signal de perte de lien : passe à `true` si la radio ne
    /// donne plus signe de vie (voir [`LINK_TIMEOUT`]). Le canal se ferme sans
    /// signal si la session est détruite proprement.
    pub fn link_lost_watch(&self) -> watch::Receiver<bool> {
        self.link_lost_rx.clone()
    }

    /// Déconnexion propre : ferme le canal serial, envoie les déconnexions et
    /// arrête toutes les tâches de fond. Prend `&self` (la session vit dans un
    /// `Arc` partagé) ; un `send_civ` encore en vol se termine par une erreur
    /// bornée, sans panique.
    pub async fn disconnect(&self) {
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

/// Vérifie qu'une trame reçue peut être la réponse à `request`.
///
/// Pour une trame CI-V standard, les adresses source/destination doivent être
/// inversées et le code commande identique. Les acquittements `FB`/`FA` sont
/// valables pour toute commande d'écriture. Les saisies non standard gardent
/// un comportement bas niveau : la première trame différente de l'écho est
/// acceptée.
fn is_response_to(request: &[u8], received: &[u8]) -> bool {
    if received == request {
        return false;
    }

    let standard_request =
        request.len() >= 6 && request.starts_with(&[0xFE, 0xFE]) && request.last() == Some(&0xFD);
    let standard_response = received.len() >= 6
        && received.starts_with(&[0xFE, 0xFE])
        && received.last() == Some(&0xFD);

    if !standard_request || !standard_response {
        return true;
    }

    let addressed_to_requester = received[2] == request[3] && received[3] == request[2];
    let command_matches = if matches!(received[4], 0xFA | 0xFB) {
        true
    } else if received[4] != request[4] {
        false
    } else if command_has_subcommand(request[4]) && request.len() >= 7 && received.len() >= 7 {
        received[5] == request[5]
    } else {
        true
    };
    addressed_to_requester && command_matches
}

/// Familles CI-V dont le premier octet de données identifie une sous-commande.
fn command_has_subcommand(command: u8) -> bool {
    matches!(
        command,
        0x14 | 0x15 | 0x16 | 0x1A | 0x1B | 0x1C | 0x21 | 0x25 | 0x26 | 0x27
    )
}

impl Drop for Session {
    fn drop(&mut self) {
        for h in &self.handles {
            h.abort();
        }
    }
}

/// Vérifie périodiquement que la radio émet toujours ; signale la perte de
/// lien quand les deux streams sont muets depuis plus de [`LINK_TIMEOUT`].
fn spawn_link_watchdog(
    control: Arc<StreamCommon>,
    serial: Arc<StreamCommon>,
    link_tx: watch::Sender<bool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(LINK_CHECK_INTERVAL);
        ticker.tick().await; // consomme le tick immédiat
        loop {
            ticker.tick().await;
            if control.silent_for() > LINK_TIMEOUT && serial.silent_for() > LINK_TIMEOUT {
                let _ = link_tx.send(true);
                break; // le signal est permanent : la tâche a fini son travail
            }
        }
    })
}

/// Tests d'intégration : une radio IC-705 simulée (UDP local) déroule le
/// protocole complet (handshake, login, auth, 0xa8/0x90, open, tunnel CI-V).
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UdpSocket;

    const RADIO_SID: u32 = 0xDEAD_BEEF;
    const CIV_RESPONSE: [u8; 11] = [
        0xFE, 0xFE, 0xE0, 0xA4, 0x03, 0x00, 0x00, 0x05, 0x45, 0x01, 0xFD,
    ];
    const UNSOLICITED_MODE: [u8; 8] = [0xFE, 0xFE, 0xE0, 0xA4, 0x04, 0x01, 0x02, 0xFD];

    /// Sérialise les tests qui réservent les ports UDP 50001/50002 (sinon ils se
    /// disputeraient le bind en parallèle). Mutex tokio : sûr à tenir au travers
    /// des `.await` du test.
    static PORT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[test]
    fn correlates_civ_responses_and_rejects_echo_or_unsolicited_frames() {
        let request = [0xFE, 0xFE, 0xA4, 0xE0, 0x03, 0xFD];
        let response = [
            0xFE, 0xFE, 0xE0, 0xA4, 0x03, 0x00, 0x00, 0x05, 0x45, 0x01, 0xFD,
        ];
        let unsolicited_mode = [0xFE, 0xFE, 0xE0, 0xA4, 0x04, 0x01, 0x02, 0xFD];
        let wrong_radio = [0xFE, 0xFE, 0xE0, 0xA5, 0x03, 0xFD];
        let ack = [0xFE, 0xFE, 0xE0, 0xA4, 0xFB, 0xFD];
        let smeter_request = [0xFE, 0xFE, 0xA4, 0xE0, 0x15, 0x02, 0xFD];
        let other_meter = [0xFE, 0xFE, 0xE0, 0xA4, 0x15, 0x11, 0x00, 0x00, 0xFD];

        assert!(!is_response_to(&request, &request));
        assert!(is_response_to(&request, &response));
        assert!(!is_response_to(&request, &unsolicited_mode));
        assert!(!is_response_to(&request, &wrong_radio));
        assert!(is_response_to(&request, &ack));
        assert!(!is_response_to(&smeter_request, &other_meter));
    }

    /// Arrête une radio simulée et ATTEND sa fin réelle : son socket doit être
    /// libéré avant de relâcher [`PORT_LOCK`], sinon le test suivant peut
    /// trouver le port encore occupé (le drop des tâches détachées n'arrive
    /// qu'au shutdown du runtime, APRÈS la libération du verrou).
    async fn stop_mock(handle: tokio::task::JoinHandle<()>) {
        handle.abort();
        let _ = handle.await;
    }

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
            let Ok((n, peer)) = socket.recv_from(&mut buf).await else {
                return;
            };
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
            let Ok((n, peer)) = socket.recv_from(&mut buf).await else {
                return;
            };
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
                    // Une notification sans rapport ne doit pas être attribuée
                    // à la transaction en cours.
                    let _ = socket
                        .send_to(&data_packet(&UNSOLICITED_MODE, client_sid), peer)
                        .await;
                    let _ = socket
                        .send_to(&data_packet(&CIV_RESPONSE, client_sid), peer)
                        .await;
                }
            }
        }
    }

    #[tokio::test]
    async fn full_session_over_mock_radio() {
        let _lock = PORT_LOCK.lock().await;
        let control = UdpSocket::bind("127.0.0.1:50001")
            .await
            .expect("port UDP 50001 occupé (RS-BA1 / le bridge tourne ?)");
        let serial = UdpSocket::bind("127.0.0.1:50002")
            .await
            .expect("port UDP 50002 occupé (RS-BA1 / le bridge tourne ?)");
        let mock_ctl = tokio::spawn(mock_control(control));
        let mock_ser = tokio::spawn(mock_serial(serial));

        let session = Session::connect("127.0.0.1", "user", "pass", None)
            .await
            .expect("connexion à la radio simulée");

        let tx = [0xFE, 0xFE, 0xA4, 0xE0, 0x03, 0xFD];
        let resp = session.send_civ(&tx).await.expect("envoi CI-V");
        // L'écho de notre trame doit être écarté, seule la réponse reste.
        assert_eq!(resp, CIV_RESPONSE);

        session.disconnect().await;
        stop_mock(mock_ctl).await;
        stop_mock(mock_ser).await;
    }

    /// Radio qui disparaît (éteinte / Wi-Fi coupé) : le watchdog doit signaler
    /// la perte de lien au lieu de laisser la session « prête » pour toujours.
    #[tokio::test]
    async fn watchdog_detects_dead_radio() {
        let _lock = PORT_LOCK.lock().await;
        let control = UdpSocket::bind("127.0.0.1:50001")
            .await
            .expect("port UDP 50001 occupé (RS-BA1 / le bridge tourne ?)");
        let serial = UdpSocket::bind("127.0.0.1:50002")
            .await
            .expect("port UDP 50002 occupé (RS-BA1 / le bridge tourne ?)");
        let mock_ctl = tokio::spawn(mock_control(control));
        let mock_ser = tokio::spawn(mock_serial(serial));

        let session = Session::connect("127.0.0.1", "user", "pass", None)
            .await
            .expect("connexion à la radio simulée");
        let mut link = session.link_lost_watch();
        assert!(
            !*link.borrow(),
            "le lien doit être vivant après la connexion"
        );

        // La radio « meurt » : plus aucune réponse (attendre la fin réelle des
        // mocks pour que leurs sockets soient bien fermés).
        stop_mock(mock_ctl).await;
        stop_mock(mock_ser).await;

        tokio::time::timeout(Duration::from_secs(5), link.wait_for(|lost| *lost))
            .await
            .expect("le watchdog n'a pas signalé la perte de lien à temps")
            .expect("canal du watchdog fermé sans signal");

        session.disconnect().await;
    }

    /// Non-régression : des identifiants refusés doivent (a) renvoyer
    /// `InvalidCredentials` et (b) envoyer un disconnect à la radio pour libérer
    /// la session de contrôle — sinon le login suivant reste bloqué.
    #[tokio::test]
    async fn bad_credentials_release_radio_session() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let _lock = PORT_LOCK.lock().await;
        let got_disconnect = Arc::new(AtomicBool::new(false));

        let control = UdpSocket::bind("127.0.0.1:50001")
            .await
            .expect("port UDP 50001 occupé (RS-BA1 / le bridge tourne ?)");
        let gd = got_disconnect.clone();
        let server = tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            loop {
                let Ok((n, peer)) = control.recv_from(&mut buf).await else {
                    return;
                };
                let r = &buf[..n];
                if r.len() < 16 {
                    continue;
                }
                let client_sid = u32::from_be_bytes([r[8], r[9], r[10], r[11]]);
                match (r.len(), r[4]) {
                    (16, 0x03) => {
                        let p = header(16, 0x04, 0, RADIO_SID, client_sid);
                        let _ = control.send_to(&p, peer).await;
                    }
                    (16, 0x06) => {
                        let mut p = header(16, 0x06, 0, RADIO_SID, client_sid);
                        p[6] = 0x01;
                        let _ = control.send_to(&p, peer).await;
                    }
                    (16, 0x05) => gd.store(true, Ordering::SeqCst), // disconnect reçu
                    (128, 0x00) if r[0] == 0x80 => {
                        // login -> réponse 0x60 avec le marqueur « identifiants refusés ».
                        let mut p = header(96, 0x00, 0, RADIO_SID, client_sid);
                        p.resize(96, 0);
                        p[48..52].copy_from_slice(&[0xff, 0xff, 0xff, 0xfe]);
                        let _ = control.send_to(&p, peer).await;
                    }
                    _ => {}
                }
            }
        });

        let res = crate::rsba1::control::connect_control("127.0.0.1", "user", "badpass").await;
        assert!(
            matches!(res, Err(crate::error::BridgeError::InvalidCredentials)),
            "des identifiants refusés doivent renvoyer InvalidCredentials"
        );

        // Laisse le disconnect best-effort atteindre la radio simulée.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            got_disconnect.load(Ordering::SeqCst),
            "la radio doit recevoir un disconnect pour libérer la session de contrôle"
        );

        stop_mock(server).await;
    }
}

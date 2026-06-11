//! Stream de contrôle RS-BA1 (port 50001) : login, authentification et
//! négociation d'ouverture du stream serial+audio.
//!
//! Porté depuis kappanhang (`controlstream.go`).

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use super::passcode::passcode;
use super::stream::{spawn_pkt0_idle, spawn_pkt7, spawn_reader, spawn_rx_retransmit, StreamCommon};
use crate::error::{BridgeError, Result};

pub const CONTROL_PORT: u16 = 50001;
pub const SERIAL_PORT: u16 = 50002;
pub const AUDIO_PORT: u16 = 50003;

const AUDIO_SAMPLE_RATE: u16 = 48000;
const TX_SEQ_BUF_MS: u16 = 300;
const REAUTH_INTERVAL: Duration = Duration::from_secs(60);
const READY_TIMEOUT: Duration = Duration::from_secs(8);

/// Stream de contrôle authentifié.
pub struct ControlStream {
    pub common: Arc<StreamCommon>,
    auth_inner_seq: AtomicU16,
    auth_id: Mutex<[u8; 6]>,
    username: String,
    password: String,
}

/// Résultat d'une connexion control réussie (jusqu'à l'ouverture du serial).
pub struct ControlConnection {
    pub control: Arc<ControlStream>,
    pub handles: Vec<JoinHandle<()>>,
}

impl ControlStream {
    fn next_inner_seq(&self) -> u16 {
        self.auth_inner_seq.fetch_add(1, Ordering::Relaxed)
    }

    /// Paquet de login (0x80, 128 octets) avec username/password encodés.
    async fn send_login(&self) -> Result<()> {
        let seq = self.next_inner_seq();
        let user = passcode(&self.username);
        let pass = passcode(&self.password);
        let start_id = [rand::random::<u8>(), rand::random::<u8>()];

        let mut p = self.common.header(128, 0x00, 0).to_vec();
        // offset 16..32
        p.extend_from_slice(&[
            0x00, 0x00, 0x00, 0x70, 0x01, 0x00, 0x00, seq as u8, (seq >> 8) as u8, 0x00,
            start_id[0], start_id[1], 0x00, 0x00, 0x00, 0x00,
        ]);
        p.extend_from_slice(&[0u8; 32]); // offset 32..64
        p.extend_from_slice(&user); // 64..80
        p.extend_from_slice(&pass); // 80..96
        p.extend_from_slice(&[0x69, 0x63, 0x6f, 0x6d, 0x2d, 0x70, 0x63, 0x00]); // "icom-pc\0" 96..104
        p.extend_from_slice(&[0u8; 24]); // 104..128
        debug_assert_eq!(p.len(), 128);
        self.common.send_tracked(p).await
    }

    /// Paquet d'authentification (0x40, 64 octets). magic : 0x02 / 0x05 / 0x01 (deauth).
    pub async fn send_auth(&self, magic: u8) -> Result<()> {
        let seq = self.next_inner_seq();
        let id = *self.auth_id.lock().unwrap();
        let mut p = self.common.header(64, 0x00, 0).to_vec();
        p.extend_from_slice(&[
            0x00, 0x00, 0x00, 0x30, 0x01, magic, 0x00, seq as u8, (seq >> 8) as u8, 0x00,
            id[0], id[1], id[2], id[3], id[4], id[5],
        ]); // 16..32
        p.extend_from_slice(&[0u8; 32]); // 32..64
        debug_assert_eq!(p.len(), 64);
        self.common.send_tracked(p).await
    }

    /// Demande d'ouverture du stream serial+audio (0x90, 144 octets).
    async fn send_request_serial_audio(&self, a8: &[u8; 16]) -> Result<()> {
        let seq = self.next_inner_seq();
        let id = *self.auth_id.lock().unwrap();
        let user = passcode(&self.username);
        let mut p = self.common.header(144, 0x00, 0).to_vec();
        p.extend_from_slice(&[
            0x00, 0x00, 0x00, 0x80, 0x01, 0x03, 0x00, seq as u8, (seq >> 8) as u8, 0x00,
            id[0], id[1], id[2], id[3], id[4], id[5],
        ]); // 16..32
        p.extend_from_slice(a8); // 32..48
        p.extend_from_slice(&[0u8; 16]); // 48..64
        p.extend_from_slice(&[0x49, 0x43, 0x2d, 0x37, 0x30, 0x35, 0x00, 0x00]); // "IC-705" 64..72
        p.extend_from_slice(&[0u8; 24]); // 72..96
        p.extend_from_slice(&user); // 96..112
        p.extend_from_slice(&[
            0x01, 0x01, 0x04, 0x04, 0x00, 0x00, (AUDIO_SAMPLE_RATE >> 8) as u8, AUDIO_SAMPLE_RATE as u8,
            0x00, 0x00, (AUDIO_SAMPLE_RATE >> 8) as u8, AUDIO_SAMPLE_RATE as u8,
            0x00, 0x00, (SERIAL_PORT >> 8) as u8, SERIAL_PORT as u8,
            0x00, 0x00, (AUDIO_PORT >> 8) as u8, AUDIO_PORT as u8,
            0x00, 0x00, (TX_SEQ_BUF_MS >> 8) as u8, TX_SEQ_BUF_MS as u8,
            0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ]); // 112..144
        debug_assert_eq!(p.len(), 144);
        self.common.send_tracked(p).await
    }
}

/// Établit le stream de contrôle : handshake, login, auth, puis attend l'ouverture
/// du serveur serial côté radio. Renvoie une erreur claire si les identifiants sont refusés.
pub async fn connect_control(host: &str, username: &str, password: &str) -> Result<ControlConnection> {
    let common = StreamCommon::connect("control", host, CONTROL_PORT).await?;
    common.handshake().await?;

    let ctrl = Arc::new(ControlStream {
        common: common.clone(),
        auth_inner_seq: AtomicU16::new(0),
        auth_id: Mutex::new([0u8; 6]),
        username: username.to_string(),
        password: password.to_string(),
    });

    // Login (avant le démarrage du reader : réponse lue en direct).
    // Réponse 0x60, len >= 96 ; le seq (bytes 6-7) dépend de la radio, on ne le contraint pas.
    ctrl.send_login().await?;
    let r = common
        .expect(Duration::from_secs(2), 96, &[0x60, 0x00, 0x00, 0x00, 0x00, 0x00])
        .await
        .ok_or_else(|| BridgeError::Timeout("réponse de login".into()))?;
    if r[48..52] == [0xff, 0xff, 0xff, 0xfe] {
        return Err(BridgeError::InvalidCredentials);
    }
    ctrl.auth_id.lock().unwrap().copy_from_slice(&r[26..32]);

    // Boucles de fond + machine à états d'authentification.
    let (data_tx, data_rx) = mpsc::unbounded_channel();
    let (ready_tx, ready_rx) = oneshot::channel();
    let mut handles = vec![
        spawn_reader(common.clone(), data_tx),
        spawn_pkt7(common.clone(), 2),
        spawn_pkt0_idle(common.clone()),
        spawn_rx_retransmit(common.clone()),
        spawn_control_owner(ctrl.clone(), data_rx, ready_tx),
    ];

    ctrl.send_auth(0x02).await?;
    ctrl.send_auth(0x05).await?;
    handles.push(spawn_reauth(ctrl.clone()));

    match tokio::time::timeout(READY_TIMEOUT, ready_rx).await {
        Ok(Ok(Ok(()))) => Ok(ControlConnection { control: ctrl, handles }),
        Ok(Ok(Err(e))) => Err(e),
        Ok(Err(_)) => Err(BridgeError::Protocol("canal de disponibilité fermé".into())),
        Err(_) => Err(BridgeError::Timeout(
            "ouverture du stream serial+audio (la radio n'a pas confirmé)".into(),
        )),
    }
}

/// Machine à états : surveille les réponses de la radio, déclenche la demande
/// serial+audio quand l'auth est validée, et signale la disponibilité.
fn spawn_control_owner(
    ctrl: Arc<ControlStream>,
    mut data_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    ready_tx: oneshot::Sender<Result<()>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut auth_ok = false;
        let mut a8: Option<[u8; 16]> = None;
        let mut requested = false;
        let mut ready_tx = Some(ready_tx);

        while let Some(r) = data_rx.recv().await {
            match r.len() {
                // Capabilities 0xa8 : len >= 168 selon la radio (spec §7, étape 4).
                len if len >= 168 && r[..6] == [0xa8, 0x00, 0x00, 0x00, 0x00, 0x00] => {
                    let mut id = [0u8; 16];
                    id.copy_from_slice(&r[66..82]);
                    a8 = Some(id);
                }
                64 if r[..6] == [0x40, 0x00, 0x00, 0x00, 0x00, 0x00] => {
                    if r[21] == 0x05 {
                        auth_ok = true;
                    }
                }
                80 if r[..6] == [0x50, 0x00, 0x00, 0x00, 0x00, 0x00] => {
                    if r[48..51] == [0xff, 0xff, 0xff] {
                        if let Some(tx) = ready_tx.take() {
                            let _ = tx.send(Err(BridgeError::AuthFailed(
                                "refus de la radio (essayer de redémarrer l'IC-705)".into(),
                            )));
                        }
                        break;
                    }
                    if r[48..51] == [0x00, 0x00, 0x00] && r[64] == 0x01 {
                        if let Some(tx) = ready_tx.take() {
                            let _ = tx.send(Err(BridgeError::AuthFailed("radio déconnectée".into())));
                        }
                        break;
                    }
                }
                144 if r[..6] == [0x90, 0x00, 0x00, 0x00, 0x00, 0x00] && r[96] == 1 => {
                    // Succès : la radio a ouvert son serveur serial.
                    let remote = u32::from_be_bytes([r[8], r[9], r[10], r[11]]);
                    let local = u32::from_be_bytes([r[12], r[13], r[14], r[15]]);
                    ctrl.common.remote_sid.store(remote, Ordering::Relaxed);
                    ctrl.common.local_sid.store(local, Ordering::Relaxed);
                    ctrl.auth_id.lock().unwrap().copy_from_slice(&r[26..32]);
                    if let Some(tx) = ready_tx.take() {
                        let _ = tx.send(Ok(()));
                    }
                }
                _ => {}
            }

            // Quand auth validée + a8replyID reçu : demander serial+audio (une fois).
            if auth_ok && !requested {
                if let Some(ref id) = a8 {
                    requested = true;
                    if let Err(e) = ctrl.send_request_serial_audio(id).await {
                        if let Some(tx) = ready_tx.take() {
                            let _ = tx.send(Err(e));
                        }
                        break;
                    }
                }
            }
        }
    })
}

/// Ré-authentification périodique (auth 0x05 toutes les 60 s).
fn spawn_reauth(ctrl: Arc<ControlStream>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(REAUTH_INTERVAL);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if ctrl.send_auth(0x05).await.is_err() {
                break;
            }
        }
    })
}

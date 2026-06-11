//! Stream serial RS-BA1 (port 50002) : transport des trames CI-V (TX/RX).
//!
//! Porté depuis kappanhang (`serialstream.go`) avec le re-send de l'open de
//! wfview (`startCivDataTimer`, voir spec §8). Les trames CI-V brutes
//! (`FE FE A4 E0 ... FD`) sont encapsulées telles quelles.

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use super::control::SERIAL_PORT;
use super::stream::{spawn_pkt0_idle, spawn_pkt7, spawn_reader, spawn_rx_retransmit, StreamCommon};
use crate::error::Result;

/// Cadence de renvoi de l'open tant qu'aucune trame CI-V n'est reçue (§8).
const OPEN_RESEND_INTERVAL: Duration = Duration::from_millis(100);
/// Watchdog : sans trame CI-V pendant ce délai, on relance le renvoi de l'open.
const CIV_WATCHDOG: Duration = Duration::from_secs(2);
/// Cadence de vérification du watchdog.
const WATCHDOG_CHECK: Duration = Duration::from_millis(500);

/// Stream serial ouvert, prêt à transporter du CI-V.
pub struct SerialStream {
    pub common: Arc<StreamCommon>,
    /// Numéro de séquence propre au stream serial (distinct du seq pkt0), big-endian.
    serial_seq: AtomicU16,
    /// Instant de la dernière trame CI-V reçue (pilote le re-send de l'open).
    last_civ_rx: StdMutex<Option<Instant>>,
}

impl SerialStream {
    /// Envoie une trame CI-V brute à la radio (paquet data 0xc1).
    pub async fn send_civ(&self, data: &[u8]) -> Result<()> {
        // Longueur CI-V : champ 2 octets little-endian (bytes 17-18, §12-F).
        let l = data.len() as u16;
        let seq = self.serial_seq.fetch_add(1, Ordering::Relaxed);
        let mut p = self.common.header(21 + data.len() as u32, 0x00, 0).to_vec();
        p.extend_from_slice(&[0xc1, l as u8, (l >> 8) as u8, (seq >> 8) as u8, seq as u8]);
        p.extend_from_slice(data);
        self.common.send_tracked(p).await
    }

    /// Ouvre (`false`) ou ferme (`true`) le canal serial côté radio (paquet 0x16).
    pub async fn send_open_close(&self, close: bool) -> Result<()> {
        let magic = if close { 0x00 } else { 0x05 };
        let seq = self.serial_seq.fetch_add(1, Ordering::Relaxed);
        let mut p = self.common.header(22, 0x00, 0).to_vec();
        p.extend_from_slice(&[0xc0, 0x01, 0x00, (seq >> 8) as u8, seq as u8, magic]);
        self.common.send_tracked(p).await
    }
}

/// Établit le stream serial et démarre la réception CI-V (diffusée sur `civ_tx`).
/// L'open du canal est géré par une tâche dédiée (renvoi 100 ms + watchdog 2 s).
pub async fn connect_serial(
    host: &str,
    civ_tx: broadcast::Sender<Vec<u8>>,
) -> Result<(Arc<SerialStream>, Vec<JoinHandle<()>>)> {
    let common = StreamCommon::connect("serial", host, SERIAL_PORT).await?;
    common.handshake().await?;

    let serial = Arc::new(SerialStream {
        common: common.clone(),
        serial_seq: AtomicU16::new(0),
        last_civ_rx: StdMutex::new(None),
    });

    let (data_tx, data_rx) = mpsc::unbounded_channel();
    let handles = vec![
        spawn_reader(common.clone(), data_tx),
        spawn_pkt7(common.clone(), 1),
        spawn_pkt0_idle(common.clone()),
        spawn_rx_retransmit(common.clone()),
        spawn_serial_owner(serial.clone(), data_rx, civ_tx),
        spawn_open_keeper(serial.clone()),
    ];

    Ok((serial, handles))
}

/// Extrait les trames CI-V des paquets data entrants et les diffuse.
fn spawn_serial_owner(
    serial: Arc<SerialStream>,
    mut data_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    civ_tx: broadcast::Sender<Vec<u8>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(r) = data_rx.recv().await {
            // Paquet data serial : type 0x00, marqueur 0xc1, longueur CI-V en
            // 2 octets little-endian (bytes 17-18, §12-F : un seul octet
            // tronquait les trames >= 256 o, ex. waveform scope ~497 o).
            if r.len() > 21 && r[4] == 0x00 && r[16] == 0xc1 {
                let l = u16::from_le_bytes([r[17], r[18]]) as usize;
                let end = (21 + l).min(r.len());
                if end > 21 {
                    *serial.last_civ_rx.lock().unwrap() = Some(Instant::now());
                    // Un receiver absent (capacité dépassée) n'est pas une erreur ici.
                    let _ = civ_tx.send(r[21..end].to_vec());
                }
            }
        }
    })
}

/// Gère l'ouverture du canal CI-V côté radio (wfview `startCivDataTimer`, §8) :
/// envoie l'open puis le renvoie toutes les 100 ms jusqu'à la première trame
/// CI-V reçue ; ensuite, si aucune trame n'arrive pendant 2 s, relance le renvoi.
fn spawn_open_keeper(serial: Arc<SerialStream>) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            // Phase d'ouverture : renvoi de l'open jusqu'à la 1re trame CI-V.
            let phase_start = Instant::now();
            loop {
                if serial.send_open_close(false).await.is_err() {
                    return;
                }
                tokio::time::sleep(OPEN_RESEND_INTERVAL).await;
                let opened = serial
                    .last_civ_rx
                    .lock()
                    .unwrap()
                    .is_some_and(|t| t >= phase_start);
                if opened {
                    break;
                }
            }
            // Phase de surveillance : watchdog sur le flux CI-V entrant.
            loop {
                tokio::time::sleep(WATCHDOG_CHECK).await;
                let stale = serial
                    .last_civ_rx
                    .lock()
                    .unwrap()
                    .is_none_or(|t| t.elapsed() > CIV_WATCHDOG);
                if stale {
                    break; // retour en phase d'ouverture
                }
            }
        }
    })
}

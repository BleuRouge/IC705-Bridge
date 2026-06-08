//! Stream serial RS-BA1 (port 50002) : transport des trames CI-V (TX/RX).
//!
//! Porté depuis kappanhang (`serialstream.go`). Les trames CI-V brutes
//! (`FE FE A4 E0 ... FD`) sont encapsulées telles quelles.

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;

use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use super::control::SERIAL_PORT;
use super::stream::{spawn_pkt0_idle, spawn_pkt7, spawn_reader, StreamCommon};
use crate::error::Result;

/// Stream serial ouvert, prêt à transporter du CI-V.
pub struct SerialStream {
    pub common: Arc<StreamCommon>,
    /// Numéro de séquence propre au stream serial (distinct du seq pkt0), big-endian.
    serial_seq: AtomicU16,
}

impl SerialStream {
    /// Envoie une trame CI-V brute à la radio (paquet data 0xc1).
    pub async fn send_civ(&self, data: &[u8]) -> Result<()> {
        let l = data.len() as u8;
        let seq = self.serial_seq.fetch_add(1, Ordering::Relaxed);
        let mut p = self.common.header(21 + data.len() as u32, 0x00, 0).to_vec();
        p.extend_from_slice(&[0xc1, l, 0x00, (seq >> 8) as u8, seq as u8]);
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
pub async fn connect_serial(
    host: &str,
    civ_tx: broadcast::Sender<Vec<u8>>,
) -> Result<(Arc<SerialStream>, Vec<JoinHandle<()>>)> {
    let common = StreamCommon::connect("serial", host, SERIAL_PORT).await?;
    common.handshake().await?;

    let serial = Arc::new(SerialStream {
        common: common.clone(),
        serial_seq: AtomicU16::new(0),
    });

    let (data_tx, data_rx) = mpsc::unbounded_channel();
    let handles = vec![
        spawn_reader(common.clone(), data_tx),
        spawn_pkt7(common.clone(), 1),
        spawn_pkt0_idle(common.clone()),
        spawn_serial_owner(data_rx, civ_tx),
    ];

    serial.send_open_close(false).await?; // ouverture du canal

    Ok((serial, handles))
}

/// Extrait les trames CI-V des paquets data entrants et les diffuse.
fn spawn_serial_owner(
    mut data_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    civ_tx: broadcast::Sender<Vec<u8>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(r) = data_rx.recv().await {
            // Paquet data serial : len>=22, marqueur 0xc1, longueur cohérente.
            if r.len() >= 22 && r[16] == 0xc1 && r[0].wrapping_sub(0x15) == r[17] {
                let l = r[17] as usize;
                let end = (21 + l).min(r.len());
                if end > 21 {
                    // Un receiver absent (capacité dépassée) n'est pas une erreur ici.
                    let _ = civ_tx.send(r[21..end].to_vec());
                }
            }
        }
    })
}

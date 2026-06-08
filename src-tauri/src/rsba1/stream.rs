//! Machinerie commune à chaque session UDP RS-BA1 (control et serial).
//!
//! Porté depuis kappanhang (`streamcommon.go`, `pkt0.go`, `pkt7.go`).
//! Chaque [`StreamCommon`] possède son propre socket, ses `localSID`/`remoteSID`
//! et ses boucles de keepalive (pkt0 idle + pkt7 ping) avec gestion de la
//! retransmission demandée par la radio.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::error::{BridgeError, Result};

/// Délai d'attente d'une réponse pendant le handshake.
const EXPECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Intervalle d'envoi des pings pkt7.
const PKT7_INTERVAL: Duration = Duration::from_secs(3);
/// Intervalle d'envoi des idle pkt0 après activité récente.
const PKT0_ACTIVE_INTERVAL: Duration = Duration::from_millis(100);
/// Intervalle d'envoi des idle pkt0 au repos.
const PKT0_IDLE_INTERVAL: Duration = Duration::from_secs(1);
/// Au-delà de ce délai sans paquet "tracked", on repasse en cadence repos.
const PKT0_IDLE_AFTER: Duration = Duration::from_secs(1);
/// Taille max du buffer de retransmission (≈ 300 ms d'historique).
const TX_BUF_MAX: usize = 256;

/// État du sous-protocole pkt0 (idle keepalive + retransmission).
struct Pkt0State {
    send_seq: u16,
    tx_buf: VecDeque<(u16, Vec<u8>)>,
    last_tracked_at: Instant,
}

/// État du sous-protocole pkt7 (ping/keepalive).
struct Pkt7State {
    send_seq: u16,
    inner_seq: u16,
}

/// Une session UDP RS-BA1 (un stream).
pub struct StreamCommon {
    pub name: &'static str,
    socket: Arc<UdpSocket>,
    pub local_sid: AtomicU32,
    pub remote_sid: AtomicU32,
    got_remote_sid: AtomicBool,
    pkt0: Mutex<Pkt0State>,
    pkt7: Mutex<Pkt7State>,
}

impl StreamCommon {
    /// Crée le socket, se connecte à `host:port` et calcule le `localSID`.
    pub async fn connect(name: &'static str, host: &str, port: u16) -> Result<Arc<Self>> {
        let remote: SocketAddr = format!("{host}:{port}")
            .parse()
            .map_err(|_| BridgeError::Protocol(format!("adresse invalide : {host}:{port}")))?;

        // kappanhang lie le port local au même numéro que le stream. On essaie,
        // puis on retombe sur un port éphémère si déjà utilisé (reconnexion).
        let socket = match UdpSocket::bind(("0.0.0.0", port)).await {
            Ok(s) => s,
            Err(_) => UdpSocket::bind(("0.0.0.0", 0)).await?,
        };
        socket.connect(remote).await?;

        let local_sid = compute_local_sid(socket.local_addr()?);

        Ok(Arc::new(Self {
            name,
            socket: Arc::new(socket),
            local_sid: AtomicU32::new(local_sid),
            remote_sid: AtomicU32::new(0),
            got_remote_sid: AtomicBool::new(false),
            pkt0: Mutex::new(Pkt0State {
                send_seq: 1,
                tx_buf: VecDeque::new(),
                last_tracked_at: Instant::now(),
            }),
            pkt7: Mutex::new(Pkt7State {
                send_seq: 0,
                inner_seq: 0x8304,
            }),
        }))
    }

    fn local_sid(&self) -> u32 {
        self.local_sid.load(Ordering::Relaxed)
    }

    fn remote_sid(&self) -> u32 {
        self.remote_sid.load(Ordering::Relaxed)
    }

    /// 16 octets d'en-tête : `[len LE u32][type][00][seq LE u16][localSID BE][remoteSID BE]`.
    pub fn header(&self, len: u32, typ: u8, seq: u16) -> [u8; 16] {
        let ls = self.local_sid().to_be_bytes();
        let rs = self.remote_sid().to_be_bytes();
        [
            len as u8,
            (len >> 8) as u8,
            (len >> 16) as u8,
            (len >> 24) as u8,
            typ,
            0x00,
            seq as u8,
            (seq >> 8) as u8,
            ls[0], ls[1], ls[2], ls[3],
            rs[0], rs[1], rs[2], rs[3],
        ]
    }

    /// Envoi brut (non bufferisé) sur le socket.
    pub async fn send_raw(&self, d: &[u8]) -> Result<()> {
        self.socket.send(d).await?;
        Ok(())
    }

    /// Envoi d'un paquet "tracked" : on lui attribue le prochain seq pkt0,
    /// on le bufferise pour la retransmission, puis on l'émet.
    pub async fn send_tracked(&self, mut packet: Vec<u8>) -> Result<()> {
        let mut st = self.pkt0.lock().await;
        let seq = st.send_seq;
        packet[6] = seq as u8;
        packet[7] = (seq >> 8) as u8;
        st.tx_buf.push_back((seq, packet.clone()));
        if st.tx_buf.len() > TX_BUF_MAX {
            st.tx_buf.pop_front();
        }
        st.send_seq = st.send_seq.wrapping_add(1);
        let is_idle = packet.len() == 16 && packet[4] == 0x00;
        if !is_idle {
            st.last_tracked_at = Instant::now();
        }
        self.socket.send(&packet).await?;
        Ok(())
    }

    /// Envoi d'un idle pkt0 (tracked ou non, avec un seq imposé si non tracked).
    async fn send_idle(&self, tracked: bool, seq_if_untracked: u16) -> Result<()> {
        let p = self.header(16, 0x00, seq_if_untracked).to_vec();
        if tracked {
            self.send_tracked(p).await
        } else {
            // Retransmission d'un seq précis : pas de réécriture du seq.
            self.send_raw(&p).await
        }
    }

    // --- Handshake commun (pkt3 -> pkt4 -> pkt6 -> pkt6 answer) ---

    /// Effectue le handshake d'ouverture de session.
    pub async fn handshake(&self) -> Result<()> {
        self.send_pkt3().await?;
        self.wait_pkt4().await?;
        self.send_pkt6().await?;
        self.wait_pkt6_answer().await?;
        Ok(())
    }

    async fn send_pkt3(&self) -> Result<()> {
        let p = self.header(16, 0x03, 0).to_vec();
        self.send_raw(&p).await?;
        self.send_raw(&p).await
    }

    async fn wait_pkt4(&self) -> Result<()> {
        let r = self
            .recv_matching(EXPECT_TIMEOUT, 16, &[0x10, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00])
            .await
            .ok_or_else(|| {
                BridgeError::Timeout(format!(
                    "{}/pkt4 : la radio ne répond pas (vérifier IP / RS-BA1 activé)",
                    self.name
                ))
            })?;
        let rsid = u32::from_be_bytes([r[8], r[9], r[10], r[11]]);
        self.remote_sid.store(rsid, Ordering::Relaxed);
        self.got_remote_sid.store(true, Ordering::Relaxed);
        Ok(())
    }

    async fn send_pkt6(&self) -> Result<()> {
        let mut p = self.header(16, 0x06, 0).to_vec();
        p[6] = 0x01; // {0x06, 0x00, 0x01, 0x00}
        self.send_raw(&p).await?;
        self.send_raw(&p).await
    }

    async fn wait_pkt6_answer(&self) -> Result<()> {
        self.recv_matching(EXPECT_TIMEOUT, 16, &[0x10, 0x00, 0x00, 0x00, 0x06, 0x00, 0x01, 0x00])
            .await
            .ok_or_else(|| BridgeError::Timeout(format!("{}/pkt6 answer", self.name)))?;
        Ok(())
    }

    /// Attend (en lecture directe sur le socket) un paquet précis. À n'utiliser
    /// qu'AVANT le démarrage du reader (sinon les paquets sont consommés ailleurs).
    pub async fn expect(&self, timeout: Duration, len: usize, prefix: &[u8]) -> Option<Vec<u8>> {
        self.recv_matching(timeout, len, prefix).await
    }

    /// Reçoit en boucle jusqu'à trouver un paquet de longueur `len` commençant par
    /// `prefix`, ou `None` après `timeout`. À n'utiliser qu'avant le démarrage du reader.
    async fn recv_matching(&self, timeout: Duration, len: usize, prefix: &[u8]) -> Option<Vec<u8>> {
        let deadline = Instant::now() + timeout;
        let mut buf = [0u8; 1500];
        loop {
            let remaining = deadline.checked_duration_since(Instant::now())?;
            match tokio::time::timeout(remaining, self.socket.recv(&mut buf)).await {
                Ok(Ok(n)) => {
                    let r = &buf[..n];
                    if r.len() == len && r.len() >= prefix.len() && &r[..prefix.len()] == prefix {
                        return Some(r.to_vec());
                    }
                    // Sinon : paquet non attendu (ping, etc.) -> on continue.
                }
                _ => return None,
            }
        }
    }

    // --- pkt7 (ping) ---

    fn is_pkt7(r: &[u8]) -> bool {
        r.len() == 21 && r[1..6] == [0x00, 0x00, 0x00, 0x07, 0x00]
    }

    /// Traite un pkt7 entrant : répond aux requêtes de la radio, ignore les acks.
    async fn handle_pkt7(&self, r: &[u8]) -> Result<()> {
        if r[16] == 0x00 {
            // Requête de la radio -> on renvoie le même replyID avec le flag 0x01.
            let seq = u16::from_le_bytes([r[6], r[7]]);
            let reply_id = [r[17], r[18], r[19], r[20]];
            self.send_pkt7(Some(reply_id), seq).await?;
        }
        // Sinon : réponse à notre ping (mesure de latence ignorée ici).
        Ok(())
    }

    /// Émet un pkt7. `reply_id = None` => ping de notre côté, sinon réponse.
    async fn send_pkt7(&self, reply_id: Option<[u8; 4]>, seq: u16) -> Result<()> {
        let (reply_flag, id) = match reply_id {
            Some(id) => (0x01u8, id),
            None => {
                let mut st = self.pkt7.lock().await;
                let inner = st.inner_seq;
                st.inner_seq = st.inner_seq.wrapping_add(1);
                (0x00u8, [rand::random::<u8>(), inner as u8, (inner >> 8) as u8, 0x06])
            }
        };
        let mut d = self.header(21, 0x07, seq).to_vec();
        d.extend_from_slice(&[reply_flag, id[0], id[1], id[2], id[3]]);
        self.send_raw(&d).await
    }

    async fn send_ping(&self) -> Result<()> {
        let seq = {
            let mut st = self.pkt7.lock().await;
            let s = st.send_seq;
            st.send_seq = st.send_seq.wrapping_add(1);
            s
        };
        self.send_pkt7(None, seq).await
    }

    // --- pkt0 retransmission (requêtes de la radio) ---

    fn is_pkt0_retransmit(r: &[u8]) -> bool {
        r.len() >= 16
            && (r[..6] == [0x10, 0x00, 0x00, 0x00, 0x01, 0x00]
                || r[..6] == [0x18, 0x00, 0x00, 0x00, 0x01, 0x00])
    }

    async fn handle_pkt0_retransmit(&self, r: &[u8]) -> Result<()> {
        if r[..6] == [0x10, 0x00, 0x00, 0x00, 0x01, 0x00] {
            let seq = u16::from_le_bytes([r[6], r[7]]);
            self.retransmit_one(seq).await?;
        } else if r[..6] == [0x18, 0x00, 0x00, 0x00, 0x01, 0x00] {
            let mut body = &r[16..];
            while body.len() >= 4 {
                let start = u16::from_le_bytes([body[0], body[1]]);
                let end = u16::from_le_bytes([body[2], body[3]]);
                let mut s = start;
                loop {
                    self.retransmit_one(s).await?;
                    if s == end {
                        break;
                    }
                    s = s.wrapping_add(1);
                }
                body = &body[4..];
            }
        }
        Ok(())
    }

    async fn retransmit_one(&self, seq: u16) -> Result<()> {
        let found = {
            let st = self.pkt0.lock().await;
            st.tx_buf.iter().find(|(s, _)| *s == seq).map(|(_, d)| d.clone())
        };
        match found {
            Some(d) => {
                self.send_raw(&d).await?;
                self.send_raw(&d).await?;
            }
            None => {
                // Paquet introuvable : on renvoie un idle untracked avec ce seq.
                self.send_idle(false, seq).await?;
                self.send_idle(false, seq).await?;
            }
        }
        Ok(())
    }

    /// Envoi du paquet de déconnexion (`type 0x05`, ×2).
    pub async fn send_disconnect(&self) -> Result<()> {
        let p = self.header(16, 0x05, 0).to_vec();
        self.send_raw(&p).await?;
        self.send_raw(&p).await
    }
}

/// Calcule le `localSID` : `(IP_BE << 16) | (port & 0xffff)` (wrap u32).
fn compute_local_sid(addr: SocketAddr) -> u32 {
    let ip_u32 = match addr {
        SocketAddr::V4(v4) => u32::from_be_bytes(v4.ip().octets()),
        SocketAddr::V6(v6) => {
            let o = v6.ip().octets();
            u32::from_be_bytes([o[12], o[13], o[14], o[15]])
        }
    };
    (ip_u32 << 16) | (addr.port() as u32 & 0xffff)
}

/// Démarre la boucle de lecture : gère pkt7 et les retransmissions en interne,
/// et transmet tous les autres paquets (idle + data) sur `data_tx`.
pub fn spawn_reader(common: Arc<StreamCommon>, data_tx: mpsc::UnboundedSender<Vec<u8>>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut buf = [0u8; 1500];
        loop {
            let n = match common.socket.recv(&mut buf).await {
                Ok(n) => n,
                Err(_) => break,
            };
            let r = buf[..n].to_vec();
            if StreamCommon::is_pkt7(&r) {
                let _ = common.handle_pkt7(&r).await;
                continue;
            }
            if StreamCommon::is_pkt0_retransmit(&r) {
                let _ = common.handle_pkt0_retransmit(&r).await;
                continue;
            }
            if data_tx.send(r).is_err() {
                break;
            }
        }
    })
}

/// Démarre l'émission périodique des pings pkt7.
pub fn spawn_pkt7(common: Arc<StreamCommon>, first_seq: u16) -> JoinHandle<()> {
    tokio::spawn(async move {
        {
            let mut st = common.pkt7.lock().await;
            st.send_seq = first_seq;
        }
        let mut ticker = tokio::time::interval(PKT7_INTERVAL);
        ticker.tick().await; // consomme le tick immédiat
        loop {
            ticker.tick().await;
            if common.send_ping().await.is_err() {
                break;
            }
        }
    })
}

/// Démarre l'émission périodique des idle pkt0 (cadence rapide après activité).
pub fn spawn_pkt0_idle(common: Arc<StreamCommon>) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let interval = {
                let st = common.pkt0.lock().await;
                if st.last_tracked_at.elapsed() >= PKT0_IDLE_AFTER {
                    PKT0_IDLE_INTERVAL
                } else {
                    PKT0_ACTIVE_INTERVAL
                }
            };
            tokio::time::sleep(interval).await;
            if common.send_idle(true, 0).await.is_err() {
                break;
            }
        }
    })
}

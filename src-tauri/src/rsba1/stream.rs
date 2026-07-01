//! Machinerie commune à chaque session UDP RS-BA1 (control et serial).
//!
//! Porté depuis kappanhang (`streamcommon.go`, `pkt0.go`, `pkt7.go`).
//! Chaque [`StreamCommon`] possède son propre socket, ses `localSID`/`remoteSID`
//! et ses boucles de keepalive (pkt0 idle + pkt7 ping) avec gestion de la
//! retransmission demandée par la radio.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
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
/// Période d'envoi groupé des demandes de retransmission RX (façon wfview).
const RX_RETRANSMIT_INTERVAL: Duration = Duration::from_millis(100);
/// Nombre max de demandes de retransmission par seq manquant.
const RX_MAX_ATTEMPTS: u8 = 4;
/// Au-delà de ce nombre de seq manquants, on abandonne tout (flush).
const RX_MISSING_FLUSH: usize = 50;

/// État du sous-protocole pkt0 (idle keepalive + retransmission).
struct Pkt0State {
    send_seq: u16,
    tx_buf: VecDeque<(u16, Vec<u8>)>,
    last_tracked_at: Instant,
    last_send_at: Instant,
}

/// Suivi des seq reçus (type 0x00) : dédoublonnage + détection des trous.
struct RxState {
    last_seq: Option<u16>,
    /// seq manquants → nombre de demandes de retransmission déjà émises.
    missing: HashMap<u16, u8>,
}

impl RxState {
    /// Enregistre un seq reçu. Renvoie `true` si le paquet doit être livré,
    /// `false` si c'est un duplicata (retransmission déjà vue).
    fn on_seq(&mut self, seq: u16) -> bool {
        let Some(last) = self.last_seq else {
            self.last_seq = Some(seq);
            return true;
        };
        let diff = seq.wrapping_sub(last);
        if diff == 0 {
            return false; // duplicata du dernier paquet livré
        }
        if diff < 0x8000 {
            // Paquet "en avant" : les seq intermédiaires sont manquants.
            if diff > 1 {
                if (diff - 1) as usize > RX_MISSING_FLUSH {
                    // Trou trop grand : on resynchronise sans rien réclamer.
                    self.missing.clear();
                } else {
                    for k in 1..diff {
                        self.missing.entry(last.wrapping_add(k)).or_insert(0);
                    }
                    if self.missing.len() > RX_MISSING_FLUSH {
                        self.missing.clear();
                    }
                }
            }
            self.last_seq = Some(seq);
            true
        } else {
            // Paquet "en arrière" : retransmission attendue, ou duplicata.
            self.missing.remove(&seq).is_some()
        }
    }
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
    rx: StdMutex<RxState>,
    /// Instant du dernier paquet reçu, quel qu'il soit (pings inclus) : sert au
    /// watchdog de perte de lien. En fonctionnement normal la radio émet en
    /// continu (réponses pkt7, idles) — un long silence = lien mort.
    last_rx_at: StdMutex<Instant>,
}

impl StreamCommon {
    /// Crée le socket, se connecte à `host:port` et calcule le `localSID`.
    /// `host` accepte une IP (`192.168.1.200`) ou un nom résolvable (`ic705.local`).
    pub async fn connect(name: &'static str, host: &str, port: u16) -> Result<Arc<Self>> {
        let remote: SocketAddr = tokio::net::lookup_host((host, port))
            .await
            .map_err(|_| {
                BridgeError::Protocol(format!("hôte introuvable : « {host} » (vérifier l'IP ou le nom)"))
            })?
            .next()
            .ok_or_else(|| {
                BridgeError::Protocol(format!("hôte introuvable : « {host} »"))
            })?;

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
                last_send_at: Instant::now(),
            }),
            pkt7: Mutex::new(Pkt7State {
                send_seq: 0,
                inner_seq: 0x8304,
            }),
            rx: StdMutex::new(RxState {
                last_seq: None,
                missing: HashMap::new(),
            }),
            last_rx_at: StdMutex::new(Instant::now()),
        }))
    }

    /// Durée écoulée depuis le dernier paquet reçu de la radio.
    pub fn silent_for(&self) -> Duration {
        self.last_rx_at.lock().unwrap().elapsed()
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
        // Réarme le timer idle après CHAQUE envoi suivi (§6.1 / §12-C) ; les
        // envois non-idle maintiennent en plus la cadence "active" (100 ms).
        st.last_send_at = Instant::now();
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
    pub async fn expect(&self, timeout: Duration, min_len: usize, prefix: &[u8]) -> Option<Vec<u8>> {
        self.recv_matching(timeout, min_len, prefix).await
    }

    /// Reçoit en boucle jusqu'à trouver un paquet d'au moins `min_len` octets
    /// commençant par `prefix`, ou `None` après `timeout`. À n'utiliser qu'avant
    /// le démarrage du reader.
    async fn recv_matching(&self, timeout: Duration, min_len: usize, prefix: &[u8]) -> Option<Vec<u8>> {
        let deadline = Instant::now() + timeout;
        let mut buf = [0u8; 1500];
        loop {
            let remaining = deadline.checked_duration_since(Instant::now())?;
            match tokio::time::timeout(remaining, self.socket.recv(&mut buf)).await {
                Ok(Ok(n)) => {
                    let r = &buf[..n];
                    if r.len() >= min_len && r.len() >= prefix.len() && &r[..prefix.len()] == prefix {
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
                // Borne la plage : on ne bufferise que TX_BUF_MAX paquets, une
                // plage plus large est forcément invalide (paquet corrompu ou
                // forgé — sans borne, `start > end` ferait émettre ~131 000
                // datagrammes d'un coup).
                if end.wrapping_sub(start) as usize >= TX_BUF_MAX {
                    body = &body[4..];
                    continue;
                }
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

    /// Suivi RX (§6.3) : enregistre le seq d'un paquet type 0x00 reçu.
    /// Renvoie `true` si le paquet doit être livré, `false` si duplicata.
    fn on_rx_seq(&self, seq: u16) -> bool {
        self.rx.lock().unwrap().on_seq(seq)
    }

    /// Envoi du paquet de déconnexion (`type 0x05`, ×2).
    pub async fn send_disconnect(&self) -> Result<()> {
        let p = self.header(16, 0x05, 0).to_vec();
        self.send_raw(&p).await?;
        self.send_raw(&p).await
    }
}

/// Garde RAII sur des tâches de fond : si elle est droppée sans `disarm()`,
/// elle **abandonne** (`abort`) toutes les tâches qu'elle détient.
///
/// Indispensable côté connexion : un `JoinHandle` simplement droppé ne tue PAS
/// la tâche (tokio la détache). Sans cette garde, un échec de connexion après le
/// démarrage des boucles de keepalive laissait ces boucles tourner indéfiniment
/// — la radio voyait alors une session toujours active et refusait les
/// reconnexions suivantes.
pub struct TaskGuard {
    handles: Vec<JoinHandle<()>>,
}

impl TaskGuard {
    pub fn new(handles: Vec<JoinHandle<()>>) -> Self {
        Self { handles }
    }

    pub fn push(&mut self, handle: JoinHandle<()>) {
        self.handles.push(handle);
    }

    /// Désarme la garde et restitue les handles (cas succès : les tâches vivent).
    pub fn disarm(mut self) -> Vec<JoinHandle<()>> {
        std::mem::take(&mut self.handles)
    }
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        for h in &self.handles {
            h.abort();
        }
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
/// déduplique les paquets type 0x00 et transmet le reste (idle + data) sur `data_tx`.
pub fn spawn_reader(common: Arc<StreamCommon>, data_tx: mpsc::UnboundedSender<Vec<u8>>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut buf = [0u8; 1500];
        loop {
            let n = match common.socket.recv(&mut buf).await {
                Ok(n) => n,
                Err(_) => break,
            };
            *common.last_rx_at.lock().unwrap() = Instant::now();
            let r = buf[..n].to_vec();
            if StreamCommon::is_pkt7(&r) {
                let _ = common.handle_pkt7(&r).await;
                continue;
            }
            if StreamCommon::is_pkt0_retransmit(&r) {
                let _ = common.handle_pkt0_retransmit(&r).await;
                continue;
            }
            // Suivi des paquets "tracked" de la radio (type 0x00, seq != 0) :
            // écarte les duplicatas, note les trous pour retransmission.
            if r.len() >= 16 && r[4] == 0x00 && r[5] == 0x00 {
                let seq = u16::from_le_bytes([r[6], r[7]]);
                if seq != 0 && !common.on_rx_seq(seq) {
                    continue;
                }
            }
            if data_tx.send(r).is_err() {
                break;
            }
        }
    })
}

/// Démarre la boucle de demandes de retransmission RX groupées (§6.3, façon
/// wfview) : toutes les 100 ms, réclame les seq manquants, max 4 essais chacun.
pub fn spawn_rx_retransmit(common: Arc<StreamCommon>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(RX_RETRANSMIT_INTERVAL);
        ticker.tick().await; // consomme le tick immédiat
        loop {
            ticker.tick().await;
            let due: Vec<u16> = {
                let mut st = common.rx.lock().unwrap();
                let mut due = Vec::new();
                st.missing.retain(|&seq, attempts| {
                    *attempts += 1;
                    if *attempts > RX_MAX_ATTEMPTS {
                        false // on abandonne ce seq
                    } else {
                        due.push(seq);
                        true
                    }
                });
                due
            };
            for seq in due {
                // Demande simple : `10 00 00 00 01 00 [seq LE] [SIDs]`.
                let p = common.header(16, 0x01, seq).to_vec();
                if common.send_raw(&p).await.is_err() {
                    return;
                }
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

/// Démarre l'émission périodique des idle pkt0. Le timer est réarmé par chaque
/// paquet suivi (§6.1) : un idle ne part que si rien n'a été émis depuis
/// l'intervalle courant (100 ms en activité, 1 s au repos).
pub fn spawn_pkt0_idle(common: Arc<StreamCommon>) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let (last_send, active) = {
                let st = common.pkt0.lock().await;
                (st.last_send_at, st.last_tracked_at.elapsed() < PKT0_IDLE_AFTER)
            };
            let interval = if active { PKT0_ACTIVE_INTERVAL } else { PKT0_IDLE_INTERVAL };
            let due = last_send + interval;
            let now = Instant::now();
            if now >= due {
                if common.send_idle(true, 0).await.is_err() {
                    break;
                }
            } else {
                tokio::time::sleep(due - now).await;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> RxState {
        RxState { last_seq: None, missing: HashMap::new() }
    }

    #[test]
    fn rx_delivers_in_order_and_drops_duplicates() {
        let mut rx = fresh();
        assert!(rx.on_seq(10));
        assert!(rx.on_seq(11));
        assert!(!rx.on_seq(11)); // duplicata du dernier
        assert!(rx.on_seq(12));
        assert!(rx.missing.is_empty());
    }

    #[test]
    fn rx_tracks_gaps_and_accepts_retransmission_once() {
        let mut rx = fresh();
        assert!(rx.on_seq(10));
        assert!(rx.on_seq(13)); // trou : 11 et 12 manquants
        assert_eq!(rx.missing.len(), 2);
        assert!(rx.missing.contains_key(&11));
        assert!(rx.missing.contains_key(&12));
        assert!(rx.on_seq(11)); // retransmission attendue -> livrée
        assert!(!rx.on_seq(11)); // re-duplicata -> écarté
        assert_eq!(rx.missing.len(), 1);
    }

    #[test]
    fn rx_flushes_on_huge_gap() {
        let mut rx = fresh();
        assert!(rx.on_seq(10));
        assert!(rx.on_seq(11));
        assert!(rx.on_seq(1000)); // trou > flush : resynchronisation
        assert!(rx.missing.is_empty());
        assert_eq!(rx.last_seq, Some(1000));
    }

    #[test]
    fn rx_handles_u16_wraparound() {
        let mut rx = fresh();
        assert!(rx.on_seq(0xFFFE));
        assert!(rx.on_seq(0x0001)); // wrap : 0xFFFF et 0x0000 manquants
        assert!(rx.missing.contains_key(&0xFFFF));
        assert!(rx.missing.contains_key(&0x0000));
    }

    #[tokio::test]
    async fn task_guard_aborts_on_drop() {
        let ran = Arc::new(AtomicBool::new(false));
        let r = ran.clone();
        let guard = TaskGuard::new(vec![tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            r.store(true, Ordering::SeqCst);
        })]);
        drop(guard); // doit abandonner la tâche avant qu'elle ne s'exécute
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert!(!ran.load(Ordering::SeqCst), "la tâche aurait dû être abandonnée");
    }

    #[tokio::test]
    async fn task_guard_disarm_keeps_tasks() {
        let ran = Arc::new(AtomicBool::new(false));
        let r = ran.clone();
        let guard = TaskGuard::new(vec![tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            r.store(true, Ordering::SeqCst);
        })]);
        for h in guard.disarm() {
            let _ = h.await; // désarmée : la tâche survit et termine
        }
        assert!(ran.load(Ordering::SeqCst), "la tâche désarmée aurait dû s'exécuter");
    }
}

# Protocole réseau RS-BA1 (IC-705) — spec d'implémentation

Spec dérivée de **kappanhang** (Go, MIT) : https://github.com/nonoo/kappanhang
Fichiers de référence (clonés dans `reference/kappanhang/`, hors git) :
`streamcommon.go`, `pkt0.go`, `pkt7.go`, `controlstream.go`, `serialstream.go`, `passcode.go`.

L'IC-705 expose le protocole RS-BA1 sur 3 ports UDP :

| Port  | Stream         | Usage                                   |
|-------|----------------|-----------------------------------------|
| 50001 | control        | handshake, login, auth, négociation     |
| 50002 | serial / CI-V  | trames CI-V (TX/RX)                      |
| 50003 | audio          | audio (non utilisé par IC705 Bridge)    |

Chaque stream est une **session UDP indépendante** : socket dédié, handshake propre,
`localSID`/`remoteSID` propres, boucles keepalive propres.

---

## 1. En-tête commun des paquets de contrôle (16 octets)

| Offset | Taille | Champ        | Endian | Notes                                       |
|--------|--------|--------------|--------|---------------------------------------------|
| 0      | 4      | length       | LE u32 | longueur totale du paquet                   |
| 4      | 1      | type         | u8     | 0x00 idle, 0x01 retransmit, 0x03/0x04/0x06 handshake, 0x05 disconnect, 0x07 ping |
| 5      | 1      | (0x00)       |        |                                             |
| 6      | 2      | seq          | LE u16 | seq de transport (pkt0)                      |
| 8      | 4      | localSID     | BE u32 | "sent id" (notre session)                    |
| 12     | 4      | remoteSID    | BE u32 | "rcvd id" (session radio)                     |

`localSID` = `(u32_BE(ip_locale[4 derniers octets]) << 16) | (port_local & 0xffff)` (wrap u32).
Recalculé pour chaque socket. **Écrasé** par la radio à l'ouverture serial+audio
(reply 0x90 : `remoteSID = r[8:12]`, `localSID = r[12:16]`).

---

## 2. Handshake commun (`streamCommon.start`, pour CHAQUE stream)

1. **pkt3** (×2) — `10 00 00 00 03 00 00 00 [localSID] [remoteSID]` (remoteSID=0 au début)
2. **attendre pkt4** — len 16, type 0x04. Capture `remoteSID = r[8:12]` (BE).
3. **pkt6** (×2) — `10 00 00 00 06 00 01 00 [localSID] [remoteSID]`
4. **attendre pkt6 answer** — len 16, `10 00 00 00 06 00 01 00`.

Timeout d'attente : 1 s.

---

## 3. pkt7 — ping / keepalive (len 21)

Détection : `len==21 && r[1:6]==00 00 00 07 00` (byte0 = 0x15 ou 0x00).
- bytes 6-7 : seq (LE u16)
- byte 16 : 0x00 = requête radio→nous (on répond) ; sinon = réponse à notre ping
- bytes 17-20 : replyID

Envoi (`sendDo`) — len 21 :
```
15 00 00 00 07 00 [seqLo seqHi] [localSID] [remoteSID] [replyFlag] [replyID0..3]
```
- Notre ping : replyFlag=0x00, replyID = `[rand, innerSeqLo, innerSeqHi, 0x06]`, innerSeq++ (init 0x8304).
- Réponse à la radio : replyFlag=0x01, replyID = octets reçus `r[17:21]`, seq = seq reçu.

Intervalle d'envoi : 3 s. control stream démarre pkt7 à seq=2 ; serial à seq=1.

---

## 4. pkt0 — idle keepalive + retransmission (len 16, ou data)

Détection idle : `len==16 && r[:6]==10 00 00 00 00 00`.
- Requête retransmit (radio→nous) : `r[:6]==10 00 00 00 01 00`, seq=`r[6:8]` LE → renvoyer le paquet bufferisé, ou idle avec ce seq si absent.
- Requête retransmit par plages : `r[:6]==18 00 00 00 01 00`, puis 4 octets/plage (start LE, end LE).

Idle envoyé (`sendIdle`) :
```
10 00 00 00 00 00 [seqLo seqHi] [localSID] [remoteSID]
```
Intervalle : 100 ms après activité, 1 s en idle. `sendSeq` init = 1.

**txSeqBuf** : tout paquet "tracked" (login, auth, data, idle tracked) est bufferisé par
seq (300 ms) pour répondre aux retransmit. `sendTrackedPacket` écrit le seq pkt0 dans
les bytes 6-7 (LE) AVANT envoi, puis incrémente.

---

## 5. Stream control (port 50001) — login & auth

Après handshake :
1. `pkt0.init` (sendSeq=1).
2. **Login** (paquet 0x80, len 128) — `sendTrackedPacket` :
   ```
   80 00 00 00 00 00 00 00 [localSID] [remoteSID]
   00 00 00 70 01 00 00 [innerSeqLo innerSeqHi] 00 [authStartID0 authStartID1] 00 00 00 00
   <32 octets 0x00>
   [username encodé 16o @offset 64]
   [password encodé 16o @offset 80]
   69 63 6f 6d 2d 70 63 00   ("icom-pc\0" @offset 96)
   <24 octets 0x00>
   ```
   `authStartID` = 2 octets aléatoires. `innerSeq` (authInnerSendSeq) incrémenté.
3. **Attendre login answer** : len 96, `60 00 00 00 00 00 01 00`.
   Si `r[48:52]==ff ff ff fe` → identifiants invalides.
   `authID = r[26:32]` (6 octets).
4. `pkt7.startPeriodicSend(seq=2)`.
5. `sendPktAuth(0x02)` puis `pkt0.startPeriodicSend` puis `sendPktAuth(0x05)`.

**sendPktAuth(magic)** (paquet 0x40, len 64) :
```
40 00 00 00 00 00 00 00 [localSID] [remoteSID]
00 00 00 30 01 [magic] 00 [innerSeqLo innerSeqHi] 00 [authID 6o @offset 26]
<32 octets 0x00>
```
magic : 0x02 = 1er auth, 0x05 = 2e auth / réauth périodique (60 s), 0x01 = deauth (déconnexion).

**Réponses control** (dans la boucle de lecture) :
- len 64 `40..` : auth ok ; si `r[21]==0x05` → `authOk=true` → demander serial+audio.
- len 80 `50..` : `r[48:51]==ff ff ff` → auth failed ; `r[48:51]==00 00 00 && r[64]==0x01` → radio déconnectée.
- len 168 `a8..` : capture `a8replyID = r[66:82]` (16 octets).
- len 144 `90..` & `r[96]==1` : **succès serial+audio**. Re-lire SIDs et authID, `devName = r[64:]` (string), puis init des streams serial (et audio).

**Demande serial+audio** (`sendRequestSerialAndAudio`, paquet 0x90, len 144),
envoyée quand `authOk && gotA8ReplyID` :
```
90 00 00 00 00 00 00 00 [localSID] [remoteSID]
00 00 00 80 01 03 00 [innerSeqLo innerSeqHi] 00 [authID 6o]
[a8replyID 16o]
<16 octets 0x00>
49 43 2d 37 30 35 00 00   ("IC-705\0\0")
<24 octets 0x00>
[username encodé 16o]
01 01 04 04 00 00 [rxRate_hi rxRate_lo] 00 00 [txRate_hi txRate_lo]
00 00 [serialPort_hi serialPort_lo] 00 00 [audioPort_hi audioPort_lo] 00 00
[txSeqBufLenMs_hi lo] 01 00 00 00 00 00 00 00
```
audioSampleRate=48000, serialPort=50002, audioPort=50003, txSeqBufLen=300ms.
→ **IC705 Bridge** envoie cette demande à l'identique mais n'ouvre QUE le stream serial
(le stream audio 50003 est ignoré ; l'audio entrant n'est pas traité).

Réauth : `sendPktAuth(0x05)` toutes les 60 s.

---

## 6. Stream serial (port 50002) — CI-V

Init : handshake commun, puis `pkt7.startPeriodicSend(seq=1)`, `pkt0.init`+periodic,
puis **sendOpenClose(open)**.

**Open/Close** (paquet 0x16, len 22) :
```
16 00 00 00 00 00 00 00 [localSID] [remoteSID]
c0 01 00 [serialSeqHi serialSeqLo] [magic]
```
magic 0x05 = open, 0x00 = close. serialSeq (BE !) ++.

**Envoi CI-V** (`send`, len 21+l, l = longueur trame) — `sendTrackedPacket` :
```
[0x15+l] 00 00 00 00 00 00 00 [localSID] [remoteSID]
c1 [l] 00 [serialSeqHi serialSeqLo] [data...]
```
⚠ deux seq : bytes 6-7 = seq pkt0 (LE, posé par sendTrackedPacket) ;
bytes 19-20 = serialSeq (BE). data CI-V à l'offset 21.

**Réception CI-V** : paquet data si `len>=22 && r[16]==0xc1 && r[0]-0x15==r[17]`.
Trame CI-V = `r[21:]` (longueur `r[17]`). seq de transport = `r[6:8]` (LE).
Les trames CI-V (ex. `FE FE A4 E0 03 ... FD`) sont transmises telles quelles.

---

## 7. Encodage username/password (`passcode`)

```
res[16]
pour i de 0 à min(len(s),16)-1 :
    p = s[i] + i
    si p > 126 : p = 32 + (p % 127)
    res[i] = TABLE[p]      // TABLE indexée de 32 à 126 (voir passcode.rs)
```
La table de substitution (32→0x47, 33→0x5d, … 126→0x52) est reproduite dans
`src-tauri/src/rsba1/passcode.rs`.

---

## 8. Déconnexion

- serial : `sendOpenClose(close)` puis disconnect (`10 00 00 00 05 00 ...` ×2).
- control : `sendPktAuth(0x01)` (deauth), attendre ~500 ms, puis disconnect.
- Arrêter les boucles pkt0/pkt7, fermer les sockets.

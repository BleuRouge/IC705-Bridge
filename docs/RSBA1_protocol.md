# Protocole réseau RS-BA1 (IC-705) — spec d'implémentation canonique

> **Statut : source de vérité unique** pour le transport réseau IC705 Bridge.
> Ce document consolide et remplace les anciennes notes réseau
> (`ic705-network-protocol.md`, `ic705-network-protocol-analysis.md`, supprimés).
> Il décrit le protocole **et** l'état actuel de l'implémentation (§12-13).

Spec dérivée par **rétro-ingénierie d'interopérabilité** de deux implémentations
indépendantes (aucun code copié) :
- **kappanhang** (Go, GPL) : https://github.com/nonoo/kappanhang —
  `streamcommon.go`, `pkt0.go`, `pkt7.go`, `controlstream.go`, `serialstream.go`,
  `seqbuf.go`, `passcode.go`.
- **wfview** (C++/Qt, GPLv3) : `packettypes.h`, `icomudpbase.cpp`,
  `icomudphandler.cpp`, `icomudpcivdata.cpp`.

Croiser les deux est précieux : là où l'un est cryptique, l'autre clarifie, et
les **divergences** révèlent ce qui est toléré vs strict côté radio. Le code
IC705 Bridge **réimplémente** d'après cette compréhension ; les valeurs marquées
« à confirmer » doivent être validées sur capture / IC-705 réel.

---

## 0. Pré-requis radio (une fois)

`MENU > SET > WLAN SET` → Station ou Access Point ;
`Remote Settings > Network Control = ON` ; `Network User1` (ID + password,
Admin = YES) ; `CI-V > CI-V Transceive = ON`. Port de contrôle = 50001.

L'IC-705 expose le protocole RS-BA1 sur **3 ports UDP indépendants** :

| Port  | Stream         | Login ? | Usage                                   |
|-------|----------------|---------|-----------------------------------------|
| 50001 | control        | **oui** (passcode) | handshake, login, token, autorise les autres flux |
| 50002 | serial / CI-V  | non     | trames CI-V `FE FE … FD` (TX/RX)        |
| 50003 | audio          | non     | audio (hors périmètre IC705 Bridge)     |

Chaque stream est une **session UDP indépendante** : socket dédié, handshake
propre, `localSID`/`remoteSID` propres, compteurs `seq` propres, boucles
keepalive propres. Seul le **control** fait le login passcode ; les flux
serial/audio sont **autorisés par le control** via le paquet conninfo `0x90`.

---

## 1. En-tête commun des paquets (16 octets)

| Offset | Taille | Champ        | Endian | Notes                                       |
|--------|--------|--------------|--------|---------------------------------------------|
| 0      | 4      | length       | LE u32 | longueur totale du paquet                   |
| 4      | 2      | type         | LE u16 | 0x00 idle/data/login-famille, 0x01 retransmit, 0x03 are-you-there, 0x04 i-am-here, 0x05 disconnect, 0x06 are-you-ready/ready, 0x07 ping |
| 6      | 2      | seq          | LE u16 | n° de séquence (selon le compteur, voir §5) |
| 8      | 4      | localSID     | BE u32 | "sent id" (notre session)                   |
| 12     | 4      | remoteSID    | BE u32 | "rcvd id" (session radio)                    |

`localSID` : kappanhang/wfview le **dérivent de l'IP+port locaux**
(`(ip>>8&0xff)<<24 | (ip&0xff)<<16 | (localPort&0xffff)`). IC705 Bridge suit ce
calcul à partir de l'adresse locale de chaque socket. La radio confirme `remoteSID` à l'ouverture
serial+audio (reply 0x90 : `remoteSID = r[8:12]`, `localSID = r[12:16]`).

---

## 2. Handshake commun (`streamCommon.start`, pour CHAQUE stream)

1. **pkt3** (×2) — are-you-there : `10 00 00 00 03 00 00 00 [localSID] [remoteSID=0]`
2. **attendre pkt4** — len 16, type 0x04 (i-am-here). Capture `remoteSID = r[8:12]` (BE).
3. **pkt6** (×2) — are-you-ready : `10 00 00 00 06 00 01 00 [localSID] [remoteSID]`
4. **attendre pkt6 answer** — len 16, `10 00 00 00 06 00 01 00` (ready).

Les paquets « importants » sont émis deux fois. Timeout d'attente : ~1 s,
répété jusqu'à réponse. ✅ Handshake **validé sur IC-705 réel**.

---

## 3. Catalogue des paquets (longueur → rôle)

D'après `packettypes.h` (wfview) — les longueurs nomment les paquets :

| Len  | Nom             | Rôle |
|------|-----------------|------|
| 0x10 | control         | are-you-there/here/ready, disconnect, **idle**, retransmit simple |
| 0x15 | ping (pkt7) **et** en-tête CI-V data | latence / trame data CI-V |
| 0x16 | openclose       | ouvre/ferme le canal data (CI-V/audio) |
| 0x18 | retransmit_range| demande de retransmission par plages |
| 0x40 | token (auth)    | demande/renouvellement de token |
| 0x50 | status          | statut de login |
| 0x60 | login_response  | réponse au login (token à 0x1a..0x20) |
| 0x80 | login           | identifiants (passcode) |
| 0x90 | conninfo        | **autorise serial+audio** (ports, codecs, capabilities) |
| 0xA8 | capabilities    | la radio annonce ses capacités + un reply id |

---

## 4. pkt7 — ping / keepalive (len 21)

Détection : `len==21 && r[4:6]==07 00`.
- bytes 6-7 : seq (LE u16) — **compteur pkt7 propre** (voir §5).
- byte 16 : 0x00 = requête radio→nous (on répond) ; 0x01 = réponse à notre ping.
- bytes 17-20 : replyID.

Envoi (`sendDo`) — len 21 :
```
15 00 00 00 07 00 [seqLo seqHi] [localSID] [remoteSID] [replyFlag] [replyID0..3]
```
- **Notre ping** : `replyFlag=0x00`, `replyID = [rand, innerSeqLo, innerSeqHi, 0x06]`,
  `innerSendSeq++` (init `0x8304`).
- **Réponse à la radio** : `replyFlag=0x01`, `replyID = r[17:21]` (échoté), `seq` reçu.

Intervalle d'envoi : **3 s** (kappanhang ; wfview = 500 ms, les deux marchent).
`pkt7.startPeriodicSend(firstSeqNo)` : **control démarre à seq=2**, **serial à seq=1**.

---

## 5. ⚠️ MODÈLE DE SÉQUENCE — point critique (corrigé à la source)

**Il y a DEUX compteurs `seq` indépendants par flux**, tous deux écrits dans les
bytes 6-7 de l'en-tête selon le type de paquet. C'est le point que les anciennes
notes confondaient ; **vérifié dans kappanhang `pkt0.go` / `pkt7.go`** :

| Compteur            | Init | Incrémenté par | Paquets concernés |
|---------------------|------|----------------|-------------------|
| **pkt0 `sendSeq`**  | 1    | `sendTrackedPacket` | idle *tracked*, login, auth, conninfo, **CI-V data**, **open** |
| **pkt7 `sendSeq`**  | control 2 / serial 1 | `pkt7.send` | **ping uniquement** |
| **pkt7 `innerSendSeq`** | 0x8304 | nouvelle requête ping | champ replyID interne |
| **serial `sendseq`** (data, bytes 19-20, **BE**) | 0 | open + chaque data | ordre des données CI-V |

IC705 Bridge maintient ces compteurs séparément dans `StreamCommon` : les pings
ne créent donc aucun trou artificiel dans la séquence pkt0.

`sendTrackedPacket` : pose `seq` (LE) dans bytes 6-7 **avant** envoi, stocke le
paquet dans `txSeqBuf` (clé = seq), puis `sendSeq++`. Départ du compteur : wfview
**0**, kappanhang **1** — la radio est tolérante.

---

## 6. pkt0 — idle keepalive + couche de fiabilité (len 16, ou data)

### 6.1 Idle

Détection idle : `len==16 && r[4:6]==00 00`.
```
10 00 00 00 00 00 [seqLo seqHi] [localSID] [remoteSID]
```
Intervalle : **100 ms**, **réarmé** à chaque paquet *tracked* envoyé (évite des
idles superflus qui gonflent le seq). En vrai idle, kappanhang relâche à ~1 s.

IC705 Bridge réarme le timer après chaque paquet suivi, puis passe d'une cadence
active de 100 ms à une cadence de repos de 1 s.

### 6.2 La radio nous réclame une retransmission (type 0x01) — *côté TX*

Deux formats :
- **simple** (16 o) : `r[4:6]==01 00`, seq voulu en bytes 6-7 (LE).
- **plage** (`18 …`, `r[4:6]==01 00`) : paires `(start,end)` LE depuis l'offset 16.

Réponse : renvoyer le paquet depuis `txSeqBuf` (×2) ; **si absent → envoyer un
IDLE estampillé avec ce seq** pour combler le trou (sinon la radio attend ce seq
pour toujours et **bloque tout le CI-V**). ✅ IC705 Bridge effectue ce gap-fill.

### 6.3 NOUS réclamons une retransmission (`rxSeqBuf`) — *côté RX*

On suit le seq des paquets **reçus** (idle + data, type 0x00, seq≠0). Sur trou :
- **wfview** : accumule les seq manquants dans `rxMissing`, un **timer 100 ms**
  (`RETRANSMIT_PERIOD`) envoie les demandes **groupées**, **max 4 essais/seq**,
  **flush si > 50 manquants**. → jamais de tempête.
- **kappanhang** : `rxSeqBuf` avec **délai de réordonnancement 100 ms** + cap
  `maxRetransmitRequestPacketCount = 10`.

IC705 Bridge suit l'approche wfview : collecte, timer 100 ms, quatre essais au
maximum et abandon lorsque plus de 50 séquences manquent.

---

## 7. Stream control (port 50001) — login & token

Après handshake : `pkt0.init` (sendSeq=1), puis :

1. **Login** (paquet 0x80, len 128) via `sendTrackedPacket` :
   ```
   80 00 00 00 00 00 [seq] [localSID] [remoteSID]
   00 00 00 70 01 00 00 [innerSeqLo innerSeqHi] 00 [authStartID0 authStartID1] 00 00 00 00
   <32 octets 0x00>
   [passcode(username) 16o @64]
   [passcode(password) 16o @80]
   [connection_name ASCII @96, ex. "icom-pc\0"]
   <reste 0x00>
   ```
   `authStartID` = 2 octets aléatoires ; `innerSeq` (authInnerSendSeq) incrémenté.
2. **Attendre login answer** (`0x60`, len ≥ 96) : token à `r[26:32]` (6 o).
   `r[48:52] == FF FF FF FE` → identifiants invalides.
3. `sendPktAuth(0x02)` (token request), puis keepalive (pkt0 100 ms + pkt7 3 s),
   puis `sendPktAuth(0x05)`. **Réauth `0x05` toutes les 60 s**.

**sendPktAuth(magic)** (paquet 0x40, len 64) via `sendTrackedPacket` :
```
40 00 00 00 00 00 [seq] [localSID] [remoteSID]
00 00 00 30 01 [magic] 00 [innerSeqLo innerSeqHi] 00 [token 6o @26]
<32 octets 0x00>
```
magic : `0x02` = 1er token, `0x05` = renew / réauth (60 s), `0x01` = deauth.

**Autorisation serial+audio** :
4. La radio envoie un **`0xa8`** (len ≥ 168, commence par `A8 00 00 00 00 00`).
   **`a8replyID = r[66:82]`** (16 o). Y figurent aussi `commoncap`, modèle, rates.
5. Control → **conninfo `0x90`** (len 144) via `sendTrackedPacket` :
   ```
   90 00 00 00 00 00 [seq] [localSID] [remoteSID]
   00 00 00 80 01 03 00 [innerSeqLo innerSeqHi] 00 [token 6o]
   [a8replyID 16o @32]
   <16 octets 0x00>
   49 43 2d 37 30 35 00 00   ("IC-705\0\0" @64 — chaîne modèle, voir §13 multi-radio)
   <24 octets 0x00>
   [passcode(username) 16o @96]
   01 01 04 04 00 00 [rxRate] 00 00 [txRate] 00 00 [serialPort=50002] 00 00 [audioPort=50003] 00 00
   [txSeqBufLenMs=300] 01 00 00 00 00 00 00 00
   ```
6. Réponse `0x90` (len 144) avec `r[96]==1` → **succès serial+audio**.

IC705 Bridge envoie le `a8replyID`, le username, les ports serial/audio et les
paramètres de codec/tampon attendus par la radio.

**Réponses control utiles** (boucle de lecture) :
- len 64 `40..` : auth ok ; `r[21]==0x05` → `authOk` → demander serial+audio.
- len 80 `50..` : `r[48:51]==FF FF FF` → auth failed ; `00 00 00 && r[64]==1` → radio déconnectée.
- len 168 `a8..` : capture `a8replyID = r[66:82]`.
- len 144 `90..` & `r[96]==1` : succès serial+audio ; relire SIDs/token, `devName = r[64:]`.

✅ Control (handshake + login + token) **validé fiable sur IC-705 réel**.

---

## 8. Stream serial (port 50002) — tunnel CI-V

Init : handshake commun, **PAS de login**, puis `pkt7.startPeriodicSend(seq=1)`,
`pkt0.init` + periodic, puis **open**.

**Open/Close** (paquet 0x16, len 22) via `sendTrackedPacket` :
```
16 00 00 00 00 00 [seq] [localSID] [remoteSID]
c0 01 00 [sendseqHi sendseqLo] [magic]
```
- bytes 16-18 = `c0 01 00` ; bytes 19-20 = `sendseq` (**BE**, compteur serial) ;
  byte 21 = magic : **`0x05` = open** (kappanhang), `0x00` = close.
  ⚠ wfview utilise `0x04` pour l'open → à tester si l'open `0x05` pose problème.

**🔑 RE-ENVOI DE L'OPEN** (wfview `startCivDataTimer`) :
- envoyer l'open dès le ready, puis le **renvoyer toutes les 100 ms** ;
- **stopper dès la 1re trame CI-V data reçue** ;
- **watchdog** : si aucune trame CI-V pendant **2 s**, relancer le renvoi.
- ✅ IC705 Bridge effectue ce re-send dans `spawn_open_keeper` avec son watchdog.

**Envoi CI-V** (`send`, len 21+l) via `sendTrackedPacket` :
```
[0x15+l] 00 00 00 00 00 [seq] [localSID] [remoteSID]
c1 [l] 00 [sendseqHi sendseqLo] [trame CI-V…]
```
⚠ **deux seq** : bytes 6-7 = seq pkt0 (LE, posé par sendTrackedPacket) ;
bytes 19-20 = sendseq serial (BE). Trame CI-V à l'offset 21.

**Réception CI-V** : paquet data si `len>21 && r[16]==0xc1`. Longueur trame =
`u16::from_le_bytes(r[17:19])`, trame à partir de `r[21]`, seq de transport =
`r[6:8]` (LE).
La radio renvoie l'**écho** de notre trame (to/from inversés) **puis** la vraie
réponse ; `Session::send_civ` écarte l'écho et corrèle la réponse attendue.

✅ Tunnel CI-V bidirectionnel obtenu sur IC-705 réel. Les corrections de
fiabilité décrites au §12 doivent être revalidées avec le smoke test à chaque
release destinée à une démonstration.

---

## 9. Encodage username/password (`passcode`)

```
res[16]                                  # pré-rempli de zéros
pour i de 0 à min(len(s),16)-1 :
    p = s[i] + i                         # s[i] = code ASCII
    si p > 126 : p = 32 + (p % 127)
    res[i] = SEQUENCE[p]                 # table indexée 32..126
```

Table `SEQUENCE` (index ASCII → octet), 95 entrées :
```
32:47 33:5d 34:4c 35:42 36:66 37:20 38:23 39:46 40:4e 41:57 42:45 43:3d
44:67 45:76 46:60 47:41 48:62 49:39 50:59 51:2d 52:68 53:7e 54:7c 55:65
56:7d 57:49 58:29 59:72 60:73 61:78 62:21 63:6e 64:5a 65:5e 66:4a 67:3e
68:71 69:2c 70:2a 71:54 72:3c 73:3a 74:63 75:4f 76:43 77:75 78:27 79:79
80:5b 81:35 82:70 83:48 84:6b 85:56 86:6f 87:34 88:32 89:6c 90:30 91:61
92:6d 93:7b 94:2f 95:4b 96:64 97:38 98:2b 99:2e 100:50 101:40 102:3f 103:55
104:33 105:37 106:25 107:77 108:24 109:26 110:74 111:6a 112:28 113:53 114:4d
115:69 116:22 117:5c 118:44 119:31 120:36 121:58 122:3b 123:7a 124:51 125:5f
126:52
```
Vecteur de test : `passcode("beer")` = `2B 3F 55 5C 00 …` (b=98→2B, e i1→3F,
e i2→55, r i3→5C). Reproduit dans le code + testé unitairement. ✅ validé radio.

---

## 10. Déconnexion

- **serial** : `open/close(close, magic 0x00)` puis disconnect (`10 00 00 00 05 00 …` ×2).
- **control** : `sendPktAuth(0x01)` (deauth), attendre ~500 ms, puis disconnect ×2.
- Arrêter les boucles pkt0/pkt7, fermer les sockets.

---

## 11. Tableau de corrélation (divergences qui comptent)

| Brique | wfview | kappanhang | IC705 Bridge | État |
|---|---|---|---|---|
| Compteur seq pkt7 | propre | **propre** (control 2 / serial 1) | propre, séparé de pkt0 | ✅ |
| Retransmit RX | batch 100 ms, max 4, flush 50 | délai 100 ms, cap 10 | batch 100 ms, max 4, flush 50 | ✅ |
| Idle reset | oui | oui | 100 ms actif, 1 s au repos, timer réarmé | ✅ |
| Conninfo `0x90` | ports+codecs+user | user + replyid | replyid+user+ports+codecs | ✅ |
| Open re-send | boucle 100 ms + watchdog 2 s | 1 seule fois | boucle 100 ms + watchdog 2 s | ✅ |
| Open magic (byte 21) | 0x04 | 0x05 | 0x05 | validé par l'implémentation retenue |
| Ping | 500 ms | 3 s | 3 s | ✅ |
| Gap-fill TX | oui | oui | oui | ✅ |
| localSID | dérivé IP+port | dérivé IP+port | dérivé IP+port | ✅ |
| Login passcode | oui | oui | oui | ✅ |
| Handshake | oui | oui | oui | ✅ |

---

## 12. État de fiabilisation et limites restantes

Les anomalies initialement observées sur IC-705 réel (compteur pkt7 partagé,
tempête de retransmissions et idle non réarmé) sont corrigées dans
`rsba1/stream.rs`. La fermeture normale, Cmd+Q et l'installation d'une mise à
jour envoient aussi le deauth/disconnect afin de ne pas laisser la radio occupée.

La couche requête/réponse CI-V :

- sérialise les transactions provenant du terminal et de l'API ;
- écarte l'écho et les trames spontanées dont l'adresse, la commande ou la
  sous-commande connue diffère ;
- accepte les acquittements `FB`/`FA` ;
- renvoie un timeout explicite si aucune réponse corrélée n'arrive sous 1,2 s.

Limites connues :

- les paquets serial retransmis sont dédupliqués mais pas remis en ordre strict ;
- la corrélation porte sur les adresses, la commande et les sous-commandes
  connues, le protocole CI-V ne fournissant pas d'identifiant de transaction ;
- le format du scope `27 00` doit rester couvert par un essai matériel. Le
  transport lit correctement sa longueur sur deux octets et diffuse la trame
  complète vers le frontend et `/stream`. `python/monitor.py` effectue ensuite
  l'interprétation du sweep.

---

## 13. Architecture actuelle du module `rsba1/`

```
src-tauri/src/rsba1/
├── mod.rs       # déclaration des sous-modules
├── passcode.rs  # table SEQUENCE + encodage username/password
├── stream.rs    # socket commun, handshake, séquences, keepalives, retransmissions
├── control.rs   # port 50001 : login, token, 0xa8 et conninfo 0x90
└── serial.rs    # port 50002 : open/watchdog et encapsulation CI-V
```

`src-tauri/src/session.rs` orchestre les deux streams, corrèle les réponses et
diffuse les trames vers Tauri et l'API. Les tests Rust couvrent les fonctions
pures ainsi qu'une session complète sur radio UDP simulée. Le smoke test
`scripts/verify_demo.py --live` complète cette couverture sur matériel réel.

---

## 14. Licence / légal

Protocole propriétaire non publié, compris par **rétro-ingénierie
d'interopérabilité** (usage radioamateur). wfview = GPLv3, kappanhang = GPL :
**aucun code copié** dans IC705 Bridge. La table `SEQUENCE`, l'algorithme
`passcode` et les layouts sont des données/algorithmes d'interopérabilité,
réimplémentés en Rust. À assumer comme usage personnel.

## Sources consultées

- nonoo/kappanhang (Go) — `pkt0.go`, `pkt7.go`, `streamcommon.go`,
  `controlstream.go`, `serialstream.go`, `seqbuf.go`, `passcode.go`.
- wfview (eliggett, C++/Qt) — `packettypes.h`, `icomudpbase.cpp`,
  `icomudphandler.cpp`, `icomudpcivdata.cpp`.
- microenh/NetworkIcom (Swift) — `Packet/`, `UDP/`.
- Icom, `IC-705 CI-V Reference Guide` (juillet 2020).
- Guides WLAN IC-705 (VK3FS, M0IAX) — ports 50001/50002/50003.

# Protocole réseau RS-BA1 (IC-705) — spec d'implémentation canonique

> **Statut : source de vérité unique** pour le transport réseau RadioForge.
> Ce document consolide et remplace les anciennes notes réseau
> (`ic705-network-protocol.md`, `ic705-network-protocol-analysis.md`, supprimés).
> Il décrit le protocole **et** l'état de l'implémentation, avec la liste des
> écarts à corriger (§12) et le plan du module `rsba1/` propre (§13).

Spec dérivée par **rétro-ingénierie d'interopérabilité** de deux implémentations
indépendantes (aucun code copié) :
- **kappanhang** (Go, GPL) : https://github.com/nonoo/kappanhang —
  `streamcommon.go`, `pkt0.go`, `pkt7.go`, `controlstream.go`, `serialstream.go`,
  `seqbuf.go`, `passcode.go`.
- **wfview** (C++/Qt, GPLv3) : `packettypes.h`, `icomudpbase.cpp`,
  `icomudphandler.cpp`, `icomudpcivdata.cpp`.

Croiser les deux est précieux : là où l'un est cryptique, l'autre clarifie, et
les **divergences** révèlent ce qui est toléré vs strict côté radio. Le code
RadioForge **réimplémente** d'après cette compréhension ; les valeurs marquées
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
| 50003 | audio          | non     | audio (hors périmètre RadioForge)       |

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
(`(ip>>8&0xff)<<24 | (ip&0xff)<<16 | (localPort&0xffff)`). RadioForge utilise un
aléatoire u32 → **toléré** (la radio ne fait que le renvoyer en `remoteSID`).
Recalculé pour chaque socket. La radio confirme `remoteSID` à l'ouverture
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

> 🔴 **Le code actuel (`StreamSender.seq`) utilise UN SEUL compteur partagé** pour
> idle + ping + data. Conséquence : chaque ping (type 0x07) consomme un numéro
> dans la séquence pkt0 suivie par la radio pour la retransmission → la radio voit
> des « trous » et **fige la livraison CI-V après le 1er échange**. Suspect n°1 du
> blocage. **Fix : compteur pkt7 séparé** (voir §12-A).

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

> 🟠 Le code actuel envoie l'idle à 100 ms fixe **sans reset** → seq gonflé
> (aggrave §5). Fix : réarmer le timer idle après chaque paquet suivi (§12-C).

### 6.2 La radio nous réclame une retransmission (type 0x01) — *côté TX*

Deux formats :
- **simple** (16 o) : `r[4:6]==01 00`, seq voulu en bytes 6-7 (LE).
- **plage** (`18 …`, `r[4:6]==01 00`) : paires `(start,end)` LE depuis l'offset 16.

Réponse : renvoyer le paquet depuis `txSeqBuf` (×2) ; **si absent → envoyer un
IDLE estampillé avec ce seq** pour combler le trou (sinon la radio attend ce seq
pour toujours et **bloque tout le CI-V**). ✅ RadioForge fait déjà ce gap-fill.

### 6.3 NOUS réclamons une retransmission (`rxSeqBuf`) — *côté RX*

On suit le seq des paquets **reçus** (idle + data, type 0x00, seq≠0). Sur trou :
- **wfview** : accumule les seq manquants dans `rxMissing`, un **timer 100 ms**
  (`RETRANSMIT_PERIOD`) envoie les demandes **groupées**, **max 4 essais/seq**,
  **flush si > 50 manquants**. → jamais de tempête.
- **kappanhang** : `rxSeqBuf` avec **délai de réordonnancement 100 ms** + cap
  `maxRetransmitRequestPacketCount = 10`.

> 🟠 Le code actuel (`track_rx`) redemande **immédiatement** à chaque trou (cap
> diff ≤ 11), sans batch ni délai → **tempête ~150 paquets/s**. Fix : refaire
> façon wfview (collecte + timer 100 ms + max 4 + flush > 50) (§12-B).

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
   [connection_name ASCII @96, ex. "RadioForge\0"]
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

> 🟡 RadioForge n'envoie aujourd'hui que `username + a8replyID` dans le `0x90`
> (champs ports/codecs à zéro). wfview met `civport`/`audioport`/`commoncap` →
> **piste pour le `0xa8`/`0x90` intermittent** (§12-D).

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
- ✅ RadioForge fait déjà ce re-send + watchdog (`maybe_resend_open`).

**Envoi CI-V** (`send`, len 21+l) via `sendTrackedPacket` :
```
[0x15+l] 00 00 00 00 00 [seq] [localSID] [remoteSID]
c1 [l] 00 [sendseqHi sendseqLo] [trame CI-V…]
```
⚠ **deux seq** : bytes 6-7 = seq pkt0 (LE, posé par sendTrackedPacket) ;
bytes 19-20 = sendseq serial (BE). Trame CI-V à l'offset 21.

**Réception CI-V** : paquet data si `len>21 && r[16]==0xc1`. Longueur trame =
`r[17]`, trame = `r[21:21+r[17]]`, seq de transport = `r[6:8]` (LE).
La radio renvoie l'**écho** de notre trame (to/from inversés) **puis** la vraie
réponse ; le codec existant (`validate_response`) distingue déjà les deux.

✅ Tunnel CI-V bidirectionnel **obtenu sur IC-705 réel** (1er échange), mais
**instable** ensuite (cf. §12).

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

| Brique | wfview | kappanhang | RadioForge (actuel) | Verdict |
|---|---|---|---|---|
| Compteur seq pkt7 | propre | **propre** (control 2 / serial 1) | **partagé avec pkt0** ❌ | **CAUSE probable du fige CI-V** (§12-A) |
| Retransmit RX | batch 100 ms, max 4, flush 50 | rxSeqBuf délai 100 ms, cap 10 | **immédiat par trou** ❌ | **CAUSE de la tempête** (§12-B) |
| Idle reset | oui (100 ms réarmé) | oui | **non** ❌ | gonfle le seq (§12-C) |
| Conninfo `0x90` | civport+audioport+codecs+user | user + replyid | **user + replyid** | piste `0x90` intermittent (§12-D) |
| Open re-send | **boucle 100 ms + watchdog 2 s** | 1 seule fois | **boucle + watchdog** ✅ | OK |
| Open magic (byte 21) | 0x04 | 0x05 | 0x05 | tester 0x04 si besoin |
| Ping | 500 ms | 3 s | 3 s | OK |
| Gap-fill TX (idle au seq réclamé) | oui | oui | **oui** ✅ | OK |
| localSID | dérivé IP+port | dérivé IP+port | aléatoire | toléré ✅ |
| Login passcode | oui | oui | **oui** ✅ | OK (validé radio) |
| Handshake | oui | oui | **oui** ✅ | OK (validé radio) |

---

## 12. Écarts à corriger (fix-list, par priorité)

> Symptômes IC-705 réel : control fiable ✅ ; `0x90` intermittent ; CI-V → le
> **1er paquet data après l'open répond, les suivants non**, avec **tempête ~150 pkt/s**.

- **A 🔴 Compteur pkt7 séparé.** Donner au ping son propre `sendSeq` (control 2 /
  serial 1) + `innerSendSeq` (init 0x8304), distinct du `sendSeq` pkt0. Ne PAS
  passer le ping par `sendTrackedPacket`. → cible directe du fige CI-V.
- **B 🟠 Retransmit RX groupé.** Remplacer le redemande-immédiat de `track_rx` :
  collecter les seq manquants, timer 100 ms, max 4 essais/seq, flush si > 50.
- **C 🟠 Idle reset.** Réarmer le timer idle après chaque paquet suivi.
- **D 🟡 Conninfo enrichi.** Ajouter civport/audioport locaux + `commoncap` lu
  dans le `0xa8`, comme wfview, si le `0xa8`/`0x90` reste intermittent.
- **E 🟡 Détails layout.** Aligner replyID/innerSeq du ping (§4) et innerSeq du
  login (§7) sur la spec.
- **F ✅ Scope réseau (fait).** `set_scope_streaming` est câblé sur le transport
  réseau : un flag `streaming` partagé (`SerialChannel` ↔ `RsBa1Session`) pilote le
  worker serial, qui draine les trames `27 00`, les réassemble via le
  `ScopeAssembler` mutualisé de `protocols/civ/scope.rs`, et émet les events
  `scope-frame` (même pipeline que l'USB). `27 11` data-output est envoyé par la
  commande `ic705_set_scope_streaming`.
  - 🔴 **Bug corrigé (capture IC-705 réelle)** : la longueur CI-V du paquet data
    `0xc1` est un champ **2 octets little-endian** (bytes 17-18), pas 1 seul.
    `extract_civ_payload` ne lisait que `bytes[17]` → toute trame ≥ 256 o était
    tronquée à `len & 0xFF` (le waveform `27 00` de ~497 o arrivait coupé à 241 o,
    sans `FD` final → jamais parsé en scope, ré-émis en CI-V). Corrigé.
  - 📐 **Format scope RS-BA1 ≠ USB** : en réseau, l'IC-705 envoie **tout le sweep
    dans UNE trame `27 00` (total=01)** — en-tête info de 16 o
    (`sub, scope_id, seq, total, mode, center/edge 5B, span/edge 5B, oor`) suivi
    directement de ~475 échantillons — au lieu des 11 parties de l'USB.
    `try_parse_scope_waveform` extrait les échantillons après l'en-tête et
    `ScopeAssembler` émet le sweep directement quand `total ≤ 1`.

---

## 13. Plan du module `rsba1/` (réécriture propre)

Objectif : sortir le protocole RS-BA1 de `transports/network.rs` (1500 l.
monolithiques) vers un module dédié, testable couche par couche, sur lequel le
transport réseau s'appuie. **Réseau d'abord** : on fait marcher l'IC-705, puis on
généralise multi-radio (le champ modèle `"IC-705"` du `0x90` devient un paramètre).

```
src-tauri/src/rsba1/
├── mod.rs            # API publique : RsBa1Session (connect/disconnect/civ_io), re-exports
├── header.rs         # en-tête 16 o : encode/parse, types de paquet (§1)
├── passcode.rs       # table SEQUENCE + passcode() (§9) — déjà référencé
├── packets.rs        # builders/parsers purs : pkt3/4/6/7, login, auth, 0x90, open, data (§3-8)
├── seq.rs            # Pkt0Seq (txSeqBuf, gap-fill) + Pkt7Seq (ping) SÉPARÉS (§5-6)
├── reliability.rs    # rxSeqBuf : collecte trous + retransmit groupé 100 ms (§6.3)
├── control.rs        # flux 50001 : handshake → login → token → 0xa8 → 0x90 (§7)
├── serial.rs         # flux 50002 : handshake → open(+resend/watchdog) → tunnel CI-V (§8)
└── session.rs        # orchestration des deux flux + boucles keepalive (owner-thread)
```

Le transport `transports/network.rs` devient une **fine couche d'adaptation** :
implémente `CivLink` en déléguant à `rsba1::RsBa1Session`, expose
`write_and_read_civ_response` / `set_scope_streaming`. Tout le reste de l'app
(commandes, codec CI-V, scope) est réutilisé inchangé.

**Stratégie de validation** : chaque couche (`header`, `packets`, `passcode`,
`seq`, `reliability`) garde ses tests de layout déterministes (déjà ~70
aujourd'hui) ; l'intégration vivante (`control`/`serial`/`session`) est validée
sur IC-705 réel via l'event `network-log` (déjà en place).

---

## 14. Licence / légal

Protocole propriétaire non publié, compris par **rétro-ingénierie
d'interopérabilité** (usage radioamateur). wfview = GPLv3, kappanhang = GPL :
**aucun code copié** dans RadioForge. La table `SEQUENCE`, l'algorithme
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

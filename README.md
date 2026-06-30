# IC705 Bridge

**Passerelle desktop légère et cross-platform pour l'Icom IC-705.**

IC705 Bridge se connecte à l'IC-705 en Wi-Fi (protocole RS-BA1), expose un
**terminal CI-V intégré** pour envoyer/recevoir des trames manuellement, et une
**API HTTP locale** pour automatiser les échanges en Python.

Il remplace le workflow lourd
(*driver Icom + logiciel de connexion + port COM virtuel + HTerm + scripts série*)
par : **IC705 Bridge → connexion directe → terminal CI-V → API locale**.

> Outil **pédagogique** : il ne cache pas le protocole. L'étudiant lit la doc CI-V
> de l'IC-705, construit ses trames (`FE FE A4 E0 03 FD`), analyse les réponses,
> puis automatise en Python.

Voir [docs/IC705_Bridge_description.md](docs/IC705_Bridge_description.md) pour la
description fonctionnelle et [docs/RSBA1_protocol.md](docs/RSBA1_protocol.md) pour
la spec du protocole réseau.

---

## Architecture

```
Frontend React (onglets Connection + CI-V Terminal)
        │  commandes / événements Tauri
Backend Rust (Tauri v2)
        ├── cœur réseau RS-BA1 (tokio)
        │     ├── stream control (UDP 50001) : login, auth, négociation
        │     └── stream serial  (UDP 50002) : trames CI-V (TX/RX)
        └── API HTTP locale (axum, 127.0.0.1:8765) ──► lib Python
                                                          │
                                              IC-705 (Wi-Fi RS-BA1)
```

Le cœur RS-BA1 est porté depuis [kappanhang](https://github.com/nonoo/kappanhang)
(Go) et [wfview](https://gitlab.com/eliggett/wfview) (C++).

| Composant            | Emplacement                       |
|----------------------|-----------------------------------|
| Protocole RS-BA1     | `src-tauri/src/rsba1/`            |
| Orchestrateur        | `src-tauri/src/session.rs`        |
| Commandes Tauri      | `src-tauri/src/commands.rs`       |
| API HTTP locale      | `src-tauri/src/api.rs`            |
| Frontend             | `src/`                            |
| Librairie Python     | `python/ic705bridge.py`           |

---

## Téléchargement & installation

Les installeurs macOS / Windows / Linux sont publiés sur la page
[**Releases**](https://github.com/BleuRouge/IC705-Bridge/releases). Téléchargez
celui de votre système :

- **macOS** : `.dmg` (universel Intel + Apple Silicon)
- **Windows** : `.exe` (NSIS) ou `.msi`
- **Linux** : `.AppImage` ou `.deb`

Les binaires ne sont pas signés (déploiement pédagogique/intranet) : au premier
lancement, contournez l'avertissement une fois (macOS : clic droit → **Ouvrir** ;
Windows : SmartScreen → **Informations complémentaires** → **Exécuter quand
même**). Les **mises à jour suivantes sont automatiques** (l'app les propose au
démarrage). Détails et procédure de publication : [docs/RELEASING.md](docs/RELEASING.md).

---

## Développement

Pré-requis : [Rust](https://rustup.rs/), [Node.js](https://nodejs.org/) + `pnpm`,
et les [dépendances système Tauri](https://tauri.app/start/prerequisites/).

```bash
pnpm install
pnpm tauri dev      # lance l'app (frontend + backend) en mode dev
pnpm tauri build    # build de production
```

Tests / vérifications :

```bash
cd src-tauri && cargo test     # tests Rust (passcode, parsing hex)
pnpm build                     # type-check + build frontend
```

---

## Utilisation

1. Lancer l'application.
2. Onglet **Connection** : saisir l'IP, le username et le password RS-BA1 de
   l'IC-705, puis **Connect**. Les ports sont fixes (control 50001, CI-V 50002).
3. Onglet **CI-V Terminal** : saisir une trame hex et **Send** ; les réponses
   s'affichent horodatées.
4. **API locale** : une fois connecté, l'API tourne sur `http://127.0.0.1:8765`.

### API HTTP locale

| Méthode | Route     | Corps                         | Réponse                       |
|---------|-----------|-------------------------------|-------------------------------|
| `GET`   | `/status` | —                             | état de la connexion          |
| `POST`  | `/civ`    | `{"frame": "FE FE A4 E0 03 FD"}` | `{"tx": "...", "response": "..."}` |
| `GET`   | `/stream` | —                             | flux SSE des trames CI-V reçues (une trame hex/événement) |

> **Sécurité.** L'API n'écoute que sur la boucle locale, valide l'en-tête `Host`
> (anti-DNS-rebinding) et exige l'en-tête `X-IC705-Bridge` sur `/civ` et `/stream`.
> Une page web ne peut pas ajouter cet en-tête en cross-origin (préflight refusé,
> CORS absent), ce qui empêche un site malveillant d'envoyer des trames (PTT
> inclus). La librairie Python l'envoie automatiquement.

Un moniteur d'exemple ([`python/monitor.py`](python/monitor.py)) consomme
`/stream` pour afficher un **waterfall** du scope + les paramètres de la radio
(`pip install matplotlib numpy`).

### Librairie Python

Paquet installable (`pip install ./python` ou `uv add ./python`), sans dépendance
externe (stdlib uniquement).

```python
from ic705bridge import IC705Bridge, split_frames

rig = IC705Bridge()                 # http://127.0.0.1:8765 par défaut
print(rig.status())

rep = rig.send_civ("FE FE A4 E0 03 FD")
print("TX:", rep["tx"])
print("RX:", rep["response"])
for f in split_frames(rep["response"]):    # sépare écho + réponse radio
    print(f)

for frame in rig.stream_civ():      # flux CI-V temps réel (générateur)
    print(frame)
```

API bas niveau : `status()`, `send_civ(frame)` (hex ou octets), `is_ready()`,
`stream_civ()` ; utilitaires `split_frames()` / `to_hex()`. Catalogue des trames
STATUS/RX/TX dans [`python/COMMANDS.md`](python/COMMANDS.md), démo dans
[`python/example.py`](python/example.py), tests dans `python/tests/`.

---

## État du projet

- [x] Cœur réseau RS-BA1 (handshake, login/auth, keepalive, CI-V TX/RX)
- [x] Onglets Connection & CI-V Terminal
- [x] API HTTP locale (`/status`, `/civ`, `/stream` SSE) + garde locale (Host + en-tête)
- [x] Librairie Python packagée pip/uv (STATUS/RX/TX, `stream_civ()`, catalogue, tests)
- [ ] Test contre un IC-705 réel
- [ ] Corrélation réponse↔commande CI-V + erreur explicite au timeout
- [ ] Retransmission RX / réordonnancement des paquets serial

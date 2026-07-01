# ic705bridge

Client Python **bas niveau** pour [IC705 Bridge](https://github.com/BleuRouge/IC705-Bridge).

IC705 Bridge expose une API HTTP locale (`http://127.0.0.1:8765`) qui remplace le
port COM virtuel : on lui envoie des trames CI-V brutes et elle renvoie la réponse
de l'Icom IC-705. Cette librairie est un mince wrapper autour de cette API — elle
n'interprète pas les commandes CI-V (c'est à l'utilisateur de construire ses
trames). Aucune dépendance externe (stdlib uniquement).

## Installation

```bash
pip install ic705bridge          # depuis PyPI (recommandé)
```

Ou depuis les sources (dossier `python/` du dépôt) :

```bash
pip install .          # depuis le dossier python/
# ou
uv add ./python
```

## Usage

```python
from ic705bridge import IC705Bridge, split_frames

rig = IC705Bridge()                 # http://127.0.0.1:8765 par défaut
print(rig.status())                 # STATUS

rep = rig.send_civ("FE FE A4 E0 03 FD")   # lecture fréquence (RX)
print("TX:", rep["tx"])
print("RX:", rep["response"])
for f in split_frames(rep["response"]):    # sépare écho + réponse radio
    print(f)

for frame in rig.stream_civ():      # flux CI-V temps réel (générateur bloquant)
    print(frame)
```

## API

| Élément                     | Rôle                                                    |
|-----------------------------|---------------------------------------------------------|
| `IC705Bridge(url, timeout)` | client de l'API locale                                  |
| `.status()`                 | état de la connexion (dict)                             |
| `.send_civ(frame)`          | envoi d'une trame (hex ou octets) → `{tx, response}`   |
| `.is_ready()`               | `True` si la radio est prête                           |
| `.stream_civ(timeout=None)` | générateur des trames CI-V reçues (SSE)                |
| `split_frames(response)`    | découpe une réponse en trames `FE FE … FD`             |
| `to_hex(frame)`             | normalise hex/octets en `"FE FE A4 E0 03 FD"`          |
| `BridgeError`               | toute erreur réseau/protocole                          |

Catalogue des trames utiles (STATUS / RX / TX) : voir [COMMANDS.md](COMMANDS.md).
Démonstration complète : [example.py](example.py).

## Tests

```bash
pip install .[dev]
pytest
```

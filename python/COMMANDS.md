# Catalogue des trames CI-V utiles (IC-705)

> La librairie Python reste **bas niveau** : elle envoie des trames brutes via
> `send_civ(...)` et ne les interprète pas. Ce document liste les trames utiles
> pour la démo / le TP, regroupées en **STATUS / RX / TX**. À adapter selon la
> documentation CI-V officielle de l'IC-705.

## Convention

```
FE FE  A4   E0   <cmd> [sub] [data...]  FD
└─┬─┘  │    │
préamb radio ctrl
       0xA4 0xE0
```

- **A4** = adresse CI-V de l'IC-705 (défaut), **E0** = contrôleur (PC).
- Réponse de la radio : `FE FE E0 A4 <cmd> <data...> FD`.
- Accusé d'écriture : `FE FE E0 A4 FB FD` (OK) ou `FE FE E0 A4 FA FD` (NG).

```python
from ic705bridge import IC705Bridge, split_frames

rig = IC705Bridge()
rep = rig.send_civ("FE FE A4 E0 03 FD")
for f in split_frames(rep["response"]):   # sépare écho + réponse radio
    print(f)
```

---

## STATUS — état du pont (pas une trame CI-V)

| Appel                 | Rôle                                             |
|-----------------------|--------------------------------------------------|
| `rig.status()`        | état complet (`state`, `host`, `api_url`, …)     |
| `rig.is_ready()`      | `True` si `state == "civ_ready"` (radio prête)   |
| `rig.stream_civ()`    | générateur bloquant : *yield* chaque trame CI-V reçue |

---

## RX — lectures (la radio renvoie une valeur)

| Commande            | Trame                          | Réponse (payload)                     |
|---------------------|--------------------------------|---------------------------------------|
| Lire fréquence      | `FE FE A4 E0 03 FD`            | 5 octets BCD LE (Hz)                  |
| Lire mode           | `FE FE A4 E0 04 FD`            | `<mode> <filtre>`                     |
| Lire S-mètre        | `FE FE A4 E0 15 02 FD`         | `02` + 2 octets BCD (0000–0255)       |
| Lire statut PTT     | `FE FE A4 E0 1C 00 FD`         | `00` + `00`=RX / `01`=TX              |
| Lire VFO sélectionné| `FE FE A4 E0 07 FD`            | —                                     |

Codes de mode courants : `00`=LSB, `01`=USB, `02`=AM, `03`=CW, `04`=RTTY,
`05`=FM, `07`=CW-R, `08`=RTTY-R, `17`=DV.

---

## TX — écritures (réglages, émission)

| Commande                 | Trame                                   | Effet                       |
|--------------------------|-----------------------------------------|-----------------------------|
| Régler fréquence 145.5 MHz | `FE FE A4 E0 05 00 00 50 45 01 FD`    | BCD LE de 145 500 000 Hz    |
| Régler mode (USB)        | `FE FE A4 E0 06 01 FD`                  | mode = `01`                 |
| Régler mode + filtre     | `FE FE A4 E0 06 01 02 FD`               | USB, filtre 2               |
| **PTT ON** (émission RF) | `FE FE A4 E0 1C 00 01 FD`               | ⚠ met en émission           |
| **PTT OFF**              | `FE FE A4 E0 1C 00 00 FD`               | repasse en réception        |

> ⚠ **PTT ON fait réellement émettre la radio.** Ne l'utiliser qu'avec une
> antenne adaptée ou une charge fictive, et le plus brièvement possible.

### Encodage d'une fréquence (BCD little-endian, 5 octets)

`145 500 000 Hz` → chiffres `0 1 4 5 5 0 0 0 0 0` → octets `00 00 50 45 01`
(octet de poids faible d'abord ; dans chaque octet, le quartet bas est le
chiffre de poids faible). Voir `decode_bcd_freq` / l'inverse dans `example.py`.

"""Exemple d'automatisation CI-V via IC705 Bridge (Phase 2 du TP).

Pré-requis : lancer l'application IC705 Bridge et se connecter à l'IC-705
dans l'onglet Connection.

Les trames ci-dessous sont des exemples : à toi de construire les bonnes
trames à partir de la documentation CI-V de l'IC-705.
"""

from ic705bridge import IC705Bridge

rig = IC705Bridge()  # http://127.0.0.1:8765

if not rig.is_ready():
    print("⚠ L'IC-705 n'est pas connecté. Connecte-toi dans IC705 Bridge d'abord.")
    raise SystemExit(1)

# Lecture de la fréquence (commande 03)
rep = rig.send_civ("FE FE A4 E0 03 FD")
print("TX:", rep["tx"])
print("RX:", rep["response"])

# Autres exemples (à adapter selon le TP)
for frame in [
    "FE FE A4 E0 04 FD",  # lecture du mode
    "FE FE A4 E0 15 02 FD",  # lecture du S-mètre
]:
    rep = rig.send_civ(frame)
    print(f"{rep['tx']}  ->  {rep['response']}")

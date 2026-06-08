# IC705 Bridge — Description de l’application

**IC705 Bridge** est une application desktop légère et cross-platform destinée aux travaux pratiques autour de l’Icom IC-705.

Son objectif est de remplacer le workflow actuel complexe :

```txt
driver Icom
+ logiciel Icom de connexion
+ port COM virtuel
+ HTerm
+ scripts Python sur port série
```

par un workflow plus simple :

```txt
IC705 Bridge
→ connexion directe à l’IC-705 en Wi-Fi / RS-BA1
→ terminal CI-V intégré
→ API locale pour Python
```

L’application ne cherche pas à devenir une workstation radio complète. Elle sert uniquement de **passerelle pédagogique** entre l’IC-705 et les outils utilisés par les étudiants.

---

## Objectif principal

Permettre à un étudiant de :

```txt
1. lancer l’application ;
2. renseigner l’IP, le nom d’utilisateur et le mot de passe de l’IC-705 ;
3. se connecter au transceiver ;
4. envoyer manuellement des trames CI-V ;
5. observer les réponses ;
6. automatiser les mêmes échanges avec Python.
```

L’étudiant continue donc à travailler au niveau protocole :

```txt
FE FE A4 E0 03 FD
```

Il doit toujours lire la documentation CI-V de l’IC-705, comprendre les commandes, construire les trames, analyser les réponses et automatiser la séquence.

L’app ne cache pas le protocole. Elle remplace seulement la partie lourde : connexion, tunnel CI-V, port COM virtuel et terminal externe.

---

## Structure de l’application

## 1. Onglet Connection

Cet onglet permet d’établir la connexion avec l’IC-705.

### Champs principaux

```txt
Host / IP IC-705
Username
Password
Control port : 50001
CI-V port : 50002
```

### Boutons

```txt
Connect
Disconnect
Check link
```

### États affichés

```txt
Disconnected
Connecting
Authenticated
CI-V tunnel ready
Error
```

L’objectif est que l’étudiant sache immédiatement si l’IC-705 est prêt à recevoir des trames.

Exemple d’état attendu :

```txt
✓ Network connected
✓ Authenticated
✓ CI-V tunnel ready
✓ Local API running at http://127.0.0.1:8765
```

---

## 2. Onglet CI-V Terminal

Cet onglet remplace HTerm.

Il permet d’envoyer une trame CI-V brute, écrite manuellement en hexadécimal et afficher la réponse.

Fonctions attendues :

```txt
- validation de la saisie hexadécimale ;
- envoi de trame ;
- affichage TX ;
- affichage RX ;
- horodatage ;
- bouton Clear ;
- messages d’erreur lisibles.
```

---

## 3. API locale pour Python

Une fois l’app connectée, elle expose une API locale.

Adresse typique :

```txt
http://127.0.0.1:8765
```


Le but est de permettre à Python d’utiliser IC705 Bridge comme remplacement du port COM virtuel.

Exemple côté Python :

```python
from radioforge import RadioForge

rf = RadioForge("http://127.0.0.1:8765")

print(rf.status())

response = rf.send_civ("FE FE A4 E0 03 FD")

print("TX:", response["tx"])
print("RX:", response["response"])
```

la lib ne s'appellera pas radioforge.
La librairie Python reste volontairement bas niveau.

Elle fournit seulement :

```txt
status()
send_civ(frame_hex)
stream_civ() plus tard
```

L’étudiant construit lui-même les trames demandées dans le TP.

---

## Usage dans le TP

## Phase 1 — Étude manuelle des commandes

L’étudiant utilise l’onglet **CI-V Terminal** pour envoyer les commandes demandées.
Il observe les réponses dans le terminal intégré.

---

## Phase 2 — Automatisation Python

L’étudiant réutilise les mêmes trames, mais cette fois dans un script Python.

Exemple :

```python
from radioforge import RadioForge

rf = RadioForge()

rf.send_civ("FE FE A4 E0 03 FD")
rf.send_civ("FE FE A4 E0 05 00 00 05 45 01 FD")
rf.send_civ("FE FE A4 E0 06 07 FD")
```

L’app sert alors de passerelle :

```txt
Python
→ API locale
→ IC705 Bridge
→ tunnel CI-V RS-BA1
→ IC-705
```

---

## Description courte

**IC705 Bridge est une passerelle desktop légère et cross-platform permettant de connecter un Icom IC-705 en remote, d’envoyer et recevoir des trames CI-V manuellement via un terminal intégré, puis d’automatiser ces échanges en Python via une API locale simple.**

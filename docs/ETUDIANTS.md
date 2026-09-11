# Étudiants — accéder aux trames en Python

IC705 Bridge assure la connexion réseau avec l'IC-705. Votre script communique
avec l'application en **HTTP sur un port TCP local**, pas avec un port série.
Lancer l'application, connecter la radio, puis relever l'adresse de l'API dans
l'onglet **Connexion**. Les exemples utilisent le port par défaut `8765`.

Python 3 et sa bibliothèque standard suffisent. Exécuter ces extraits dans un
script ou un interpréteur Python sur le même ordinateur que l'application.

## Vérifier la connexion

```python
import json
from urllib.request import Request, urlopen

base = "http://127.0.0.1:8765"

with urlopen(base + "/status", timeout=5) as response:
    print(json.load(response))
```

Le champ `state` vaut `civ_ready` quand le tunnel radio est prêt.

## Envoyer une trame et lire la réponse brute

```python
frame = input("Votre trame CI-V en hexadécimal : ")
request = Request(
    base + "/civ",
    data=json.dumps({"frame": frame}).encode("utf-8"),
    headers={"Content-Type": "application/json", "X-IC705-Bridge": "1"},
    method="POST",
)

with urlopen(request, timeout=5) as response:
    print(json.load(response))
```

La réponse JSON contient `tx` (trame envoyée) et `response` (réponse radio en
hexadécimal, éventuellement plusieurs trames concaténées). Ces échanges sont
aussi visibles dans le terminal de l'application.

## Recevoir le flux de trames

`GET /stream` ouvre un flux HTTP **SSE**. Chaque ligne `data:` contient une
trame reçue en hexadécimal. Les lignes vides et celles commençant par `:` ne
contiennent pas de trame.

```python
request = Request(base + "/stream", headers={"X-IC705-Bridge": "1"})

with urlopen(request, timeout=30) as response:
    for line in response:
        text = line.decode("utf-8").strip()
        if text.startswith("data:"):
            print(text[5:].strip())
```

Arrêter avec `Ctrl+C`. Ouvrir le flux avant de provoquer des échanges dans le
terminal : il n'inclut aucun historique et ne déclenche aucune commande radio.
Il peut contenir des échos et des émissions spontanées. Après une déconnexion,
rouvrir le flux. Un lecteur trop lent peut manquer des trames.

## En cas d'erreur

- Connexion refusée : vérifier que l'application est ouverte et le port correct.
- HTTP `403` : ajouter l'en-tête `X-IC705-Bridge: 1` sur `/civ` et `/stream`.
- HTTP `400` : vérifier la saisie hexadécimale et la structure de la trame.
- HTTP `503` : connecter la radio dans l'application.
- HTTP `504` : aucune réponse radio correspondante dans le délai imparti.

`urlopen` lève une exception en cas d'erreur HTTP ou de délai dépassé.
L'adresse `127.0.0.1` désigne cet ordinateur ; l'API n'est pas accessible depuis
un autre poste.

Pour le contenu des trames, se reporter au **CI-V Reference Guide de l'IC-705**
et aux consignes du TP. Le choix des commandes, la construction des trames,
le découpage et le décodage des réponses font partie du travail demandé.

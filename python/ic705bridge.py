"""Client Python bas niveau pour IC705 Bridge.

IC705 Bridge expose une API HTTP locale (par défaut http://127.0.0.1:8765).
Cette librairie remplace le port COM virtuel : on lui envoie des trames CI-V
brutes et elle renvoie la réponse de l'IC-705.

La connexion à la radio (IP, username, password) se fait dans l'application
IC705 Bridge. Côté Python, on se contente d'envoyer des trames une fois l'app
connectée.

Aucune dépendance externe (urllib de la stdlib uniquement).

Exemple
-------
    from ic705bridge import IC705Bridge

    rig = IC705Bridge()                 # http://127.0.0.1:8765 par défaut
    print(rig.status())

    rep = rig.send_civ("FE FE A4 E0 03 FD")
    print("TX:", rep["tx"])
    print("RX:", rep["response"])
"""

from __future__ import annotations

import json
import urllib.error
import urllib.request

__all__ = ["IC705Bridge", "BridgeError"]

DEFAULT_URL = "http://127.0.0.1:8765"


class BridgeError(RuntimeError):
    """Erreur renvoyée par l'API IC705 Bridge (ou de transport HTTP)."""


class IC705Bridge:
    """Client de l'API locale IC705 Bridge."""

    def __init__(self, url: str = DEFAULT_URL, timeout: float = 5.0) -> None:
        self.url = url.rstrip("/")
        self.timeout = timeout

    # -- API publique --------------------------------------------------------

    def status(self) -> dict:
        """Renvoie l'état de la connexion (dict : state, host, api_url, ...)."""
        return self._get("/status")

    def send_civ(self, frame_hex: str) -> dict:
        """Envoie une trame CI-V (hex, ex. "FE FE A4 E0 03 FD").

        Renvoie un dict ``{"tx": "...", "response": "..."}`` où ``response``
        contient les octets reçus de la radio (chaîne hex, éventuellement vide
        si la commande n'attend pas de réponse).
        """
        return self._post("/civ", {"frame": frame_hex})

    def is_ready(self) -> bool:
        """True si le tunnel CI-V est prêt (radio connectée)."""
        try:
            return self.status().get("state") == "civ_ready"
        except BridgeError:
            return False

    # -- stream_civ() : à venir ---------------------------------------------

    # -- Transport HTTP ------------------------------------------------------

    def _get(self, path: str) -> dict:
        return self._request("GET", path, None)

    def _post(self, path: str, body: dict) -> dict:
        return self._request("POST", path, body)

    def _request(self, method: str, path: str, body: dict | None) -> dict:
        data = json.dumps(body).encode("utf-8") if body is not None else None
        headers = {"Content-Type": "application/json"} if data else {}
        req = urllib.request.Request(self.url + path, data=data, headers=headers, method=method)
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                return json.loads(resp.read().decode("utf-8"))
        except urllib.error.HTTPError as e:
            detail = e.read().decode("utf-8", "replace")
            try:
                detail = json.loads(detail).get("error", detail)
            except json.JSONDecodeError:
                pass
            raise BridgeError(f"{e.code} {e.reason}: {detail}") from e
        except urllib.error.URLError as e:
            raise BridgeError(
                f"IC705 Bridge injoignable sur {self.url} "
                f"(l'application est-elle lancée ?) : {e.reason}"
            ) from e


if __name__ == "__main__":
    rig = IC705Bridge()
    print("Status:", rig.status())

from __future__ import annotations

import json
import urllib.error
import urllib.request
from typing import Iterable, Iterator, Optional, Union

__all__ = ["IC705Bridge", "BridgeError", "split_frames", "to_hex", "__version__"]

__version__ = "0.1.1"

DEFAULT_URL = "http://127.0.0.1:8765"

#: En-tête envoyé par la lib ; l'app le réclame pour distinguer un vrai client
#: d'une page web malveillante (qui ne peut pas l'ajouter sans préflight CORS).
AUTH_HEADER = "X-IC705-Bridge"

Frame = Union[str, bytes, bytearray, Iterable[int]]


class BridgeError(RuntimeError):
    pass


class IC705Bridge:
    def __init__(self, url: str = DEFAULT_URL, timeout: float = 5.0) -> None:
        self.url = url.rstrip("/")
        self.timeout = timeout

    def __repr__(self) -> str:
        return f"IC705Bridge(url={self.url!r})"

    def status(self) -> dict:
        return self._get("/status")

    def send_civ(self, frame: Frame) -> dict:
        return self._post("/civ", {"frame": to_hex(frame)})

    def is_ready(self) -> bool:
        try:
            return self.status().get("state") == "civ_ready"
        except BridgeError:
            return False

    def stream_civ(self, timeout: Optional[float] = None) -> Iterator[str]:
        req = urllib.request.Request(
            self.url + "/stream",
            headers={"Accept": "text/event-stream", AUTH_HEADER: "1"},
        )
        try:
            resp = urllib.request.urlopen(req, timeout=timeout)
        except urllib.error.HTTPError as e:
            raise BridgeError(_http_detail(e)) from e
        except urllib.error.URLError as e:
            raise BridgeError(_unreachable(self.url, e)) from e
        try:
            for raw in resp:
                line = raw.decode("utf-8", "replace").rstrip("\r\n")
                if line.startswith("data:"):
                    yield line[5:].strip()
        finally:
            resp.close()

    def _get(self, path: str) -> dict:
        return self._request("GET", path, None)

    def _post(self, path: str, body: dict) -> dict:
        return self._request("POST", path, body)

    def _request(self, method: str, path: str, body: Optional[dict]) -> dict:
        data = json.dumps(body).encode("utf-8") if body is not None else None
        headers = {AUTH_HEADER: "1"}
        if data:
            headers["Content-Type"] = "application/json"
        req = urllib.request.Request(self.url + path, data=data, headers=headers, method=method)
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                raw = resp.read().decode("utf-8")
        except urllib.error.HTTPError as e:
            raise BridgeError(_http_detail(e)) from e
        except urllib.error.URLError as e:
            raise BridgeError(_unreachable(self.url, e)) from e
        try:
            payload = json.loads(raw)
        except json.JSONDecodeError as e:
            raise BridgeError(f"réponse non-JSON de {self.url}{path} : {raw[:120]!r}") from e
        if not isinstance(payload, dict):
            raise BridgeError(f"réponse JSON inattendue (objet attendu) : {payload!r}")
        return payload


def to_hex(frame: Frame) -> str:
    if isinstance(frame, str):
        cleaned = (
            frame.replace("0x", " ").replace("0X", " ")
            .replace(",", " ").replace(";", " ").replace(":", " ").replace("-", " ")
        )
        digits = "".join(cleaned.split())
        if len(digits) % 2 != 0:
            raise BridgeError(f"nombre impair de chiffres hexadécimaux ({len(digits)})")
        try:
            data = bytes.fromhex(digits)
        except ValueError as e:
            raise BridgeError(f"trame hexadécimale invalide : {frame!r}") from e
    else:
        try:
            data = bytes(frame)
        except (TypeError, ValueError) as e:
            raise BridgeError(f"octets de trame invalides : {frame!r}") from e
    return " ".join(f"{b:02X}" for b in data)


def split_frames(response: Frame) -> list[str]:
    hexstr = to_hex(response)
    data = bytes.fromhex(hexstr.replace(" ", "")) if hexstr else b""
    frames: list[str] = []
    i, n = 0, len(data)
    while i + 1 < n:
        if data[i] == 0xFE and data[i + 1] == 0xFE:
            end = data.find(0xFD, i + 2)
            if end == -1:
                break
            frames.append(" ".join(f"{b:02X}" for b in data[i : end + 1]))
            i = end + 1
        else:
            i += 1
    return frames


def _http_detail(e: urllib.error.HTTPError) -> str:
    detail = e.read().decode("utf-8", "replace")
    try:
        detail = json.loads(detail).get("error", detail)
    except json.JSONDecodeError:
        pass
    return f"{e.code} {e.reason}: {detail}"


def _unreachable(url: str, e: urllib.error.URLError) -> str:
    return (
        f"IC705 Bridge injoignable sur {url} "
        f"(l'application est-elle lancée ?) : {e.reason}"
    )


if __name__ == "__main__":
    print("Status:", IC705Bridge().status())

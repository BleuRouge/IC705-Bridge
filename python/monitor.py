#!/usr/bin/env python3
"""Moniteur temps réel pour IC705 Bridge : paramètres + waterfall (matplotlib).

Cette application de test se connecte à l'API locale d'IC705 Bridge (l'app doit
être lancée ET connectée à l'IC-705), puis :

  * lit en continu les paramètres de la radio (fréquence, mode, S-mètre) ;
  * active la sortie des données du spectre scope (CI-V ``27 11 01``) ;
  * affiche un panadapter (spectre courant) + un waterfall qui défile.

Le flux de trames CI-V est lu via l'endpoint SSE ``/stream`` (toutes les trames
reçues de la radio : réponses aux commandes + données scope ``27 00``).

Pré-requis :
    pip install matplotlib numpy

Pré-requis radio :
    MENU > SCOPE : le scope doit être affiché/actif sur l'IC-705.
    CI-V Transceive (réglage 1A 05 0131) : vérifié au démarrage et activé
    automatiquement s'il est OFF (nécessaire aux mises à jour automatiques).

Usage :
    python monitor.py [--url http://127.0.0.1:8765] [--rows 200] [--radio A4]

Note : le format exact de la trame scope ``27 00`` réseau est documenté dans
docs/RSBA1_protocol.md (§12-F). Les offsets d'en-tête sont des constantes
ajustables (SCOPE_HEADER_LEN) si une capture réelle révèle un décalage.
"""

from __future__ import annotations

import argparse
import threading
import time
from collections import deque

import numpy as np

try:
    import matplotlib.pyplot as plt
    from matplotlib.animation import FuncAnimation
except ImportError as exc:  # pragma: no cover
    raise SystemExit(
        "matplotlib est requis : pip install matplotlib numpy"
    ) from exc

from ic705bridge import BridgeError, IC705Bridge

# --- Adresses CI-V (modifiables via --radio) -------------------------------
RADIO_ADDR = 0xA4  # IC-705 par défaut
CTRL_ADDR = 0xE0  # contrôleur (ce script)

# --- Commandes CI-V --------------------------------------------------------
CMD_READ_FREQ = (0x03,)
CMD_READ_MODE = (0x04,)
CMD_READ_SMETER = (0x15, 0x02)
CMD_SCOPE_ON = (0x27, 0x10, 0x01)  # scope display ON
CMD_SCOPE_DATA_ON = (0x27, 0x11, 0x01)  # sortie des données waveform ON
CMD_SCOPE_DATA_OFF = (0x27, 0x11, 0x00)
# CI-V Transceive (IC-705 : réglage 1A 05 n°0131, cf. CI-V Reference Guide).
CMD_TRANSCEIVE_READ = (0x1A, 0x05, 0x01, 0x31)
CMD_TRANSCEIVE_ON = (0x1A, 0x05, 0x01, 0x31, 0x01)

# --- Format de la trame scope 27 00 (réseau, voir spec §12-F) --------------
# Après "27 00" : en-tête d'info de 16 octets, puis les échantillons, puis FD.
SCOPE_HEADER_LEN = 16
# Amplitude des échantillons scope IC-705 : ~0..160.
SCOPE_AMP_MAX = 160

MODES = {
    0x00: "LSB", 0x01: "USB", 0x02: "AM", 0x03: "CW", 0x04: "RTTY",
    0x05: "FM", 0x06: "WFM", 0x07: "CW-R", 0x08: "RTTY-R", 0x17: "DV",
}


# ---------------------------------------------------------------------------
# Helpers CI-V
# ---------------------------------------------------------------------------
def build_frame(*payload: int) -> str:
    """Construit une trame CI-V hex : ``FE FE <radio> <ctrl> <payload...> FD``."""
    body = [0xFE, 0xFE, RADIO_ADDR, CTRL_ADDR, *payload, 0xFD]
    return " ".join(f"{b:02X}" for b in body)


def hex_to_bytes(hexstr: str) -> bytes:
    return bytes(int(tok, 16) for tok in hexstr.split())


def split_frames(data: bytes) -> list[bytes]:
    """Découpe un flux d'octets en trames CI-V individuelles (FE FE … FD)."""
    frames: list[bytes] = []
    i = 0
    n = len(data)
    while i < n - 1:
        if data[i] == 0xFE and data[i + 1] == 0xFE:
            j = i + 2
            while j < n and data[j] != 0xFD:
                j += 1
            if j < n:
                frames.append(data[i : j + 1])
                i = j + 1
                continue
            break  # trame incomplète
        i += 1
    return frames


def bcd_le_to_int(b: bytes) -> int:
    """BCD little-endian (octet de poids faible en premier) → entier."""
    value = 0
    for i, byte in enumerate(b):
        value += ((byte & 0x0F) + (byte >> 4) * 10) * (100 ** i)
    return value


def bcd_be_to_int(b: bytes) -> int:
    """BCD big-endian → entier (S-mètre : 2 octets, 0000-0255)."""
    value = 0
    for byte in b:
        value = value * 100 + ((byte >> 4) * 10 + (byte & 0x0F))
    return value


def smeter_label(value: int) -> str:
    """Étiquette approximative du S-mètre IC-705 (0..255 ≈ S0..S9+60dB)."""
    if value <= 120:
        return f"S{value * 9 // 120}"
    return f"S9+{(value - 120) * 60 // 135}dB"


# ---------------------------------------------------------------------------
# Moniteur : état partagé + threads réseau
# ---------------------------------------------------------------------------
class Monitor:
    def __init__(self, bridge: IC705Bridge, rows: int):
        self.bridge = bridge
        self.rows = rows
        self.lock = threading.Lock()
        self.stop = threading.Event()

        # Paramètres courants (mis à jour par le thread de flux).
        self.freq_hz: int | None = None
        self.mode: str | None = None
        self.smeter: int | None = None
        self.smeter_hist: deque[int] = deque(maxlen=200)

        # Spectre / waterfall.
        self.bins: int | None = None
        self.spectrum: np.ndarray | None = None  # dernier sweep
        self.waterfall: np.ndarray | None = None  # (rows, bins)
        self.scope_center: int | None = None
        self.scope_span: int | None = None
        self.frame_count = 0
        self.scope_count = 0

    # -- Threads ------------------------------------------------------------
    def start(self) -> None:
        threading.Thread(target=self._stream_loop, daemon=True).start()
        threading.Thread(target=self._poll_loop, daemon=True).start()

    def _stream_loop(self) -> None:
        """Lit le flux SSE et dispatche chaque trame reçue."""
        while not self.stop.is_set():
            try:
                for hexframe in self.bridge.stream_civ():
                    if self.stop.is_set():
                        break
                    for frame in split_frames(hex_to_bytes(hexframe)):
                        self._dispatch(frame)
            except BridgeError as e:
                print(f"[flux] {e} — nouvelle tentative dans 2 s")
                time.sleep(2.0)
            except Exception as e:  # robustesse : on ne meurt jamais sur une trame
                print(f"[flux] erreur inattendue : {e}")
                time.sleep(1.0)

    def _poll_loop(self) -> None:
        """Active le scope puis interroge périodiquement freq/mode/S-mètre."""
        # Attendre que la radio soit prête, puis activer transceive + scope.
        while not self.stop.is_set():
            try:
                if self.bridge.is_ready():
                    self._ensure_transceive()
                    self._send(*CMD_SCOPE_ON)
                    self._send(*CMD_SCOPE_DATA_ON)
                    break
            except BridgeError:
                pass
            time.sleep(1.0)

        while not self.stop.is_set():
            for cmd in (CMD_READ_FREQ, CMD_READ_MODE, CMD_READ_SMETER):
                if self.stop.is_set():
                    break
                self._send(*cmd)
                time.sleep(0.2)
            time.sleep(0.4)

    def _read_transceive(self) -> int | None:
        """Lit le réglage CI-V Transceive (1A 05 0131). None si indéterminé."""
        try:
            rep = self.bridge.send_civ(build_frame(*CMD_TRANSCEIVE_READ))
        except BridgeError as e:
            print(f"[transceive] lecture impossible : {e}")
            return None
        for frame in split_frames(hex_to_bytes(rep.get("response", ""))):
            # Réponse attendue : FE FE E0 A4 1A 05 01 31 <val> FD.
            if (
                len(frame) >= 10
                and frame[3] == RADIO_ADDR
                and frame[4:8] == bytes(CMD_TRANSCEIVE_READ)
            ):
                return frame[8]
        return None

    def _ensure_transceive(self) -> None:
        """Active CI-V Transceive si la radio l'a sur OFF (requis pour le flux)."""
        value = self._read_transceive()
        if value is None:
            print("[transceive] état CI-V Transceive indéterminé — on continue")
            return
        if value == 0x01:
            return
        print("[transceive] CI-V Transceive est OFF — activation automatique…")
        self._send(*CMD_TRANSCEIVE_ON)
        if self._read_transceive() == 0x01:
            print("[transceive] CI-V Transceive activé.")
        else:
            print("[transceive] échec d'activation — mises à jour auto indisponibles "
                  "(activez-le sur la radio : MENU > SET > Connectors > CI-V)")

    def _send(self, *payload: int) -> None:
        """Envoie une commande CI-V (la réponse arrivera via le flux SSE)."""
        try:
            self.bridge.send_civ(build_frame(*payload))
        except BridgeError as e:
            print(f"[envoi] {e}")

    # -- Dispatch des trames reçues ----------------------------------------
    def _dispatch(self, frame: bytes) -> None:
        # On ne traite que les trames émises par la radio (from == RADIO_ADDR),
        # ce qui écarte l'écho de nos propres commandes (from == CTRL_ADDR).
        if len(frame) < 6 or frame[3] != RADIO_ADDR:
            return
        with self.lock:
            self.frame_count += 1
        cmd = frame[4]
        payload = frame[5:-1]  # entre le code commande et FD

        if cmd == 0x03 and len(payload) >= 5:
            with self.lock:
                self.freq_hz = bcd_le_to_int(payload[:5])
        elif cmd == 0x04 and len(payload) >= 1:
            with self.lock:
                self.mode = MODES.get(payload[0], f"0x{payload[0]:02X}")
        elif cmd == 0x15 and len(payload) >= 3 and payload[0] == 0x02:
            with self.lock:
                self.smeter = bcd_be_to_int(payload[1:3])
                self.smeter_hist.append(self.smeter)
        elif cmd == 0x27 and len(payload) >= 1 and payload[0] == 0x00:
            self._handle_scope(payload[1:])  # après le sous-code 00

    def _handle_scope(self, body: bytes) -> None:
        """Extrait l'en-tête + les échantillons d'une trame scope 27 00."""
        if len(body) <= SCOPE_HEADER_LEN:
            return
        header = body[:SCOPE_HEADER_LEN]
        samples = np.frombuffer(body[SCOPE_HEADER_LEN:], dtype=np.uint8).astype(float)
        if samples.size == 0:
            return

        # En-tête (spec §12-F) : sub, scope_id, seq, total, mode, center 5B, span 5B, oor.
        center = bcd_le_to_int(header[5:10])
        span = bcd_le_to_int(header[10:15])

        with self.lock:
            self.scope_count += 1
            self.scope_center = center or None
            self.scope_span = span or None
            # (Ré)initialise les buffers si la largeur change.
            if self.bins != samples.size:
                self.bins = samples.size
                self.waterfall = np.zeros((self.rows, self.bins))
            self.spectrum = samples
            self.waterfall = np.roll(self.waterfall, 1, axis=0)
            self.waterfall[0, :] = samples


# ---------------------------------------------------------------------------
# Affichage matplotlib (panadapter + waterfall)
# ---------------------------------------------------------------------------
def run_ui(mon: Monitor) -> None:
    fig, (ax_spec, ax_water) = plt.subplots(
        2, 1, figsize=(10, 7), gridspec_kw={"height_ratios": [1, 3]}, sharex=True
    )
    fig.canvas.manager.set_window_title("IC705 Bridge — Monitor")

    (spec_line,) = ax_spec.plot([], [], lw=1.0, color="#2bd")
    ax_spec.set_ylabel("Amplitude")
    ax_spec.set_ylim(0, SCOPE_AMP_MAX)
    ax_spec.grid(alpha=0.2)

    water_img = ax_water.imshow(
        np.zeros((mon.rows, 1)),
        aspect="auto", origin="upper", cmap="viridis",
        interpolation="nearest", vmin=0, vmax=SCOPE_AMP_MAX,
    )
    ax_water.set_ylabel("Temps (défilement)")
    ax_water.set_xlabel("Bin de fréquence")

    def fmt_freq(hz: int | None) -> str:
        return f"{hz / 1e6:.6f} MHz" if hz else "—"

    def update(_frame):
        with mon.lock:
            freq, mode, smeter = mon.freq_hz, mon.mode, mon.smeter
            spectrum = None if mon.spectrum is None else mon.spectrum.copy()
            waterfall = None if mon.waterfall is None else mon.waterfall.copy()
            center, span = mon.scope_center, mon.scope_span
            nf, ns = mon.frame_count, mon.scope_count

        title = (
            f"Freq {fmt_freq(freq)}   |   Mode {mode or '—'}   |   "
            f"S-mètre {('—' if smeter is None else f'{smeter_label(smeter)} ({smeter})')}"
        )
        sub = f"trames CI-V: {nf}   sweeps scope: {ns}"
        if center and span:
            sub += f"   |   centre {center/1e6:.4f} MHz   span ±{span/2e6:.3f} MHz"
        fig.suptitle(title + "\n" + sub, fontsize=11, family="monospace")

        artists = [spec_line, water_img]
        if spectrum is not None and spectrum.size:
            x = np.arange(spectrum.size)
            spec_line.set_data(x, spectrum)
            ax_spec.set_xlim(0, spectrum.size - 1)
            # Autoscale doux de l'amplitude.
            ymax = max(SCOPE_AMP_MAX * 0.5, float(spectrum.max()) * 1.1)
            ax_spec.set_ylim(0, ymax)
        if waterfall is not None:
            water_img.set_data(waterfall)
            water_img.set_extent((0, waterfall.shape[1], waterfall.shape[0], 0))
            vmax = max(1.0, float(np.percentile(waterfall, 99)))
            water_img.set_clim(float(np.percentile(waterfall, 5)), vmax)
        return artists

    # cache_frame_data=False : le contenu vient d'un état mutable partagé.
    _anim = FuncAnimation(fig, update, interval=120, blit=False, cache_frame_data=False)

    try:
        plt.tight_layout(rect=(0, 0, 1, 0.92))
        plt.show()
    finally:
        mon.stop.set()
        # Couper la sortie scope proprement à la fermeture de la fenêtre.
        try:
            mon.bridge.send_civ(build_frame(*CMD_SCOPE_DATA_OFF))
        except BridgeError:
            pass


def main() -> None:
    global RADIO_ADDR
    parser = argparse.ArgumentParser(description="Moniteur + waterfall IC705 Bridge")
    parser.add_argument("--url", default="http://127.0.0.1:8765", help="URL de l'API locale")
    parser.add_argument("--rows", type=int, default=200, help="Hauteur du waterfall (lignes)")
    parser.add_argument("--radio", default="A4", help="Adresse CI-V de la radio (hex, déf. A4)")
    args = parser.parse_args()

    RADIO_ADDR = int(args.radio, 16)

    bridge = IC705Bridge(args.url)
    try:
        status = bridge.status()
    except BridgeError as e:
        raise SystemExit(f"Impossible de joindre IC705 Bridge : {e}")
    print(f"État du bridge : {status.get('state')} (host {status.get('host')})")
    if status.get("state") != "civ_ready":
        print("⚠ La radio n'est pas encore connectée — connectez-la dans l'app, "
              "le moniteur démarrera dès que le tunnel CI-V sera prêt.")

    mon = Monitor(bridge, rows=args.rows)
    mon.start()
    run_ui(mon)


if __name__ == "__main__":
    main()

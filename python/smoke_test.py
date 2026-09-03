#!/usr/bin/env python3
"""Smoke test non destructif d'une chaîne IC705 Bridge + IC-705 réelle."""

from __future__ import annotations

import argparse
import threading
import time

from ic705bridge import BridgeError, IC705Bridge, split_frames


READS = (
    ("fréquence", "FE FE A4 E0 03 FD", 0x03, 5),
    ("mode", "FE FE A4 E0 04 FD", 0x04, 1),
    ("S-mètre", "FE FE A4 E0 15 02 FD", 0x15, 3),
    ("PTT", "FE FE A4 E0 1C 00 FD", 0x1C, 2),
)
SCOPE_DATA_ON = "FE FE A4 E0 27 11 01 FD"
SCOPE_DATA_OFF = "FE FE A4 E0 27 11 00 FD"


def response_frame(response: str, command: int, min_payload: int) -> bytes:
    for frame_hex in split_frames(response):
        frame = bytes.fromhex(frame_hex)
        if (
            len(frame) >= 6 + min_payload
            and frame[:4] == bytes((0xFE, 0xFE, 0xE0, 0xA4))
            and frame[4] == command
        ):
            return frame
        if len(frame) >= 6 and frame[:4] == bytes((0xFE, 0xFE, 0xE0, 0xA4)):
            if frame[4] == 0xFA:
                raise BridgeError("la radio a refusé la commande (CI-V NG/FA)")
    raise BridgeError(f"réponse CI-V 0x{command:02X} absente ou trop courte : {response!r}")


def bcd_le(data: bytes) -> int:
    value = 0
    for index, byte in enumerate(data):
        value += ((byte >> 4) * 10 + (byte & 0x0F)) * (100 ** index)
    return value


def describe(label: str, frame: bytes) -> str:
    payload = frame[5:-1]
    if label == "fréquence":
        return f"{bcd_le(payload[:5]) / 1_000_000:.6f} MHz"
    if label == "mode":
        return f"code 0x{payload[0]:02X}"
    if label == "S-mètre":
        return f"données {' '.join(f'{b:02X}' for b in payload[1:3])}"
    if label == "PTT":
        return "TX" if payload[1] else "RX"
    return "OK"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default="http://127.0.0.1:8765")
    parser.add_argument("--timeout", type=float, default=5.0)
    parser.add_argument(
        "--scope",
        action="store_true",
        help="active temporairement les données du scope et attend un sweep 27 00",
    )
    args = parser.parse_args()

    bridge = IC705Bridge(args.url, timeout=args.timeout)
    status = bridge.status()
    if status.get("state") != "civ_ready":
        raise BridgeError(
            f"tunnel non prêt (state={status.get('state')!r}) : "
            "lancer l'application et connecter l'IC-705"
        )
    if not status.get("api_running"):
        raise BridgeError("l'API locale ne se déclare pas active")
    print(f"✓ API et tunnel prêts — radio {status.get('host') or '—'}")

    streamed: list[str] = []
    stream_error: list[Exception] = []
    scope_frame: list[str] = []
    scope_seen = threading.Event()

    def watch_stream() -> None:
        try:
            for frame in bridge.stream_civ(timeout=max(args.timeout, 3.0)):
                streamed.append(frame)
                for part in split_frames(frame):
                    data = bytes.fromhex(part)
                    if len(data) > 32 and data[:6] == bytes(
                        (0xFE, 0xFE, 0xE0, 0xA4, 0x27, 0x00)
                    ):
                        scope_frame.append(part)
                        scope_seen.set()
        except Exception as exc:  # propagé dans le thread principal ci-dessous
            stream_error.append(exc)

    watcher = threading.Thread(target=watch_stream, daemon=True)
    watcher.start()
    time.sleep(0.25)  # laisse la connexion SSE s'établir avant les commandes

    for label, request, command, min_payload in READS:
        result = bridge.send_civ(request)
        frame = response_frame(result.get("response", ""), command, min_payload)
        print(f"✓ {label}: {describe(label, frame)}")

    requested_commands = {command for _, _, command, _ in READS}

    def stream_has_exchange() -> bool:
        return any(
            len(data) >= 6
            and data[:2] == bytes((0xFE, 0xFE))
            and data[4] in requested_commands
            for event in streamed
            for part in split_frames(event)
            for data in (bytes.fromhex(part),)
        )

    deadline = time.monotonic() + 1.5
    while not stream_has_exchange() and time.monotonic() < deadline:
        time.sleep(0.05)
    if not stream_has_exchange():
        detail = f" ({stream_error[0]})" if stream_error else ""
        raise BridgeError(f"aucun échange de test reçu sur /stream{detail}")
    print(f"✓ Flux SSE opérationnel ({len(streamed)} trame(s) observée(s))")

    if args.scope:
        scope_problem: BridgeError | None = None
        try:
            scope_frame.clear()
            scope_seen.clear()
            bridge.send_civ(SCOPE_DATA_ON)
            if not scope_seen.wait(timeout=max(args.timeout, 3.0)):
                raise BridgeError(
                    "aucun sweep 27 00 reçu (afficher/activer le scope sur l'IC-705)"
                )
            bins = max(0, len(bytes.fromhex(scope_frame[0])) - 23)
            print(f"✓ Scope/waterfall : sweep 27 00 reçu (~{bins} échantillons)")
        except BridgeError as exc:
            scope_problem = exc
        finally:
            # Toujours couper la sortie de données, même si le test du scope échoue.
            try:
                bridge.send_civ(SCOPE_DATA_OFF)
            except BridgeError as exc:
                if scope_problem is None:
                    scope_problem = BridgeError(f"arrêt du flux scope impossible : {exc}")
        if scope_problem is not None:
            raise scope_problem

    print("✓ Smoke test réel terminé sans modification persistante ni PTT ON.")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except BridgeError as exc:
        raise SystemExit(f"✗ {exc}") from exc

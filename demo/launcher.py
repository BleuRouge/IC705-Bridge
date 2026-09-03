#!/usr/bin/env python3
"""Launch IC705 Bridge and the existing Python waterfall demo."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen


DEMO_DIR = Path(__file__).resolve().parent
REPO_ROOT = DEMO_DIR.parent
MONITOR = REPO_ROOT / "python" / "monitor.py"
LOG_FILE = DEMO_DIR / "ic705-bridge.log"
DEFAULT_API_PORT = 8765
MIN_API_PORT = 1024


def fetch_status(base_url: str, timeout: float = 1.0) -> dict[str, Any] | None:
    """Return the bridge status, or None while the local API is unavailable."""

    request = Request(f"{base_url.rstrip('/')}/status", headers={"Accept": "application/json"})
    try:
        with urlopen(request, timeout=timeout) as response:
            payload = json.load(response)
    except (HTTPError, URLError, TimeoutError, OSError, json.JSONDecodeError):
        return None
    return payload if isinstance(payload, dict) else None


def bridge_is_ready(status: dict[str, Any] | None) -> bool:
    return bool(status and status.get("state") == "civ_ready")


def installed_app_command() -> list[str] | None:
    """Find an installed desktop application on the current platform."""

    configured = os.environ.get("IC705_BRIDGE_APP")
    if configured:
        candidate = Path(configured).expanduser()
        if not candidate.exists():
            raise RuntimeError(f"IC705_BRIDGE_APP pointe vers un chemin absent : {candidate}")
        if sys.platform == "darwin" and candidate.suffix == ".app":
            return ["open", str(candidate)]
        return [str(candidate)]

    if sys.platform == "darwin":
        candidates = (
            Path("/Applications/IC705 Bridge.app"),
            Path.home() / "Applications" / "IC705 Bridge.app",
            REPO_ROOT
            / "src-tauri"
            / "target"
            / "release"
            / "bundle"
            / "macos"
            / "IC705 Bridge.app",
        )
        for candidate in candidates:
            if candidate.exists():
                return ["open", str(candidate)]
        return None

    if os.name == "nt":
        on_path = shutil.which("ic705-bridge.exe")
        if on_path:
            return [on_path]
        candidates = []
        for variable in ("LOCALAPPDATA", "ProgramFiles"):
            base = os.environ.get(variable)
            if base:
                candidates.extend(
                    [
                        Path(base) / "IC705 Bridge" / "ic705-bridge.exe",
                        Path(base) / "IC705 Bridge" / "IC705 Bridge.exe",
                    ]
                )
        for candidate in candidates:
            if candidate.exists():
                return [str(candidate)]
        return None

    on_path = shutil.which("ic705-bridge")
    return [on_path] if on_path else None


def source_app_command() -> list[str]:
    pnpm = shutil.which("pnpm")
    if not pnpm:
        raise RuntimeError(
            "IC705 Bridge n'est pas installé et pnpm est introuvable. "
            "Installez l'application ou les prérequis de développement."
        )
    return [pnpm, "tauri", "dev"]


def detached_process_options() -> dict[str, Any]:
    if os.name == "nt":
        return {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP}
    return {"start_new_session": True}


def launch_app(force_source: bool = False) -> tuple[subprocess.Popen[bytes] | None, str]:
    command = None if force_source else installed_app_command()
    if command:
        subprocess.Popen(command, cwd=REPO_ROOT, **detached_process_options())
        return None, "application installée"

    command = source_app_command()
    log_handle = LOG_FILE.open("wb")
    try:
        process = subprocess.Popen(
            command,
            cwd=REPO_ROOT,
            stdout=log_handle,
            stderr=subprocess.STDOUT,
            **detached_process_options(),
        )
    finally:
        log_handle.close()
    return process, "sources (pnpm tauri dev)"


def log_tail(lines: int = 20) -> str:
    try:
        return "\n".join(LOG_FILE.read_text(errors="replace").splitlines()[-lines:])
    except OSError:
        return ""


def wait_for_api(
    base_url: str,
    timeout: float,
    app_process: subprocess.Popen[bytes] | None,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        status = fetch_status(base_url)
        if status is not None:
            return status
        if app_process is not None and app_process.poll() is not None:
            details = log_tail()
            suffix = f"\n\nDernières lignes du journal :\n{details}" if details else ""
            raise RuntimeError(
                f"IC705 Bridge s'est arrêté avec le code {app_process.returncode}.{suffix}"
            )
        time.sleep(0.5)
    raise RuntimeError(
        f"L'API locale n'a pas répondu sur {base_url} après {timeout:.0f} s. "
        f"Consultez {LOG_FILE}."
    )


def wait_for_radio(
    base_url: str,
    initial_status: dict[str, Any],
    app_process: subprocess.Popen[bytes] | None,
) -> None:
    status = initial_status
    last_state: str | None = None
    print("\nDans IC705 Bridge :")
    print("  1. sélectionnez l'IC-705 ;")
    print("  2. cliquez sur Connect ;")
    print("  3. laissez l'application ouverte en arrière-plan.\n")
    print("Le lanceur attend la connexion au poste…")

    unavailable_since: float | None = None
    while not bridge_is_ready(status):
        state = str(status.get("state", "inconnu")) if status else "api_indisponible"
        if state != last_state:
            message = status.get("message") if status else None
            suffix = f" — {message}" if message else ""
            print(f"  État : {state}{suffix}")
            last_state = state
        if app_process is not None and app_process.poll() is not None:
            raise RuntimeError("IC705 Bridge a été fermé avant la connexion au poste.")
        if status is None:
            unavailable_since = unavailable_since or time.monotonic()
            if time.monotonic() - unavailable_since >= 10:
                raise RuntimeError("IC705 Bridge ne répond plus sur son API locale.")
        else:
            unavailable_since = None
        time.sleep(1.0)
        status = fetch_status(base_url)


def run_monitor(base_url: str, radio_address: int | None, rows: int) -> int:
    command = [sys.executable, str(MONITOR), "--url", base_url, "--rows", str(rows)]
    if radio_address is not None:
        command.extend(["--radio", hex(radio_address)])
    print("\nIC-705 prêt. Lancement du moniteur waterfall…\n")
    return subprocess.run(command, cwd=MONITOR.parent, check=False).returncode


def parse_radio_address(value: str) -> int:
    try:
        parsed = int(value, 0)
    except ValueError as error:
        raise argparse.ArgumentTypeError("adresse attendue, par exemple 0xA4") from error
    if not 0 <= parsed <= 0xFF:
        raise argparse.ArgumentTypeError("l'adresse doit tenir sur un octet")
    return parsed


def parse_api_port(value: str) -> int:
    try:
        parsed = int(value, 10)
    except ValueError as error:
        raise argparse.ArgumentTypeError("port TCP attendu, par exemple 8765") from error
    if not MIN_API_PORT <= parsed <= 65535:
        raise argparse.ArgumentTypeError(
            f"le port doit être compris entre {MIN_API_PORT} et 65535"
        )
    return parsed


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--port",
        type=parse_api_port,
        default=DEFAULT_API_PORT,
        help=f"port TCP local de l'API (défaut : {DEFAULT_API_PORT})",
    )
    parser.add_argument("--url", help="URL complète de l'API (prioritaire sur --port)")
    parser.add_argument("--rows", type=int, default=160, help="profondeur du waterfall")
    parser.add_argument("--radio", type=parse_radio_address, help="adresse CI-V, ex. 0xA4")
    parser.add_argument(
        "--source",
        action="store_true",
        help="lancer l'application depuis les sources même si elle est installée",
    )
    parser.add_argument(
        "--no-launch",
        action="store_true",
        help="ne pas lancer IC705 Bridge (utile s'il est déjà ouvert)",
    )
    parser.add_argument(
        "--app-timeout",
        type=float,
        default=180.0,
        help="délai maximal de démarrage de l'application en secondes",
    )
    return parser


def main() -> int:
    args = build_parser().parse_args()
    if args.rows <= 0:
        raise SystemExit("--rows doit être strictement positif")

    app_process: subprocess.Popen[bytes] | None = None
    base_url = args.url or f"http://127.0.0.1:{args.port}"
    status = fetch_status(base_url)
    if status is None and not args.no_launch:
        print("Démarrage d'IC705 Bridge…")
        app_process, method = launch_app(args.source)
        print(f"  Mode : {method}")
        if method.startswith("sources"):
            print(f"  Journal : {LOG_FILE}")
        status = wait_for_api(base_url, args.app_timeout, app_process)
    elif status is not None:
        print("IC705 Bridge est déjà ouvert.")
    else:
        status = wait_for_api(base_url, args.app_timeout, app_process)

    wait_for_radio(base_url, status, app_process)
    exit_code = run_monitor(base_url, args.radio, args.rows)
    print("\nDémo terminée. IC705 Bridge reste ouvert pour permettre une déconnexion propre.")
    return exit_code


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        print("\nDémo interrompue. IC705 Bridge reste ouvert.", file=sys.stderr)
        raise SystemExit(130)
    except RuntimeError as error:
        print(f"\nErreur : {error}", file=sys.stderr)
        raise SystemExit(1)

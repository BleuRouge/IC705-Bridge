#!/usr/bin/env python3
"""Validation complète d'IC705 Bridge avant une démonstration.

Sans option, exécute tous les contrôles locaux (l'application doit être fermée,
car les tests Rust réservent les ports UDP 50001/50002).

Avec ``--live``, vérifie une application déjà lancée et connectée à l'IC-705 :
API locale, commandes CI-V de lecture et flux SSE. L'option ``--scope`` active
temporairement les données waterfall puis les coupe. Le PTT n'est jamais activé.
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def executable(name: str) -> str:
    path = shutil.which(name)
    if not path:
        raise SystemExit(f"Outil requis introuvable : {name}")
    return path


def run(label: str, command: list[str], cwd: Path = ROOT) -> None:
    print(f"\n=== {label} ===", flush=True)
    subprocess.run(command, cwd=cwd, check=True)


def extract(pattern: str, path: Path) -> str:
    match = re.search(pattern, path.read_text(encoding="utf-8"), re.MULTILINE)
    if not match:
        raise SystemExit(f"Version introuvable dans {path.relative_to(ROOT)}")
    return match.group(1)


def check_versions() -> None:
    package_version = json.loads(
        (ROOT / "package.json").read_text(encoding="utf-8")
    )["version"]
    app_versions = {
        "package.json": package_version,
        "src-tauri/Cargo.toml": extract(
            r'^version\s*=\s*"([^"]+)"', ROOT / "src-tauri" / "Cargo.toml"
        ),
        "src-tauri/tauri.conf.json": json.loads(
            (ROOT / "src-tauri" / "tauri.conf.json").read_text(encoding="utf-8")
        )["version"],
    }
    if len(set(app_versions.values())) != 1:
        details = ", ".join(
            f"{path}={version}" for path, version in app_versions.items()
        )
        raise SystemExit(f"Versions incohérentes : {details}")
    python_version = extract(
        r'^__version__\s*=\s*"([^"]+)"', ROOT / "python" / "ic705bridge.py"
    )
    print(f"Version application cohérente : {package_version}")
    print(f"Version librairie Python : {python_version}")


def local_checks(quick: bool) -> None:
    pnpm = executable("pnpm")
    cargo = executable("cargo")
    if not (ROOT / "node_modules").is_dir():
        raise SystemExit("Dépendances frontend absentes : exécuter d'abord `pnpm install`.")

    check_versions()
    run(
        "Syntaxe Python",
        [
            sys.executable,
            "-m",
            "py_compile",
            "ic705bridge.py",
            "example.py",
            "monitor.py",
            "smoke_test.py",
            "tests/test_ic705bridge.py",
            "../demo/launcher.py",
            "../demo/test_launcher.py",
        ],
        ROOT / "python",
    )
    run(
        "Tests Python",
        [sys.executable, "-m", "unittest", "discover", "-s", "tests", "-v"],
        ROOT / "python",
    )
    run(
        "Tests du lanceur de démo",
        [
            sys.executable,
            "-m",
            "unittest",
            "discover",
            "-s",
            "demo",
            "-p",
            "test_*.py",
            "-v",
        ],
    )
    run(
        "Formatage Rust",
        [cargo, "fmt", "--manifest-path", "src-tauri/Cargo.toml", "--", "--check"],
    )
    run(
        "Clippy",
        [
            cargo,
            "clippy",
            "--manifest-path",
            "src-tauri/Cargo.toml",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    )
    run("Tests Rust", [cargo, "test", "--manifest-path", "src-tauri/Cargo.toml"])
    run("Build frontend", [pnpm, "build"])
    if not quick:
        run("Build Tauri intégré", [pnpm, "tauri", "build", "--debug", "--no-bundle"])

    print("\n✓ Tous les contrôles locaux sont passés.")


def live_checks(url: str, timeout: float, scope: bool) -> None:
    command = [
        sys.executable,
        "smoke_test.py",
        "--url",
        url,
        "--timeout",
        str(timeout),
    ]
    if scope:
        command.append("--scope")
    run(
        "Test réel IC-705 non destructif",
        command,
        ROOT / "python",
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--live",
        action="store_true",
        help="teste une application lancée et déjà connectée à l'IC-705",
    )
    parser.add_argument(
        "--quick",
        action="store_true",
        help="omet le build Tauri intégré (sans effet avec --live)",
    )
    parser.add_argument(
        "--scope",
        action="store_true",
        help="avec --live, vérifie aussi une trame waterfall 27 00 (scope radio affiché)",
    )
    parser.add_argument("--url", default="http://127.0.0.1:8765")
    parser.add_argument("--timeout", type=float, default=5.0)
    args = parser.parse_args()

    if args.scope and not args.live:
        parser.error("--scope doit être utilisé avec --live")

    try:
        if args.live:
            live_checks(args.url, args.timeout, args.scope)
        else:
            local_checks(args.quick)
    except subprocess.CalledProcessError as exc:
        print(f"\n✗ Échec de la commande (code {exc.returncode}).", file=sys.stderr)
        return exc.returncode or 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

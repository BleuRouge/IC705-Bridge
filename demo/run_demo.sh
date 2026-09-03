#!/usr/bin/env sh
set -eu

DEMO_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

if ! command -v uv >/dev/null 2>&1; then
  echo "uv est requis : https://docs.astral.sh/uv/getting-started/installation/" >&2
  exit 1
fi

exec uv run --project "$DEMO_DIR" python "$DEMO_DIR/launcher.py" "$@"

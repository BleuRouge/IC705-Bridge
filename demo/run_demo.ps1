$ErrorActionPreference = "Stop"

if (-not (Get-Command uv -ErrorAction SilentlyContinue)) {
    Write-Error "uv est requis : https://docs.astral.sh/uv/getting-started/installation/"
    exit 1
}

& uv run --project $PSScriptRoot python (Join-Path $PSScriptRoot "launcher.py") @args
exit $LASTEXITCODE

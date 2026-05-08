#!/usr/bin/env bash
# Shared launcher for the IronHermes gateway.
# Used by launchd (macOS), systemd (Linux), and the cron watchdog.
# Loads ~/.ironhermes/.env and execs the gateway so OS signals reach it directly.

set -euo pipefail

IRONHERMES_HOME_DIR="${IRONHERMES_HOME:-$HOME/.ironhermes}"
ENV_FILE="$IRONHERMES_HOME_DIR/.env"
BIN="${IRONHERMES_BIN:-$HOME/.local/bin/ironhermes}"

if [ ! -x "$BIN" ]; then
    echo "gateway-run: binary not found or not executable: $BIN" >&2
    exit 127
fi

cd "$HOME"

if [ -f "$ENV_FILE" ]; then
    set -a
    # shellcheck disable=SC1090
    . "$ENV_FILE"
    set +a
fi

args=(gateway)
if [ -n "${IRONHERMES_PROFILE:-}" ]; then
    args=(--profile "$IRONHERMES_PROFILE" gateway)
fi

exec "$BIN" "${args[@]}" "$@"

#!/usr/bin/env bash
# Cron-driven watchdog: ensure the IronHermes gateway is running.
# Reads ~/.ironhermes/gateway.pid (3-line YAML written by the binary),
# probes the pid with kill -0, and relaunches via gateway-run.sh if dead.
#
# Suggested crontab entry (every minute):
#   * * * * * $HOME/.ironhermes/scripts/gateway-watchdog.sh >/dev/null 2>&1

set -uo pipefail

IRONHERMES_HOME_DIR="${IRONHERMES_HOME:-$HOME/.ironhermes}"
PID_FILE="$IRONHERMES_HOME_DIR/gateway.pid"
LOG_DIR="$IRONHERMES_HOME_DIR/logs"
LOG_FILE="$LOG_DIR/gateway.log"
RUNNER="$IRONHERMES_HOME_DIR/scripts/gateway-run.sh"

mkdir -p "$LOG_DIR"

# 48.3 D-06/T-48.3-21: create the log 0600 before this watchdog's first
# append — a supervisor (or this script) opening a missing append target
# otherwise creates it with default (typically 0644) permissions, and
# gateway startup errors can quote provider keys. Guarded so a chmod
# failure cannot abort the watchdog's real job of restarting a dead gateway.
if [ ! -e "$LOG_FILE" ]; then
    : >"$LOG_FILE" 2>/dev/null || true
fi
chmod 600 "$LOG_FILE" 2>/dev/null || true

is_alive() {
    local pid="$1"
    [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null
}

read_pid() {
    [ -f "$PID_FILE" ] || return 1
    awk '/^pid:[[:space:]]/ {print $2; exit}' "$PID_FILE"
}

PID="$(read_pid || true)"

if is_alive "${PID:-}"; then
    exit 0
fi

if [ ! -x "$RUNNER" ]; then
    echo "[$(date -u +%FT%TZ)] watchdog: runner not executable: $RUNNER" >>"$LOG_FILE"
    exit 1
fi

echo "[$(date -u +%FT%TZ)] watchdog: gateway not running (pid=${PID:-none}); relaunching" >>"$LOG_FILE"
nohup "$RUNNER" >>"$LOG_FILE" 2>&1 </dev/null &
disown || true
exit 0

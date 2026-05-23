#!/usr/bin/env bash
# IronHermes Gateway — uninstaller.
#
# Usage:
#   uninstall.sh           # remove native service for this OS (launchd/systemd)
#   uninstall.sh --cron    # remove the watchdog crontab entry
#   uninstall.sh --all     # remove both, plus staged scripts in ~/.ironhermes/scripts/
#
# Logs in ~/.ironhermes/logs/ are kept.

set -euo pipefail

HOME_DIR="$HOME"
IRONHERMES_HOME_DIR="${IRONHERMES_HOME:-$HOME_DIR/.ironhermes}"
SCRIPTS_DIR="$IRONHERMES_HOME_DIR/scripts"

LABEL="com.ironhermes.gateway"
PLIST_DEST="$HOME_DIR/Library/LaunchAgents/${LABEL}.plist"
SERVICE_DEST="$HOME_DIR/.config/systemd/user/ironhermes-gateway.service"
CRON_MARK="# ironhermes-gateway-watchdog"

MODE="auto"
PURGE_SCRIPTS=0

for arg in "$@"; do
    case "$arg" in
        --cron) MODE="cron" ;;
        --all)  MODE="all"; PURGE_SCRIPTS=1 ;;
        -h|--help) sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown arg: $arg" >&2; exit 2 ;;
    esac
done

log() { printf '[uninstall] %s\n' "$*"; }

remove_macos() {
    if launchctl print "gui/$UID/$LABEL" >/dev/null 2>&1; then
        launchctl bootout "gui/$UID/$LABEL" || true
        log "booted out $LABEL"
    fi
    if [ -f "$PLIST_DEST" ]; then
        rm -f "$PLIST_DEST"
        log "removed $PLIST_DEST"
    fi
}

remove_linux() {
    if systemctl --user list-unit-files 2>/dev/null | grep -q '^ironhermes-gateway.service'; then
        systemctl --user disable --now ironhermes-gateway.service || true
    fi
    if [ -f "$SERVICE_DEST" ]; then
        rm -f "$SERVICE_DEST"
        systemctl --user daemon-reload || true
        log "removed $SERVICE_DEST"
    fi
}

remove_cron() {
    local existing
    existing="$(crontab -l 2>/dev/null || true)"
    if printf '%s\n' "$existing" | grep -Fq "$CRON_MARK"; then
        printf '%s\n' "$existing" | grep -Fv "$CRON_MARK" | crontab -
        log "removed watchdog crontab entry"
    else
        log "no watchdog crontab entry found"
    fi
}

case "$MODE" in
    cron) remove_cron ;;
    all)
        remove_cron
        case "$(uname -s)" in
            Darwin) remove_macos ;;
            Linux)  remove_linux ;;
        esac
        ;;
    auto)
        case "$(uname -s)" in
            Darwin) remove_macos ;;
            Linux)  remove_linux ;;
            *) echo "unsupported OS: $(uname -s)" >&2; exit 1 ;;
        esac
        ;;
esac

if [ "$PURGE_SCRIPTS" -eq 1 ] && [ -d "$SCRIPTS_DIR" ]; then
    rm -f "$SCRIPTS_DIR/gateway-run.sh" "$SCRIPTS_DIR/gateway-watchdog.sh"
    rmdir "$SCRIPTS_DIR" 2>/dev/null || true
    log "removed staged scripts"
fi

log "done"

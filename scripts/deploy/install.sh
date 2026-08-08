#!/usr/bin/env bash
# IronHermes Gateway — installer.
#
# Usage:
#   install.sh             # detect OS, install native service (launchd/systemd)
#   install.sh --cron      # add the watchdog cron entry instead
#   install.sh --no-start  # write files / register, but don't start the service
#   install.sh --force     # overwrite existing service files / cron entry
#
# Effects:
#   1. Installs the freshly built binary (target/release/ironhermes, or
#      $IRONHERMES_RELEASE_BIN) to ~/.local/bin/ironhermes (or $IRONHERMES_BIN)
#   2. Creates ~/.ironhermes/{scripts,logs}/
#   3. Copies gateway-run.sh + gateway-watchdog.sh into ~/.ironhermes/scripts/
#   4. macOS: renders + bootstraps com.ironhermes.gateway in ~/Library/LaunchAgents/
#      Linux: renders + enables ironhermes-gateway.service in ~/.config/systemd/user/
#      --cron: appends a 1-min crontab entry calling the watchdog
#   5. Prints verification commands.

set -euo pipefail

SOURCE_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SOURCE_DIR/../.." && pwd)"
HOME_DIR="$HOME"
IRONHERMES_HOME_DIR="${IRONHERMES_HOME:-$HOME_DIR/.ironhermes}"
SCRIPTS_DIR="$IRONHERMES_HOME_DIR/scripts"
LOGS_DIR="$IRONHERMES_HOME_DIR/logs"
ENV_FILE="$IRONHERMES_HOME_DIR/.env"
BIN="${IRONHERMES_BIN:-$HOME_DIR/.local/bin/ironhermes}"
RELEASE_BIN="${IRONHERMES_RELEASE_BIN:-$REPO_ROOT/target/release/ironhermes}"

LABEL="com.ironhermes.gateway"
PLIST_DEST="$HOME_DIR/Library/LaunchAgents/${LABEL}.plist"
SERVICE_DEST="$HOME_DIR/.config/systemd/user/ironhermes-gateway.service"
CRON_MARK="# ironhermes-gateway-watchdog"
CRON_LINE="* * * * * $SCRIPTS_DIR/gateway-watchdog.sh >/dev/null 2>&1 $CRON_MARK"

MODE="auto"   # auto | cron
START=1
FORCE=0

for arg in "$@"; do
    case "$arg" in
        --cron)     MODE="cron" ;;
        --no-start) START=0 ;;
        --force)    FORCE=1 ;;
        -h|--help)
            sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *) echo "unknown arg: $arg" >&2; exit 2 ;;
    esac
done

log()  { printf '[install] %s\n' "$*"; }
die()  { printf '[install] ERROR: %s\n' "$*" >&2; exit 1; }
warn() { printf '[install] WARN: %s\n' "$*" >&2; }

# ---------- install binary ----------
# The service execs $BIN (see gateway-run.sh) — a rebuild does nothing until it
# lands there, so copy the release build in as the first real install step.
# `install` unlinks the destination first, so overwriting a currently-running
# binary is safe (no ETXTBSY); the service picks it up on its next (re)start.
if [ -x "$RELEASE_BIN" ]; then
    mkdir -p "$(dirname "$BIN")"
    install -m 755 "$RELEASE_BIN" "$BIN"
    log "installed $RELEASE_BIN -> $BIN"
elif [ -x "$BIN" ]; then
    warn "no release build at $RELEASE_BIN — keeping the EXISTING (possibly stale) $BIN; run 'cargo build --release' and re-run to deploy fresh code"
else
    die "no binary: neither $RELEASE_BIN nor $BIN exists (build with 'cargo build --release' or set IRONHERMES_RELEASE_BIN/IRONHERMES_BIN)"
fi

# ---------- preflight ----------
[ -x "$BIN" ] || die "binary not executable: $BIN (build with 'cargo build --release' or set IRONHERMES_BIN)"

if [ ! -f "$ENV_FILE" ]; then
    warn "$ENV_FILE not found — gateway will rely on the environment for TELEGRAM_BOT_TOKEN"
elif ! grep -qE '^[[:space:]]*TELEGRAM_BOT_TOKEN[[:space:]]*=' "$ENV_FILE"; then
    warn "$ENV_FILE has no TELEGRAM_BOT_TOKEN — set one before the gateway can connect"
fi

log "running 'ironhermes doctor' as a sanity check"
"$BIN" doctor || warn "ironhermes doctor returned non-zero (continuing)"

# ---------- stage scripts ----------
mkdir -p "$SCRIPTS_DIR" "$LOGS_DIR"
install -m 755 "$SOURCE_DIR/gateway-run.sh"      "$SCRIPTS_DIR/gateway-run.sh"
install -m 755 "$SOURCE_DIR/gateway-watchdog.sh" "$SCRIPTS_DIR/gateway-watchdog.sh"
log "staged scripts in $SCRIPTS_DIR"

render_plist() {
    sed "s|__HOME__|$HOME_DIR|g" "$SOURCE_DIR/com.ironhermes.gateway.plist"
}

install_macos() {
    mkdir -p "$(dirname "$PLIST_DEST")"
    if [ -f "$PLIST_DEST" ] && [ "$FORCE" -ne 1 ]; then
        log "$PLIST_DEST exists; bootout first to refresh"
        launchctl bootout "gui/$UID/$LABEL" 2>/dev/null || true
    elif [ -f "$PLIST_DEST" ]; then
        launchctl bootout "gui/$UID/$LABEL" 2>/dev/null || true
    fi
    render_plist > "$PLIST_DEST"
    chmod 644 "$PLIST_DEST"
    log "wrote $PLIST_DEST"

    launchctl bootstrap "gui/$UID" "$PLIST_DEST"
    log "bootstrapped $LABEL"

    if [ "$START" -eq 1 ]; then
        # -k kills a running instance first so a re-install picks up the new binary.
        launchctl kickstart -k "gui/$UID/$LABEL"
        log "kickstarted $LABEL"
    fi

    cat <<EOF

Installed via launchd. Verify with:
  launchctl print gui/$UID/$LABEL | grep -E 'state|pid|last exit'
  tail -f $LOGS_DIR/gateway.err.log

Stop:    launchctl bootout gui/$UID/$LABEL
Restart: launchctl kickstart -k gui/$UID/$LABEL
EOF
}

install_linux() {
    mkdir -p "$(dirname "$SERVICE_DEST")"
    install -m 644 "$SOURCE_DIR/ironhermes-gateway.service" "$SERVICE_DEST"
    log "wrote $SERVICE_DEST"

    systemctl --user daemon-reload
    systemctl --user enable ironhermes-gateway.service
    if [ "$START" -eq 1 ]; then
        # restart (not enable --now) so a re-install over a RUNNING service
        # actually swaps in the new binary instead of no-opping.
        systemctl --user restart ironhermes-gateway.service
    fi

    cat <<EOF

Installed via systemd --user. Verify with:
  systemctl --user status ironhermes-gateway
  journalctl --user -u ironhermes-gateway -f

Stop:    systemctl --user stop ironhermes-gateway
Restart: systemctl --user restart ironhermes-gateway

Headless host? Run once: loginctl enable-linger $USER
EOF
}

install_cron() {
    local existing
    existing="$(crontab -l 2>/dev/null || true)"
    if printf '%s\n' "$existing" | grep -Fq "$CRON_MARK"; then
        if [ "$FORCE" -ne 1 ]; then
            log "watchdog cron entry already present; pass --force to replace"
            return 0
        fi
        existing="$(printf '%s\n' "$existing" | grep -Fv "$CRON_MARK")"
    fi
    printf '%s\n%s\n' "$existing" "$CRON_LINE" | crontab -
    log "added watchdog crontab entry"

    if [ "$START" -eq 1 ]; then
        "$SCRIPTS_DIR/gateway-watchdog.sh" || true
        log "kicked watchdog once to start gateway"
    fi

    cat <<EOF

Installed via cron watchdog. Verify with:
  crontab -l | grep ironhermes
  tail -f $LOGS_DIR/gateway.log

Remove: scripts/deploy/uninstall.sh --cron
EOF
}

# ---------- dispatch ----------
if [ "$MODE" = "cron" ]; then
    install_cron
else
    case "$(uname -s)" in
        Darwin) install_macos ;;
        Linux)  install_linux ;;
        *)      die "unsupported OS: $(uname -s) (try --cron)" ;;
    esac
fi

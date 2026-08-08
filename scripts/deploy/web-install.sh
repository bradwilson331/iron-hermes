#!/usr/bin/env bash
# IronHermes Web UI — installer.
#
# Usage:
#   web-install.sh             # detect OS, stage the bundle, register + start the service
#   web-install.sh --no-start  # stage and register, but don't start the service
#   web-install.sh --force     # overwrite existing service files
#
# Effects:
#   1. Stages the built web bundle (target/dx/iron_hermes_ui/release/web, or
#      $IRONHERMES_UI_BUNDLE_DIR) as a whole directory into
#      ~/.ironhermes/web/ — binary + public/ + manifest together, since
#      assets resolve relative to the running binary's own directory.
#   2. Stages web-run.sh into ~/.ironhermes/scripts/
#   3. macOS: renders + bootstraps com.ironhermes.web in ~/Library/LaunchAgents/
#      Linux: renders + enables ironhermes-web.service in ~/.config/systemd/user/
#   4. Prints verification commands.
#
# The staged server still defaults to a loopback bind. Widening it requires
# setting IRONHERMES_WEB_ALLOW_PUBLIC_BIND=1 in ~/.ironhermes/.env — do that
# only if you understand iron_hermes_ui has no authentication of any kind.

set -euo pipefail

SOURCE_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SOURCE_DIR/../.." && pwd)"
HOME_DIR="$HOME"
IRONHERMES_HOME_DIR="${IRONHERMES_HOME:-$HOME_DIR/.ironhermes}"
SCRIPTS_DIR="$IRONHERMES_HOME_DIR/scripts"
LOGS_DIR="$IRONHERMES_HOME_DIR/logs"
WEB_STAGE_DIR="$IRONHERMES_HOME_DIR/web"
BUNDLE_DIR="${IRONHERMES_UI_BUNDLE_DIR:-$REPO_ROOT/target/dx/iron_hermes_ui/release/web}"

LABEL="com.ironhermes.web"
PLIST_DEST="$HOME_DIR/Library/LaunchAgents/${LABEL}.plist"
SERVICE_DEST="$HOME_DIR/.config/systemd/user/ironhermes-web.service"

START=1
FORCE=0

for arg in "$@"; do
    case "$arg" in
        --no-start) START=0 ;;
        --force)    FORCE=1 ;;
        -h|--help)
            sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *) echo "unknown arg: $arg" >&2; exit 2 ;;
    esac
done

log()  { printf '[web-install] %s\n' "$*"; }
die()  { printf '[web-install] ERROR: %s\n' "$*" >&2; exit 1; }
warn() { printf '[web-install] WARN: %s\n' "$*" >&2; }

# ---------- preflight ----------
[ -x "$BUNDLE_DIR/iron_hermes_ui" ] || die "no bundle binary at $BUNDLE_DIR/iron_hermes_ui (build with scripts/deploy/web-build.sh, or set IRONHERMES_UI_BUNDLE_DIR)"

# ---------- stage the bundle as a whole directory ----------
# Assets resolve as current_exe().parent()/public (dioxus-server) — copying
# the binary alone breaks asset serving, so the whole web/ bundle directory
# is staged as a unit. The staged public/ is replaced (not merged) so assets
# removed in a newer build cannot linger and be served.
mkdir -p "$WEB_STAGE_DIR"
rm -rf "$WEB_STAGE_DIR/public"
cp -R "$BUNDLE_DIR/public" "$WEB_STAGE_DIR/public"
install -m 755 "$BUNDLE_DIR/iron_hermes_ui" "$WEB_STAGE_DIR/iron_hermes_ui"
if [ -f "$BUNDLE_DIR/.manifest.json" ]; then
    install -m 644 "$BUNDLE_DIR/.manifest.json" "$WEB_STAGE_DIR/.manifest.json"
fi
log "staged bundle: $BUNDLE_DIR -> $WEB_STAGE_DIR"

# ---------- stage scripts ----------
mkdir -p "$SCRIPTS_DIR" "$LOGS_DIR"
install -m 755 "$SOURCE_DIR/web-run.sh" "$SCRIPTS_DIR/web-run.sh"
log "staged $SCRIPTS_DIR/web-run.sh"

render_plist() {
    sed "s|__HOME__|$HOME_DIR|g" "$SOURCE_DIR/com.ironhermes.web.plist"
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
        # -k kills a running instance first so a re-install picks up the new bundle.
        launchctl kickstart -k "gui/$UID/$LABEL"
        log "kickstarted $LABEL"
    fi

    cat <<EOF

Installed via launchd. Verify with:
  launchctl print gui/$UID/$LABEL | grep -E 'state|pid|last exit'
  tail -f $LOGS_DIR/web.err.log

Stop:    launchctl bootout gui/$UID/$LABEL
Restart: launchctl kickstart -k gui/$UID/$LABEL
EOF
}

install_linux() {
    mkdir -p "$(dirname "$SERVICE_DEST")"
    install -m 644 "$SOURCE_DIR/ironhermes-web.service" "$SERVICE_DEST"
    log "wrote $SERVICE_DEST"

    systemctl --user daemon-reload
    systemctl --user enable ironhermes-web.service
    if [ "$START" -eq 1 ]; then
        # restart (not enable --now) so a re-install over a RUNNING service
        # actually swaps in the new bundle instead of no-opping.
        systemctl --user restart ironhermes-web.service
    fi

    cat <<EOF

Installed via systemd --user. Verify with:
  systemctl --user status ironhermes-web
  journalctl --user -u ironhermes-web -f

Stop:    systemctl --user stop ironhermes-web
Restart: systemctl --user restart ironhermes-web

Headless host? Run once: loginctl enable-linger $USER
EOF
}

# ---------- dispatch ----------
case "$(uname -s)" in
    Darwin) install_macos ;;
    Linux)  install_linux ;;
    *)      die "unsupported OS: $(uname -s)" ;;
esac

cat <<EOF

The staged server still defaults to a loopback bind (127.0.0.1:8080).
Widening it requires IRONHERMES_WEB_ALLOW_PUBLIC_BIND=1 in $IRONHERMES_HOME_DIR/.env
— iron_hermes_ui has NO authentication of any kind, so only opt in if you
understand anyone reaching that address could read and write configuration
and reach vault-backed secrets.

Manual removal (no dedicated uninstall path is provided for the web service):
  macOS:  launchctl bootout gui/\$UID/$LABEL ; rm -f $PLIST_DEST
  Linux:  systemctl --user disable --now ironhermes-web ; rm -f $SERVICE_DEST ; systemctl --user daemon-reload
  Both:   rm -rf $WEB_STAGE_DIR $SCRIPTS_DIR/web-run.sh
EOF

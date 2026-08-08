#!/usr/bin/env bash
# Production launcher for the iron_hermes_ui web server.
# Used directly, and by launchd/systemd via web-install.sh's registered service.
#
# Usage:
#   web-run.sh [args forwarded to the server binary]
#   web-run.sh -h|--help
#
# Resolves the built bundle, sources ~/.ironhermes/.env, gates the bind
# address (loopback-only unless explicitly opted in), then execs the server
# so SIGTERM/SIGINT reach it directly.
#
# Env vars:
#   IRONHERMES_HOME                    IronHermes home dir. Default: ~/.ironhermes
#   IRONHERMES_UI_BUNDLE_DIR           Explicit bundle dir override.
#   IRONHERMES_UI_BIN                  Explicit server binary override.
#   IRONHERMES_WEB_BIND                Bind address. Default: 127.0.0.1
#   IRONHERMES_WEB_PORT                Bind port. Default: 8080
#   IRONHERMES_WEB_ALLOW_PUBLIC_BIND   Set to 1 to allow a non-loopback bind.
#   DIOXUS_PUBLIC_PATH                 Escape hatch if the binary is not
#                                       sitting next to its bundle's public/.
#
# SECURITY: authentication is opt-in via web_ui.auth.password_hash. The
# binary itself refuses to bind a non-loopback address with no hash
# configured (bind_guard_allows in main.rs, checked before TcpListener::bind)
# -- this is the real, fail-closed guarantee, reachable no matter how the
# binary is launched. The check below is an earlier, friendlier layer in
# front of that same rule: a non-loopback IRONHERMES_WEB_BIND is refused
# unless IRONHERMES_WEB_ALLOW_PUBLIC_BIND=1, and even then a reminder that a
# password hash is required is printed before the server starts. See
# docs/DEPLOYMENT.md.

set -euo pipefail

for arg in "$@"; do
    case "$arg" in
        -h|--help)
            sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
    esac
done

SOURCE_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SOURCE_DIR/../.." && pwd)"
IRONHERMES_HOME_DIR="${IRONHERMES_HOME:-$HOME/.ironhermes}"
ENV_FILE="$IRONHERMES_HOME_DIR/.env"

log()  { printf '[web-run] %s\n' "$*"; }
die()  { printf '[web-run] ERROR: %s\n' "$*" >&2; exit 1; }
warn() { printf '[web-run] WARN: %s\n' "$*" >&2; }

# ---------- resolve bundle dir ----------
# Precedence: explicit override -> staged install -> workspace release build.
if [ -n "${IRONHERMES_UI_BUNDLE_DIR:-}" ]; then
    BUNDLE_DIR="$IRONHERMES_UI_BUNDLE_DIR"
    log "bundle dir (IRONHERMES_UI_BUNDLE_DIR): $BUNDLE_DIR"
elif [ -d "$IRONHERMES_HOME_DIR/web" ]; then
    BUNDLE_DIR="$IRONHERMES_HOME_DIR/web"
    log "bundle dir (staged by web-install.sh): $BUNDLE_DIR"
else
    BUNDLE_DIR="$REPO_ROOT/target/dx/iron_hermes_ui/release/web"
    log "bundle dir (workspace release build): $BUNDLE_DIR"
fi

BIN="${IRONHERMES_UI_BIN:-$BUNDLE_DIR/iron_hermes_ui}"

# gateway-run.sh's missing-binary check exits 127; mirror that exact exit code
# here so process supervisors (systemd/launchd) see the same "not found"
# signal for either service.
if [ ! -x "$BIN" ]; then
    printf '[web-run] ERROR: server binary not found or not executable: %s (run scripts/deploy/web-build.sh)\n' "$BIN" >&2
    exit 127
fi

# Assets resolve as current_exe().parent()/public (dioxus-server), NOT
# cwd-relative — so a binary moved away from its sibling public/ dir serves a
# blank page. Warn, don't die: DIOXUS_PUBLIC_PATH is a legitimate escape hatch
# an operator may already have set.
BIN_DIR="$(cd -- "$(dirname -- "$BIN")" && pwd)"
if [ ! -d "$BIN_DIR/public" ] && [ -z "${DIOXUS_PUBLIC_PATH:-}" ]; then
    warn "no public/ directory next to $BIN — assets will not be found unless DIOXUS_PUBLIC_PATH is set"
fi

# ---------- load .env ----------
if [ -f "$ENV_FILE" ]; then
    set -a
    # shellcheck disable=SC1090
    . "$ENV_FILE"
    set +a
fi

# ---------- resolve bind (AFTER .env is sourced) ----------
# Load-bearing ordering: we resolve IRONHERMES_WEB_BIND/PORT, gate them, and
# export IP/PORT AFTER .env is sourced, and nothing below re-sources .env
# afterward. If the order were reversed (export first, source .env second), a
# stale or attacker-planted raw IP=/PORT= line left in .env would silently
# overwrite our already-gated export, bypassing the check entirely. Resolving
# and exporting last means our export is always the final word.
BIND="${IRONHERMES_WEB_BIND:-127.0.0.1}"
PORT="${IRONHERMES_WEB_PORT:-8080}"

case "$BIND" in
    127.0.0.1|::1|localhost)
        : # loopback — always allowed
        ;;
    *)
        if [ "${IRONHERMES_WEB_ALLOW_PUBLIC_BIND:-}" != "1" ]; then
            die "refusing non-loopback bind '$BIND': iron_hermes_ui requires web_ui.auth.password_hash to be configured before it will accept connections on a non-loopback address (the binary itself enforces this at startup). Set IRONHERMES_WEB_ALLOW_PUBLIC_BIND=1 if you have already configured a password hash and intend this."
        fi
        warn "binding iron_hermes_ui to non-loopback address $BIND:$PORT"
        warn "the server itself will refuse to start on this address unless web_ui.auth.password_hash is configured — this script's check is an earlier, friendlier version of that same fail-closed rule, not the only guard."
        warn "generate a password hash with: ironhermes web set-password"
        warn "on plain LAN (no Tailscale/WireGuard overlay), the login credential and session cookie are sniffable — see docs/DEPLOYMENT.md's security section."
        ;;
esac

# dioxus_cli_config::fullstack_address_or_localhost() reads exactly these two
# env var names (SERVER_IP_ENV=IP, SERVER_PORT_ENV=PORT) — do not rename.
export IP="$BIND"
export PORT="$PORT"
log "binding to $BIND:$PORT"

mkdir -p "$IRONHERMES_HOME_DIR"
cd "$IRONHERMES_HOME_DIR"

# exec (not a subshell) so SIGTERM/SIGINT reach the server directly, letting
# with_graceful_shutdown finish and the tracing WorkerGuards flush before exit.
exec "$BIN" "$@"

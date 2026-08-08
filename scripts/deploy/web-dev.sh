#!/usr/bin/env bash
# Local / staging dev server for the iron_hermes_ui web UI.
#
# This is a DEV server, NOT the production path. `dx serve` proxies requests
# through the CLI's own dev server, which imposes a WebSocket idle timeout
# (see docs/DEVELOPMENT.md) — production uses the standalone binary via
# scripts/deploy/web-build.sh + scripts/deploy/web-run.sh instead.
#
# Usage:
#   web-dev.sh [args forwarded to `dx serve`, e.g. --platform desktop]
#   web-dev.sh -h|--help
#
# Env vars (SAME contract as web-run.sh — the two scripts must agree so an
# operator does not learn one gate for dev and a different one for prod):
#   IRONHERMES_WEB_BIND                Bind address. Default: 127.0.0.1
#   IRONHERMES_WEB_PORT                Bind port. Default: 8080
#   IRONHERMES_WEB_ALLOW_PUBLIC_BIND   Set to 1 to allow a non-loopback bind.
#
# SECURITY: authentication is opt-in via web_ui.auth.password_hash. The
# binary itself refuses to bind a non-loopback address with no hash
# configured (bind_guard_allows in main.rs, checked before TcpListener::bind)
# -- this is the real, fail-closed guarantee. The check below is an earlier,
# friendlier layer in front of that same rule: a non-loopback
# IRONHERMES_WEB_BIND is refused unless IRONHERMES_WEB_ALLOW_PUBLIC_BIND=1,
# and even then a reminder that a password hash is required is printed
# before the dev server starts. See docs/DEPLOYMENT.md.

set -euo pipefail

for arg in "$@"; do
    case "$arg" in
        -h|--help)
            sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
    esac
done

SOURCE_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SOURCE_DIR/../.." && pwd)"

log()  { printf '[web-dev] %s\n' "$*"; }
die()  { printf '[web-dev] ERROR: %s\n' "$*" >&2; exit 1; }
warn() { printf '[web-dev] WARN: %s\n' "$*" >&2; }

command -v dx >/dev/null 2>&1 || die "dx not found on PATH (install with: cargo install dioxus-cli)"

log "this is the LOCAL/STAGING dev server, not the production path"
log "production: scripts/deploy/web-build.sh + scripts/deploy/web-run.sh"

# ---------- resolve bind — SAME contract and gate as web-run.sh ----------
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
        warn "binding iron_hermes_ui dev server to non-loopback address $BIND:$PORT"
        warn "the server itself will refuse to start on this address unless web_ui.auth.password_hash is configured — this script's check is an earlier, friendlier version of that same fail-closed rule, not the only guard."
        warn "generate a password hash with: ironhermes web set-password"
        warn "on plain LAN (no Tailscale/WireGuard overlay), the login credential and session cookie are sniffable — see docs/DEPLOYMENT.md's security section."
        ;;
esac

# Never cd into the crate directory: iron_hermes_ui is excluded from
# [workspace] default-members, and `dx` run from inside the crate dir panics
# in Dioxus's find_main_package. Always run from the workspace root.
cd "$REPO_ROOT"

log "running: dx serve --package iron_hermes_ui --addr $BIND --port $PORT $*"
exec dx serve --package iron_hermes_ui --addr "$BIND" --port "$PORT" "$@"

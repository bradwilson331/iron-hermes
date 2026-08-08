#!/bin/bash
# =============================================================================
# IronHermes AaaS web-server entrypoint
# Mirrors docker/entrypoint.sh (privilege drop + template seeding) but execs the
# iron_hermes_ui fullstack server instead of the `ironhermes` CLI.
# Binds 0.0.0.0:8080 (IP/PORT env, set in the Dockerfile) — requires a web
# password hash (IRONHERMES_WEB_PASSWORD_HASH / config.yaml) or the app's
# fail-closed bind guard refuses to start.
# =============================================================================
set -euo pipefail

IRONHERMES_HOME="${IRONHERMES_HOME:-/opt/data}"
INSTALL_DIR="/opt/ironhermes"

# ─── Privilege drop (start as root, switch to ironhermes) ───
if [ "$(id -u)" = "0" ]; then
    chown -R ironhermes:ironhermes "$IRONHERMES_HOME" 2>/dev/null || \
        echo "Warning: chown failed (rootless?) — continuing"
    exec gosu ironhermes "$0" "$@"
fi

# ─── Running as ironhermes user ───
mkdir -p "$IRONHERMES_HOME"/{cron,sessions,logs,hooks,memories,skills,workspace}

# Seed templates only if absent (preserve the .env the AaaS provisioner wrote,
# which carries IRONHERMES_WEB_PASSWORD_HASH + provider keys).
[ -f "$IRONHERMES_HOME/.env" ]        || cp "$INSTALL_DIR/.env.example" "$IRONHERMES_HOME/.env"
[ -f "$IRONHERMES_HOME/config.yaml" ] || cp "$INSTALL_DIR/cli-config.yaml.example" "$IRONHERMES_HOME/config.yaml"
[ -f "$IRONHERMES_HOME/SOUL.md" ]     || cp "$INSTALL_DIR/docker/SOUL.md" "$IRONHERMES_HOME/SOUL.md"
chmod 600 "$IRONHERMES_HOME/.env" 2>/dev/null || true

# The fullstack server resolves assets relative to the bundle dir.
cd "$INSTALL_DIR/web"
exec ./iron_hermes_ui

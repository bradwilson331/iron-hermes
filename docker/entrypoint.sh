#!/bin/bash
# =============================================================================
# IronHermes Docker Entrypoint
# =============================================================================
# Ported from hermes-agent/docker/entrypoint.sh
# Handles: privilege dropping via gosu, UID/GID remapping, directory creation,
#          template seeding (.env, config.yaml, SOUL.md), then exec ironhermes.
# =============================================================================
set -euo pipefail

IRONHERMES_HOME="${IRONHERMES_HOME:-/opt/data}"
INSTALL_DIR="/opt/ironhermes"

# ─── Privilege drop (run as root initially, then switch to ironhermes) ───
if [ "$(id -u)" = "0" ]; then
    # Remap UID/GID if overridden by environment (Docker -e IRONHERMES_UID=1000)
    if [ -n "${IRONHERMES_UID:-}" ]; then
        if ! echo "$IRONHERMES_UID" | grep -qE '^[0-9]+$'; then
            echo "Error: IRONHERMES_UID must be numeric, got: $IRONHERMES_UID" >&2
            exit 1
        fi
        usermod -u "$IRONHERMES_UID" ironhermes
    fi
    if [ -n "${IRONHERMES_GID:-}" ]; then
        if ! echo "$IRONHERMES_GID" | grep -qE '^[0-9]+$'; then
            echo "Error: IRONHERMES_GID must be numeric, got: $IRONHERMES_GID" >&2
            exit 1
        fi
        groupmod -o -g "$IRONHERMES_GID" ironhermes 2>/dev/null || true
    fi

    # Fix ownership of the data volume (may fail in rootless — that's OK)
    chown -R ironhermes:ironhermes "$IRONHERMES_HOME" 2>/dev/null || \
        echo "Warning: chown failed (rootless container?) — continuing anyway"

    # Re-exec this script as the ironhermes user
    exec gosu ironhermes "$0" "$@"
fi

# ─── Running as ironhermes user from here ────────────────────────────────

# D-13: Create essential directories
mkdir -p "$IRONHERMES_HOME"/{cron,sessions,logs,hooks,memories,skills,workspace}

# ─── Skill seeding (image → volume, skip-if-exists) ───
# /opt/ironhermes/{skills,optional-skills} are the read-only copies baked into
# the image; the agent only ever scans the volume. `cp -rn` merges new
# categories and new skills in on an image upgrade while never overwriting an
# operator edit or a Hub install already on the volume.
#
# skills/ lands in $IRONHERMES_HOME/skills, a default search root, so those
# skills are live immediately. optional-skills/ lands in
# $IRONHERMES_HOME/optional-skills, which is NOT a default search root — opt in
# by adding it to skills.extra_paths in config.yaml, or import individual
# skills from it in the web UI (its Local Path quick-pick already offers this
# directory). Non-fatal: a seeding failure must never take the container down.
for _src in skills optional-skills; do
    if [ -d "$INSTALL_DIR/$_src" ]; then
        mkdir -p "$IRONHERMES_HOME/$_src"
        cp -rn "$INSTALL_DIR/$_src/." "$IRONHERMES_HOME/$_src/" 2>/dev/null || \
            echo "Warning: seeding $_src into $IRONHERMES_HOME failed — continuing" >&2
    fi
done


# D-14: Copy templates only if they don't already exist (preserve user edits)
[ ! -f "$IRONHERMES_HOME/.env" ]        && cp "$INSTALL_DIR/.env.example" "$IRONHERMES_HOME/.env"
[ ! -f "$IRONHERMES_HOME/config.yaml" ] && cp "$INSTALL_DIR/cli-config.yaml.example" "$IRONHERMES_HOME/config.yaml"
[ ! -f "$IRONHERMES_HOME/SOUL.md" ]     && cp "$INSTALL_DIR/docker/SOUL.md" "$IRONHERMES_HOME/SOUL.md"

# Restrict .env file permissions (contains API keys)
chmod 600 "$IRONHERMES_HOME/.env" 2>/dev/null || true

# Hand off to ironhermes binary
exec ironhermes "$@"

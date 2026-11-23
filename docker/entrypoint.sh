#!/bin/bash
# =============================================================================
# IronHermes Docker Entrypoint
# =============================================================================
# Ported from hermes-agent/docker/entrypoint.sh
# Handles: privilege dropping via setpriv (docker/priv-drop.sh, fail closed),
#          UID/GID remapping, directory creation,
#          template seeding (.env, config.yaml, SOUL.md), then exec ironhermes.
# =============================================================================
set -euo pipefail

IRONHERMES_HOME="${IRONHERMES_HOME:-/opt/data}"
INSTALL_DIR="/opt/ironhermes"

# ─── Privilege drop (run as root initially, then switch to ironhermes) ───
# Shared with web-entrypoint.sh (docker/priv-drop.sh); FAILS CLOSED.
if [ "$(id -u)" = "0" ]; then
    # shellcheck source=/dev/null
    . "$INSTALL_DIR/docker/priv-drop.sh" || {
        echo "Error: cannot load $INSTALL_DIR/docker/priv-drop.sh -- refusing to run as root" >&2
        exit 1
    }
    ih_drop_privileges "$0" "$@"
    echo "Error: privilege drop failed -- refusing to run as root" >&2
    exit 1
fi

# ─── Running as ironhermes user from here ────────────────────────────────

# D-16: user installs land on the volume, not the read-only rootfs. Non-fatal.
# shellcheck source=/dev/null
if . "$INSTALL_DIR/docker/priv-drop.sh" && ih_volume_env; then :; else
    echo "Warning: could not set the volume install environment -- continuing" >&2
fi

# D-13: Create essential directories
mkdir -p "$IRONHERMES_HOME"/{cron,sessions,logs,hooks,memories,skills,workspace}

# >>> skills seeding
# ─── Skill seeding (image → volume, history-aware) ───
# /opt/ironhermes/{skills,optional-skills} are the read-only copies baked into
# the image; the agent only ever scans the volume. Seeding goes through the
# shared ih_seed_bundled_tree (docker/seed-bundled.sh, the same function
# install.sh embeds): a file absent from the volume is copied, a file that is
# an unmodified OLDER bundled version (listed in skills/.bundled-history) is
# upgraded to the current one, and a file the operator edited, or a Hub
# install already on the volume, is kept. Plain `cp -rn` could never deliver a
# changed bundled skill to an existing volume.
#
# Both trees are default search roots: skills/ lands in $IRONHERMES_HOME/skills
# and optional-skills/ in $IRONHERMES_HOME/optional-skills, which is a default
# root since Phase 54.1 (Official tier), so seeded skills are live immediately.
# Non-fatal: a seeding failure must never take the container down.
_ih_seed_lib=0
if [ -f "$INSTALL_DIR/docker/seed-bundled.sh" ]; then
    # shellcheck source=/dev/null
    if . "$INSTALL_DIR/docker/seed-bundled.sh"; then
        _ih_seed_lib=1
    fi
fi
for _src in skills optional-skills; do
    if [ -d "$INSTALL_DIR/$_src" ]; then
        mkdir -p "$IRONHERMES_HOME/$_src"
        if [ "$_ih_seed_lib" = "1" ]; then
            ih_seed_bundled_tree "$INSTALL_DIR/$_src" "$IRONHERMES_HOME/$_src" \
                "$INSTALL_DIR/skills/.bundled-history" "$_src" || \
                echo "Warning: seeding $_src into $IRONHERMES_HOME was incomplete — continuing" >&2
        else
            # Older image layout without the shared library: copy-absent only.
            cp -rn "$INSTALL_DIR/$_src/." "$IRONHERMES_HOME/$_src/" 2>/dev/null || \
                echo "Warning: seeding $_src into $IRONHERMES_HOME was incomplete — continuing" >&2
        fi
    fi
done
# <<< skills seeding


# D-14: Copy templates only if they don't already exist (preserve user edits)
[ ! -f "$IRONHERMES_HOME/.env" ]        && cp "$INSTALL_DIR/.env.example" "$IRONHERMES_HOME/.env"
# D-13: config.yaml is seeded ONCE, from IRONHERMES_CONFIG_TEMPLATE when it is
# set and readable, else from the baked example. Non-fatal.
if [ ! -f "$IRONHERMES_HOME/config.yaml" ]; then
    _cfg_src="$INSTALL_DIR/cli-config.yaml.example"
    if [ -n "${IRONHERMES_CONFIG_TEMPLATE:-}" ]; then
        if [ -r "$IRONHERMES_CONFIG_TEMPLATE" ]; then
            _cfg_src="$IRONHERMES_CONFIG_TEMPLATE"
        else
            echo "Warning: IRONHERMES_CONFIG_TEMPLATE=$IRONHERMES_CONFIG_TEMPLATE is not readable -- using the baked example" >&2
        fi
    fi
    cp "$_cfg_src" "$IRONHERMES_HOME/config.yaml" || \
        echo "Warning: could not seed config.yaml from $_cfg_src -- continuing" >&2
fi
[ ! -f "$IRONHERMES_HOME/SOUL.md" ]     && cp "$INSTALL_DIR/docker/SOUL.md" "$IRONHERMES_HOME/SOUL.md"

# Restrict .env file permissions (contains API keys)
chmod 600 "$IRONHERMES_HOME/.env" 2>/dev/null || true

# Hand off to ironhermes binary
exec ironhermes "$@"

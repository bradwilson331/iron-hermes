#!/usr/bin/env bash
# IronHermes — combined gateway + web update.
#
# Usage:
#   update.sh                    # build + install + restart BOTH gateway and web
#   update.sh --gateway-only     # restrict scope to the gateway
#   update.sh --web-only         # restrict scope to the web UI
#   update.sh --skip-build       # deploy artifacts already on disk; skip both builds
#   update.sh --no-start         # forwarded to both installers; also skips the health probe
#   update.sh --force            # forwarded to both installers
#   update.sh --skip-wasm-check  # forwarded to web-build.sh
#   update.sh --cron             # forwarded to install.sh (gateway watchdog mode)
#   update.sh --dry-run          # print every step that would run; execute nothing
#   update.sh -h|--help
#
# --gateway-only and --web-only are mutually exclusive (exit 2). --cron is a
# gateway-only deployment model, so combining it with --web-only is also a
# usage error (exit 2).
#
# Effects:
#   1. PREFLIGHT — verifies cargo/dx are on PATH as needed by the selected
#      scope, and that every sibling script this run will invoke exists and
#      is executable, so a missing tool surfaces in seconds, not after a
#      multi-minute build.
#   2. BUILD (skipped entirely under --skip-build) — gateway:
#      `cargo build --release --bin ironhermes` from the repo root. Web:
#      scripts/deploy/web-build.sh. Either failing aborts the whole run
#      immediately — nothing is installed and no service is restarted.
#   3. INSTALL + RESTART — gateway: scripts/deploy/install.sh. Web:
#      scripts/deploy/web-install.sh (gateway first, so install.sh's
#      `ironhermes doctor` check runs before the web bundle is staged).
#      These delegated scripts perform the actual restart (systemctl --user
#      restart / launchctl kickstart -k) — update.sh never restarts a
#      service directly.
#   4. HEALTH PROBE (skipped under --no-start or --dry-run) — confirms every
#      component actually restarted is running, retrying briefly to absorb
#      startup latency, and exits non-zero with a log-inspection command if
#      any component is not up. A deploy that leaves a service down must
#      never report success.
#
# No debug/non-release web build is offered: web-install.sh only ever
# stages target/dx/iron_hermes_ui/release/web, so a debug bundle would be
# silently ignored in favor of a stale release bundle. Don't add a --debug
# passthrough here.
#
# This script never sources ~/.ironhermes/.env and never echoes environment
# values — secret loading belongs to gateway-run.sh / web-run.sh at launch.
#
# Platform detection, binary install, bundle staging, and service restart
# all remain in the sibling scripts above; this script only orchestrates
# them in the right order.

set -euo pipefail

SOURCE_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SOURCE_DIR/../.." && pwd)"
HOME_DIR="$HOME"
IRONHERMES_HOME_DIR="${IRONHERMES_HOME:-$HOME_DIR/.ironhermes}"

GATEWAY_ONLY=0
WEB_ONLY=0
SKIP_BUILD=0
NO_START=0
FORCE=0
SKIP_WASM_CHECK=0
CRON=0
DRY_RUN=0

for arg in "$@"; do
    case "$arg" in
        --gateway-only)     GATEWAY_ONLY=1 ;;
        --web-only)          WEB_ONLY=1 ;;
        --skip-build)        SKIP_BUILD=1 ;;
        --no-start)          NO_START=1 ;;
        --force)             FORCE=1 ;;
        --skip-wasm-check)   SKIP_WASM_CHECK=1 ;;
        --cron)               CRON=1 ;;
        --dry-run)            DRY_RUN=1 ;;
        -h|--help)
            sed -n '2,51p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *) echo "unknown arg: $arg" >&2; exit 2 ;;
    esac
done

if [ "$GATEWAY_ONLY" -eq 1 ] && [ "$WEB_ONLY" -eq 1 ]; then
    echo "usage: --gateway-only and --web-only are mutually exclusive" >&2
    exit 2
fi
if [ "$CRON" -eq 1 ] && [ "$WEB_ONLY" -eq 1 ]; then
    echo "usage: --cron is a gateway-only deployment mode and cannot be combined with --web-only" >&2
    exit 2
fi

DO_GATEWAY=1
DO_WEB=1
if [ "$GATEWAY_ONLY" -eq 1 ]; then
    DO_WEB=0
fi
if [ "$WEB_ONLY" -eq 1 ]; then
    DO_GATEWAY=0
fi

log()  { printf '[update] %s\n' "$*"; }
die()  { printf '[update] ERROR: %s\n' "$*" >&2; exit 1; }
warn() { printf '[update] WARN: %s\n' "$*" >&2; }

# run: under --dry-run, print exactly one "[update] DRY-RUN: <cmd>" line and
# return 0 without executing. Otherwise print "[update] running: <cmd>" and
# execute it. "DRY-RUN: " must appear nowhere else in this script's output.
run() {
    if [ "$DRY_RUN" -eq 1 ]; then
        printf '[update] DRY-RUN: %s\n' "$*"
        return 0
    fi
    printf '[update] running: %s\n' "$*"
    "$@"
}

is_component_running() {
    local name="$1"
    if [ "$name" = "gateway" ] && [ "$CRON" -eq 1 ]; then
        local pid
        pid="$(awk '/^pid:[[:space:]]/ {print $2; exit}' "$IRONHERMES_HOME_DIR/gateway.pid" 2>/dev/null || true)"
        [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null
        return $?
    fi
    case "$(uname -s)" in
        Linux)
            systemctl --user is-active --quiet "ironhermes-$name.service"
            ;;
        Darwin)
            launchctl print "gui/$UID/com.ironhermes.$name" 2>/dev/null | grep -q 'state = running'
            ;;
        *)
            return 1
            ;;
    esac
}

probe_component() {
    local name="$1"
    local attempt=0
    while [ "$attempt" -lt 10 ]; do
        if is_component_running "$name"; then
            log "health probe: $name is running"
            return 0
        fi
        attempt=$((attempt + 1))
        if [ "$attempt" -lt 10 ]; then
            sleep 1
        fi
    done
    warn "health probe: $name is NOT running after 10 attempts"
    return 1
}

# ---------- phase 1/4: preflight ----------
log "phase 1/4: preflight"

if [ "$SKIP_BUILD" -ne 1 ]; then
    if ! command -v cargo >/dev/null 2>&1; then
        die "cargo not found on PATH"
    fi
    if [ "$DO_WEB" -eq 1 ]; then
        if ! command -v dx >/dev/null 2>&1; then
            die "dx not found on PATH (install with: cargo install dioxus-cli)"
        fi
    fi
fi

if [ "$DO_GATEWAY" -eq 1 ]; then
    if [ ! -x "$SOURCE_DIR/install.sh" ]; then
        die "missing or not executable: $SOURCE_DIR/install.sh"
    fi
fi

if [ "$DO_WEB" -eq 1 ]; then
    if [ "$SKIP_BUILD" -ne 1 ]; then
        if [ ! -x "$SOURCE_DIR/web-build.sh" ]; then
            die "missing or not executable: $SOURCE_DIR/web-build.sh"
        fi
    fi
    if [ ! -x "$SOURCE_DIR/web-install.sh" ]; then
        die "missing or not executable: $SOURCE_DIR/web-install.sh"
    fi
fi

# ---------- phase 2/4: build ----------
log "phase 2/4: build"

if [ "$SKIP_BUILD" -eq 1 ]; then
    log "skipping build (--skip-build)"
else
    if [ "$DO_GATEWAY" -eq 1 ]; then
        if ! ( cd "$REPO_ROOT" && run cargo build --release --bin ironhermes ); then
            die "gateway build failed — aborting before any install/restart"
        fi
    fi
    if [ "$DO_WEB" -eq 1 ]; then
        WEB_BUILD_ARGS=()
        if [ "$SKIP_WASM_CHECK" -eq 1 ]; then
            WEB_BUILD_ARGS+=(--skip-wasm-check)
        fi
        if ! run "$SOURCE_DIR/web-build.sh" "${WEB_BUILD_ARGS[@]+"${WEB_BUILD_ARGS[@]}"}"; then
            die "web build failed — aborting before any install/restart"
        fi
    fi
fi

# ---------- phase 3/4: install + restart ----------
log "phase 3/4: install + restart"

if [ "$DO_GATEWAY" -eq 1 ]; then
    INSTALL_ARGS=()
    if [ "$CRON" -eq 1 ]; then
        INSTALL_ARGS+=(--cron)
    fi
    if [ "$NO_START" -eq 1 ]; then
        INSTALL_ARGS+=(--no-start)
    fi
    if [ "$FORCE" -eq 1 ]; then
        INSTALL_ARGS+=(--force)
    fi
    if ! run "$SOURCE_DIR/install.sh" "${INSTALL_ARGS[@]+"${INSTALL_ARGS[@]}"}"; then
        die "gateway install failed"
    fi
fi

if [ "$DO_WEB" -eq 1 ]; then
    WEB_INSTALL_ARGS=()
    if [ "$NO_START" -eq 1 ]; then
        WEB_INSTALL_ARGS+=(--no-start)
    fi
    if [ "$FORCE" -eq 1 ]; then
        WEB_INSTALL_ARGS+=(--force)
    fi
    if ! run "$SOURCE_DIR/web-install.sh" "${WEB_INSTALL_ARGS[@]+"${WEB_INSTALL_ARGS[@]}"}"; then
        die "web install failed"
    fi
fi

# ---------- phase 4/4: health probe ----------
log "phase 4/4: health probe"

if [ "$NO_START" -eq 1 ] || [ "$DRY_RUN" -eq 1 ]; then
    log "skipping health probe (--no-start or --dry-run)"
else
    FAILED_COMPONENTS=()
    if [ "$DO_GATEWAY" -eq 1 ]; then
        if ! probe_component gateway; then
            FAILED_COMPONENTS+=(gateway)
        fi
    fi
    if [ "$DO_WEB" -eq 1 ]; then
        if ! probe_component web; then
            FAILED_COMPONENTS+=(web)
        fi
    fi
    if [ "${#FAILED_COMPONENTS[@]}" -gt 0 ]; then
        for c in "${FAILED_COMPONENTS[@]}"; do
            case "$(uname -s)" in
                Linux)  log "  check logs: journalctl --user -u ironhermes-$c -n 50" ;;
                Darwin) log "  check logs: tail -n 50 $IRONHERMES_HOME_DIR/logs/$c.err.log" ;;
            esac
        done
        die "one or more services failed to come up: ${FAILED_COMPONENTS[*]}"
    fi
fi

# ---------- summary ----------
if [ "$DRY_RUN" -eq 1 ]; then
    BUILT_LABEL="  would build:     "
    INSTALLED_LABEL="  would install:   "
    RESTARTED_LABEL="  would restart:   "
else
    BUILT_LABEL="  built:     "
    INSTALLED_LABEL="  installed: "
    RESTARTED_LABEL="  restarted: "
fi

log "summary:"
if [ "$SKIP_BUILD" -ne 1 ]; then
    if [ "$DO_GATEWAY" -eq 1 ]; then
        log "${BUILT_LABEL}gateway"
    fi
    if [ "$DO_WEB" -eq 1 ]; then
        log "${BUILT_LABEL}web"
    fi
fi
if [ "$DO_GATEWAY" -eq 1 ]; then
    log "${INSTALLED_LABEL}gateway"
fi
if [ "$DO_WEB" -eq 1 ]; then
    log "${INSTALLED_LABEL}web"
fi
if [ "$NO_START" -eq 1 ]; then
    log "  restart:   skipped (--no-start)"
else
    if [ "$DO_GATEWAY" -eq 1 ]; then
        log "${RESTARTED_LABEL}gateway"
    fi
    if [ "$DO_WEB" -eq 1 ]; then
        log "${RESTARTED_LABEL}web"
    fi
fi

case "$(uname -s)" in
    Linux)
        if [ "$DO_GATEWAY" -eq 1 ]; then
            log "  verify: systemctl --user status ironhermes-gateway"
        fi
        if [ "$DO_WEB" -eq 1 ]; then
            log "  verify: systemctl --user status ironhermes-web"
        fi
        ;;
    Darwin)
        if [ "$DO_GATEWAY" -eq 1 ]; then
            log '  verify: launchctl print gui/$UID/com.ironhermes.gateway | grep -E "state|pid"'
        fi
        if [ "$DO_WEB" -eq 1 ]; then
            log '  verify: launchctl print gui/$UID/com.ironhermes.web | grep -E "state|pid"'
        fi
        ;;
esac

log "done"

#!/usr/bin/env bash
# Build the production release bundle for the iron_hermes_ui web server.
#
# Usage:
#   web-build.sh                    # release bundle (dx bundle --release)
#   web-build.sh --debug            # debug bundle instead (fast iteration)
#   web-build.sh --skip-wasm-check  # skip the wasm32 type-check gate
#   web-build.sh -h|--help
#
# Effects:
#   1. Runs the wasm32 type-check gate (unless --skip-wasm-check). A native
#      `cargo build`/`clippy` never compiles #[cfg(target_arch = "wasm32")]
#      code, so this is the only gate that catches wasm-only breakage before
#      a bundle ships.
#   2. Runs `dx bundle --platform web --package iron_hermes_ui` from the
#      workspace root (release profile by default).
#   3. Verifies the resulting bundle directory, binary, and public/ dir all
#      exist — a silent partial bundle must not be reported as success.
#
# Output: target/dx/iron_hermes_ui/<profile>/web/ (binary + public/ + manifest)
# Next: scripts/deploy/web-run.sh (run in place) or
#       scripts/deploy/web-install.sh (stage + register a service)

set -euo pipefail

SOURCE_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SOURCE_DIR/../.." && pwd)"

PROFILE="release"
SKIP_WASM_CHECK=0

for arg in "$@"; do
    case "$arg" in
        --debug)           PROFILE="debug" ;;
        --skip-wasm-check) SKIP_WASM_CHECK=1 ;;
        -h|--help)
            sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *) echo "unknown arg: $arg" >&2; exit 2 ;;
    esac
done

log()  { printf '[web-build] %s\n' "$*"; }
die()  { printf '[web-build] ERROR: %s\n' "$*" >&2; exit 1; }

# ---------- preflight ----------
command -v dx >/dev/null 2>&1 || die "dx not found on PATH (install with: cargo install dioxus-cli)"
command -v cargo >/dev/null 2>&1 || die "cargo not found on PATH"

# ---------- wasm type-check gate ----------
# A native `cargo build`/`clippy` never type-checks #[cfg(target_arch = "wasm32")]
# code, so this is the only gate that catches wasm-only breakage before a
# bundle ships. Skippable via --skip-wasm-check for fast local iteration.
if [ "$SKIP_WASM_CHECK" -ne 1 ]; then
    log "running wasm32 type-check gate"
    if ! ( cd "$REPO_ROOT" && RUSTFLAGS='--cfg getrandom_backend="wasm_js"' \
        cargo check --target wasm32-unknown-unknown -p iron_hermes_ui ); then
        die "wasm32 type-check failed — fix before bundling (or pass --skip-wasm-check to bypass at your own risk)"
    fi
else
    log "skipping wasm32 type-check gate (--skip-wasm-check)"
fi

# ---------- bundle ----------
# Never cd into the crate directory: iron_hermes_ui is excluded from
# [workspace] default-members, and `dx` run from inside the crate dir panics
# in Dioxus's find_main_package. Always run from the workspace root and pass
# --package explicitly.
cd "$REPO_ROOT"

case "$PROFILE" in
    release)
        log "running: dx bundle --platform web --release --package iron_hermes_ui"
        dx bundle --platform web --release --package iron_hermes_ui
        ;;
    debug)
        log "running: dx bundle --platform web --package iron_hermes_ui (debug profile)"
        dx bundle --platform web --package iron_hermes_ui
        ;;
esac

# ---------- verify bundle ----------
BUNDLE_DIR="$REPO_ROOT/target/dx/iron_hermes_ui/$PROFILE/web"
BIN="$BUNDLE_DIR/iron_hermes_ui"
PUBLIC_DIR="$BUNDLE_DIR/public"

[ -d "$BUNDLE_DIR" ] || die "bundle directory missing after build: $BUNDLE_DIR"
[ -x "$BIN" ]        || die "bundle binary missing or not executable: $BIN"
[ -d "$PUBLIC_DIR" ] || die "bundle public/ directory missing: $PUBLIC_DIR (a silent partial bundle must not be reported as success)"

log "bundle ready: $BUNDLE_DIR"
log "  binary: $BIN"
log "next: scripts/deploy/web-run.sh to run in place, scripts/deploy/web-install.sh to stage + register a service"

#!/usr/bin/env bash
# Build the release binary, put it in ~/.local/bin, then run the installer.
# Extra args (--cron, --no-start, --force) are passed through to install.sh.
set -euo pipefail

SOURCE_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SOURCE_DIR/../.." && pwd)"
BIN="${IRONHERMES_BIN:-$HOME/.local/bin/ironhermes}"
# Keep the dx CLI on the same version as the `dioxus` crate pinned in
# crates/iron_hermes_ui/Cargo.toml (=0.7.1). Override with DIOXUS_CLI_VERSION.
DIOXUS_CLI_VERSION="${DIOXUS_CLI_VERSION:-0.7.1}"

log() { printf '[install-build] %s\n' "$*"; }
die() { printf '[install-build] ERROR: %s\n' "$*" >&2; exit 1; }

# pkg-config modules needed at build time on Linux:
#   alsa    -> alsa-sys     (ALSA dev headers)
#   openssl -> openssl-sys  (needed to compile dioxus-cli from source)
LINUX_PC_MODULES="alsa openssl"

missing_pc_modules() {
    local m
    command -v pkg-config >/dev/null 2>&1 || { echo "pkg-config"; return; }
    for m in $LINUX_PC_MODULES; do
        pkg-config --exists "$m" || printf '%s ' "$m"
    done
}

# Install the dev packages providing those .pc files (plus pkg-config) with the
# host's package manager if any are missing.
ensure_linux_deps() {
    local missing
    missing="$(missing_pc_modules)"
    [ -z "${missing// /}" ] && return 0

    local sudo=""
    if [ "$(id -u)" -ne 0 ]; then
        command -v sudo >/dev/null 2>&1 || die "missing build deps ($missing) and sudo not available; install manually (Debian/Ubuntu: libasound2-dev libssl-dev pkg-config)"
        sudo="sudo"
    fi

    log "missing build dependencies: $missing- installing dev packages"
    if command -v apt-get >/dev/null 2>&1; then
        $sudo apt-get update
        $sudo apt-get install -y libasound2-dev libssl-dev pkg-config
    elif command -v dnf >/dev/null 2>&1; then
        $sudo dnf install -y alsa-lib-devel openssl-devel pkgconf-pkg-config
    elif command -v yum >/dev/null 2>&1; then
        $sudo yum install -y alsa-lib-devel openssl-devel pkgconfig
    elif command -v pacman >/dev/null 2>&1; then
        $sudo pacman -S --needed --noconfirm alsa-lib openssl pkgconf
    elif command -v zypper >/dev/null 2>&1; then
        $sudo zypper --non-interactive install alsa-devel libopenssl-devel pkg-config
    elif command -v apk >/dev/null 2>&1; then
        $sudo apk add alsa-lib-dev openssl-dev pkgconf
    else
        die "unknown package manager; install dev packages providing alsa.pc and openssl.pc, plus pkg-config, manually"
    fi

    # Package installed but a .pc file lives somewhere pkg-config doesn't look:
    # point PKG_CONFIG_PATH at it.
    local m pc
    for m in $LINUX_PC_MODULES; do
        if ! pkg-config --exists "$m"; then
            pc="$(find /usr/lib /usr/lib64 /usr/share /usr/local/lib -name "$m.pc" 2>/dev/null | head -n1 || true)"
            if [ -n "$pc" ]; then
                export PKG_CONFIG_PATH="$(dirname "$pc")${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
                log "using PKG_CONFIG_PATH=$PKG_CONFIG_PATH"
            fi
        fi
        pkg-config --exists "$m" || die "$m.pc still not found after install"
    done
}

# Install the Dioxus CLI (`dx`) at the pinned 0.7 version if it's missing or
# a different version. Prefers a prebuilt binary via cargo-binstall; falls
# back to compiling with `cargo install`.
ensure_dx() {
    if command -v dx >/dev/null 2>&1 && dx --version 2>/dev/null | grep -qF "$DIOXUS_CLI_VERSION"; then
        log "dx $DIOXUS_CLI_VERSION already installed"
    else
        log "installing dioxus-cli $DIOXUS_CLI_VERSION (dx)"
        if command -v cargo-binstall >/dev/null 2>&1; then
            cargo binstall -y --force "dioxus-cli@$DIOXUS_CLI_VERSION"
        else
            cargo install dioxus-cli --version "=$DIOXUS_CLI_VERSION" --locked --force
        fi
        command -v dx >/dev/null 2>&1 || die "dx not on PATH after install; ensure ~/.cargo/bin is in PATH"
    fi

    # dx web builds need the wasm target.
    if command -v rustup >/dev/null 2>&1; then
        rustup target add wasm32-unknown-unknown
    fi
}

if [ "$(uname -s)" = "Linux" ]; then
    ensure_linux_deps
fi

ensure_dx

cd "$REPO_ROOT"
cargo build --release

mkdir -p "$(dirname "$BIN")"
# `install` unlinks the destination first, so this is safe while the service runs.
install -m 755 target/release/ironhermes "$BIN"

"$SOURCE_DIR/install.sh" "$@"

#!/bin/bash
# =============================================================================
# IronHermes Developer Setup
# =============================================================================
# Run after cloning the repo: ./setup-ironhermes.sh
#
# Steps:
# 1. Check for Rust toolchain
# 2. Build release binary
# 3. Scaffold ~/.ironhermes/ directory structure
# 4. Copy config templates (.env, config.yaml, SOUL.md)
# 5. Optionally symlink binary and update PATH
# =============================================================================
set -e

# --- Constants ---
IRONHERMES_HOME="${IRONHERMES_HOME:-$HOME/.ironhermes}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
INSTALL_DIR="$HOME/.local/bin"

# --- Color helpers ---
RED=""
GREEN=""
YELLOW=""
BLUE=""
BOLD=""
RESET=""
if [ -t 1 ] && command -v tput >/dev/null 2>&1; then
    RED=$(tput setaf 1)
    GREEN=$(tput setaf 2)
    YELLOW=$(tput setaf 3)
    BLUE=$(tput setaf 4)
    BOLD=$(tput bold)
    RESET=$(tput sgr0)
fi

log_info()  { echo "${BLUE}${BOLD}[INFO]${RESET}  $*"; }
log_ok()    { echo "${GREEN}${BOLD}[OK]${RESET}    $*"; }
log_warn()  { echo "${YELLOW}${BOLD}[WARN]${RESET}  $*"; }
log_error() { echo "${RED}${BOLD}[ERROR]${RESET} $*" >&2; }

# --- Pre-flight checks ---
check_rust() {
    if ! command -v cargo >/dev/null 2>&1; then
        log_error "Rust toolchain not found!"
        log_error "Install Rust: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
        exit 1
    fi
    local rust_version
    rust_version=$(rustc --version 2>/dev/null || echo "unknown")
    log_ok "Rust found: ${rust_version}"
}

# --- Build ---
build_release() {
    log_info "Building IronHermes (release mode)..."
    cd "$SCRIPT_DIR"
    cargo build --release --bin ironhermes
    log_ok "Build complete: target/release/ironhermes"
}

# --- Scaffold directories ---
scaffold_home() {
    log_info "Setting up ${IRONHERMES_HOME}..."
    mkdir -p "$IRONHERMES_HOME"/{cron,sessions,logs,hooks,memories,skills,workspace}
    log_ok "Directory structure created"
}

# --- Copy templates from repo ---
seed_templates() {
    if [ ! -f "$IRONHERMES_HOME/.env" ]; then
        cp "$SCRIPT_DIR/.env.example" "$IRONHERMES_HOME/.env"
        chmod 600 "$IRONHERMES_HOME/.env"
        log_ok "Copied .env.example -> ${IRONHERMES_HOME}/.env"
    else
        log_info ".env already exists -- skipping"
    fi

    if [ ! -f "$IRONHERMES_HOME/config.yaml" ]; then
        cp "$SCRIPT_DIR/cli-config.yaml.example" "$IRONHERMES_HOME/config.yaml"
        log_ok "Copied cli-config.yaml.example -> ${IRONHERMES_HOME}/config.yaml"
    else
        log_info "config.yaml already exists -- skipping"
    fi

    if [ ! -f "$IRONHERMES_HOME/SOUL.md" ]; then
        cp "$SCRIPT_DIR/docker/SOUL.md" "$IRONHERMES_HOME/SOUL.md"
        log_ok "Copied SOUL.md -> ${IRONHERMES_HOME}/SOUL.md"
    else
        log_info "SOUL.md already exists -- skipping"
    fi
}

# --- Symlink + PATH ---
install_binary() {
    mkdir -p "$INSTALL_DIR"

    local binary="$SCRIPT_DIR/target/release/ironhermes"
    if [ -f "$binary" ]; then
        ln -sf "$binary" "$INSTALL_DIR/ironhermes"
        log_ok "Symlinked to ${INSTALL_DIR}/ironhermes"
    else
        log_warn "Binary not found at ${binary} -- skipping symlink"
        return
    fi

    # Update PATH if needed
    local path_line="export PATH=\"${INSTALL_DIR}:\$PATH\""
    local updated=false
    for rc in "$HOME/.bashrc" "$HOME/.zshrc" "$HOME/.profile"; do
        if [ -f "$rc" ] && ! grep -qF "$INSTALL_DIR" "$rc"; then
            echo "" >> "$rc"
            echo "# Added by IronHermes setup" >> "$rc"
            echo "$path_line" >> "$rc"
            updated=true
        fi
    done

    if [ "$updated" = true ]; then
        log_ok "PATH updated (restart shell or: source ~/.bashrc)"
    fi
}

# --- Main ---
main() {
    echo ""
    echo "${BOLD}IronHermes Developer Setup${RESET}"
    echo "========================================"
    echo ""

    check_rust
    build_release
    scaffold_home
    seed_templates
    install_binary

    echo ""
    echo "========================================"
    log_ok "Setup complete!"
    echo ""
    echo "  Home directory: ${IRONHERMES_HOME}"
    echo "  Binary:         ${INSTALL_DIR}/ironhermes"
    echo ""
    echo "  Next steps:"
    echo "    1. Edit ${IRONHERMES_HOME}/.env with your API keys"
    echo "    2. Run: ironhermes"
    echo "    3. Run: ironhermes doctor (check setup)"
    echo ""
}

main "$@"

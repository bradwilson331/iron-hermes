#!/bin/bash
# =============================================================================
# IronHermes Installer
# =============================================================================
# Usage: curl -fsSL https://raw.githubusercontent.com/nousresearch/ironhermes/main/install.sh | bash
#
# Installs IronHermes by:
# 1. Detecting OS and architecture
# 2. Downloading prebuilt binary from GitHub Releases (or falling back to cargo install)
# 3. Scaffolding ~/.ironhermes/ directory structure
# 4. Copying config templates
# 5. Adding binary to PATH
# =============================================================================
set -euo pipefail

# --- Constants ---
REPO_OWNER="nousresearch"
REPO_NAME="ironhermes"
INSTALL_DIR="$HOME/.local/bin"
IRONHERMES_HOME="${IRONHERMES_HOME:-$HOME/.ironhermes}"
VERSION="${IRONHERMES_VERSION:-latest}"

# --- Interactive detection (curl-pipe has no TTY) ---
IS_INTERACTIVE=false
if [ -t 0 ]; then
    IS_INTERACTIVE=true
fi

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

# --- OS / Arch Detection ---
detect_platform() {
    local os arch
    case "$(uname -s)" in
        Linux*)  os="linux"  ;;
        Darwin*) os="macos"  ;;
        *)       log_error "Unsupported OS: $(uname -s)"; exit 1 ;;
    esac
    case "$(uname -m)" in
        x86_64)        arch="x86_64"  ;;
        aarch64|arm64) arch="aarch64" ;;
        *)
            log_warn "Unknown architecture: $(uname -m) -- will fall back to cargo install"
            arch=""
            ;;
    esac
    OS="$os"
    ARCH="$arch"
    PLATFORM="${os}-${arch}"
}

# --- Resolve latest version from GitHub API ---
resolve_version() {
    if [ "$VERSION" = "latest" ]; then
        local latest
        latest=$(curl -fsSL "https://api.github.com/repos/${REPO_OWNER}/${REPO_NAME}/releases/latest" 2>/dev/null \
            | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')
        if [ -n "$latest" ]; then
            VERSION="$latest"
            log_info "Latest version: $VERSION"
        else
            log_warn "Could not determine latest version -- will try cargo install"
            VERSION=""
        fi
    fi
}

# --- Download prebuilt binary from GitHub Releases ---
download_binary() {
    if [ -z "$ARCH" ] || [ -z "$VERSION" ]; then
        return 1
    fi

    local artifact="ironhermes-${PLATFORM}.tar.gz"
    local url="https://github.com/${REPO_OWNER}/${REPO_NAME}/releases/download/${VERSION}/${artifact}"

    log_info "Downloading ${artifact}..."

    # Run in subshell so the EXIT trap is scoped and does not leak to caller
    (
        local tmpdir
        tmpdir=$(mktemp -d)
        cleanup() { rm -rf "$tmpdir"; }
        trap cleanup EXIT

        if curl -fsSL "$url" -o "${tmpdir}/ironhermes.tar.gz" 2>/dev/null; then
            tar -xzf "${tmpdir}/ironhermes.tar.gz" -C "${tmpdir}/"
            mkdir -p "$INSTALL_DIR"
            install -m 755 "${tmpdir}/ironhermes" "$INSTALL_DIR/ironhermes"
            log_ok "Binary installed to ${INSTALL_DIR}/ironhermes"
            exit 0
        else
            log_warn "No prebuilt binary available for ${PLATFORM} (${VERSION})"
            exit 1
        fi
    )
}

# --- Fallback: cargo install ---
cargo_install() {
    if ! command -v cargo >/dev/null 2>&1; then
        log_error "Neither prebuilt binary nor cargo found."
        log_error "Install Rust first: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
        exit 1
    fi

    log_info "Building from source via cargo install (this may take several minutes)..."
    cargo install ironhermes
    log_ok "Installed via cargo install"
}

# --- Scaffold ~/.ironhermes/ directories ---
scaffold_home() {
    log_info "Setting up ${IRONHERMES_HOME}..."
    mkdir -p "$IRONHERMES_HOME"/{cron,sessions,logs,hooks,memories,skills,workspace}
    log_ok "Directory structure created"
}

# --- Copy config templates ---
seed_templates() {
    # Templates may come from the repo (if running from checkout) or be downloaded.
    # For curl-pipe install: download templates from the repo.
    local base_url="https://raw.githubusercontent.com/${REPO_OWNER}/${REPO_NAME}/${VERSION:-main}"

    if [ ! -f "$IRONHERMES_HOME/.env" ]; then
        if [ -f ".env.example" ]; then
            cp ".env.example" "$IRONHERMES_HOME/.env"
        else
            curl -fsSL "${base_url}/.env.example" -o "$IRONHERMES_HOME/.env" 2>/dev/null || \
                log_warn "Could not download .env template"
        fi
    fi

    if [ ! -f "$IRONHERMES_HOME/config.yaml" ]; then
        if [ -f "cli-config.yaml.example" ]; then
            cp "cli-config.yaml.example" "$IRONHERMES_HOME/config.yaml"
        else
            curl -fsSL "${base_url}/cli-config.yaml.example" -o "$IRONHERMES_HOME/config.yaml" 2>/dev/null || \
                log_warn "Could not download config.yaml template"
        fi
    fi

    if [ ! -f "$IRONHERMES_HOME/SOUL.md" ]; then
        if [ -f "docker/SOUL.md" ]; then
            cp "docker/SOUL.md" "$IRONHERMES_HOME/SOUL.md"
        else
            curl -fsSL "${base_url}/docker/SOUL.md" -o "$IRONHERMES_HOME/SOUL.md" 2>/dev/null || \
                log_warn "Could not download SOUL.md template"
        fi
    fi

    # Protect .env file permissions (contains API keys)
    chmod 600 "$IRONHERMES_HOME/.env" 2>/dev/null || true

    log_ok "Config templates seeded"
}

# --- Update PATH in shell configs ---
update_shell_path() {
    local bin_dir="$INSTALL_DIR"
    local path_line="export PATH=\"${bin_dir}:\$PATH\""
    mkdir -p "$bin_dir"

    local updated=false
    for rc in "$HOME/.bashrc" "$HOME/.zshrc" "$HOME/.profile"; do
        if [ -f "$rc" ] && ! grep -qF "$bin_dir" "$rc"; then
            echo "" >> "$rc"
            echo "# Added by IronHermes installer" >> "$rc"
            echo "$path_line" >> "$rc"
            updated=true
        fi
    done

    if [ "$updated" = true ]; then
        log_ok "PATH updated in shell configs (restart shell or run: source ~/.bashrc)"
    else
        log_info "PATH already configured"
    fi
}

# --- Main ---
main() {
    echo ""
    echo "${BOLD}IronHermes Installer${RESET}"
    echo "========================================"
    echo ""

    detect_platform
    log_info "Detected platform: ${OS} ${ARCH}"

    resolve_version

    # Try prebuilt binary first, fall back to cargo install
    if ! download_binary; then
        cargo_install
    fi

    scaffold_home
    seed_templates
    update_shell_path

    echo ""
    echo "========================================"
    log_ok "IronHermes installed successfully!"
    echo ""
    echo "  Home directory: ${IRONHERMES_HOME}"
    echo "  Binary:         ${INSTALL_DIR}/ironhermes"
    echo ""
    echo "  Get started:"
    echo "    1. Edit ${IRONHERMES_HOME}/.env with your API keys"
    echo "    2. Run: ironhermes"
    echo ""

    if [ "$IS_INTERACTIVE" = true ]; then
        echo "  Run 'ironhermes doctor' to check your setup."
    fi
}

main "$@"

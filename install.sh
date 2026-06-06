#!/bin/bash
# =============================================================================
# IronHermes Installer
# =============================================================================
# Usage: curl -fsSL https://raw.githubusercontent.com/bradwilson331/iron-hermes/main/install.sh | bash
#
# Subcommands (default = install):
#   bash install.sh                  # install (default)
#   bash install.sh install          # install
#   bash install.sh update           # update binary + re-seed only missing config keys
#   bash install.sh reinstall        # force re-fetch + re-scaffold (preserves .env/config.yaml)
#   bash install.sh uninstall        # remove binary + PATH lines; preserve ~/.ironhermes/
#   bash install.sh uninstall --purge  # also removes ~/.ironhermes/
#
# Environment overrides:
#   IRONHERMES_REPO    Distribution source URL (default: https://github.com/bradwilson331/iron-hermes)
#   IRONHERMES_HOME    Home directory (default: ~/.ironhermes)
#   IRONHERMES_VERSION Version to install (default: latest)
#   IRONHERMES_PURGE   Set to "true" to purge on uninstall (same as --purge)
# =============================================================================
set -euo pipefail

# --- Constants ---
REPO_OWNER="bradwilson331"
REPO_NAME="iron-hermes"
INSTALL_DIR="$HOME/.local/bin"
IRONHERMES_HOME="${IRONHERMES_HOME:-$HOME/.ironhermes}"
VERSION="${IRONHERMES_VERSION:-latest}"
IRONHERMES_REPO="${IRONHERMES_REPO:-https://github.com/${REPO_OWNER}/${REPO_NAME}}"

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

# --- Security: validate IRONHERMES_REPO (T-37.1-02-01) ---
# Accept only https:// or http://192.168. (Gitea LAN); reject shell metacharacters.
validate_repo_url() {
    local url="$1"
    # Reject shell metacharacters
    case "$url" in
        *";"*|*"|"*|*"&"*|*'$'*|*"\`"*|*" "*|*"("*|*")"*|*"<"*|*">"*)
            log_error "IRONHERMES_REPO contains invalid characters: $url"
            exit 1
            ;;
    esac
    # Accept https:// or http://192.168. only
    case "$url" in
        https://*)
            ;;
        http://192.168.*)
            ;;
        *)
            log_error "IRONHERMES_REPO must start with https:// or http://192.168. (got: $url)"
            exit 1
            ;;
    esac
}

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
            # macOS quarantine strip (T-37.1-02-03, D-07): strip xattr set by curl download
            if [ "$OS" = "macos" ]; then
                xattr -d com.apple.quarantine "$INSTALL_DIR/ironhermes" 2>/dev/null || true
            fi
            log_ok "Binary installed to ${INSTALL_DIR}/ironhermes"
            exit 0
        else
            log_warn "No prebuilt binary available for ${PLATFORM} (${VERSION})"
            exit 1
        fi
    )
}

# --- Fallback: cargo install --git ---
cargo_install() {
    if ! command -v cargo >/dev/null 2>&1; then
        log_error "Neither prebuilt binary nor cargo found."
        log_error "Install Rust first: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
        exit 1
    fi

    validate_repo_url "$IRONHERMES_REPO"
    local repo="$IRONHERMES_REPO"
    log_info "Building from source via cargo install --git (this may take several minutes)..."
    cargo install --git "$repo" ironhermes
    log_ok "Installed from source"
}

# --- Scaffold ~/.ironhermes/ directories ---
scaffold_home() {
    log_info "Setting up ${IRONHERMES_HOME}..."
    mkdir -p "$IRONHERMES_HOME"/{cron,sessions,logs,hooks,memories,skills,workspace}
    log_ok "Directory structure created"
}

# --- Copy config templates ---
# mode: install | update | reinstall
# install/update: skip files that already exist (additive guard)
# reinstall: same guard (preserves .env/config.yaml; scaffold_home is called separately)
seed_templates() {
    local mode="${1:-install}"
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

    # Protect .env file permissions (contains API keys — T-37.1-02-04)
    chmod 600 "$IRONHERMES_HOME/.env" 2>/dev/null || true

    if [ ! -f "$IRONHERMES_HOME/config.yaml" ]; then
        if [ -f "cli-config.yaml.example" ]; then
            cp "cli-config.yaml.example" "$IRONHERMES_HOME/config.yaml"
        else
            curl -fsSL "${base_url}/cli-config.yaml.example" -o "$IRONHERMES_HOME/config.yaml" 2>/dev/null || \
                log_warn "Could not download config.yaml template"
        fi
    elif [ "$mode" = "reinstall" ]; then
        log_warn "config.yaml exists — only missing sections will be appended"
        # Additive-merge: append missing top-level sections as commented blocks.
        # For each section key in the example, only append if the key is absent.
        if [ -f "cli-config.yaml.example" ]; then
            while IFS= read -r line; do
                # Match uncommented top-level keys (no leading spaces/tabs, ends with ':')
                if echo "$line" | grep -qE '^[a-z_]+:'; then
                    local section
                    section=$(echo "$line" | sed 's/:.*//')
                    if ! grep -qE "^${section}:" "$IRONHERMES_HOME/config.yaml" 2>/dev/null; then
                        echo "" >> "$IRONHERMES_HOME/config.yaml"
                        echo "# --- ${section} (appended by reinstall) ---" >> "$IRONHERMES_HOME/config.yaml"
                        echo "# ${line}" >> "$IRONHERMES_HOME/config.yaml"
                    fi
                fi
            done < "cli-config.yaml.example"
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

# --- Detect existing installations (D-11) ---
# Sets IRONHERMES_EXISTS, HERMES_AGENT_EXISTS, OPENCLAW_EXISTS globals.
# NEVER reads or writes ~/.hermes/ contents (T-37.1-02-05).
detect_existing_install() {
    IRONHERMES_EXISTS=false
    HERMES_AGENT_EXISTS=false
    OPENCLAW_EXISTS=false

    if [ -f "${IRONHERMES_HOME}/config.yaml" ] || [ -f "${IRONHERMES_HOME}/.env" ]; then
        IRONHERMES_EXISTS=true
    fi

    # Detection is existence-check only; never read contents, never write (T-37.1-02-05)
    if [ -d "$HOME/.hermes" ] || [ -f "$HOME/.hermes/config.yaml" ]; then
        HERMES_AGENT_EXISTS=true
        log_info "Legacy hermes-agent detected at ~/.hermes/. IronHermes uses ~/.ironhermes/ — your existing hermes-agent data is untouched."
    fi

    if [ -d "$HOME/.openclaw" ]; then
        OPENCLAW_EXISTS=true
        log_info "openclaw installation detected at ~/.openclaw/ (forward-compat notice only)."
    fi
}

# --- Verb: install ---
cmd_install() {
    echo ""
    echo "${BOLD}IronHermes Installer${RESET}"
    echo "========================================"
    echo ""

    detect_platform
    log_info "Detected platform: ${OS} ${ARCH}"

    detect_existing_install

    resolve_version

    validate_repo_url "$IRONHERMES_REPO"

    # Try prebuilt binary first, fall back to cargo install --git
    if ! download_binary; then
        cargo_install
    fi

    scaffold_home
    seed_templates install
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

# --- Verb: update ---
# Swaps the binary; never clobbers SET config values (additive merge via seed_templates update).
cmd_update() {
    echo ""
    echo "${BOLD}IronHermes Update${RESET}"
    echo "========================================"
    echo ""

    detect_platform
    log_info "Detected platform: ${OS} ${ARCH}"

    resolve_version

    validate_repo_url "$IRONHERMES_REPO"

    # Re-fetch binary
    if ! download_binary; then
        cargo_install
    fi

    # Re-seed only missing templates (update mode: skip if file exists)
    seed_templates update

    # Re-enforce .env permissions on update (T-37.1-02-04)
    chmod 600 "$IRONHERMES_HOME/.env" 2>/dev/null || true

    echo ""
    echo "========================================"
    log_ok "IronHermes updated successfully!"
    echo ""
}

# --- Verb: reinstall ---
# Force re-fetch + scaffold_home; preserves .env/config.yaml (additive-merge).
cmd_reinstall() {
    echo ""
    echo "${BOLD}IronHermes Reinstall${RESET}"
    echo "========================================"
    echo ""

    detect_platform
    log_info "Detected platform: ${OS} ${ARCH}"

    resolve_version

    validate_repo_url "$IRONHERMES_REPO"

    # Force re-fetch binary
    if ! download_binary; then
        cargo_install
    fi

    scaffold_home
    seed_templates reinstall
    update_shell_path

    echo ""
    echo "========================================"
    log_ok "IronHermes reinstalled successfully!"
    echo ""
}

# --- Verb: uninstall ---
# Removes binary and installer-added PATH lines.
# Preserves ~/.ironhermes/ by default; --purge or IRONHERMES_PURGE=true removes it.
cmd_uninstall() {
    local purge="${IRONHERMES_PURGE:-false}"
    for arg in "$@"; do
        [ "$arg" = "--purge" ] && purge=true
    done

    echo ""
    echo "${BOLD}IronHermes Uninstall${RESET}"
    echo "========================================"
    echo ""

    # Remove binary (T-37.1-02-03: only remove the named ironhermes binary)
    if [ -f "$INSTALL_DIR/ironhermes" ]; then
        rm -f "$INSTALL_DIR/ironhermes"
        log_ok "Binary removed from $INSTALL_DIR"
    else
        log_info "Binary not found at $INSTALL_DIR/ironhermes (already removed?)"
    fi

    # Remove installer-added PATH lines from shell RCs (T-37.1-02-02).
    # Keys on the literal '# Added by IronHermes installer' marker + exact PATH line.
    for rc in "$HOME/.bashrc" "$HOME/.zshrc" "$HOME/.profile"; do
        if [ -f "$rc" ]; then
            grep -v "# Added by IronHermes installer" "$rc" | \
            grep -v "export PATH=\"${INSTALL_DIR}:" > "${rc}.ironhermes_tmp" \
                && mv "${rc}.ironhermes_tmp" "$rc" \
                || rm -f "${rc}.ironhermes_tmp"
        fi
    done
    log_ok "PATH entries removed from shell configs"

    # Preserve ~/.ironhermes/ by default; --purge removes it
    if [ "$purge" = "true" ]; then
        rm -rf "$IRONHERMES_HOME"
        log_ok "~/.ironhermes/ purged (--purge flag)"
    else
        log_info "~/.ironhermes/ preserved (use --purge to remove user data)"
    fi

    echo ""
    echo "========================================"
    log_ok "IronHermes uninstalled."
    echo ""
}

# --- Subcommand dispatch (D-02) ---
# First positional arg = verb; default = install (curl-pipe safe).
VERB="${1:-install}"
case "$VERB" in
    install)
        cmd_install
        ;;
    update)
        cmd_update
        ;;
    reinstall)
        cmd_reinstall
        ;;
    uninstall)
        shift
        cmd_uninstall "$@"
        ;;
    *)
        log_error "Unknown subcommand: $VERB"
        log_error "Usage: $0 [install|update|reinstall|uninstall [--purge]]"
        exit 1
        ;;
esac

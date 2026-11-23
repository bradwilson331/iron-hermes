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
#   IRONHERMES_SKILLS_ARCHIVE  Bundled-skills archive to seed instead of the release asset:
#                      a local ironhermes-skills.tar.gz path or an https URL (a sibling
#                      .sha256 is verified when present)
#   IRONHERMES_INSTALL_LIB_ONLY  Set to "1" to source this file for its functions only (tests)
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

# --- Security sidecar (Phase 57, D-02) ---
# The detection sidecar `ironhermes-tirith` ships in the same tarball as
# `ironhermes`. With the default DEFCON level 3 a missing sidecar blocks
# terminal/execute_code (D-12), so the installer treats it as part of the
# install. An older tarball without it is a loud warning, not a failure: the
# main binary still works, and `ironhermes doctor` states the consequence.
#
# The sidecar is AGPL-3.0-only, so its LICENSE and the NOTICE with the source
# link are copied next to the install (D-01, Pitfall 11).
SIDECAR_BIN="ironhermes-tirith"

# install_sidecar_from_dir EXTRACTED_DIR -- install the sidecar (and its licence
# files) from an extracted release tarball. Never fails the install.
install_sidecar_from_dir() {
    local src="$1"
    if [ ! -f "${src}/${SIDECAR_BIN}" ]; then
        log_warn "This release tarball has no ${SIDECAR_BIN} (an older release?)."
        log_warn "Without it the default security level (DEFCON 3) blocks terminal/execute_code."
        log_warn "Run 'ironhermes doctor' to see the sidecar state, then update to a newer release."
        return 0
    fi
    if ! install -m 755 "${src}/${SIDECAR_BIN}" "$INSTALL_DIR/${SIDECAR_BIN}"; then
        log_warn "Could not install ${SIDECAR_BIN} to ${INSTALL_DIR}; run 'ironhermes doctor' for the consequence."
        return 0
    fi
    # macOS quarantine strip, same as the main binary (T-37.1-02-03).
    if [ "$OS" = "macos" ]; then
        xattr -d com.apple.quarantine "$INSTALL_DIR/${SIDECAR_BIN}" 2>/dev/null || true
    fi
    log_ok "Security sidecar installed to ${INSTALL_DIR}/${SIDECAR_BIN}"

    local doc_dir="${INSTALL_DIR}/../share/doc/${SIDECAR_BIN}"
    if mkdir -p "$doc_dir" 2>/dev/null \
        && cp "${src}/LICENSE-AGPL-${SIDECAR_BIN}" "${doc_dir}/LICENSE" 2>/dev/null \
        && cp "${src}/NOTICE-${SIDECAR_BIN}" "${doc_dir}/NOTICE" 2>/dev/null; then
        log_ok "Sidecar licence (AGPL-3.0) and source notice installed to ${doc_dir}"
    else
        log_warn "Could not install the sidecar LICENSE/NOTICE to ${doc_dir}; they remain in the release tarball."
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
            install_sidecar_from_dir "$tmpdir"
            exit 0
        else
            log_warn "No prebuilt binary available for ${PLATFORM} (${VERSION})"
            exit 1
        fi
    )
}

# --- Revision shared by the cargo fallback and the skills source archive (D-24) ---
# SKILLS_REF_KIND is tag | rev | branch; SKILLS_REF is the tag name, the 40-hex
# commit sha, or "main". Chosen once so the binary and the skills come from the
# same revision: VERSION when a release tag is known, else the sha git ls-remote
# reports for HEAD, else the default branch (unpinned -- warned about).
SKILLS_REF=""
SKILLS_REF_KIND=""
SKILLS_INCOMPLETE=0

resolve_skills_ref() {
    if [ -n "$SKILLS_REF" ]; then
        return 0
    fi
    local sha=""
    if [ -n "$VERSION" ] && [ "$VERSION" != "latest" ]; then
        SKILLS_REF="$VERSION"
        SKILLS_REF_KIND="tag"
        return 0
    fi
    if command -v git >/dev/null 2>&1; then
        sha="$(git ls-remote "$IRONHERMES_REPO" HEAD 2>/dev/null | awk 'NR==1{print $1}')" || sha=""
        if printf '%s' "$sha" | grep -Eq '^[0-9a-f]{40}$'; then
            SKILLS_REF="$sha"
            SKILLS_REF_KIND="rev"
            return 0
        fi
    fi
    SKILLS_REF="main"
    SKILLS_REF_KIND="branch"
    log_warn "installing the default branch without a pinned revision"
    return 0
}

# --- Fallback: cargo install --git ---
# The package is ironhermes-cli (binary: ironhermes), installed --locked from the
# revision resolve_skills_ref picked.
cargo_install() {
    if ! command -v cargo >/dev/null 2>&1; then
        log_error "Neither prebuilt binary nor cargo found."
        log_error "Install Rust first: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
        exit 1
    fi

    validate_repo_url "$IRONHERMES_REPO"
    local repo="$IRONHERMES_REPO"
    local -a rev_args
    resolve_skills_ref
    case "$SKILLS_REF_KIND" in
        tag)  rev_args=(--tag "$SKILLS_REF") ;;
        rev)  rev_args=(--rev "$SKILLS_REF") ;;
        *)    rev_args=(--branch main) ;;
    esac
    log_info "Building from source via cargo install --git (this may take several minutes)..."
    # Both installs share one build directory so the dependency graph is compiled
    # once, and both use the same revision, so the sidecar is the same build as
    # the main binary (D-02). The sidecar goes first so that a failed sidecar
    # build only warns (doctor states the consequence) and the main install still
    # runs; a failed main install is the script's failure.
    local build_dir rc=0
    build_dir="$(mktemp -d)"
    if ! cargo install --locked --git "$repo" "${rev_args[@]}" --target-dir "$build_dir" \
        --bin "$SIDECAR_BIN" "$SIDECAR_BIN"; then
        log_warn "Could not build the security sidecar (${SIDECAR_BIN})."
        log_warn "Without it the default security level (DEFCON 3) blocks terminal/execute_code."
        log_warn "Run 'ironhermes doctor' for details."
    fi
    cargo install --locked --git "$repo" "${rev_args[@]}" --target-dir "$build_dir" \
        --bin ironhermes ironhermes-cli || rc=$?
    rm -rf "$build_dir"
    if [ "$rc" -ne 0 ]; then
        exit "$rc"
    fi
    log_ok "Installed from source"
}

# --- Scaffold ~/.ironhermes/ directories ---
scaffold_home() {
    log_info "Setting up ${IRONHERMES_HOME}..."
    mkdir -p "$IRONHERMES_HOME"/{cron,sessions,logs,hooks,memories,skills,optional-skills,workspace}
    log_ok "Directory structure created"
}

# --- Bundled skills seeding (D-24, D-19) ---
# The block below is synced byte-identically from docker/seed-bundled.sh (the
# container entrypoints source that file); scripts/tests/install_seed_skills_test.sh
# case seed_function_parity diffs the two. Edit docker/seed-bundled.sh, then re-splice.
# >>> ih_seed_bundled_tree
# ih_sha256 FILE -- print the lowercase hex sha256 of FILE.
ih_sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

# ih_seed_bundled_tree SRC_DIR DEST_DIR HISTORY_FILE TREE_PREFIX
# Prints `Skills (<prefix>): N new, U upgraded, K kept, S unchanged, R refused`.
# Returns 0, or 2 when at least one file was refused (nothing is written for a
# refused file), or 1 when SRC_DIR is not a directory.
ih_seed_bundled_tree() {
    local src="${1%/}" dest="${2%/}" hist="$3" prefix="$4"
    local n_new=0 n_up=0 n_keep=0 n_same=0 n_ref=0
    local f rel dst parent cur seg rest dsha refused

    if [ ! -d "$src" ]; then
        echo "ih_seed_bundled_tree: source is not a directory: $src" >&2
        return 1
    fi

    # The destination root itself must not be a symlink and must be (or become) a directory.
    if [ -L "$dest" ]; then
        echo "refused: $prefix/ (destination root is a symlink: $dest)" >&2
        echo "Skills ($prefix): 0 new, 0 upgraded, 0 kept, 0 unchanged, 1 refused"
        return 2
    fi
    if [ -e "$dest" ] && [ ! -d "$dest" ]; then
        echo "refused: $prefix/ (destination root is not a directory: $dest)" >&2
        echo "Skills ($prefix): 0 new, 0 upgraded, 0 kept, 0 unchanged, 1 refused"
        return 2
    fi
    if [ ! -d "$dest" ]; then
        mkdir -p "$dest" || return 1
    fi

    # `find -type f` never lists a symlink, so a source symlink is never copied.
    while IFS= read -r f; do
        rel="${f#"$src"/}"
        [ "$rel" = ".bundled-history" ] && continue
        dst="$dest/$rel"
        refused=""

        # Walk the destination ancestors below DEST_DIR: a symlink or a
        # non-directory anywhere on the path refuses the file.
        parent="${rel%/*}"
        if [ "$parent" != "$rel" ]; then
            cur="$dest"
            rest="$parent"
            while [ -n "$rest" ]; do
                seg="${rest%%/*}"
                if [ "$seg" = "$rest" ]; then rest=""; else rest="${rest#*/}"; fi
                cur="$cur/$seg"
                if [ -L "$cur" ]; then
                    refused="ancestor is a symlink: $cur"
                    break
                elif [ -e "$cur" ] && [ ! -d "$cur" ]; then
                    refused="ancestor is not a directory: $cur"
                    break
                elif [ ! -e "$cur" ]; then
                    break
                fi
            done
        fi
        if [ -z "$refused" ] && [ -L "$dst" ]; then
            refused="destination is a symlink"
        fi
        if [ -z "$refused" ] && [ -e "$dst" ] && [ ! -f "$dst" ]; then
            refused="destination is not a regular file"
        fi
        if [ -n "$refused" ]; then
            echo "refused: $prefix/$rel ($refused)" >&2
            n_ref=$((n_ref + 1))
            continue
        fi

        if [ ! -e "$dst" ]; then
            if mkdir -p "${dst%/*}" && cp -p "$f" "$dst"; then
                n_new=$((n_new + 1))
            else
                echo "refused: $prefix/$rel (copy failed)" >&2
                n_ref=$((n_ref + 1))
            fi
        elif cmp -s "$f" "$dst"; then
            n_same=$((n_same + 1))
        else
            dsha="$(ih_sha256 "$dst")" || dsha=""
            if [ -n "$dsha" ] && [ -f "$hist" ] && grep -qxF "$dsha  $prefix/$rel" "$hist"; then
                if cp -p "$f" "$dst"; then
                    n_up=$((n_up + 1))
                else
                    echo "refused: $prefix/$rel (upgrade copy failed)" >&2
                    n_ref=$((n_ref + 1))
                fi
            else
                n_keep=$((n_keep + 1))
            fi
        fi
    done < <(find "$src" -type f -print | LC_ALL=C sort)

    echo "Skills ($prefix): $n_new new, $n_up upgraded, $n_keep kept, $n_same unchanged, $n_ref refused"
    if [ "$n_ref" -gt 0 ]; then
        return 2
    fi
    return 0
}
# <<< ih_seed_bundled_tree

# ih_validate_skills_archive ARCHIVE [release|source]
# Validate the archive listing BEFORE anything is extracted (T-54.8-14, T-54.8-57):
#   - every extracted entry is a regular file or a directory: symlinks, hard links,
#     devices and FIFOs are refused (read from the verbose listing);
#   - no absolute path, no `..` segment;
#   - release mode: the first path segment is `skills` or `optional-skills`;
#   - source mode (GitHub's source archive): exactly one top-level directory, and only
#     its skills/ and optional-skills/ children are examined (and later extracted).
# In source mode it sets IH_ARCHIVE_TOP, IH_HAVE_SKILLS and IH_HAVE_OPTIONAL.
ih_validate_skills_archive() {
    local archive="$1" mode="${2:-release}"
    local scratch names types name line n first rest sub c top=""
    IH_ARCHIVE_TOP=""
    IH_HAVE_SKILLS=0
    IH_HAVE_OPTIONAL=0
    scratch="$(mktemp -d)" || return 1
    if ! tar -tzf "$archive" >"$scratch/names" 2>/dev/null \
        || ! tar -tvzf "$archive" >"$scratch/types" 2>/dev/null; then
        rm -rf "$scratch"
        log_error "skills archive is not a readable tar.gz: $archive"
        return 1
    fi
    names="$(grep -c '' "$scratch/names" || true)"
    types="$(grep -c '' "$scratch/types" || true)"
    if [ "${names:-0}" -eq 0 ] || [ "$names" != "$types" ]; then
        rm -rf "$scratch"
        log_error "skills archive listing is empty or ambiguous"
        return 1
    fi
    while IFS= read -r name <&3 && IFS= read -r line <&4; do
        n="${name#./}"
        [ -z "$n" ] && continue
        case "$n" in
            /*)
                log_error "skills archive has an absolute path: $name"
                rm -rf "$scratch"; return 1 ;;
        esac
        case "/$n/" in
            */../*)
                log_error "skills archive has a '..' path segment: $name"
                rm -rf "$scratch"; return 1 ;;
        esac
        n="${n%/}"
        first="${n%%/*}"
        if [ "$mode" = "source" ]; then
            if [ -z "$top" ]; then
                top="$first"
            elif [ "$first" != "$top" ]; then
                log_error "source archive has more than one top-level entry: $name"
                rm -rf "$scratch"; return 1
            fi
            if [ "$n" = "$first" ]; then
                continue
            fi
            rest="${n#*/}"
            sub="${rest%%/*}"
            case "$sub" in
                skills) IH_HAVE_SKILLS=1 ;;
                optional-skills) IH_HAVE_OPTIONAL=1 ;;
                *) continue ;;
            esac
        else
            case "$first" in
                skills|optional-skills) ;;
                *)
                    log_error "skills archive has an unexpected top-level entry: $name"
                    rm -rf "$scratch"; return 1 ;;
            esac
        fi
        c="${line:0:1}"
        case "$c" in
            -|d) ;;
            *)
                log_error "skills archive has a link/device/FIFO entry (type '$c'): $name"
                rm -rf "$scratch"; return 1 ;;
        esac
        case "$line" in
            *" link to "*)
                log_error "skills archive has a hard-link entry: $name"
                rm -rf "$scratch"; return 1 ;;
        esac
    done 3<"$scratch/names" 4<"$scratch/types"
    rm -rf "$scratch"
    IH_ARCHIVE_TOP="$top"
    return 0
}

# ih_skills_source_url -- GitHub's source archive for SKILLS_REF (resolve_skills_ref first).
ih_skills_source_url() {
    local base="https://github.com/${REPO_OWNER}/${REPO_NAME}/archive"
    case "$SKILLS_REF_KIND" in
        tag)  echo "${base}/refs/tags/${SKILLS_REF}.tar.gz" ;;
        rev)  echo "${base}/${SKILLS_REF}.tar.gz" ;;
        *)    echo "${base}/refs/heads/main.tar.gz" ;;
    esac
}

# seed_skills -- seed the bundled skills into $IRONHERMES_HOME/skills and
# $IRONHERMES_HOME/optional-skills (copy-absent / upgrade-pristine / keep-edited).
# Source, in order: IRONHERMES_SKILLS_ARCHIVE (local file or https URL; no fallback),
# else the release asset for VERSION, else GitHub's source archive for the same
# revision the cargo fallback uses (only skills/ and optional-skills/ are extracted).
# A present .sha256 must match. Returns non-zero when nothing could be seeded or any
# file was refused.
seed_skills() {
    local rc=0
    if ! mkdir -p "$IRONHERMES_HOME/skills" "$IRONHERMES_HOME/optional-skills"; then
        log_error "Could not create ${IRONHERMES_HOME}/skills and optional-skills"
        return 1
    fi
    log_info "Seeding bundled skills..."
    (
        # No `local` here: the EXIT trap runs after a subshell's function scope is gone.
        tmp="$(mktemp -d)" || exit 1
        trap 'rm -rf "$tmp"' EXIT
        archive="$tmp/ironhermes-skills.tar.gz"
        sumfile="$archive.sha256"
        mode="release"
        got=0

        if [ -n "${IRONHERMES_SKILLS_ARCHIVE:-}" ]; then
            case "$IRONHERMES_SKILLS_ARCHIVE" in
                https://*)
                    url="$IRONHERMES_SKILLS_ARCHIVE"
                    curl -fsSL "$url" -o "$archive" 2>/dev/null \
                        || { log_error "Could not download skills archive: $url"; exit 1; }
                    curl -fsSL "$url.sha256" -o "$sumfile" 2>/dev/null || rm -f "$sumfile"
                    ;;
                *)
                    [ -f "$IRONHERMES_SKILLS_ARCHIVE" ] \
                        || { log_error "Skills archive not found: $IRONHERMES_SKILLS_ARCHIVE"; exit 1; }
                    cp "$IRONHERMES_SKILLS_ARCHIVE" "$archive" || exit 1
                    if [ -f "$IRONHERMES_SKILLS_ARCHIVE.sha256" ]; then
                        cp "$IRONHERMES_SKILLS_ARCHIVE.sha256" "$sumfile" || exit 1
                    fi
                    ;;
            esac
        else
            if [ -n "$VERSION" ] && [ "$VERSION" != "latest" ]; then
                url="https://github.com/${REPO_OWNER}/${REPO_NAME}/releases/download/${VERSION}/ironhermes-skills.tar.gz"
                if curl -fsSL "$url" -o "$archive" 2>/dev/null; then
                    got=1
                    curl -fsSL "$url.sha256" -o "$sumfile" 2>/dev/null || rm -f "$sumfile"
                else
                    rm -f "$archive"
                    log_warn "No skills release asset for ${VERSION} -- using the source archive"
                fi
            fi
            if [ "$got" -eq 0 ]; then
                validate_repo_url "$IRONHERMES_REPO"
                resolve_skills_ref
                url="$(ih_skills_source_url)"
                curl -fsSL "$url" -o "$archive" 2>/dev/null \
                    || { log_error "Could not download the skills source archive: $url"; exit 1; }
                mode="source"
            fi
        fi

        if [ -f "$sumfile" ]; then
            expected="$(awk 'NR==1{print tolower($1)}' "$sumfile")"
            actual="$(ih_sha256 "$archive")" || actual=""
            if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
                log_error "Skills archive checksum mismatch -- not seeding"
                exit 1
            fi
        fi

        ih_validate_skills_archive "$archive" "$mode" || exit 1

        x="$tmp/x"
        mkdir -p "$x" || exit 1
        if [ "$mode" = "source" ]; then
            if [ "$IH_HAVE_SKILLS" -ne 1 ] || [ "$IH_HAVE_OPTIONAL" -ne 1 ]; then
                log_error "Source archive is missing skills/ or optional-skills/"
                exit 1
            fi
            tar -xzf "$archive" -C "$x" --strip-components=1 \
                "$IH_ARCHIVE_TOP/skills" "$IH_ARCHIVE_TOP/optional-skills" \
                || { log_error "Could not extract the skills source archive"; exit 1; }
        else
            tar -xzf "$archive" -C "$x" || { log_error "Could not extract skills archive"; exit 1; }
        fi
        if [ ! -d "$x/skills" ] || [ ! -d "$x/optional-skills" ]; then
            log_error "Skills archive is missing skills/ or optional-skills/"
            exit 1
        fi

        seed_rc=0
        ih_seed_bundled_tree "$x/skills" "$IRONHERMES_HOME/skills" "$x/skills/.bundled-history" skills \
            || seed_rc=1
        ih_seed_bundled_tree "$x/optional-skills" "$IRONHERMES_HOME/optional-skills" "$x/skills/.bundled-history" optional-skills \
            || seed_rc=1
        exit "$seed_rc"
    ) || rc=$?
    if [ "$rc" -ne 0 ]; then
        log_error "Bundled skills were not fully seeded"
        return 1
    fi
    log_ok "Bundled skills seeded"
    return 0
}

# ih_finish_install -- last step of install/update/reinstall: when bundled skills
# could not be seeded the binary and home steps are already done, but a scripted
# install must be able to tell, so exit status 3 (T-54.8-79).
ih_finish_install() {
    if [ "${SKILLS_INCOMPLETE:-0}" = "1" ]; then
        echo "INCOMPLETE INSTALL: bundled skills were not seeded — re-run: bash install.sh reinstall" >&2
        return 3
    fi
    return 0
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
    seed_skills || SKILLS_INCOMPLETE=1
    seed_templates install
    update_shell_path

    echo ""
    echo "========================================"
    if [ "$SKILLS_INCOMPLETE" = "1" ]; then
        log_warn "IronHermes installed, but the bundled skills were not seeded"
    else
        log_ok "IronHermes installed successfully!"
    fi
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

    ih_finish_install
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

    # Re-seed bundled skills: absent files are copied, pristine older copies are
    # upgraded, operator edits are kept.
    seed_skills || SKILLS_INCOMPLETE=1

    # Re-enforce .env permissions on update (T-37.1-02-04)
    chmod 600 "$IRONHERMES_HOME/.env" 2>/dev/null || true

    echo ""
    echo "========================================"
    if [ "$SKILLS_INCOMPLETE" = "1" ]; then
        log_warn "IronHermes updated, but the bundled skills were not seeded"
    else
        log_ok "IronHermes updated successfully!"
    fi
    echo ""

    ih_finish_install
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
    seed_skills || SKILLS_INCOMPLETE=1
    seed_templates reinstall
    update_shell_path

    echo ""
    echo "========================================"
    if [ "$SKILLS_INCOMPLETE" = "1" ]; then
        log_warn "IronHermes reinstalled, but the bundled skills were not seeded"
    else
        log_ok "IronHermes reinstalled successfully!"
    fi
    echo ""

    ih_finish_install
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

    # The security sidecar installed next to the binary (Phase 57), with the
    # licence directory install_sidecar_from_dir created. Named files only.
    if [ -f "$INSTALL_DIR/${SIDECAR_BIN}" ]; then
        rm -f "$INSTALL_DIR/${SIDECAR_BIN}"
        log_ok "Security sidecar removed from $INSTALL_DIR"
    fi
    rm -f "${INSTALL_DIR}/../share/doc/${SIDECAR_BIN}/LICENSE" "${INSTALL_DIR}/../share/doc/${SIDECAR_BIN}/NOTICE"
    rmdir "${INSTALL_DIR}/../share/doc/${SIDECAR_BIN}" 2>/dev/null || true

    # Remove installer-added PATH lines from shell RCs (T-37.1-02-02).
    # Whole-line fixed-string match (grep -vxF) on the literal marker + the exact
    # PATH line the installer wrote, so unrelated user lines that merely contain the
    # substring are never deleted. The grep exit code is captured (|| rc_rc=$?) so
    # neither an all-lines-removed result (exit 1) nor a real error (exit >1) trips
    # `set -euo pipefail`; on a real error the original rc is left untouched.
    local path_line="export PATH=\"${INSTALL_DIR}:\$PATH\""
    local rc rc_rc
    for rc in "$HOME/.bashrc" "$HOME/.zshrc" "$HOME/.profile"; do
        [ -f "$rc" ] || continue
        rc_rc=0
        grep -vxF \
            -e "# Added by IronHermes installer" \
            -e "$path_line" \
            "$rc" > "${rc}.ironhermes_tmp" || rc_rc=$?
        if [ "$rc_rc" -le 1 ]; then
            mv "${rc}.ironhermes_tmp" "$rc"
        else
            rm -f "${rc}.ironhermes_tmp"
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

# --- Library guard: tests source this file to call functions without installing ---
if [ "${IRONHERMES_INSTALL_LIB_ONLY:-}" = "1" ]; then
    return 0 2>/dev/null || exit 0
fi

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

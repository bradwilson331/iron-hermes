# shellcheck shell=bash
# Resolve the Dioxus CLI binary into $DX.
#
# `dx` is not a unique name: Deno 2.x installs its own `dx` (an npm-package
# runner), and Homebrew's /opt/homebrew/bin usually precedes ~/.cargo/bin on
# PATH. A shell alias does not reach scripts, so a bare `dx serve` can run Deno's
# `serve` package instead of Dioxus. Order: $IRONHERMES_DX, then every `dx` on
# PATH, then ~/.cargo/bin/dx; the first whose `--version` starts with "dioxus"
# wins.
#
# Usage: source this file, then `resolve_dioxus_dx || die "..."`; run "$DX" ...

resolve_dioxus_dx() {
    local candidate
    local -a candidates=()
    if [ -n "${IRONHERMES_DX:-}" ]; then
        candidates+=("$IRONHERMES_DX")
    fi
    while IFS= read -r candidate; do
        [ -n "$candidate" ] && candidates+=("$candidate")
    done < <(type -ap dx 2>/dev/null)
    candidates+=("${CARGO_HOME:-$HOME/.cargo}/bin/dx")

    for candidate in "${candidates[@]}"; do
        [ -x "$candidate" ] || continue
        case "$("$candidate" --version 2>/dev/null)" in
            dioxus*) DX="$candidate"; return 0 ;;
        esac
    done
    return 1
}

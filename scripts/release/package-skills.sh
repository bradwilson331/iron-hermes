#!/usr/bin/env bash
# =============================================================================
# scripts/release/package-skills.sh -- build the bundled-skills release archive
# and maintain its prior-version history manifest
# =============================================================================
# Usage:
#   bash scripts/release/package-skills.sh [OUT_DIR]            # default OUT_DIR: dist
#   bash scripts/release/package-skills.sh --write-history [REF...]
#   bash scripts/release/package-skills.sh --check-history [REF...]
#
# Package mode writes OUT_DIR/ironhermes-skills.tar.gz and
# OUT_DIR/ironhermes-skills.tar.gz.sha256 from `git ls-files skills optional-skills`
# (tracked files only, so local __pycache__ / .DS_Store files never ship).
# install.sh seeds the archive into $IRONHERMES_HOME/skills and
# $IRONHERMES_HOME/optional-skills (D-24). The archive carries the committed
# skills/.bundled-history manifest.
#
# History mode. skills/.bundled-history lists `<sha256>  <path>` for EVERY
# version of every CURRENTLY tracked path under skills/ and optional-skills/
# found in any commit that touched those trees and is reachable from HEAD or
# from the extra REFs. The seeder (docker/seed-bundled.sh) treats a destination
# file whose sha256 is listed for its path as a pristine older bundled copy
# and upgrades it; anything else is an operator edit and is kept.
#   --write-history  regenerate skills/.bundled-history
#   --check-history  regenerate into a temp file; exit 1 when the committed
#                    manifest differs ("bundled-history is stale")
# Both modes refuse a shallow clone (exit 3). With no REF given and a local
# `version2github` branch present, that branch is the default extra ref, so
# both modes see the same set: public installs and public images received the
# scrubbed versions published there.
# =============================================================================
set -euo pipefail

ARCHIVE_NAME="ironhermes-skills.tar.gz"
HISTORY_PATH="skills/.bundled-history"
DEFAULT_EXTRA_REF="version2github"

ih_sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

ih_sha256_stdin() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum | awk '{print $1}'
    else
        shasum -a 256 | awk '{print $1}'
    fi
}

package_archive() {
    local out_dir="$1" list count
    mkdir -p "$out_dir"
    list="$(mktemp "${TMPDIR:-/tmp}/ih-skills-list.XXXXXX")"
    # Capture git's output and status first -- never pipe git straight into tar.
    if ! git ls-files skills optional-skills >"$list"; then
        rm -f "$list"
        echo "package-skills: git ls-files failed" >&2
        return 1
    fi
    count="$(grep -c . "$list" || true)"
    if [ "${count:-0}" -eq 0 ]; then
        rm -f "$list"
        echo "package-skills: no tracked files under skills/ or optional-skills/" >&2
        return 1
    fi
    tar -czf "$out_dir/$ARCHIVE_NAME" -T "$list"
    rm -f "$list"
    printf '%s  %s\n' "$(ih_sha256_of "$out_dir/$ARCHIVE_NAME")" "$ARCHIVE_NAME" \
        >"$out_dir/$ARCHIVE_NAME.sha256"
    echo "package-skills: packaged $count files into $out_dir/$ARCHIVE_NAME"
}

require_full_history() {
    local shallow
    shallow="$(git rev-parse --is-shallow-repository)"
    if [ "$shallow" = "true" ]; then
        echo "package-skills: needs full history (this clone is shallow) -- run: git fetch --unshallow" >&2
        exit 3
    fi
}

# generate_history OUT REF... -- write the manifest for HEAD plus the given refs.
generate_history() {
    local out="$1"
    shift
    local work all tracked commits pairs blobs map c b h
    work="$(mktemp -d "${TMPDIR:-/tmp}/ih-skills-history.XXXXXX")"
    all="$work/all"
    tracked="$work/tracked"
    commits="$work/commits"
    pairs="$work/pairs"
    blobs="$work/blobs"
    map="$work/map"

    git ls-files skills optional-skills >"$all"
    { grep -vxF "$HISTORY_PATH" "$all" || true; } | LC_ALL=C sort >"$tracked"
    if grep -q '[[:space:]]' "$tracked"; then
        echo "package-skills: a tracked bundled path contains whitespace; the manifest cannot represent it" >&2
        rm -rf "$work"
        return 1
    fi

    git log --format=%H HEAD "$@" -- skills optional-skills >"$commits"

    : >"$pairs.all"
    while IFS= read -r c; do
        [ -n "$c" ] || continue
        git ls-tree -r "$c" -- skills optional-skills \
            | awk -F'\t' '{ split($1, a, " "); print a[3] " " $2 }' >>"$pairs.all"
    done <"$commits"
    LC_ALL=C sort -u "$pairs.all" >"$pairs.sorted"
    awk 'NR==FNR { t[$0] = 1; next } ($2 in t)' "$tracked" "$pairs.sorted" >"$pairs"

    cut -d' ' -f1 "$pairs" | LC_ALL=C sort -u >"$blobs"
    : >"$map"
    while IFS= read -r b; do
        [ -n "$b" ] || continue
        h="$(git cat-file blob "$b" | ih_sha256_stdin)"
        printf '%s %s\n' "$b" "$h" >>"$map"
    done <"$blobs"

    {
        echo "# skills/.bundled-history -- sha256 of every version of every bundled skill path."
        echo "# Generated by scripts/release/package-skills.sh --write-history; do not edit by hand."
        echo "# Regenerate: bash scripts/release/package-skills.sh --write-history"
        echo "# Format: <sha256>  <path>   (install.sh / docker/seed-bundled.sh upgrade a destination"
        echo "# file whose sha256 is listed for its path; any other content is an operator edit)."
        awk 'NR==FNR { m[$1] = $2; next } { print m[$1] "  " $2 }' "$map" "$pairs" | LC_ALL=C sort -u
    } >"$out"
    rm -rf "$work"
}

history_mode() {
    local mode="$1"
    shift
    local refs r tmp rc
    require_full_history

    refs=""
    if [ "$#" -gt 0 ]; then
        for r in "$@"; do
            if ! git rev-parse --verify --quiet "$r^{commit}" >/dev/null; then
                echo "package-skills: unknown ref: $r" >&2
                exit 2
            fi
            refs="$refs $r"
        done
    elif git show-ref --verify --quiet "refs/heads/$DEFAULT_EXTRA_REF"; then
        refs=" refs/heads/$DEFAULT_EXTRA_REF"
    fi

    tmp="$(mktemp "${TMPDIR:-/tmp}/ih-skills-history-out.XXXXXX")"
    # refs holds simple ref names (no whitespace), so word splitting is intended.
    # shellcheck disable=SC2086
    generate_history "$tmp" $refs

    case "$mode" in
        write)
            cp "$tmp" "$HISTORY_PATH"
            rc=0
            echo "package-skills: wrote $HISTORY_PATH ($(grep -cvE '^(#|$)' "$HISTORY_PATH" || true) entries)"
            ;;
        check)
            if [ -f "$HISTORY_PATH" ] && cmp -s "$tmp" "$HISTORY_PATH"; then
                rc=0
                echo "package-skills: $HISTORY_PATH is fresh"
            else
                rc=1
                echo "bundled-history is stale — run scripts/release/package-skills.sh --write-history" >&2
            fi
            ;;
        *)
            rc=2
            ;;
    esac
    rm -f "$tmp"
    return "$rc"
}

MODE="package"
case "${1:-}" in
    --write-history) MODE="write"; shift ;;
    --check-history) MODE="check"; shift ;;
    -h|--help)
        sed -n '2,30p' "$0"
        exit 0
        ;;
esac

OUT_DIR="dist"
if [ "$MODE" = "package" ]; then
    OUT_DIR="${1:-dist}"
    case "$OUT_DIR" in
        /*) ;;
        *) OUT_DIR="$PWD/$OUT_DIR" ;;
    esac
fi

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

case "$MODE" in
    package) package_archive "$OUT_DIR" ;;
    write)   history_mode write "$@" ;;
    check)   history_mode check "$@" ;;
esac

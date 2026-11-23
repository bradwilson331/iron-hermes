#!/bin/bash
# =============================================================================
# docker/seed-bundled.sh -- shared bundled-skills seeding library (phase 54.8)
# =============================================================================
# SOURCED, never executed: it has no top-level side effects. Consumers:
#   - install.sh embeds the marker-delimited block below byte-identically
#     (scripts/tests/install_seed_skills_test.sh case seed_function_parity
#     diffs the two copies).
#   - docker/entrypoint.sh and docker/web-entrypoint.sh source this file at
#     /opt/ironhermes/docker/seed-bundled.sh (image path, plan 11).
#
# Behaviour of ih_seed_bundled_tree, per file of the bundled tree:
#   absent at the destination ................ copied                 (new)
#   byte-identical to the bundled file ....... left alone             (unchanged)
#   sha256 listed in the history manifest for
#     <TREE_PREFIX>/<relative path> .......... a pristine OLDER bundled copy,
#                                              replaced by the current one (upgraded)
#   anything else ............................ an operator edit, kept  (kept)
#   destination root, an ancestor directory or the file itself is a symlink,
#   or an ancestor is not a directory ........ refused, nothing written (refused)
#
# The history manifest (skills/.bundled-history) lists `<sha256>  <path>` for
# every version of every bundled path; scripts/release/package-skills.sh
# generates it. A missing or stale manifest fails safe: an unknown old version
# reads as an operator edit and is kept.
#
# Portable to macOS /bin/bash 3.2: no mapfile, no associative arrays, no
# ${var,,}, no readlink -f, no grep -P.
# =============================================================================

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

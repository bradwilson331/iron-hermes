#!/usr/bin/env bash
# =============================================================================
# scripts/tests/install_seed_skills_test.sh -- harness for the bundled-skills
# seeding path (phase 54.8, plan 06, D-24 / D-19)
# =============================================================================
# Covers: scripts/release/package-skills.sh, docker/seed-bundled.sh
# (ih_seed_bundled_tree), and install.sh (seed_skills, cargo_install,
# ih_finish_install).
#
# Usage:
#   /bin/bash scripts/tests/install_seed_skills_test.sh            # every case
#   /bin/bash scripts/tests/install_seed_skills_test.sh <case>     # one case
#
# Prints `PASS <case>` / `FAIL <case>` / `SKIP <case> (history-derived)` and
# exits non-zero on any FAIL (an unknown case name is a FAIL).
#
# HISTORY PARTITION. The exported public tree has a different git lineage from
# develop, so a case that reads any commit other than HEAD cannot pass there.
# Those cases are listed in HISTORY_CASES; with IH_SEED_TEST_NO_HISTORY=1 each
# listed case prints `SKIP <case> (history-derived)` and is not run, and every
# other case runs normally. Shared setup below reads only HEAD's tree
# (git ls-files, the working tree); a history-derived case computes its own
# older-version fixture inside its own body.
#
# Run it under /bin/bash on macOS so a bash-4-only construct fails here and not
# on an operator's Mac.

# The cases set variables (VERSION, SKILLS_REF*, SKILLS_INCOMPLETE) that the
# sourced install.sh functions read; shellcheck cannot see across the source.
# shellcheck disable=SC2034
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
cd "$REPO_ROOT"

# A case belongs in HISTORY_CASES when it reads any commit other than HEAD
# (git log, git show <rev>:<path>, package-skills.sh --check-history). Later
# plans append to this list (plan 11 adds its two entrypoint upgrade cases).
HISTORY_CASES="upgrade_pristine_bundled_file history_is_fresh entrypoint_seeding_upgrades_pristine_wrangler entrypoint_sh_seeding_matches"

ALL_CASES="fresh_install preserve_edit upgrade_pristine_bundled_file reject_absolute reject_dotdot reject_foreign_top_level reject_symlink_entry reject_hardlink_entry reject_bad_checksum dest_symlink_not_followed seed_function_parity history_is_fresh forced_cargo_fallback_pins_revision source_mode_fallback incomplete_install_exits_3 cmd_wiring entrypoint_seeding_upgrades_pristine_wrangler entrypoint_sh_seeding_matches"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ih-seed-test.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

FAILS=0

die() {
    echo "$*" >&2
    exit 1
}

is_history_case() {
    case " $HISTORY_CASES " in
        *" $1 "*) return 0 ;;
    esac
    return 1
}

new_home() {
    # Unique per call: each case runs in its own subshell, so a counter would repeat.
    mktemp -d "$WORK/home.XXXXXX"
}

# load_install HOME ARCHIVE -- source install.sh as a library with a clean env.
load_install() {
    export IRONHERMES_HOME="$1"
    export IRONHERMES_INSTALL_LIB_ONLY=1
    if [ -n "${2:-}" ]; then
        export IRONHERMES_SKILLS_ARCHIVE="$2"
    else
        unset IRONHERMES_SKILLS_ARCHIVE
    fi
    unset IRONHERMES_VERSION
    # shellcheck source=/dev/null
    source "$REPO_ROOT/install.sh"
    # shellcheck disable=SC2034  # read by the sourced install.sh functions
    VERSION=""
}

count_files() {
    find "$@" -type f 2>/dev/null | wc -l | tr -d ' '
}

# --- Shared setup (HEAD's tree only) ---------------------------------------
TRACKED_LIST="$WORK/tracked.txt"
git ls-files skills optional-skills >"$TRACKED_LIST"
TRACKED_COUNT="$(grep -c . "$TRACKED_LIST" || true)"
DIST="$WORK/dist"
bash "$REPO_ROOT/scripts/release/package-skills.sh" "$DIST" >/dev/null
TARBALL="$DIST/ironhermes-skills.tar.gz"

# expected_seed_count -- tracked files minus the history manifest when tracked.
expected_seed_count() {
    local n="$TRACKED_COUNT"
    if grep -qxF "skills/.bundled-history" "$TRACKED_LIST"; then
        n=$((n - 1))
    fi
    echo "$n"
}

# --- Cases -----------------------------------------------------------------
case_fresh_install() {
    local home want got
    home="$(new_home)"
    want="$(expected_seed_count)"
    load_install "$home" "$TARBALL"
    seed_skills
    got="$(count_files "$home/skills" "$home/optional-skills")"
    [ "$got" = "$want" ] || die "seeded $got files, expected $want"
    [ ! -e "$home/skills/.bundled-history" ] || die "history manifest was copied into the home"
}

case_preserve_edit() {
    local home f total out
    home="$(new_home)"
    load_install "$home" "$TARBALL"
    seed_skills >/dev/null
    f="$(find "$home/skills" -type f -name SKILL.md | LC_ALL=C sort | sed -n 1p)"
    [ -n "$f" ] || die "no seeded SKILL.md to edit"
    total="$(count_files "$home/skills" "$home/optional-skills")"
    echo "OPERATOR-EDIT-MARKER" >>"$f"
    out="$(seed_skills 2>&1)"
    grep -q "OPERATOR-EDIT-MARKER" "$f" || die "operator edit was overwritten"
    printf '%s\n' "$out" | grep -q 'Skills (skills): 0 new, 0 upgraded, 1 kept' \
        || die "expected exactly one kept file, got: $out"
    [ "$(count_files "$home/skills" "$home/optional-skills")" = "$total" ] || die "file total changed on re-seed"
}

# History-derived: reads commits other than HEAD (git log / git show <rev>:<path>).
case_upgrade_pristine_bundled_file() {
    local home c f old cur target="" out
    home="$(new_home)"
    old="$WORK/old-version.txt"
    git log --format=%H -n 40 -- skills optional-skills >"$WORK/upg-commits.txt"
    while IFS= read -r c; do
        [ -n "$c" ] || continue
        git diff-tree --no-commit-id --name-only -r --diff-filter=M "$c" -- skills optional-skills >"$WORK/upg-files.txt"
        while IFS= read -r f; do
            [ -n "$f" ] || continue
            [ "$f" = "skills/.bundled-history" ] && continue
            [ -f "$REPO_ROOT/$f" ] || continue
            git show "$c^:$f" >"$old" 2>/dev/null || continue
            if ! cmp -s "$old" "$REPO_ROOT/$f"; then
                target="$f"
                break
            fi
        done <"$WORK/upg-files.txt"
        [ -n "$target" ] && break
    done <"$WORK/upg-commits.txt"
    [ -n "$target" ] || die "no file with an older bundled version found in git history"

    load_install "$home" "$TARBALL"
    seed_skills >/dev/null
    cur="$home/$target"
    [ -f "$cur" ] || die "seeded file missing: $target"
    cp "$old" "$cur"
    cmp -s "$old" "$cur" || die "could not plant the older version"
    out="$(seed_skills 2>&1)"
    cmp -s "$REPO_ROOT/$target" "$cur" || die "pristine older copy of $target was not upgraded"
    printf '%s\n' "$out" | grep -q ' 1 upgraded' || die "expected exactly one upgrade, got: $out"
}

# expect_rejected ARCHIVE -- seeding must fail and write nothing.
expect_rejected() {
    local archive="$1" home
    home="$(new_home)"
    load_install "$home" "$archive"
    if seed_skills >/dev/null 2>&1; then
        die "seed_skills accepted a hostile archive: $archive"
    fi
    [ "$(count_files "$home")" = "0" ] || die "files were seeded from a rejected archive"
    echo "$home"
}

case_reject_absolute() {
    local d
    d="$(mktemp -d "$WORK/abs.XXXXXX")"
    mkdir -p "$d/esc"
    echo x >"$d/esc/evil.txt"
    tar -czPf "$WORK/abs.tgz" "$d/esc/evil.txt"
    tar -tzf "$WORK/abs.tgz" | grep -q '^/' || die "setup: archive has no absolute member"
    rm -rf "$d/esc"
    expect_rejected "$WORK/abs.tgz" >/dev/null
    [ ! -e "$d/esc/evil.txt" ] || die "absolute member escaped"
}

case_reject_dotdot() {
    local d home
    d="$(mktemp -d "$WORK/dotdot.XXXXXX")"
    mkdir -p "$d/skills/ok"
    echo x >"$d/skills/ok/SKILL.md"
    echo x >"$d/escape.txt"
    (cd "$d" && tar -czPf "$WORK/dotdot.tgz" skills/ok/SKILL.md skills/../escape.txt)
    tar -tzf "$WORK/dotdot.tgz" | grep -q '\.\./' || die "setup: archive has no .. member"
    home="$(expect_rejected "$WORK/dotdot.tgz")"
    [ ! -e "$home/escape.txt" ] && [ ! -e "$home/../escape.txt" ] || die ".. member escaped"
}

case_reject_foreign_top_level() {
    local d
    d="$(mktemp -d "$WORK/foreign.XXXXXX")"
    mkdir -p "$d/skills/ok" "$d/other"
    echo x >"$d/skills/ok/SKILL.md"
    echo x >"$d/other/payload"
    (cd "$d" && tar -czf "$WORK/foreign.tgz" skills other)
    expect_rejected "$WORK/foreign.tgz" >/dev/null
}

case_reject_symlink_entry() {
    local d
    d="$(mktemp -d "$WORK/symlink.XXXXXX")"
    mkdir -p "$d/skills/x"
    echo x >"$d/skills/x/SKILL.md"
    ln -s /etc "$d/skills/x/link"
    (cd "$d" && tar -czf "$WORK/symlink.tgz" skills)
    tar -tvzf "$WORK/symlink.tgz" | grep -q '^l' || die "setup: archive has no symlink entry"
    expect_rejected "$WORK/symlink.tgz" >/dev/null
}

case_reject_hardlink_entry() {
    local d
    d="$(mktemp -d "$WORK/hardlink.XXXXXX")"
    mkdir -p "$d/skills/x"
    echo x >"$d/skills/x/a"
    ln "$d/skills/x/a" "$d/skills/x/b"
    (cd "$d" && tar -czf "$WORK/hardlink.tgz" skills)
    tar -tvzf "$WORK/hardlink.tgz" | grep -qE '^h| link to ' || die "setup: archive has no hard-link entry"
    expect_rejected "$WORK/hardlink.tgz" >/dev/null
}

case_reject_bad_checksum() {
    cp "$TARBALL" "$WORK/badsum.tgz"
    printf '%s  ironhermes-skills.tar.gz\n' "0000000000000000000000000000000000000000000000000000000000000000" \
        >"$WORK/badsum.tgz.sha256"
    expect_rejected "$WORK/badsum.tgz" >/dev/null
}

case_dest_symlink_not_followed() {
    local home outside cat want
    home="$(new_home)"
    outside="$(mktemp -d "$WORK/outside.XXXXXX")"
    cat="$(sed -n 's#^skills/\([^/][^/]*\)/.*#\1#p' "$TRACKED_LIST" | LC_ALL=C sort -u | sed -n 1p)"
    [ -n "$cat" ] || die "no bundled skills category found"
    mkdir -p "$home/skills"
    ln -s "$outside" "$home/skills/$cat"
    load_install "$home" "$TARBALL"
    if seed_skills >/dev/null 2>&1; then
        die "seed_skills returned success despite a symlinked destination category"
    fi
    [ -z "$(ls -A "$outside")" ] || die "seeding wrote through the destination symlink"
    [ -L "$home/skills/$cat" ] || die "the planted symlink was replaced"
    want="$(grep -c '^optional-skills/' "$TRACKED_LIST" || true)"
    [ "$(count_files "$home/optional-skills")" = "$want" ] || die "optional-skills was not fully seeded"
}

case_seed_function_parity() {
    local a="$WORK/block.seed" b="$WORK/block.install"
    sed -n '/^# >>> ih_seed_bundled_tree$/,/^# <<< ih_seed_bundled_tree$/p' "$REPO_ROOT/docker/seed-bundled.sh" >"$a"
    sed -n '/^# >>> ih_seed_bundled_tree$/,/^# <<< ih_seed_bundled_tree$/p' "$REPO_ROOT/install.sh" >"$b"
    [ -s "$a" ] || die "marker block missing from docker/seed-bundled.sh"
    [ -s "$b" ] || die "marker block missing from install.sh"
    cmp -s "$a" "$b" || die "install.sh block differs from docker/seed-bundled.sh (re-splice it)"
}

# History-derived: --check-history walks git history beyond HEAD.
case_history_is_fresh() {
    bash "$REPO_ROOT/scripts/release/package-skills.sh" --check-history
}

# make_shim DIR -- recording cargo / git / curl shims; they append to $IH_SHIM_REC.
make_shim() {
    local shim="$1"
    cat >"$shim/cargo" <<'EOF'
#!/bin/sh
echo "cargo $*" >>"$IH_SHIM_REC"
EOF
    cat >"$shim/git" <<'EOF'
#!/bin/sh
echo "git $*" >>"$IH_SHIM_REC"
if [ "$1" = "ls-remote" ]; then
    printf '%s\tHEAD\n' "$IH_FAKE_SHA"
fi
EOF
    cat >"$shim/curl" <<'EOF'
#!/bin/sh
url=""
out=""
while [ $# -gt 0 ]; do
    case "$1" in
        -o) out="$2"; shift 2; continue ;;
        http*) url="$1" ;;
    esac
    shift
done
echo "curl $url" >>"$IH_SHIM_REC"
case "$url" in
    */releases/download/*) exit 22 ;;
    */archive/*) cp "$IH_SRC_TGZ" "$out" ;;
    *) exit 22 ;;
esac
EOF
    chmod +x "$shim/cargo" "$shim/git" "$shim/curl"
}

case_forced_cargo_fallback_pins_revision() {
    local shim rec home line sha="0123456789abcdef0123456789abcdef01234567"
    shim="$(mktemp -d "$WORK/shim.XXXXXX")"
    rec="$shim/rec.txt"
    : >"$rec"
    make_shim "$shim"
    export IH_SHIM_REC="$rec" IH_FAKE_SHA="$sha"
    PATH="$shim:$PATH"
    home="$(new_home)"
    load_install "$home" ""

    # A release tag is known: pin --tag.
    VERSION="v0.5.0"
    SKILLS_REF=""
    SKILLS_REF_KIND=""
    cargo_install >/dev/null
    line="$(grep '^cargo install' "$rec")"
    case "$line" in *" --locked "*) ;; *) die "no --locked: $line" ;; esac
    case "$line" in *" --tag v0.5.0 "*) ;; *) die "no --tag v0.5.0: $line" ;; esac
    case "$line" in *" --bin ironhermes "*) ;; *) die "no --bin ironhermes: $line" ;; esac
    [ "${line##* }" = "ironhermes-cli" ] || die "package argument is not ironhermes-cli: $line"
    [ "$(printf '%s\n' "$line" | tr ' ' '\n' | grep -cx ironhermes)" = "1" ] \
        || die "a bare ironhermes package argument is present: $line"
    [ "$SKILLS_REF" = "v0.5.0" ] || die "SKILLS_REF is '$SKILLS_REF', expected v0.5.0"

    # No tag: pin the commit git ls-remote reports.
    : >"$rec"
    VERSION=""
    SKILLS_REF=""
    SKILLS_REF_KIND=""
    cargo_install >/dev/null
    line="$(grep '^cargo install' "$rec")"
    case "$line" in *" --rev $sha "*) ;; *) die "no --rev $sha: $line" ;; esac
    case "$line" in *" --locked "*) ;; *) die "no --locked: $line" ;; esac
    [ "${line##* }" = "ironhermes-cli" ] || die "package argument is not ironhermes-cli: $line"
    [ "$SKILLS_REF" = "$sha" ] || die "SKILLS_REF is '$SKILLS_REF', expected $sha"

    # No tag and no usable ls-remote answer: default branch, unpinned (warned).
    : >"$rec"
    IH_FAKE_SHA=""
    SKILLS_REF=""
    SKILLS_REF_KIND=""
    cargo_install >"$shim/out.txt"
    line="$(grep '^cargo install' "$rec")"
    case "$line" in *" --branch main "*) ;; *) die "no --branch main: $line" ;; esac
    grep -q "without a pinned revision" "$shim/out.txt" || die "missing unpinned warning"
}

case_source_mode_fallback() {
    local shim rec home want
    shim="$(mktemp -d "$WORK/shim.XXXXXX")"
    rec="$shim/rec.txt"
    : >"$rec"
    make_shim "$shim"
    git archive --format=tar --prefix=iron-hermes-test/ HEAD -- skills optional-skills README.md \
        | gzip -c >"$shim/src.tgz"
    export IH_SHIM_REC="$rec" IH_FAKE_SHA="" IH_SRC_TGZ="$shim/src.tgz"
    PATH="$shim:$PATH"
    home="$(new_home)"
    load_install "$home" ""
    VERSION="v0.5.0"
    SKILLS_REF=""
    SKILLS_REF_KIND=""
    want="$(expected_seed_count)"
    seed_skills
    grep -q 'releases/download/v0.5.0/ironhermes-skills.tar.gz' "$rec" || die "release asset was not tried first"
    grep -q 'archive/refs/tags/v0.5.0.tar.gz' "$rec" || die "source archive was not fetched"
    [ "$(count_files "$home/skills" "$home/optional-skills")" = "$want" ] \
        || die "seeded file count differs from $want"
    [ "$(ls -A "$home" | LC_ALL=C sort | tr '\n' ' ')" = "optional-skills skills " ] \
        || die "source mode left extra entries in the home: $(ls -A "$home" | tr '\n' ' ')"
}

case_incomplete_install_exits_3() {
    local home err rc=0
    home="$(new_home)"
    load_install "$home" ""
    SKILLS_INCOMPLETE=1
    err="$(ih_finish_install 2>&1 >/dev/null)" || rc=$?
    [ "$rc" = "3" ] || die "ih_finish_install returned $rc, expected 3"
    printf '%s\n' "$err" | grep -q 'INCOMPLETE INSTALL' || die "no INCOMPLETE INSTALL message: $err"
    SKILLS_INCOMPLETE=0
    rc=0
    ih_finish_install || rc=$?
    [ "$rc" = "0" ] || die "ih_finish_install returned $rc with nothing incomplete"
}

case_cmd_wiring() {
    local fn body
    for fn in cmd_install cmd_update cmd_reinstall; do
        body="$(awk -v f="${fn}() {" 'index($0, f) == 1 { on = 1 } on { print } on && /^}/ { exit }' "$REPO_ROOT/install.sh")"
        [ -n "$body" ] || die "$fn not found in install.sh"
        printf '%s\n' "$body" | grep -q 'seed_skills ||' || die "$fn does not call 'seed_skills ||'"
        printf '%s\n' "$body" | grep -q 'ih_finish_install' || die "$fn does not call ih_finish_install"
    done
}

# entrypoint_seed_check ENTRYPOINT -- run the marker-delimited skills seeding
# block of a docker entrypoint against a fake image root and a fake volume that
# holds the PRE-CHANGE (MCP-gated) wrangler skill from git history, plus an
# operator-edited sibling. The block must exit 0, upgrade the pristine wrangler
# copy, keep the edited sibling and copy the brand-new skill.
# History-derived: reads an older wrangler version with git log / git show.
entrypoint_seed_check() {
    local ep="$1" root home block old="" c found=""
    local wr="optional-skills/cloudflare/wrangler/SKILL.md"
    local sib="optional-skills/cloudflare/workers-best-practices/SKILL.md"
    local fresh="skills/devops/site-deploy/SKILL.md"

    block="$(mktemp "$WORK/ep-block.XXXXXX")"
    sed -n '/^# >>> skills seeding$/,/^# <<< skills seeding$/p' "$REPO_ROOT/$ep" >"$block"
    [ -s "$block" ] || die "no '# >>> skills seeding' block in $ep"
    grep -q 'ih_seed_bundled_tree' "$block" || die "$ep seeding block does not call ih_seed_bundled_tree"

    # The pre-change wrangler skill still declares the MCP gate.
    old="$(mktemp "$WORK/old-wrangler.XXXXXX")"
    git log --format=%H -- "$wr" >"$WORK/wr-commits.txt"
    while IFS= read -r c; do
        [ -n "$c" ] || continue
        git show "$c:$wr" >"$old" 2>/dev/null || continue
        if grep -q 'requires_mcp_servers' "$old"; then
            found=1
            break
        fi
    done <"$WORK/wr-commits.txt"
    [ -n "$found" ] || die "no gated wrangler version found in git history"
    ! grep -q 'requires_mcp_servers' "$REPO_ROOT/$wr" || die "current wrangler skill still carries the MCP gate"

    # Fake image root (what the Dockerfile bakes into /opt/ironhermes).
    root="$(mktemp -d "$WORK/image.XXXXXX")"
    mkdir -p "$root/docker" "$root/skills/devops/site-deploy" "$root/optional-skills/cloudflare"
    cp "$REPO_ROOT/docker/seed-bundled.sh" "$root/docker/seed-bundled.sh"
    cp "$REPO_ROOT/skills/.bundled-history" "$root/skills/.bundled-history"
    cp "$REPO_ROOT/$fresh" "$root/$fresh"
    cp -R "$REPO_ROOT/optional-skills/cloudflare/." "$root/optional-skills/cloudflare/"

    # Fake existing volume: same cloudflare tree, wrangler rolled back to the
    # gated version, one sibling edited by the operator.
    home="$(new_home)"
    mkdir -p "$home/optional-skills/cloudflare"
    cp -R "$REPO_ROOT/optional-skills/cloudflare/." "$home/optional-skills/cloudflare/"
    cp "$old" "$home/$wr"
    grep -q 'requires_mcp_servers' "$home/$wr" || die "could not plant the gated wrangler skill"
    echo "OPERATOR-EDIT-MARKER" >>"$home/$sib"

    # Run the block the way the entrypoint does (set -euo pipefail), exit 0 required.
    bash -c 'set -euo pipefail; INSTALL_DIR="$1"; IRONHERMES_HOME="$2"; . "$3"' _ "$root" "$home" "$block" \
        || die "$ep seeding block did not exit 0"

    ! grep -q 'requires_mcp_servers' "$home/$wr" || die "pristine gated wrangler skill was not upgraded by $ep"
    cmp -s "$REPO_ROOT/$wr" "$home/$wr" || die "wrangler skill on the volume differs from the bundled one"
    grep -q 'OPERATOR-EDIT-MARKER' "$home/$sib" || die "operator edit to $sib was overwritten"
    [ -f "$home/$fresh" ] || die "new bundled skill $fresh was not seeded"
    [ ! -e "$home/skills/.bundled-history" ] || die "history manifest was copied into the volume"
}

case_entrypoint_seeding_upgrades_pristine_wrangler() {
    entrypoint_seed_check docker/web-entrypoint.sh
}

case_entrypoint_sh_seeding_matches() {
    entrypoint_seed_check docker/entrypoint.sh
}

# --- Runner ----------------------------------------------------------------
run_case() {
    local name="$1" out pid
    if ! declare -F "case_$name" >/dev/null 2>&1; then
        echo "FAIL unknown-case $name"
        FAILS=$((FAILS + 1))
        return 0
    fi
    if [ "${IH_SEED_TEST_NO_HISTORY:-}" = "1" ] && is_history_case "$name"; then
        echo "SKIP $name (history-derived)"
        return 0
    fi
    out="$WORK/out.$name"
    # Background subshell so `set -e` stays in force inside the case.
    ( "case_$name" ) >"$out" 2>&1 &
    pid=$!
    if wait "$pid"; then
        echo "PASS $name"
    else
        echo "FAIL $name"
        sed 's/^/    /' "$out"
        FAILS=$((FAILS + 1))
    fi
}

if [ "$#" -ge 1 ]; then
    run_case "$1"
else
    for c in $ALL_CASES; do
        run_case "$c"
    done
fi

if [ "$FAILS" -gt 0 ]; then
    echo "FAILED: $FAILS case(s)"
    exit 1
fi
exit 0

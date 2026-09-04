#!/usr/bin/env bash
# =============================================================================
# scripts/install-git-hooks.sh — Phase 49.1 D-06: install the opt-in
# commit-time secret-scan hook
# =============================================================================
# Symlinks scripts/hooks/pre-commit into .git/hooks/pre-commit. REMINDER:
# .git/hooks/ is UNTRACKED and does not survive a fresh clone —
# scripts/ci-gates.sh Gate 8 (D-06) is the AUTHORITATIVE enforcement point;
# this hook only adds earlier, opt-in feedback on top of it.
#
# Usage:
#   install-git-hooks.sh [--force]
#
# Options:
#   --force   Overwrite a pre-existing non-symlink .git/hooks/pre-commit.
#             Without it, an existing non-symlink hook is left untouched
#             and the script refuses (exit non-zero).
# =============================================================================
set -euo pipefail

die() { printf 'error: %s\n' "$*" >&2; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
HOOK_SRC="${REPO_ROOT}/scripts/hooks/pre-commit"

FORCE=0
while [ "$#" -gt 0 ]; do
    case "$1" in
        --force) FORCE=1; shift ;;
        -h|--help)
            sed -n '2,17p' "$0"
            exit 0
            ;;
        *) die "unknown option: $1" ;;
    esac
done

[ -f "${HOOK_SRC}" ] || die "${HOOK_SRC} not found — is this repo checked out correctly?"

# -----------------------------------------------------------------------------
# Resolve the ACTUAL hook lookup directory. `git rev-parse --git-dir` is
# deliberately NOT used here: from inside a linked worktree it returns the
# per-worktree private dir (.git/worktrees/<name>), which git does NOT
# consult for hook execution — hooks are shared across the main checkout
# and every linked worktree via the COMMON git dir (ground-truthed while
# authoring this script: installing via --git-dir silently wrote to a path
# git never reads, so the hook never actually fired). `--git-common-dir`
# is the correct primitive; an explicit `core.hooksPath` (if the operator
# has one configured) takes precedence over the default `<common-dir>/hooks`,
# exactly as git itself resolves hook lookup.
# -----------------------------------------------------------------------------
HOOKS_PATH_CFG="$(git -C "${REPO_ROOT}" config --get core.hooksPath 2>/dev/null || true)"
if [ -n "${HOOKS_PATH_CFG}" ]; then
    case "${HOOKS_PATH_CFG}" in
        /*) HOOKS_DIR="${HOOKS_PATH_CFG}" ;;
        *)  HOOKS_DIR="${REPO_ROOT}/${HOOKS_PATH_CFG}" ;;
    esac
else
    GIT_COMMON_DIR="$(cd "${REPO_ROOT}" && git rev-parse --git-common-dir)"
    case "${GIT_COMMON_DIR}" in
        /*) : ;;
        *)  GIT_COMMON_DIR="${REPO_ROOT}/${GIT_COMMON_DIR}" ;;
    esac
    HOOKS_DIR="${GIT_COMMON_DIR}/hooks"
fi
HOOK_DEST="${HOOKS_DIR}/pre-commit"

mkdir -p "${HOOKS_DIR}"
chmod +x "${HOOK_SRC}"

# CR-02 (48.3 code review): a previous run of THIS script may have left a
# generated chaining dispatcher at HOOK_DEST. That dispatcher is a regular
# file, not a symlink, so without this guard a second --force run would treat
# it as a foreign hook: mv it over the preserved foreign hook (destroying the
# real one) and generate a dispatcher that invokes itself — infinite
# recursion that hangs every subsequent commit. Detect our own artifact by
# its marker and re-generate in place instead.
DISPATCHER_MARKER="gsd:generated-dispatcher install-git-hooks.sh"
IS_OUR_DISPATCHER=0
if [ -f "${HOOK_DEST}" ] && [ ! -L "${HOOK_DEST}" ] \
   && grep -qF "${DISPATCHER_MARKER}" "${HOOK_DEST}" 2>/dev/null; then
    IS_OUR_DISPATCHER=1
fi

if [ -e "${HOOK_DEST}" ] && [ ! -L "${HOOK_DEST}" ] \
   && [ "${IS_OUR_DISPATCHER}" -eq 0 ] && [ "${FORCE}" -ne 1 ]; then
    die "${HOOK_DEST} already exists and is not a symlink this script installed (use --force to overwrite)"
fi

# -----------------------------------------------------------------------------
# Phase 48.3 Plan 06 (D-11): a pre-existing, unrelated, non-symlink hook at
# HOOK_DEST (e.g. an OKF-stub-regeneration hook installed via
# `git config core.hooksPath .githooks`, whose destination under a bare
# `core.hooksPath`-less default would resolve to exactly this same path)
# must not be silently clobbered by --force. Preserve it alongside this
# hook and generate a small dispatcher that runs both, in order, aborting
# the commit if either fails. When HOOK_DEST is absent, or is already the
# symlink this script itself installs, behavior is unchanged from before
# this fix (plain rm -f + ln -s).
# -----------------------------------------------------------------------------
if [ -e "${HOOK_DEST}" ] && [ ! -L "${HOOK_DEST}" ] && [ "${IS_OUR_DISPATCHER}" -eq 0 ]; then
    # FORCE=1 here — the die() above already exited for FORCE=0.
    PRESERVED_NAME="pre-commit.pre-48.3"
    PRESERVED="${HOOKS_DIR}/${PRESERVED_NAME}"
    mv -f "${HOOK_DEST}" "${PRESERVED}"
    chmod +x "${PRESERVED}"

    cat > "${HOOK_DEST}" <<HOOKEOF
#!/usr/bin/env bash
# gsd:generated-dispatcher install-git-hooks.sh
# GENERATED by scripts/install-git-hooks.sh (Phase 48.3 Plan 06, D-11
# collision-safe install). Chains the pre-existing hook this installer
# found at this path — preserved alongside it as "${PRESERVED_NAME}" —
# with the D-06 secret-scan hook (scripts/hooks/pre-commit), aborting the
# commit if either exits non-zero. This file is generated: do not edit it
# by hand. Re-run scripts/install-git-hooks.sh --force to regenerate it.
set -euo pipefail
HOOK_DIR="\$(cd "\$(dirname "\${BASH_SOURCE[0]}")" && pwd)"
"\${HOOK_DIR}/${PRESERVED_NAME}"
"${HOOK_SRC}"
HOOKEOF
    chmod +x "${HOOK_DEST}"

    echo "Preserved pre-existing hook: ${HOOK_DEST} -> ${PRESERVED}"
    echo "Installed chained dispatcher: ${HOOK_DEST} (runs ${PRESERVED_NAME} then ${HOOK_SRC})"
elif [ "${IS_OUR_DISPATCHER}" -eq 1 ]; then
    # CR-02: our own dispatcher from a previous run. Re-generate it in place,
    # leaving the already-preserved foreign hook untouched. Never mv it.
    PRESERVED_NAME="pre-commit.pre-48.3"
    PRESERVED="${HOOKS_DIR}/${PRESERVED_NAME}"

    if [ ! -e "${PRESERVED}" ]; then
        # The chained hook is gone; nothing left to chain. Fall back to the
        # plain symlink rather than generating a dispatcher that calls a
        # file that does not exist.
        rm -f "${HOOK_DEST}"
        ln -s "${HOOK_SRC}" "${HOOK_DEST}"
        echo "Chained hook ${PRESERVED_NAME} no longer exists; installed plain hook: ${HOOK_DEST} -> ${HOOK_SRC}"
    else
        cat > "${HOOK_DEST}" <<HOOKEOF
#!/usr/bin/env bash
# gsd:generated-dispatcher install-git-hooks.sh
# GENERATED by scripts/install-git-hooks.sh (Phase 48.3 Plan 06, D-11
# collision-safe install). Chains the pre-existing hook this installer
# found at this path — preserved alongside it as "${PRESERVED_NAME}" —
# with the D-06 secret-scan hook (scripts/hooks/pre-commit), aborting the
# commit if either exits non-zero. This file is generated: do not edit it
# by hand. Re-run scripts/install-git-hooks.sh --force to regenerate it.
set -euo pipefail
HOOK_DIR="\$(cd "\$(dirname "\${BASH_SOURCE[0]}")" && pwd)"
"\${HOOK_DIR}/${PRESERVED_NAME}"
"${HOOK_SRC}"
HOOKEOF
        chmod +x "${HOOK_DEST}"
        echo "Re-generated chained dispatcher: ${HOOK_DEST} (runs ${PRESERVED_NAME} then ${HOOK_SRC}; ${PRESERVED_NAME} left untouched)"
    fi
else
    rm -f "${HOOK_DEST}"
    ln -s "${HOOK_SRC}" "${HOOK_DEST}"

    echo "Installed pre-commit hook: ${HOOK_DEST} -> ${HOOK_SRC}"
fi

echo
echo "REMINDER: this hook lives in .git/hooks/, which is UNTRACKED and does"
echo "NOT survive a fresh clone. scripts/ci-gates.sh Gate 8 (D-06) is the"
echo "authoritative enforcement point — this hook only adds earlier, opt-in"
echo "commit-time feedback for anyone who has run this installer."

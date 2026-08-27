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

if [ -e "${HOOK_DEST}" ] && [ ! -L "${HOOK_DEST}" ] && [ "${FORCE}" -ne 1 ]; then
    die "${HOOK_DEST} already exists and is not a symlink this script installed (use --force to overwrite)"
fi

rm -f "${HOOK_DEST}"
ln -s "${HOOK_SRC}" "${HOOK_DEST}"

echo "Installed pre-commit hook: ${HOOK_DEST} -> ${HOOK_SRC}"
echo
echo "REMINDER: this hook lives in .git/hooks/, which is UNTRACKED and does"
echo "NOT survive a fresh clone. scripts/ci-gates.sh Gate 8 (D-06) is the"
echo "authoritative enforcement point — this hook only adds earlier, opt-in"
echo "commit-time feedback for anyone who has run this installer."

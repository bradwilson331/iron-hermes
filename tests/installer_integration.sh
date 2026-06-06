#!/usr/bin/env bash
# tests/installer_integration.sh — bash-level integration tests for install.sh subcommands.
# REQ-37.1-05/-06/-08: verify uninstall, update (additive-merge), hermes-agent detection.
#
# Usage:
#   bash tests/installer_integration.sh           # run all tests (FAILS today — red Wave 0)
#   bash tests/installer_integration.sh --verify  # harness self-check only (exits 0)
#   bash tests/installer_integration.sh --clean   # remove temp dirs
#   bash tests/installer_integration.sh --help    # show usage
#
# WAVE-0 RED STATE: The full (no-arg) run FAILS because install.sh has no verb dispatch yet.
# verify_uninstall / verify_update_additive / verify_hermes_agent_detection etc. will fail
# when they call `bash install.sh uninstall / update / install` — those verbs do not exist.
# This test becomes GREEN in Plan 02 (installer verb dispatch implementation).
#
# Security (T-37.1-01-01, T-37.1-01-02):
#   All writes are confined to TMPDIR_ROOT=$(mktemp -d).
#   HOME and IRONHERMES_HOME are overridden per-invocation — the real ~/.ironhermes and
#   ~/.hermes are NEVER touched.
#   trap cleanup EXIT guarantees temp removal on any exit path.

set -euo pipefail

# ---------------------------------------------------------------------------
# Minimal log helpers (match install.sh style)
# ---------------------------------------------------------------------------
log()  { printf '[installer-test] %s\n' "$*"; }
warn() { printf '[installer-test] WARN: %s\n' "$*" >&2; }
fail() { printf '[installer-test] FAIL: %s\n' "$*" >&2; exit 1; }
pass() { printf '[installer-test] PASS: %s\n' "$*"; }

# ---------------------------------------------------------------------------
# Fixture setup (T-37.1-01-01: confined to mktemp -d, never touches real HOME)
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
INSTALL_SH="$REPO_ROOT/install.sh"

TMPDIR_ROOT=$(mktemp -d)
FAKE_HOME="${TMPDIR_ROOT}/home"
FAKE_BIN="${FAKE_HOME}/.local/bin"
FAKE_IH_HOME="${FAKE_HOME}/.ironhermes"

mkdir -p "$FAKE_BIN" "$FAKE_IH_HOME"

cleanup() { rm -rf "$TMPDIR_ROOT"; }
trap cleanup EXIT

# ---------------------------------------------------------------------------
# Helper: reset fixture state between tests
# ---------------------------------------------------------------------------
reset_fixtures() {
    rm -rf "$FAKE_HOME"
    mkdir -p "$FAKE_BIN" "$FAKE_IH_HOME"
}

# ---------------------------------------------------------------------------
# Helper: run install.sh with a sandboxed HOME and IRONHERMES_HOME.
# Never references the real $HOME for writes.
# ---------------------------------------------------------------------------
run_installer() {
    # $1 = verb (install / update / uninstall / reinstall) or flags
    HOME="$FAKE_HOME" \
    IRONHERMES_HOME="$FAKE_IH_HOME" \
    bash "$INSTALL_SH" "$@" 2>&1
}

# ---------------------------------------------------------------------------
# REQ-37.1-05: verify_uninstall
# After `uninstall` (no --purge): binary file absent, FAKE_IH_HOME preserved.
# WAVE-0 RED: install.sh has no `uninstall` verb yet — fails with "Unknown subcommand".
# Turns GREEN in Plan 02.
# ---------------------------------------------------------------------------
verify_uninstall() {
    log "verify_uninstall: checking uninstall removes binary but preserves ~/.ironhermes"
    reset_fixtures
    # Seed a fake binary so there is something to remove
    touch "$FAKE_BIN/ironhermes"

    run_installer uninstall || true   # allowed to exit non-zero (verb unimplemented)

    # Binary must be absent after uninstall
    [ ! -f "${FAKE_BIN}/ironhermes" ] \
        || fail "uninstall: binary should be removed from FAKE_BIN"

    # Home dir must be preserved (no --purge)
    [ -d "${FAKE_IH_HOME}" ] \
        || fail "uninstall: ~/.ironhermes should be preserved without --purge"

    pass "verify_uninstall"
}

# ---------------------------------------------------------------------------
# REQ-37.1-05: verify_uninstall_purge
# After `uninstall --purge`: binary absent AND FAKE_IH_HOME removed.
# WAVE-0 RED: install.sh has no `uninstall` verb yet.
# Turns GREEN in Plan 02.
# ---------------------------------------------------------------------------
verify_uninstall_purge() {
    log "verify_uninstall_purge: checking --purge removes ~/.ironhermes"
    reset_fixtures
    touch "$FAKE_BIN/ironhermes"

    run_installer uninstall --purge || true

    [ ! -f "${FAKE_BIN}/ironhermes" ] \
        || fail "uninstall --purge: binary should be removed"

    [ ! -d "${FAKE_IH_HOME}" ] \
        || fail "uninstall --purge: ~/.ironhermes should be removed with --purge flag"

    pass "verify_uninstall_purge"
}

# ---------------------------------------------------------------------------
# REQ-37.1-06: verify_update_additive
# After `update`: a user-set key in config.yaml must remain byte-unchanged.
# Additive merge must NEVER clobber existing set values.
# WAVE-0 RED: install.sh has no `update` verb yet.
# Turns GREEN in Plan 02.
# ---------------------------------------------------------------------------
verify_update_additive() {
    log "verify_update_additive: checking update does not clobber user-set config values"
    reset_fixtures

    # Seed a config with a known user-set value
    local user_config="$FAKE_IH_HOME/config.yaml"
    cat > "$user_config" <<'YAML'
model:
  api_key: "sk-user-set-value-must-survive"
  default: "openai/gpt-4o"
YAML

    local original_content
    original_content=$(cat "$user_config")

    run_installer update || true

    # The user key value must be byte-unchanged
    local after_content
    after_content=$(cat "$user_config" 2>/dev/null || echo "FILE_MISSING")

    [ "$after_content" = "$original_content" ] \
        || fail "verify_update_additive: config.yaml user values were mutated by update \
(REQ-37.1-06 additive-merge contract)"

    pass "verify_update_additive"
}

# ---------------------------------------------------------------------------
# REQ-37.1-08: verify_hermes_agent_detection
# During `install`, if ~/.hermes/config.yaml exists:
#   1. stdout must contain "hermes-agent detected"
#   2. ~/.hermes/config.yaml must be byte-unchanged (T-37.1-01-02)
# WAVE-0 RED: install.sh has no detect_existing_install with hermes-agent check yet.
# Turns GREEN in Plan 02.
# ---------------------------------------------------------------------------
verify_hermes_agent_detection() {
    log "verify_hermes_agent_detection: checking hermes-agent coexistence notice and no mutation"
    reset_fixtures

    # Create fake hermes-agent config (T-37.1-01-02: never touch real ~/.hermes)
    local fake_hermes_cfg="${FAKE_HOME}/.hermes/config.yaml"
    mkdir -p "${FAKE_HOME}/.hermes"
    echo "hermes_agent_user_config: preserved" > "$fake_hermes_cfg"

    local original_content
    original_content=$(cat "$fake_hermes_cfg")

    local output
    output=$(run_installer install || true)

    # Installer must print the detection notice
    echo "$output" | grep -q "hermes-agent detected" \
        || fail "verify_hermes_agent_detection: installer must print 'hermes-agent detected' notice (REQ-37.1-08)"

    # ~/.hermes/config.yaml must be byte-unchanged (T-37.1-01-02)
    local after_content
    after_content=$(cat "$fake_hermes_cfg" 2>/dev/null || echo "FILE_MISSING")

    [ "$after_content" = "$original_content" ] \
        || fail "verify_hermes_agent_detection: ~/.hermes/config.yaml was mutated — must be untouched (T-37.1-01-02)"

    pass "verify_hermes_agent_detection"
}

# ---------------------------------------------------------------------------
# verify_openclaw_detection
# During `install`, if ~/.openclaw/ exists: stdout must contain "openclaw" notice.
# WAVE-0 RED: install.sh has no openclaw detection yet.
# Turns GREEN in Plan 02.
# ---------------------------------------------------------------------------
verify_openclaw_detection() {
    log "verify_openclaw_detection: checking openclaw coexistence notice"
    reset_fixtures

    # Create fake openclaw dir
    mkdir -p "${FAKE_HOME}/.openclaw"

    local output
    output=$(run_installer install || true)

    echo "$output" | grep -qi "openclaw" \
        || fail "verify_openclaw_detection: installer must print 'openclaw' notice when ~/.openclaw exists"

    pass "verify_openclaw_detection"
}

# ---------------------------------------------------------------------------
# --verify: harness self-check only (T-37.1-01-01 mitigation audit)
# Exits 0 if the harness infrastructure is sound.
# install.sh does NOT need to exist at this path for --verify to pass.
# ---------------------------------------------------------------------------
verify_harness() {
    log "Harness self-check"

    # mktemp works
    local tmp
    tmp=$(mktemp -d)
    [ -d "$tmp" ] || fail "harness: mktemp -d failed"
    rm -rf "$tmp"
    pass "mktemp -d works"

    # install.sh path is resolvable
    [ -f "$INSTALL_SH" ] \
        || fail "harness: install.sh not found at expected path: $INSTALL_SH"
    pass "install.sh found at $INSTALL_SH"

    # FAKE_HOME is under TMPDIR_ROOT (not real HOME — T-37.1-01-01)
    [[ "$FAKE_HOME" == "$TMPDIR_ROOT"* ]] \
        || fail "harness: FAKE_HOME ($FAKE_HOME) is not under TMPDIR_ROOT ($TMPDIR_ROOT)"
    pass "FAKE_HOME is correctly scoped under TMPDIR_ROOT"

    # FAKE_IH_HOME is under TMPDIR_ROOT
    [[ "$FAKE_IH_HOME" == "$TMPDIR_ROOT"* ]] \
        || fail "harness: FAKE_IH_HOME ($FAKE_IH_HOME) is not under TMPDIR_ROOT"
    pass "FAKE_IH_HOME is correctly scoped under TMPDIR_ROOT"

    log "Harness self-check PASSED"
    return 0
}

# ---------------------------------------------------------------------------
# main() — flag dispatch
# ---------------------------------------------------------------------------
main() {
    case "${1:-}" in
        --verify)
            verify_harness
            exit 0
            ;;
        --clean)
            cleanup
            exit 0
            ;;
        -h|--help)
            sed -n '2,12p' "$0"
            exit 0
            ;;
        "")
            ;;
        *)
            warn "Unknown flag: $1 (use --verify | --clean | --help)"
            exit 2
            ;;
    esac

    log "Running installer integration tests (Wave-0 red — install.sh lacks verb dispatch)"
    verify_uninstall
    verify_uninstall_purge
    verify_update_additive
    verify_hermes_agent_detection
    verify_openclaw_detection
    log "All installer integration tests passed."
}

main "$@"

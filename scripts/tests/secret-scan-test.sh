#!/usr/bin/env bash
# =============================================================================
# scripts/tests/secret-scan-test.sh — D-11 regression test for
# scripts/lib/secret-scan.sh
# =============================================================================
# Runnable, self-contained: sources the shared secret-scan library, writes
# synthetic fixtures into a `mktemp -d` scratch directory, and asserts:
#   - a uuid-shaped bearer credential is caught
#   - a 32+ character opaque bearer credential is caught
#   - a short/labelled bearer string ("Bearer FIRECRAWL_API_KEY") is NOT
#     caught — the false-positive guard that keeps Gate 8 usable
#   - a line carrying `secret-scan:allow` is suppressed and counted
#   - each pre-existing family (sk-, Slack, Telegram, Discord, canary)
#     still fires
#   - calling secret_scan_tracked with zero files still fails closed
#
# This script is itself a tracked file that CI Gate 8 scans with the very
# pattern it is testing here — so every credential-shaped fixture below is
# ASSEMBLED AT RUNTIME (shell parameter expansion / printf), never written
# as a source literal. Suppressing a literal fixture with `secret-scan:allow`
# would also defeat this test, since the suppression path is itself one of
# the behaviors under test.
#
# Run with: bash scripts/tests/secret-scan-test.sh
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
# shellcheck source=../lib/secret-scan.sh
source "${REPO_ROOT}/scripts/lib/secret-scan.sh"

FAIL=0
SCRATCH="$(mktemp -d)"
trap 'rm -rf "${SCRATCH}"' EXIT

pass() { echo "PASS: $1"; }
fail() {
    echo "FAIL: $1"
    FAIL=1
}

# -----------------------------------------------------------------------------
# Runtime-assembled fixture builders. No credential-shaped literal appears
# in this file's source — every value below is built character-by-character
# or via `printf`/parameter expansion at run time.
# -----------------------------------------------------------------------------

repeat_char() {
    # repeat_char CHAR COUNT — prints COUNT copies of CHAR, no trailing newline.
    local ch="$1" n="$2"
    printf "%${n}s" '' | tr ' ' "${ch}"
}

fixture_uuid_bearer() {
    # A synthetic uuid-shaped (8-4-4-4-12) credential with sequential hex
    # digits — the exact shape behind the 2026-08-27 incident.
    local hex="0123456789abcdef" seq="" i
    for ((i = 0; i < 32; i++)); do
        seq+="${hex:i%16:1}"
    done
    printf 'Bearer %s-%s-%s-%s-%s\n' \
        "${seq:0:8}" "${seq:8:4}" "${seq:12:4}" "${seq:16:4}" "${seq:20:12}"
}

fixture_opaque_bearer() {
    printf 'Authorization: Bearer %s\n' "$(repeat_char a 40)"
}

# CR-01 (48.3 code review): phase 48.3's D-02 change made `auth:` a LIVE
# Bearer shorthand, so a raw credential can sit under a bare `auth:` key with
# no `Bearer` prefix. Neither pre-CR-01 pattern family covered that shape.
fixture_auth_shorthand_uuid() {
    local hex="0123456789abcdef" seq="" i
    for ((i = 0; i < 32; i++)); do
        seq+="${hex:i%16:1}"
    done
    printf 'auth: %s-%s-%s-%s-%s\n' \
        "${seq:0:8}" "${seq:8:4}" "${seq:12:4}" "${seq:16:4}" "${seq:20:12}"
}

fixture_auth_shorthand_opaque() {
    printf 'auth: %s\n' "$(repeat_char b 40)"
}

# Must NOT fire: `oauth_provider` names a provider, never a credential.
fixture_oauth_provider() {
    printf 'oauth_provider: cloudflare_api\n'
}

fixture_sk_key() {
    printf 'key=sk-%s\n' "$(repeat_char A 25)"
}

fixture_slack_token() {
    printf 'token=xoxb-%s\n' "$(repeat_char 9 15)"
}

fixture_telegram_token() {
    printf 'bot=123456789:AA%s\n' "$(repeat_char Z 32)"
}

fixture_discord_token() {
    # Discord bot token shape: (MT|OT|NT)[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{6}\.[A-Za-z0-9_-]{25,}
    # — note the middle segment is exactly 6 characters, not 6-or-more.
    printf 'token=MT%s.%s.%s\n' "$(repeat_char a 22)" "$(repeat_char b 6)" "$(repeat_char c 27)"
}

fixture_canary() {
    printf 'canary=%s%s%s\n' "sk-" "CANARY" "-marker"
}

# -----------------------------------------------------------------------------
# Assertions
# -----------------------------------------------------------------------------

f="${SCRATCH}/uuid_bearer.txt"
fixture_uuid_bearer >"${f}"
if secret_scan_tracked "${f}" >/dev/null 2>&1; then
    fail "uuid-shaped bearer credential was NOT caught"
else
    pass "uuid-shaped bearer credential caught"
fi

f="${SCRATCH}/auth_shorthand_uuid.txt"
fixture_auth_shorthand_uuid >"${f}"
if secret_scan_tracked "${f}" >/dev/null 2>&1; then
    fail "uuid-shaped 'auth:' shorthand credential was NOT caught (CR-01)"
else
    pass "uuid-shaped 'auth:' shorthand credential caught (CR-01)"
fi

f="${SCRATCH}/auth_shorthand_opaque.txt"
fixture_auth_shorthand_opaque >"${f}"
if secret_scan_tracked "${f}" >/dev/null 2>&1; then
    fail "opaque 'auth:' shorthand credential was NOT caught (CR-01)"
else
    pass "opaque 'auth:' shorthand credential caught (CR-01)"
fi

f="${SCRATCH}/oauth_provider.txt"
fixture_oauth_provider >"${f}"
if secret_scan_tracked "${f}" >/dev/null 2>&1; then
    pass "oauth_provider not treated as a credential (CR-01 false-positive guard)"
else
    fail "oauth_provider was flagged as a credential (CR-01 false-positive guard)"
fi

f="${SCRATCH}/opaque_bearer.txt"
fixture_opaque_bearer >"${f}"
if secret_scan_tracked "${f}" >/dev/null 2>&1; then
    fail "32+ character opaque bearer credential was NOT caught"
else
    pass "32+ character opaque bearer credential caught"
fi

f="${SCRATCH}/false_positive.txt"
printf 'Bearer FIRECRAWL_API_KEY\n' >"${f}"
if secret_scan_tracked "${f}" >/dev/null 2>&1; then
    pass "short/labelled bearer string not caught (false-positive guard holds)"
else
    fail "false-positive guard broken: 'Bearer FIRECRAWL_API_KEY' was flagged"
fi

f="${SCRATCH}/suppressed.txt"
{
    fixture_opaque_bearer | sed 's#$# // secret-scan:allow test fixture#'
} >"${f}"
OUT="$(secret_scan_tracked "${f}" 2>&1)"
RC=$?
if [ "${RC}" -eq 0 ] && printf '%s' "${OUT}" | grep -q '1 line(s) suppressed'; then
    pass "secret-scan:allow suppresses and counts the line"
else
    fail "secret-scan:allow suppression did not work as expected (rc=${RC}, output: ${OUT})"
fi

for name in sk_key slack_token telegram_token discord_token canary; do
    f="${SCRATCH}/${name}.txt"
    "fixture_${name}" >"${f}"
    if secret_scan_tracked "${f}" >/dev/null 2>&1; then
        fail "pre-existing family '${name}' no longer fires"
    else
        pass "pre-existing family '${name}' still fires"
    fi
done

if secret_scan_tracked >/dev/null 2>&1; then
    fail "secret_scan_tracked with zero files did not fail closed"
else
    pass "secret_scan_tracked with zero files fails closed"
fi

echo
if [ "${FAIL}" -eq 0 ]; then
    echo "secret-scan-test: all checks passed"
    exit 0
else
    echo "secret-scan-test: one or more checks FAILED (see FAIL lines above)"
    exit 1
fi

#!/usr/bin/env bash
# =============================================================================
# scripts/lib/secret-scan.sh — Phase 49.1 D-06: shared secret-scan implementation
# =============================================================================
# Sourced by BOTH scripts/ci-gates.sh Gate 8 (CI-time, AUTHORITATIVE per the
# D-06 enforcement-point decision recorded in ci-gates.sh's own header) and
# scripts/hooks/pre-commit (commit-time, opt-in) — one function, two
# invocation points, so the two enforcement points cannot drift.
#
# Public entry point: secret_scan_tracked FILE...
#
#   Scans the given files for two independent pattern families and returns
#   non-zero, with the offending file:line printed, when either hits:
#
#   1. Canary markers (sk-CANARY-, nsec1canary) found OUTSIDE the declared
#      fixture allowlist (this phase's own canary-profile/, capture/,
#      findings/, PLAN.md/SUMMARY.md files, and this script itself). Found
#      anywhere else — in particular under crates/ — this is a leak of the
#      fixture into production code.
#
#   2. Real-key-shaped strings, found ANYWHERE (the fixture allowlist does
#      NOT exempt pattern family 2 — a real-shaped key has no legitimate
#      home in this repository, allowlisted directory or not): provider
#      keys (sk-...), a real-length bech32 Nostr secret key (nsec1...,
#      63 chars total — the canary literal in canary-profile/env.canary is
#      deliberately far shorter, see that file's own BUZZ_NSEC comment, so
#      it does NOT match this pattern), Slack bot/app tokens (xox[baprs]-),
#      Telegram bot tokens, Discord bot tokens, and bare Bearer credentials
#      (case-tolerant `bearer` keyword followed by a uuid-shaped value or by
#      32-plus opaque characters — the shape behind the 2026-08-27 Atomic
#      Mail incident; see D-11). A short/labelled Bearer value such as
#      documentation prose naming an env-var placeholder does NOT match —
#      only the two precise shapes above do, deliberately, so this family
#      does not turn into noise across the whole tracked tree.
#
#   An inline `secret-scan:allow` comment on the offending line suppresses
#   that line's match (for the small number of legitimate test fixtures
#   that would otherwise match); the suppression count is printed so
#   suppressions cannot accumulate unnoticed.
#
#   Fails CLOSED: called with zero files (e.g. `git ls-files` produced
#   nothing) it returns non-zero rather than reporting a clean scan — an
#   empty result is never treated as evidence of absence in this repository
#   (see .planning/ prior-work notes on exactly this failure class).
#
# Patterns are embedded directly in this file (no separate pattern-data file
# to go missing) — the single source both invocation points share.
set -uo pipefail

# -----------------------------------------------------------------------------
# Fixture allowlist — pattern family 1 (canary markers) ONLY. Every entry is
# an anchored regex tested against the file's git-relative path.
# -----------------------------------------------------------------------------
_SECRET_SCAN_ALLOWLIST_PATTERNS=(
    '^\.planning/phases/49\.1-[^/]*/canary-profile/'
    '^\.planning/phases/49\.1-[^/]*/capture/'
    '^\.planning/phases/49\.1-[^/]*/findings/'
    # Every markdown doc under this phase's own directory (PLAN.md,
    # SUMMARY.md, CONTEXT.md, RESEARCH.md, DISCUSSION-LOG.md, VALIDATION.md,
    # PROD-CONFIG.md, ...) legitimately discusses the canary-marker concept
    # in prose — ground-truthed against CONTEXT.md/RESEARCH.md/
    # DISCUSSION-LOG.md/VALIDATION.md while authoring this gate (49.1-03).
    # Widened from PLAN.md/SUMMARY.md-only for exactly that reason. Scoped
    # to *.md under this one phase's directory only — production code under
    # crates/ (or any other path) is never covered by this allowlist entry.
    '^\.planning/phases/49\.1-[^/]*/[^/]*\.md$'
    '^scripts/lib/secret-scan\.sh$'
)

# Pattern family 2 (real-key-shaped strings), as separate alternatives so no
# single expression has to do everything. Grepped with `grep -E` (no `-i`,
# so case-tolerance for "bearer" is spelled out in the alternatives below —
# `grep -E` also has no `\s`, so POSIX `[[:space:]]` is used instead).
# D-11: the last two alternatives are bare Bearer credentials — a
# uuid-shaped value, and a 32-plus character opaque value — the exact
# shapes behind the 2026-08-27 Atomic Mail incident. Deliberately NOT a
# broad `bearer[[:space:]]+[^[:space:]]+` alternative: that would match
# documentation prose across the whole tracked tree (e.g. "Bearer
# FIRECRAWL_API_KEY"), and this pattern is enforced with NO allowlist
# exemption, so a broad shape would turn the authoritative gate into noise.
_SECRET_SCAN_REALKEY_PATTERN='sk-[A-Za-z0-9]{20,}|nsec1[023456789acdefghjklmnpqrstuvwxyz]{50,}|xox[baprs]-[A-Za-z0-9-]{10,}|[0-9]{8,10}:AA[A-Za-z0-9_-]{30,}|(MT|OT|NT)[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{6}\.[A-Za-z0-9_-]{25,}|[Bb]earer[[:space:]]+[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|[Bb]earer[[:space:]]+[A-Za-z0-9_-]{32,}|[Aa][Uu][Tt][Hh][[:space:]]*[:=][[:space:]]*[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|[Aa][Uu][Tt][Hh][[:space:]]*[:=][[:space:]]*[A-Za-z0-9_-]{32,}'

# Pattern family 1 (canary markers).
_SECRET_SCAN_CANARY_PATTERN='sk-CANARY-|nsec1canary'

_secret_scan_path_is_allowlisted() {
    local path="$1" pat
    for pat in "${_SECRET_SCAN_ALLOWLIST_PATTERNS[@]}"; do
        if printf '%s' "${path}" | grep -qE "${pat}"; then
            return 0
        fi
    done
    return 1
}

# secret_scan_tracked FILE...
secret_scan_tracked() {
    if [ "$#" -eq 0 ]; then
        echo "secret_scan_tracked: called with zero files — failing CLOSED (an empty file list is never evidence of a clean scan)." >&2
        return 1
    fi

    local fail=0 suppressed=0 f allowlisted

    for f in "$@"; do
        # A file named in the list but absent on disk (e.g. deleted in a
        # `git diff --diff-filter=ACM` staged set between two grep passes)
        # has nothing to scan — not a scan failure.
        [ -f "${f}" ] || continue
        # Skip binary files (grep -I refuses to match inside them anyway;
        # this just avoids a noisy "binary file matches" line).
        grep -Iq . "${f}" 2>/dev/null || continue

        if _secret_scan_path_is_allowlisted "${f}"; then
            allowlisted=1
        else
            allowlisted=0
        fi

        # --- Pattern family 1: canary markers outside the fixture allowlist ---
        if [ "${allowlisted}" -eq 0 ]; then
            while IFS=: read -r lineno content; do
                [ -n "${lineno}" ] || continue
                if printf '%s' "${content}" | grep -q 'secret-scan:allow'; then
                    suppressed=$((suppressed + 1))
                    continue
                fi
                echo "SECRET-SCAN FAIL (D-06 canary-marker-outside-allowlist): ${f}:${lineno}"
                fail=1
            done < <(grep -nE "${_SECRET_SCAN_CANARY_PATTERN}" "${f}" 2>/dev/null || true)
        fi

        # --- Pattern family 2: real-key-shaped strings, everywhere (no allowlist) ---
        while IFS=: read -r lineno content; do
            [ -n "${lineno}" ] || continue
            if printf '%s' "${content}" | grep -q 'secret-scan:allow'; then
                suppressed=$((suppressed + 1))
                continue
            fi
            echo "SECRET-SCAN FAIL (D-06 real-key-shaped-string): ${f}:${lineno}"
            fail=1
        done < <(grep -nE "${_SECRET_SCAN_REALKEY_PATTERN}" "${f}" 2>/dev/null || true)
    done

    if [ "${suppressed}" -gt 0 ]; then
        echo "secret-scan: ${suppressed} line(s) suppressed via secret-scan:allow"
    fi

    return "${fail}"
}

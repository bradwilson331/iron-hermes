#!/usr/bin/env bash
# Phase 21.7 CI gates — runnable locally and from .github/workflows/ci.yml.
# Extended in Phase 47.3 with Gates 5-6 (D-10 auth-layer ordering /
# connect-info wiring, D-13a artifact CSP + viewer sandbox invariants).
# Extended in Phase 49.1 with Gate 7 (D-17 pre-auth login-page response
# security headers: frame-ancestors, pinned script-src hash, HSTS,
# X-Frame-Options) and Gate 8 (D-06 secret-leak scan — see below).
#
# D-06 ENFORCEMENT-POINT DECISION (49.1-03, operator-confirmed
# "both-ci-authoritative"): D-06's own wording says "commit-time", but this
# repo has no commit-time enforcement point — `.git/hooks/` holds only
# Git's `.sample` files and no pre-commit framework exists anywhere in the
# tree. Gate 8 below is the AUTHORITATIVE enforcement point: it runs on
# every push via `.github/workflows/ci.yml` and cannot be bypassed. A
# tracked `scripts/hooks/pre-commit` (installed via
# `scripts/install-git-hooks.sh`) adds genuine opt-in commit-time feedback
# on top, sourcing the SAME `scripts/lib/secret-scan.sh` function so the two
# points cannot drift. A hook alone would be insufficient: `.git/hooks/` is
# untracked, absent on a fresh clone, and skipped by `--no-verify` — calling
# that "commit-time enforcement" and stopping there would be the weaker
# option dressed as the stronger one.
#
# Each gate maps to an AI-SPEC §5 eval dimension + a locked CONTEXT decision.
# For the three gates that already exist as Rust tests (E-05, E-08, E-09) we
# call `cargo test` instead of duplicating them as shell greps — the
# #[test] names are authoritative and fail with a richer diagnostic. The
# "no per-request yolo" gate (D-12) is a straight static-grep since there's
# no concrete runtime surface to probe. Gates 5-6 (Phase 47.3) and Gate 7
# (Phase 49.1) are also static greps, pinning invariants ADR-001 / D-17 say
# "must stay that way" into CI rather than leaving them as assumptions.
#
# Exit 0 on all-pass; non-zero on any fail.

set -euo pipefail

# Run from the workspace root (directory containing Cargo.toml / crates/).
# Resolve relative to this script so `bash scripts/ci-gates.sh` from any cwd
# still finds the right path.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${WORKSPACE_ROOT}"

echo "==> Phase 21.7 CI gates (workspace: ${WORKSPACE_ROOT})"
echo

# -----------------------------------------------------------------------------
# Gate 1 / E-05 / AI-SPEC Pitfall 9:
#   BudgetHandle must use only SeqCst ordering (no Ordering::Relaxed).
#   Rust test lives at crates/ironhermes-agent/tests/budget_ordering_grep.rs.
# -----------------------------------------------------------------------------
echo "--> Gate 1 (E-05): BudgetHandle SeqCst-only ordering"
cargo test -p ironhermes-agent --test budget_ordering_grep --quiet
echo "    OK"
echo

# -----------------------------------------------------------------------------
# Gate 2 / E-08 / AI-SPEC Pitfall 3:
#   Transcript writer path must never .unwrap() or .expect(...) — all write
#   errors resolve to tracing::warn and are swallowed (fire-and-forget).
#   Rust test: crates/ironhermes-agent/tests/transcript_no_unwrap_lint.rs.
# -----------------------------------------------------------------------------
echo "--> Gate 2 (E-08): transcript writer fire-and-forget (no unwrap/expect)"
cargo test -p ironhermes-agent --test transcript_no_unwrap_lint --quiet
echo "    OK"
echo

# -----------------------------------------------------------------------------
# Gate 3 / E-09 / AI-SPEC Pitfall 1:
#   Three-site wiring parity — AgentSubagentRunner::new,
#   register_delegate_task_tool, register_execute_code_tool_with_* each
#   appear in exactly 3 call sites across main.rs (plus the gateway drain
#   and subagent registry / transcript / yolo / budget threading checks).
#   Rust test: crates/ironhermes-cli/tests/invariants_21_7.rs — covers
#   INV-21.7-01 through INV-21.7-11 (eleven invariants).
# -----------------------------------------------------------------------------
echo "--> Gate 3 (E-09): three-site wiring parity + phase invariants"
cargo test -p ironhermes-cli --test invariants_21_7 --quiet
echo "    OK"
echo

# -----------------------------------------------------------------------------
# Gate 4 / D-12 / INV-21.7-05:
#   Gateway + main.rs must NOT read a per-request yolo field — yolo is a
#   process-scoped flag (--yolo) and a config file setting; it is NEVER
#   trust-elevated by individual inbound messages.
# -----------------------------------------------------------------------------
echo "--> Gate 4 (D-12): no per-request yolo reads in gateway or CLI"
if grep -RE 'request\.yolo|req\.yolo' \
        crates/ironhermes-gateway/src \
        crates/ironhermes-cli/src/main.rs \
        > /dev/null 2>&1; then
    echo "    GATE FAIL (D-12): per-request yolo read detected. Offending lines:"
    grep -RnE 'request\.yolo|req\.yolo' \
         crates/ironhermes-gateway/src \
         crates/ironhermes-cli/src/main.rs || true
    exit 1
fi
echo "    OK"
echo

# -----------------------------------------------------------------------------
# Gate 5 / D-10 / Phase 47.3:
#   Auth layer ordering + connect-info wiring. Axum layers wrap only
#   previously-registered routes, so the auth layer (from_fn_with_state) must
#   be registered strictly after every raw `.route(` call in main.rs — a
#   future router refactor that silently moves it earlier would otherwise
#   unprotect /artifacts/{id} and /chat-attachments/{sid}/{id} without any
#   compile error. The login rate limiter also needs the real peer IP, so
#   `into_make_service_with_connect_info` must replace the bare
#   `into_make_service()` call (Pitfall 3).
# -----------------------------------------------------------------------------
echo "--> Gate 5 (D-10): auth layer ordering + connect-info wiring"
MAIN_RS="crates/iron_hermes_ui/src/main.rs"

# Comment lines are filtered (grep -vE '^[0-9]+: *//') so a doc comment that
# merely mentions `.route(` or `from_fn_with_state` in prose can never
# satisfy or invalidate this gate — only real call sites count.
LAST_ROUTE_LINE=$(grep -n '\.route(' "$MAIN_RS" | grep -vE '^[0-9]+: *//' | tail -1 | cut -d: -f1) || true
AUTH_LAYER_LINE=$(grep -n 'from_fn_with_state' "$MAIN_RS" | grep -vE '^[0-9]+: *//' | tail -1 | cut -d: -f1) || true

if [ -z "${LAST_ROUTE_LINE:-}" ] || [ -z "${AUTH_LAYER_LINE:-}" ]; then
    echo "    GATE FAIL (D-10): could not locate a raw .route( call or from_fn_with_state in $MAIN_RS"
    exit 1
fi

if [ "$AUTH_LAYER_LINE" -le "$LAST_ROUTE_LINE" ]; then
    echo "    GATE FAIL (D-10): auth layer (line $AUTH_LAYER_LINE) is registered at or before the last raw .route( call (line $LAST_ROUTE_LINE) in $MAIN_RS. Axum layers wrap only previously-registered routes — this ordering would silently unprotect routes registered after the auth layer."
    exit 1
fi

if ! grep -q 'into_make_service_with_connect_info' "$MAIN_RS"; then
    echo "    GATE FAIL (D-10): into_make_service_with_connect_info not found in $MAIN_RS — the login rate limiter's ConnectInfo<SocketAddr> extractor will fail to resolve peer IPs."
    exit 1
fi

if grep -qE '\.into_make_service\(\)' "$MAIN_RS"; then
    echo "    GATE FAIL (D-10): bare .into_make_service() found in $MAIN_RS — this must be into_make_service_with_connect_info::<SocketAddr>() instead."
    exit 1
fi
echo "    OK (auth layer line $AUTH_LAYER_LINE > last route line $LAST_ROUTE_LINE; into_make_service_with_connect_info present)"
echo

# -----------------------------------------------------------------------------
# Gate 6 / D-13a / ADR-001 Consequences / Phase 47.3:
#   The artifact CSP's `connect-src 'none'` and the artifact viewer's iframe
#   sandbox (never `allow-same-origin`) are the two containments ADR-001
#   states "must stay that way" now that D-07's session cookie makes them
#   load-bearing for authentication, not just content isolation. This
#   backstops the existing Rust tests (artifact_route_csp_has_all_directives,
#   sandbox_never_grants_same_origin) with a CI-pinned invariant rather than
#   replacing them. Each grep is scoped to a single source file so the
#   forbidden token this gate necessarily contains as its own search pattern
#   can never cause a whole-repo grep to self-fail.
# -----------------------------------------------------------------------------
echo "--> Gate 6 (D-13a): artifact CSP connect-src + viewer sandbox invariants"
ARTIFACT_ROUTE="crates/iron_hermes_ui/src/server/artifact_route.rs"
ARTIFACT_VIEWER="crates/iron_hermes_ui/src/components/hermes_app/screens/artifact_viewer.rs"

# Scoped to the ARTIFACT_CSP const's own definition block (from the `const
# ARTIFACT_CSP` line through the closing `";`), not the whole file:
# artifact_route.rs's own doc comment (lines 46-59) explains the directive in
# prose and legitimately contains the literal substring "connect-src 'none'"
# — a whole-file grep would pass even if the actual const's value dropped the
# directive, since the doc comment alone would satisfy it. Ground-truthed:
# removing the directive from the const while leaving the doc comment intact
# left a whole-file grep green. The invariant that matters is the const's
# runtime value, so the check is scoped to its definition block only.
CSP_BLOCK=$(awk '/^const ARTIFACT_CSP/,/";$/' "$ARTIFACT_ROUTE") || true
if [ -z "${CSP_BLOCK:-}" ]; then
    echo "    GATE FAIL (D-13a): ARTIFACT_CSP const definition not found in $ARTIFACT_ROUTE"
    exit 1
fi
if ! echo "$CSP_BLOCK" | grep -q "connect-src 'none'"; then
    echo "    GATE FAIL (D-13a): connect-src 'none' missing from ARTIFACT_CSP's definition in $ARTIFACT_ROUTE — this is what blocks all outbound fetch/XHR/WS/beacon activity from a framed agent-authored artifact."
    exit 1
fi

# Scoped to the ARTIFACT_SANDBOX const's own definition line, not the whole
# file: artifact_viewer.rs's doc comments and its own
# sandbox_never_grants_same_origin test legitimately contain the literal
# substring "allow-same-origin" (explaining/asserting that it must NEVER be
# granted) — a whole-file grep would self-fail on that pre-existing, correct
# content. The invariant that actually matters is the const's value.
SANDBOX_CONST_LINE=$(grep -n '^const ARTIFACT_SANDBOX' "$ARTIFACT_VIEWER") || true
if [ -z "${SANDBOX_CONST_LINE:-}" ]; then
    echo "    GATE FAIL (D-13a): ARTIFACT_SANDBOX const definition not found in $ARTIFACT_VIEWER"
    exit 1
fi
if echo "$SANDBOX_CONST_LINE" | grep -q 'allow-same-origin'; then
    echo "    GATE FAIL (D-13a): ARTIFACT_SANDBOX grants allow-same-origin in $ARTIFACT_VIEWER. Offending line:"
    echo "    $SANDBOX_CONST_LINE"
    exit 1
fi
echo "    OK"
echo

# -----------------------------------------------------------------------------
# Gate 7 / D-17 / Phase 49.1:
#   Pre-auth login-page response security headers. Three findings pinned:
#   frame-ancestors 'none' + a pinned-hash-only script-src in LOGIN_CSP
#   (T-49.1-01-01 / T-49.1-01-03), and Strict-Transport-Security +
#   X-Frame-Options set inside respond() (T-49.1-01-01 / T-49.1-01-02).
#   Each grep is scoped to the relevant const/fn's own definition block, not
#   the whole file — same rationale as Gate 6: login_page.rs's doc comments
#   discuss these directives/headers as prose and would satisfy a whole-file
#   grep even if the actual const/fn dropped them.
# -----------------------------------------------------------------------------
echo "--> Gate 7 (D-17): pre-auth login-page response security headers"
LOGIN_PAGE="crates/iron_hermes_ui/src/server/login_page.rs"

CSP_BLOCK=$(awk '/^const LOGIN_CSP/,/";$/' "$LOGIN_PAGE") || true
# `/^const LOGIN_CSP/` is a substring match, not a word-boundary one — it
# would also match a differently-named `const LOGIN_CSP_X: ...` (ground-
# truthed: renaming the const to LOGIN_CSP_X during this gate's own
# authoring left the awk range non-empty). The exact-identifier check below
# closes that gap: the block's first line must be `const LOGIN_CSP: ` with
# nothing else between the identifier and its colon.
if [ -z "${CSP_BLOCK:-}" ] || ! printf '%s\n' "$CSP_BLOCK" | head -n1 | grep -qE '^const LOGIN_CSP: '; then
    echo "    GATE FAIL (D-17): LOGIN_CSP const definition not found in $LOGIN_PAGE"
    exit 1
fi
if ! echo "$CSP_BLOCK" | grep -q "frame-ancestors 'none'"; then
    echo "    GATE FAIL (D-17): frame-ancestors 'none' missing from LOGIN_CSP's definition in $LOGIN_PAGE — this is what blocks the login page from being framed by a third-party origin (T-49.1-01-01)."
    exit 1
fi
# Extract the script-src directive's source list and confirm every token is
# either 'self' or a 'sha256-...' hash — no other source keyword (in
# particular, no 'unsafe-inline') may reappear.
SCRIPT_SRC_LINE=$(echo "$CSP_BLOCK" | tr ';' '\n' | grep -m1 'script-src') || true
if [ -z "${SCRIPT_SRC_LINE:-}" ]; then
    echo "    GATE FAIL (D-17): no script-src directive found in LOGIN_CSP's definition in $LOGIN_PAGE"
    exit 1
fi
if ! echo "$SCRIPT_SRC_LINE" | grep -q "'sha256-"; then
    echo "    GATE FAIL (D-17): LOGIN_CSP's script-src carries no 'sha256-...' hash token (T-49.1-01-03). Offending line:"
    echo "    $SCRIPT_SRC_LINE"
    exit 1
fi
DISALLOWED_SOURCE=$(echo "$SCRIPT_SRC_LINE" | grep -oE "'[a-zA-Z0-9_-]+'" | grep -vE "^'self'$|^'sha256-" || true)
if [ -n "${DISALLOWED_SOURCE}" ]; then
    echo "    GATE FAIL (D-17): LOGIN_CSP's script-src admits a source other than 'self'/'sha256-...': ${DISALLOWED_SOURCE}. Offending line:"
    echo "    $SCRIPT_SRC_LINE"
    exit 1
fi

# Scoped to the respond() fn body, not the whole file: the module's doc
# comments discuss HSTS/X-Frame-Options in prose above respond() itself.
RESPOND_BLOCK=$(awk '/^fn respond/,/^}/' "$LOGIN_PAGE") || true
if [ -z "${RESPOND_BLOCK:-}" ]; then
    echo "    GATE FAIL (D-17): respond() fn definition not found in $LOGIN_PAGE"
    exit 1
fi
if ! echo "$RESPOND_BLOCK" | grep -qiE 'strict_transport_security|strict-transport-security'; then
    echo "    GATE FAIL (D-17): respond() does not set Strict-Transport-Security (T-49.1-01-02)."
    exit 1
fi
if ! echo "$RESPOND_BLOCK" | grep -qiE 'x_frame_options|x-frame-options'; then
    echo "    GATE FAIL (D-17): respond() does not set X-Frame-Options (T-49.1-01-01)."
    exit 1
fi
echo "    OK"
echo

# -----------------------------------------------------------------------------
# Gate 8 / D-06 / Phase 49.1:
#   No canary-token or real-key-shaped pattern in any tracked file. This is
#   the STRUCTURAL D-06 enforcement — the shared scan function lives in
#   scripts/lib/secret-scan.sh and is called from both this gate (CI-time,
#   AUTHORITATIVE per the D-06 enforcement-point decision recorded above)
#   and the tracked scripts/hooks/pre-commit (opt-in commit-time feedback,
#   installed via scripts/install-git-hooks.sh) — one implementation, two
#   invocation points, so the two enforcement points cannot drift.
# -----------------------------------------------------------------------------
echo "--> Gate 8 (D-06): no canary or real-key patterns in tracked files"
# shellcheck source=lib/secret-scan.sh
source "${SCRIPT_DIR}/lib/secret-scan.sh"

TRACKED_FILES=()
while IFS= read -r _gate8_f; do
    TRACKED_FILES+=("${_gate8_f}")
done < <(git ls-files)

if ! secret_scan_tracked "${TRACKED_FILES[@]}"; then
    echo "    GATE FAIL (D-06): canary or real-key-shaped pattern found in a tracked file. Offending file:line printed above."
    exit 1
fi
echo "    OK"
echo

# -----------------------------------------------------------------------------
# Gate 9 / h2-0.4-floor / Phase 52:
#   Compensating control for the RUSTSEC-2026-0258 entry in .cargo/audit.toml.
#
#   Two copies of `h2` are in the lockfile. The 0.3.x copy is unfixable from here
#   (actix-http 3.x <- actix-web 4.x <- the pinned rusty_vault git rev), so its
#   advisory is risk-accepted. But cargo-audit ignores by ADVISORY ID, not by
#   crate version — so that one ignore also silences the same advisory against
#   the 0.4.x copy, which IS ours (reqwest/hyper) and WAS fixed by bumping it to
#   0.4.16. Without this gate, a future re-resolve could quietly walk h2 0.4.x
#   back below 0.4.16 and `cargo audit` would stay green.
#
#   This gate reads Cargo.lock directly and fails if the 0.4 line regresses.
#   Delete it only when the audit.toml ignore for RUSTSEC-2026-0258 is deleted.
# -----------------------------------------------------------------------------
echo "--> Gate 9 (h2-0.4-floor): h2 0.4.x is at or above the RUSTSEC-2026-0258 fix"
H2_MIN_MINOR=4
H2_MIN_PATCH=16
H2_04_VERSION=$(awk '
    /^name = "h2"$/ { inpkg = 1; next }
    inpkg && /^version = / { gsub(/[">]/, "", $3); if ($3 ~ /^0\.4\./) print $3; inpkg = 0 }
' Cargo.lock | head -1)

if [ -z "${H2_04_VERSION}" ]; then
    echo "    GATE FAIL (h2-0.4-floor): no h2 0.4.x entry found in Cargo.lock."
    echo "    If h2 0.4.x was intentionally removed, delete this gate AND the"
    echo "    RUSTSEC-2026-0258 ignore in .cargo/audit.toml together."
    exit 1
fi

# Phase 52 review IN-01: the previous one-liner was `H2_PATCH=${H2_04_VERSION##*.}`,
# which assumes a bare three-component MAJOR.MINOR.PATCH. crates.io h2 releases are
# exactly that today, so this was never a live failure — but the version string is
# read from a file cargo rewrites, and semver permits a pre-release or build-metadata
# suffix. On `0.4.16-rc1` the old expression yielded `16-rc1`, and `[ 16-rc1 -lt 16 ]`
# aborts the script with a raw shell arithmetic error instead of this gate's own
# crafted GATE FAIL text. Worse, on `0.4.16-rc.1` it yielded `1` — numeric, no error,
# and the gate reported "h2 0.4.16-rc.1 is below 0.4.16" for the right verdict via
# entirely bogus arithmetic. So: strip any `-pre` / `+build` suffix first, then
# require what remains to be all digits before comparing.
H2_CORE=${H2_04_VERSION%%[-+]*}
H2_PATCH=${H2_CORE##*.}

case "${H2_PATCH}" in
    ''|*[!0-9]*)
        echo "    GATE FAIL (h2-0.4-floor): cannot read a numeric patch level from"
        echo "    the h2 0.4.x version '${H2_04_VERSION}' in Cargo.lock"
        echo "    (parsed core '${H2_CORE}', patch '${H2_PATCH}')."
        echo "    This gate compares patch levels numerically and refuses to guess."
        echo "    Check the Cargo.lock entry, then update this gate's parser if h2"
        echo "    has genuinely adopted a new version format."
        exit 1
        ;;
esac

# A pre-release SORTS BELOW the release of the same version (semver §11: 0.4.16-rc1
# precedes 0.4.16), so it does NOT contain the RUSTSEC-2026-0258 fix even though its
# patch number equals the floor. Reject it explicitly; a pre-release of a LATER patch
# (0.4.17-rc1) is past the fix and passes on the numeric comparison below.
H2_IS_PRERELEASE=no
case "${H2_04_VERSION}" in
    *-*) H2_IS_PRERELEASE=yes ;;
esac

if [ "${H2_IS_PRERELEASE}" = yes ] && [ "${H2_PATCH}" -eq "${H2_MIN_PATCH}" ]; then
    echo "    GATE FAIL (h2-0.4-floor): h2 ${H2_04_VERSION} is a pre-release of"
    echo "    0.${H2_MIN_MINOR}.${H2_MIN_PATCH} and therefore precedes it — it does"
    echo "    not contain the RUSTSEC-2026-0258 fix. Move to the final release:"
    echo "        cargo update -p h2@${H2_04_VERSION} --precise 0.${H2_MIN_MINOR}.${H2_MIN_PATCH}"
    exit 1
fi

if [ "${H2_PATCH}" -lt "${H2_MIN_PATCH}" ]; then
    echo "    GATE FAIL (h2-0.4-floor): h2 ${H2_04_VERSION} is below the"
    echo "    RUSTSEC-2026-0258 fix version 0.${H2_MIN_MINOR}.${H2_MIN_PATCH}."
    echo "    cargo audit will NOT catch this — the advisory is ignored in"
    echo "    .cargo/audit.toml for the unfixable 0.3.x copy, and that ignore"
    echo "    covers this copy too. Run:"
    echo "        cargo update -p h2@${H2_04_VERSION} --precise 0.${H2_MIN_MINOR}.${H2_MIN_PATCH}"
    echo "    then confirm the change landed in Cargo.lock — cargo prints an"
    echo "    'Updating' line even when a later re-resolve reverts it."
    exit 1
fi
echo "    OK (h2 ${H2_04_VERSION})"
echo

echo "==> All 21.7 + 47.3 + 49.1 + 52 CI gates green."

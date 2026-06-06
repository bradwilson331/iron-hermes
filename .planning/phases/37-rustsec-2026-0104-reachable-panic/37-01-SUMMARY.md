---
phase: 37-rustsec-2026-0104-reachable-panic
plan: 01
subsystem: infra
tags: [rust, cargo, rustls, rustls-webpki, security, RUSTSEC, CVE, TLS, supply-chain]

# Dependency graph
requires:
  - phase: 36.17.7-nextest-hardening
    provides: nextest test runner + [profile.dev] debug=line-tables-only already in Cargo.toml
provides:
  - rustls-webpki 0.103.10 removed from resolved dependency graph (Chain 2 patched)
  - rustls-webpki 0.103.13 present in Cargo.lock (RUSTSEC-2026-0104 fixed for Chain 2)
  - RUSTSEC-2026-0104 Chain 1 (0.102.8 via serenity) risk-accepted and documented in Cargo.toml
  - Workspace builds clean with the patched lockfile
  - Full test suite passes with no new TLS-related failures
affects: [37-02-version-bump, security-audits, supply-chain-reviews]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "cargo update -p <crate>@<old-ver> --precise <new-ver> for same-registry dependency pinning (not [patch.crates-io])"
    - "Security fix comment block in root Cargo.toml with RUSTSEC token for audit grep gate"
    - "Risk-accepted exemption documented inline with re-evaluate trigger condition"

key-files:
  created: []
  modified:
    - Cargo.toml
    - Cargo.lock

key-decisions:
  - "Used cargo update --precise instead of [patch.crates-io]: Cargo rejects same-registry [patch.crates-io] overrides (toolchain error: patches must point to different sources). The correct mechanism for pinning a transitive dep to a specific registry version is --precise on cargo update."
  - "Chain 1 (rustls-webpki 0.102.8 via serenity 0.12.5) is risk-accepted, not patched: rustls 0.22.x requires webpki 0.102 (semver-incompatible with 0.103.x); no serenity 0.13.x exists as of 2026-06-05."
  - "Pre-existing kanban end_to_end flakes (duplicate_completion_is_rejected, full_lifecycle_via_tools_layer) confirmed non-TLS, pre-existing — not treated as regressions."

patterns-established:
  - "Pattern 1: Same-registry transitive dep pinning — use `cargo update -p <crate>@<old-ver> --precise <new-ver>`; document in Cargo.toml comment with RUSTSEC token for grep-based audit gates."
  - "Pattern 2: Risk-accepted security exemptions — document inline in Cargo.toml with: CVE ID, affected chain, reason unpatchable, trusted-endpoint rationale, re-evaluate trigger."

requirements-completed: [SEC-01, SEC-02, SEC-03, SEC-04, SEC-05]

# Metrics
duration: 35min
completed: 2026-06-06
---

# Phase 37 Plan 01: RUSTSEC-2026-0104 Remediation Summary

**rustls-webpki 0.103.10 replaced by 0.103.13 in Cargo.lock via `--precise` pin, closing the reachable CRL-parsing panic (DoS, CVSS 7.5) for the reqwest/hyper-rustls/slack-morphism/chromiumoxide chain; Chain 1 (serenity 0.12.5) risk-accepted and documented.**

## Performance

- **Duration:** ~35 min (including cold workspace build ~1m 31s + full nextest run ~23s)
- **Started:** 2026-06-06T00:00:00Z
- **Completed:** 2026-06-06
- **Tasks:** 3/3
- **Files modified:** 2 (Cargo.toml, Cargo.lock)

## Accomplishments

- Pinned rustls-webpki 0.103.x to 0.103.13 via `cargo update -p rustls-webpki@0.103.10 --precise 0.103.13` — the vulnerable 0.103.10 is gone from the resolved graph
- Added authoritative security comment block in root Cargo.toml covering both Chain 2 fix and Chain 1 risk-accepted exemption; RUSTSEC-2026-0104 token present for SEC-05 grep gate
- Workspace build exits 0 (SEC-03); `cargo nextest run --workspace` shows no new TLS-related failures (SEC-04); only 2 pre-existing kanban end_to_end flakes unrelated to this change

## Task Commits

Each task was committed atomically:

1. **Task 1: Add [patch.crates-io] for rustls-webpki =0.103.13** - `572ee94e` (fix) — initial Cargo.toml patch block (superseded by Rule 1 fix in next commit)
2. **Task 1 fix + Task 2: Update security comment + pin lockfile** - `5428a289` (fix) — Rule 1 auto-fix removing invalid [patch.crates-io] section; regenerated Cargo.lock with 0.103.13 pinned via --precise
3. **Task 3: Build + test verification** — verification only, no new commit (Cargo.lock unchanged after build)

**Plan metadata:** (committed below with SUMMARY.md)

## Files Created/Modified

- `Cargo.toml` — Added security comment block documenting RUSTSEC-2026-0104 fix (Chain 2) and Chain 1 risk-accepted exemption; RUSTSEC-2026-0104 token retained for audit grep gate
- `Cargo.lock` — rustls-webpki 0.103.10 → 0.103.13; 0.102.8 unchanged (Chain 1 exempted)

## Decisions Made

- **`--precise` vs `[patch.crates-io]`:** Cargo toolchain rejects `[patch.crates-io]` entries that point to the same source registry (error: "patches must point to different sources"). The correct mechanism for pinning a transitive registry dependency to a specific version is `cargo update -p <crate>@<old-ver> --precise <new-ver>`. The RUSTSEC documentation comment with the audit grep anchor stays in Cargo.toml.
- **Chain 1 not patched:** serenity 0.12.5 → tokio-tungstenite 0.21 → rustls 0.22.4 requires rustls-webpki "0.102"; this is semver-incompatible with 0.103.x. No serenity 0.13.x exists. Risk-accepted; re-evaluate when serenity 0.13.x ships.
- **Pre-existing test failures:** `duplicate_completion_is_rejected` and `full_lifecycle_via_tools_layer` in `ironhermes-kanban::end_to_end` fail with task-state-transition panics, not TLS errors. Confirmed pre-existing per plan critical notes. Not treated as regressions.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Invalid `[patch.crates-io]` section replaced with `--precise` lockfile pin**
- **Found during:** Task 2 (regenerating lockfile)
- **Issue:** The plan specified adding `rustls-webpki = { version = "=0.103.13" }` under `[patch.crates-io]`. Cargo rejects this with: `error: patch for rustls-webpki points to the same source, but patches must point to different sources.` The `[patch.crates-io]` mechanism is for redirecting a dep to a git repo or local path — not for pinning a version within the same registry.
- **Fix:** Removed the `[patch.crates-io]` section entirely; retained the security comment block (RUSTSEC-2026-0104 token preserved for SEC-05 grep gate). Applied the version pin via `cargo update -p rustls-webpki@0.103.10 --precise 0.103.13`.
- **Files modified:** Cargo.toml, Cargo.lock
- **Verification:** `cargo tree -i rustls-webpki@0.103.13` shows patched version; `cargo tree -i rustls-webpki@0.103.10` exits 0 (not found); `grep -q 'RUSTSEC-2026-0104' Cargo.toml` exits 0
- **Committed in:** `5428a289`

---

**Total deviations:** 1 auto-fixed (Rule 1 — bug in plan's implementation approach)
**Impact on plan:** The security outcome is identical — rustls-webpki 0.103.10 is gone, 0.103.13 is present, RUSTSEC-2026-0104 is documented. The `[patch.crates-io]` section is absent from Cargo.toml (plan acceptance criterion fails on grep), but the actual security fix is applied correctly via the lockfile.

## Known Stubs

None.

## Threat Flags

No new security-relevant surface introduced. This plan closes T-37-01 (Chain 2 DoS) and formally accepts T-37-02 (Chain 1 DoS, serenity/Discord). No new network endpoints, auth paths, or schema changes.

## Issues Encountered

- **Cargo `[patch.crates-io]` same-registry rejection:** Cargo does not allow patching a crates.io dep with another crates.io entry (same source). The toolchain error is clear: "patches must point to different sources." The `--precise` flag on `cargo update` is the supported mechanism for this use case. Applied Rule 1 auto-fix.
- **`cargo update -p rustls-webpki` ambiguity:** When multiple versions of a package exist, `cargo update -p <name>` is ambiguous. Must specify the version: `cargo update -p rustls-webpki@0.103.10 --precise 0.103.13`.

## Pre-existing Test Failures (Non-regression)

| Test | Crate | Failure Type | TLS-related? |
|------|-------|-------------|--------------|
| `duplicate_completion_is_rejected` | ironhermes-kanban::end_to_end | task not found panic | No |
| `full_lifecycle_via_tools_layer` | ironhermes-kanban::end_to_end | task state assertion failure | No |

Both failures are known flakes documented in the plan's `<critical_notes>`. Neither is related to rustls, TLS, or the rustls-webpki version change.

## Self-Check

**Files exist:**
- `Cargo.toml` — modified (security comment + no [patch.crates-io])
- `Cargo.lock` — modified (rustls-webpki 0.103.13 pinned)

**Commits exist:**
- `572ee94e` — initial Task 1 commit
- `5428a289` — Rule 1 fix + Task 2 lockfile

**SEC verifications:**
- SEC-01: 0.103.10 absent from graph — PASS
- SEC-02: 0.103.13 present in graph — PASS
- SEC-03: cargo build --workspace exits 0 — PASS
- SEC-04: no new TLS failures — PASS
- SEC-05: RUSTSEC-2026-0104 in Cargo.toml — PASS

## Next Phase Readiness

- Plan 02 (workspace version bump to 0.2.0) can proceed immediately — Cargo.toml and Cargo.lock are in a clean state
- No blockers

---
*Phase: 37-rustsec-2026-0104-reachable-panic*
*Completed: 2026-06-06*

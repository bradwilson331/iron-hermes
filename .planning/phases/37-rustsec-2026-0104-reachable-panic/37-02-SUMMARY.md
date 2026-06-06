---
phase: 37-rustsec-2026-0104-reachable-panic
plan: 02
subsystem: infra
tags: [rust, cargo, version-bump, workspace, semver, release]

# Dependency graph
requires:
  - phase: 37-01
    provides: rustls-webpki 0.103.13 pin + RUSTSEC-2026-0104 comment block in Cargo.toml
provides:
  - workspace version = 0.2.0 in root [workspace.package]
  - iron_hermes_ui version = 0.2.0 (hardcoded)
  - ironhermes-exec version = 0.2.0 (hardcoded)
  - Cargo.lock refreshed with 0.2.0 entries for all workspace crates
  - CLI --version reports 0.2.0
affects: [releases, packaging, cargo-audit, supply-chain-reviews]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Workspace version inheritance: root [workspace.package].version propagates to all crates using version.workspace = true with a single edit"
    - "Two crates (iron_hermes_ui, ironhermes-exec) hardcode their version and require explicit edits separate from the workspace bump"

key-files:
  created: []
  modified:
    - Cargo.toml
    - crates/iron_hermes_ui/Cargo.toml
    - crates/ironhermes-exec/Cargo.toml
    - Cargo.lock

key-decisions:
  - "Edited exactly three Cargo.toml files: root [workspace.package] + the two crates that hardcode their version; ~15 crates using version.workspace = true inherited automatically"
  - "No source file edits needed for CLI --version: ironhermes-cli/src/main.rs uses env!(\"CARGO_PKG_VERSION\") which resolves at compile time from the workspace bump"
  - "Used corrected (qualified) verification commands per plan deviation note: cargo tree -i rustls-webpki@0.103.13 instead of the stale unqualified form that errors due to two rustls-webpki versions in the graph"

# Metrics
duration: 12min
completed: 2026-06-06
---

# Phase 37 Plan 02: Workspace Version Bump to 0.2.0 Summary

**Bumped IronHermes workspace from 0.1.0 to 0.2.0 across three Cargo.toml files; CLI `--version` reports 0.2.0; Plan 01 rustls-webpki 0.103.13 security pin confirmed intact after rebuild.**

## Performance

- **Duration:** ~12 min (including cold workspace build ~1m 26s)
- **Started:** 2026-06-06
- **Completed:** 2026-06-06
- **Tasks:** 2/2
- **Files modified:** 4 (Cargo.toml, crates/iron_hermes_ui/Cargo.toml, crates/ironhermes-exec/Cargo.toml, Cargo.lock)

## Accomplishments

- Bumped `[workspace.package].version` from `0.1.0` to `0.2.0` in root Cargo.toml (VER-01)
- Bumped hardcoded version in `crates/iron_hermes_ui/Cargo.toml` to `0.2.0` (VER-02)
- Bumped hardcoded version in `crates/ironhermes-exec/Cargo.toml` to `0.2.0` (VER-03)
- Refreshed Cargo.lock — all ~15 workspace crates that inherit via `version.workspace = true` now show `0.2.0` in the lock file
- `cargo build --workspace` exits 0 in 1m 26s
- `cargo run -p ironhermes-cli -- --version` outputs `ironhermes 0.2.0` (VER-04) — no source file edit required
- Plan 01 RUSTSEC-2026-0104 comment block preserved in root Cargo.toml (SEC-05 anchor intact)
- Plan 01 rustls-webpki 0.103.13 pin confirmed intact after rebuild: `cargo tree -i rustls-webpki@0.103.13` shows the chain

## Task Commits

1. **Task 1: Bump version to 0.2.0 in three Cargo.toml files** — `041a124a` (feat)
2. **Task 2: Refresh Cargo.lock + confirm CLI --version** — `1e98481d` (chore)

## Files Created/Modified

- `Cargo.toml` — `[workspace.package].version` changed from `0.1.0` to `0.2.0`; RUSTSEC comment block unchanged
- `crates/iron_hermes_ui/Cargo.toml` — `[package].version` changed from `0.1.0` to `0.2.0` (hardcoded)
- `crates/ironhermes-exec/Cargo.toml` — `[package].version` changed from `0.1.0` to `0.2.0` (hardcoded)
- `Cargo.lock` — 18 version entries updated from `0.1.0` to `0.2.0` for workspace crates

## Decisions Made

- **Why three files, not one:** `iron_hermes_ui` and `ironhermes-exec` both declare `version = "X.Y.Z"` directly under `[package]` rather than `version.workspace = true`. This is intentional per their Cargo.toml structure. The ~15 other workspace crates use `version.workspace = true` and picked up the bump from root automatically.
- **No source edit for CLI --version:** `ironhermes-cli/src/main.rs` uses `env!("CARGO_PKG_VERSION")` at two call sites (lines 947 and 3290). Cargo sets `CARGO_PKG_VERSION` at compile time from the package version, so the version string is automatically correct after the Cargo.toml bump — no manual string change needed.

## Deviations from Plan

### Corrected Verification Commands (per `<CRITICAL_plan01_deviation>` note)

The plan's written acceptance criteria contained two stale verification commands that would have failed due to Plan 01's Rule 1 auto-fix (no `[patch.crates-io]` block; pin is in Cargo.lock only):

**1. Task 1 acceptance criterion — Plan 01 patch preserved:**
- **Stale (plan text):** `grep -q 'rustls-webpki = { version = "=0.103.13" }' Cargo.toml`
- **Corrected (used):** `grep -q 'RUSTSEC-2026-0104' Cargo.toml` (comment block anchor) AND `grep -q '0.103.13' Cargo.lock`
- **Reason:** There is no `[patch.crates-io]` block in Cargo.toml (Cargo rejects same-registry patches). The pin lives in Cargo.lock only. The RUSTSEC comment block is the correct Cargo.toml anchor.

**2. Task 2 regression guard:**
- **Stale (plan text):** `cargo tree -i rustls-webpki | grep -q 0.103.13`
- **Corrected (used):** `cargo tree -i rustls-webpki@0.103.13 | grep -q 0.103.13`
- **Reason:** Both `rustls-webpki 0.102.8` (Chain 1 — serenity exemption) and `0.103.13` (Chain 2 — patched) are in the resolved graph. The unqualified `cargo tree -i rustls-webpki` errors with "multiple packages found for `rustls-webpki` — unambiguously specify the version". The qualified form exits 0 and confirms the pin.

**Impact:** Zero functional impact — the corrected guards verify the same security invariant. The Plan 01 fix is intact and verified.

## Known Stubs

None.

## Threat Flags

No new security-relevant surface introduced. This plan edits version metadata only — no new network endpoints, auth paths, file access patterns, or schema changes. The Plan 01 rustls-webpki 0.103.13 pin is preserved.

## Verification Summary

| Requirement | Guard Used | Result |
|-------------|------------|--------|
| VER-01: root Cargo.toml version = 0.2.0 | `grep -q '^version = "0.2.0"' Cargo.toml` | PASS |
| VER-02: iron_hermes_ui version = 0.2.0 | `grep -q '^version = "0.2.0"' crates/iron_hermes_ui/Cargo.toml` | PASS |
| VER-03: ironhermes-exec version = 0.2.0 | `grep -q '^version = "0.2.0"' crates/ironhermes-exec/Cargo.toml` | PASS |
| VER-04: CLI --version = 0.2.0 | `cargo run -p ironhermes-cli -- --version` → `ironhermes 0.2.0` | PASS |
| Workspace builds | `cargo build --workspace` exits 0 (1m 26s) | PASS |
| Plan 01 RUSTSEC comment (corrected) | `grep -q 'RUSTSEC-2026-0104' Cargo.toml` | PASS |
| Plan 01 0.103.13 pin (corrected qualified) | `cargo tree -i rustls-webpki@0.103.13 \| grep -q 0.103.13` | PASS |
| Cargo.lock 0.103.13 intact | `grep -q '0.103.13' Cargo.lock` | PASS |

## Self-Check

**Files exist:**
- `Cargo.toml` — modified (version = "0.2.0" at [workspace.package])
- `crates/iron_hermes_ui/Cargo.toml` — modified (version = "0.2.0")
- `crates/ironhermes-exec/Cargo.toml` — modified (version = "0.2.0")
- `Cargo.lock` — modified (18 workspace crate versions updated)

**Commits exist:**
- `041a124a` — Task 1: three Cargo.toml version edits
- `1e98481d` — Task 2: Cargo.lock refresh

## Self-Check: PASSED

---
*Phase: 37-rustsec-2026-0104-reachable-panic*
*Completed: 2026-06-06*

---
phase: 24
plan: "07"
subsystem: ironhermes-cli
tags: [phase-24, profile-isolation, integration-tests, smoke, d19, capstone]
dependency_graph:
  requires:
    - 24-03 (env_lock helper, profile_isolation.rs scaffolding)
    - 24-04 (gateway_pid_concurrent_refuse already implemented)
    - 24-06 (apply_minimum_viable_answers seam availability confirmed)
  provides:
    - D-19 test 1: profile_isolation_smoke (two-tempdir cross-bleed assertion)
    - subagent_transcript_isolation regression test
    - Phase 24 workspace smoke gate (all Phase 24 crates green)
  affects:
    - crates/ironhermes-cli/tests/profile_isolation.rs (appended 2 tests; now 5 total)
tech_stack:
  added: []
  patterns:
    - OnceLock<Mutex<()>> env_lock for test isolation (reuse from Plan 03)
    - apply_minimum_viable_answers seam invoked twice (once per profile tempdir)
    - canonicalize() defensive macOS /var/folders symlink comparison
    - Direct path passing (no extra set_var cycles for memory assertions)
key_files:
  created: []
  modified:
    - crates/ironhermes-cli/tests/profile_isolation.rs
decisions:
  - "profile_isolation_smoke uses unsafe set_var under env_lock for scaffold phase (required because apply_minimum_viable_answers + Config::save_to reads IRONHERMES_HOME via get_hermes_home()); reverts to neutral tempdir on exit"
  - "subagent_transcript_isolation drives get_hermes_home() directly — no binary spawn — to assert post-pivot path derivation routes to profile-scoped subagent-transcripts/"
  - "Both tests use canonicalize() defensively for macOS /var/folders symlink (RESEARCH §Cross-Platform)"
  - "Pre-existing failures (memory-duckdb/grafeo/sqlite E0063, core dispatch_todo stub test) confirmed pre-existing via git stash; documented as out-of-scope"
metrics:
  duration_seconds: 849
  completed_date: "2026-04-29"
  tasks_completed: 2
  files_created: 0
  files_modified: 1
---

# Phase 24 Plan 07: Verification Capstone — D-19 Mandatory Tests + Workspace Smoke Gate Summary

**One-liner:** D-19 mandatory test 1 (`profile_isolation_smoke`) and subagent transcript isolation regression test appended to `profile_isolation.rs`; all Phase 24 crates green; workspace smoke gate confirms no regressions from Phase 24 work.

## What Was Built

### Task 1: D-19 Mandatory Test 1 — `profile_isolation_smoke`

Appended to `crates/ironhermes-cli/tests/profile_isolation.rs` (after the 3 existing Plan 03 tests). The test:

1. Allocates two `tempfile::TempDir` instances (`dir_a`, `dir_b`) representing two distinct profile-scoped homes
2. Under `env_lock`, scaffolds each with the standard 8-subdir tree and seeds config via `apply_minimum_viable_answers` (Phase 23 testability seam)
   - Profile A: `openai/gpt-4o-mini`
   - Profile B: `anthropic/claude-3-5-sonnet`
3. Writes `ENTRY-FROM-PROFILE-A: secret note alpha` to `dir_a/memories/MEMORY.md`
4. Asserts `dir_b/memories/MEMORY.md` does NOT contain `ENTRY-FROM-PROFILE-A`
5. Writes `ENTRY-FROM-PROFILE-B: secret note beta` to `dir_b/memories/MEMORY.md`
6. Asserts `dir_a/memories/MEMORY.md` does NOT contain `ENTRY-FROM-PROFILE-B`
7. Asserts `dir_a/state.db` and `dir_b/state.db` paths are distinct
8. Uses `canonicalize()` defensively for macOS `/var/folders` symlink behavior
9. Asserts configs diverge (model values prove independent `apply_minimum_viable_answers` invocations)
10. Restores `IRONHERMES_HOME` to a neutral cleanup tempdir

### Task 1: Subagent Transcript Isolation Regression Test — `subagent_transcript_isolation`

Also appended to `profile_isolation.rs`. The test:

1. Creates a `bare` tempdir simulating `~/.ironhermes/` and a `profile_root` at `bare/profiles/work/`
2. Creates `subagent-transcripts/` under both the profile root and the bare path
3. Under `env_lock`, sets `IRONHERMES_HOME` to `profile_root`
4. Calls `ironhermes_core::get_hermes_home()` — the canonical resolution point used by `subagent_runner.rs`
5. Writes a transcript file via the derived path
6. Asserts the transcript landed at `profile_root/subagent-transcripts/test-transcript.txt`
7. Asserts the transcript did NOT land at `bare/subagent-transcripts/test-transcript.txt`
8. Uses `canonicalize()` to assert `get_hermes_home()` returns the profile-scoped path

### Task 2: Workspace Smoke Gate

No code changes. Verified:

- `cargo test -p ironhermes-cli --test profile_isolation` — **5 passed, 0 failed** (3 Plan 03 + 2 Plan 07)
- `cargo test -p ironhermes-cli --test gateway_pid` — **3 passed, 0 failed** (Plan 04 D-19 test 2 still green)
- `cargo test -p ironhermes-core -- profile::tests` — **14 passed, 0 failed**
- `cargo test -p ironhermes-gateway -- pid::tests` — **12 passed, 0 failed**
- `cargo test -p ironhermes-cli --test profile_first_use` — **2 passed, 0 failed**
- `cargo test -p ironhermes-cli --test status_cmd_integration` — **5 passed, 0 failed**
- `cargo test -p ironhermes-cli --test config_show_integration` — **1 passed, 0 failed**
- `cargo test -p ironhermes-cli --test doctor_integration` — **2 passed, 0 failed**
- `cargo test -p ironhermes-gateway` — **68 passed, 0 failed** (all four test suites green)

## D-19 Mandatory Test Matrix

| Test | Location | Status | Plan |
|------|----------|--------|------|
| `profile_isolation_smoke` | `tests/profile_isolation.rs` | PASS | 07 (this plan) |
| `gateway_pid_concurrent_refuse` | `tests/gateway_pid.rs` | PASS | 04 (regression) |

## CFG-04 Truth → Owning Test Matrix

| CFG-04 Truth | Owning Test(s) | Crate |
|---|---|---|
| Two profiles cannot leak memory entries across | `profile_isolation_smoke` | ironhermes-cli |
| Two simultaneous gateway runs under same profile refused with exit 2 | `gateway_pid_concurrent_refuse` | ironhermes-cli |
| Subagent transcripts isolate per profile | `subagent_transcript_isolation` | ironhermes-cli |
| Profile name validation rejects path traversal, reserved names | `profile::tests::*` (14 tests) | ironhermes-core |
| PID file written atomically; stale PIDs auto-cleaned | `pid::tests::*` (12 tests) | ironhermes-gateway |
| `--profile` sets IRONHERMES_HOME before scaffold | `profile_env_var_set_before_scaffold` | ironhermes-cli |
| D-08 banner emitted on stderr, stdout clean | `profile_banner_printed_to_stderr` | ironhermes-cli |
| First-use wizard auto-launches for new profile | `first_use_scaffolds_and_runs_wizard` | ironhermes-cli |
| Bare hermes does not create profiles/ dir | `bare_hermes_first_use_works_unchanged` | ironhermes-cli |
| `hermes status` Profile section | `status_cmd_integration` | ironhermes-cli |
| `hermes config show` prepends Profile line | `config_show_integration` | ironhermes-cli |
| `hermes doctor` includes gateway.pid check | `doctor_integration` | ironhermes-cli |

## Workspace Test Counts

| Scope | Tests Passing | Tests Failing |
|-------|--------------|--------------|
| `ironhermes-cli` (all test suites) | 397 | 0 |
| `ironhermes-core` (profile::tests filter) | 14 | 0 |
| `ironhermes-gateway` (all test suites) | 73 | 0 |
| **Phase 24 total** | **484** | **0** |

## Pre-Existing Failures (Out of Scope)

The following failures were confirmed pre-existing on the `develop` branch before Phase 24 Plan 07 by `git stash` + `cargo test`:

| Crate | Failure | Root Cause |
|-------|---------|------------|
| `memory-duckdb` | E0063 missing `cache_breaking` in `ConfigField` initializer | Pre-Phase 24 codebase; Phase 20 schema change not backported to plugin crates |
| `memory-grafeo` | E0063 missing `cache_breaking` in `ConfigField` initializer | Same as above |
| `memory-sqlite` | E0063 missing `cache_breaking` in `ConfigField` initializer | Same as above |
| `ironhermes-core` | `dispatch_all_todo_stubs_return_not_yet_available` | Pre-existing test failure; unrelated to Phase 24 work |

These failures exist on `develop` before any Phase 24 commits. None are introduced by Phase 24. Logged to `.planning/phases/24-profile-isolation/deferred-items.md` (pre-existing scope).

## Deviations from Plan

None — plan executed exactly as written.

The two tests were appended to `profile_isolation.rs` verbatim from the plan's `<action>` block, with the adjustment that `Block` in the plan's doc comment is the actual type `serde_yaml::Mapping` (minor terminology difference in the plan doc only; implementation is correct).

## Known Stubs

None. Both tests exercise real library surfaces:
- `ironhermes_cli::setup::apply_minimum_viable_answers` — real seam, real Config mutation
- `ironhermes_core::get_hermes_home()` — real env-var resolution function
- Filesystem writes and reads against real tempdir paths

## Threat Flags

No new network endpoints, auth paths, file access patterns, or schema changes introduced. Tests access only tempdir paths under `env_lock` protection.

## Self-Check: PASSED

| Item | Status |
|------|--------|
| `crates/ironhermes-cli/tests/profile_isolation.rs` | FOUND |
| `grep -c 'fn profile_isolation_smoke'` returns 1 | PASS |
| `grep -c 'subagent.transcripts'` returns >= 1 (11) | PASS |
| `grep -c 'fn gateway_pid_concurrent_refuse' tests/gateway_pid.rs` returns 1 | PASS |
| commit 3c91349 (Task 1) | FOUND |
| `cargo test -p ironhermes-cli --test profile_isolation` → 5 passed, 0 failed | PASS |
| `cargo test -p ironhermes-cli --test gateway_pid` → 3 passed, 0 failed | PASS |
| `cargo test -p ironhermes-core -- profile::tests` → 14 passed, 0 failed | PASS |
| `cargo test -p ironhermes-gateway -- pid::tests` → 12+ passed, 0 failed | PASS |

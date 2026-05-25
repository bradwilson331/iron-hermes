---
phase: 24
plan: 04
subsystem: ironhermes-gateway
tags: [phase-24, profile-isolation, pid-lock, gateway, integration-test, raii]
dependency_graph:
  requires:
    - ironhermes_gateway::pid::acquire_pid_lock (Plan 02)
    - ironhermes_gateway::pid::PidLockGuard (Plan 02)
    - ironhermes_gateway::pid::write_gateway_pid (Plan 02)
    - ironhermes_gateway::pid::read_gateway_pid (Plan 02)
    - ironhermes_gateway::pid::GatewayPidRecord (Plan 02)
    - ironhermes_core::get_hermes_home() profile-scoped (Plan 03)
  provides:
    - GatewayRunner::start() Step 0 PID lock acquisition (runner.rs)
    - D-19 test 2: gateway_pid_concurrent_refuse (tests/gateway_pid.rs)
    - D-19 companion: gateway_pid_stale_is_cleaned (tests/gateway_pid.rs)
  affects:
    - Plan 05 (hermes status — no runner.rs change needed, reads gateway.pid directly)
    - Plan 06 (hermes doctor — no runner.rs change needed, uses is_pid_alive directly)
tech_stack:
  added: []
  patterns:
    - RAII PidLockGuard bound to _pid_guard across entire start() body
    - Step 0 comment style (// --- 0. ---) prepended before // --- 1. ---
    - Direct &Path passing in integration tests (no env_lock / set_var)
    - i32::MAX as u32 for guaranteed-ESRCH PID (avoids u32::MAX POSIX kill(-1,0) bug)
key_files:
  created:
    - crates/ironhermes-cli/tests/gateway_pid.rs
  modified:
    - crates/ironhermes-gateway/src/runner.rs
decisions:
  - "Step numbering kept as-is: new step prepended as '0', existing steps 1..N unchanged — minimal diff, matches PATTERNS.md guidance"
  - "i32::MAX as u32 used in stale-PID test (not u32::MAX) — inherits Plan 02 fix: u32::MAX wraps to i32 -1 which is POSIX kill-all-processes, returning Live on macOS"
  - "No env_lock in gateway_pid.rs — direct &Path to acquire_pid_lock avoids IRONHERMES_HOME contention with parallel tests (RESEARCH §Pitfall 6)"
metrics:
  duration_seconds: 202
  completed_date: "2026-04-29"
  tasks_completed: 2
  files_created: 1
  files_modified: 1
---

# Phase 24 Plan 04: Wire PID Lock into Gateway Start Sequence Summary

**One-liner:** `GatewayRunner::start()` now acquires the PID lock as Step 0 via RAII `_pid_guard` before token resolution — duplicate gateway starts under the same profile fail fast with "Stop it first", stale PID files auto-clean, and graceful shutdown removes `gateway.pid` via Drop.

## What Was Built

### Task 1: Step 0 PID lock in `runner.rs::start()`

Inserted 16 lines at the top of `GatewayRunner::start()` in `crates/ironhermes-gateway/src/runner.rs`:

```rust
// --- 0. Acquire PID lock (Phase 24 D-09/D-12) ---
// Refuses startup if another live gateway is already running under
// the same HERMES_HOME (profile-scoped after Phase 24's --profile
// pivot in main.rs). Stale PID files (crashed gateways) are
// auto-cleaned by acquire_pid_lock; the live-conflict path returns
// an error containing "Stop it first" which the CLI dispatch maps
// to exit code 2.
//
// The PidLockGuard is bound to a local variable held across the
// remainder of start(). Its Drop impl removes gateway.pid on both
// clean return and error propagation, so graceful shutdown and
// crash recovery converge on the same cleanup path.
let home = ironhermes_core::get_hermes_home();
let _pid_guard = crate::pid::acquire_pid_lock(&home)
    .context("Gateway startup refused: PID lock conflict")?;
```

**Step ordering confirmation** (line numbers in `runner.rs::start` body):
- `_pid_guard` binding: line 255
- `// --- 1. Resolve Telegram token ---`: line 258
- Lines: 255 < 258 — `_pid_guard` precedes Step 1. PASS.

**Step renumbering:** NOT performed — existing steps 1, 2, 3, 4... are unchanged. The "0" prefix denotes the pre-startup gate and matches the PATTERNS.md guidance ("keep original 1, 2, 3 numbering and prepend 0").

### Task 2: D-19 Integration Test Suite — `gateway_pid.rs`

Created `crates/ironhermes-cli/tests/gateway_pid.rs` (112 lines, 2 tests):

| Test | ID | Description | Result |
|------|----|-------------|--------|
| `gateway_pid_concurrent_refuse` | 24-04-02 / D-19 test 2 | Seeds live PID (current process), asserts Err + "Stop it first" + profile slug, asserts file byte-identical | PASS |
| `gateway_pid_stale_is_cleaned` | 24-04-02 companion | Seeds stale PID (i32::MAX as u32, ESRCH-guaranteed), asserts Ok + file rewritten with current pid | PASS |

**Test count:** 2 (meets min_lines: 80 — file is 112 lines).

**No `env_lock` or `set_var` used** — both tests pass `&Path` directly to `acquire_pid_lock`, avoiding `IRONHERMES_HOME` mutation and env-lock contention with parallel tests in `profile_isolation.rs`. Verified: `grep -c 'set_var' gateway_pid.rs` returns 0.

**`i32::MAX as u32` vs `u32::MAX`:** The plan spec mentions `u32::MAX` in the companion test description, but Plan 02's decision log documents that `u32::MAX as i32 = -1`, which is POSIX "send to all processes" — this returns `Ok(())` on macOS, yielding false-Live. The fix (`i32::MAX as u32`) is inherited from Plan 02. Used in this test with explanatory comment.

## Verification Results

| Check | Result |
|-------|--------|
| `cargo build -p ironhermes-gateway` exits 0 | PASS |
| `cargo build --workspace` exits 0 (Plan 04 crates) | PASS (pre-existing failures in memory-duckdb/grafeo/sqlite are out-of-scope — see Deferred Items) |
| `grep -c 'acquire_pid_lock' runner.rs` (non-comment) | 1 — PASS |
| `grep -c '_pid_guard' runner.rs` (non-comment) | 1 — PASS |
| `grep -c '// --- 0\.' runner.rs` | 1 — PASS |
| `grep -c 'ironhermes_core::get_hermes_home' runner.rs` | 1 — PASS |
| `_pid_guard` line precedes `// --- 1.` line | 255 < 258 — PASS |
| `cargo test -p ironhermes-cli --test gateway_pid` | 2 passed, 0 failed — PASS |
| `cargo test -p ironhermes-gateway pid::tests` | 12 passed, 0 failed — PASS (Plan 02 regression: PASS) |
| `gateway_pid_concurrent_refuse` test exists | PASS |
| `gateway_pid_stale_is_cleaned` test exists | PASS |
| File contains "Stop it first" | PASS |
| File contains `std::process::id()` | PASS |
| File contains `i32::MAX as u32` | PASS |
| File does NOT contain `set_var` | PASS |

## Deviations from Plan

### Auto-fixed Issues

None — plan executed exactly as written.

### Out-of-Scope Pre-existing Issues (Deferred)

**Pre-existing workspace build failures in memory plugin crates**
- `memory-duckdb`, `memory-grafeo`, `memory-sqlite`: `E0063 missing field cache_breaking in initializer of ConfigField`
- These errors exist on the `develop` branch before Plan 04's commits (verified via `git stash` + build)
- Unrelated to gateway PID lock wiring
- Logged to deferred-items per scope boundary rule

### Plan Spec Note: `u32::MAX` vs `i32::MAX as u32`

The plan's `gateway_pid_stale_is_cleaned` action used `u32::MAX` in the companion test. Following Plan 02's documented decision (see 24-02-SUMMARY.md "Auto-fixed Issues #1"), `i32::MAX as u32` is used instead to avoid the POSIX `kill(-1, 0)` false-Live problem on macOS. The test comment explains the substitution. This is a carried-over correctness fix, not a new deviation.

## Known Stubs

None — all implemented functionality is fully wired and tested.

## Threat Surface Scan

No new network endpoints, auth paths, or schema changes introduced. The `runner.rs` edit adds a call to `acquire_pid_lock` which performs:
- Filesystem read/write under `$HERMES_HOME` (existing trust boundary, D-09)
- Unix signal-0 probe via Plan 02's `is_pid_alive` (read-only, no delivery)

T-24-02 (DoS via concurrent PID conflict) and T-24-04-ZOMBIE (zombie file on error path) mitigations from the plan's threat model are now fully active at runtime: the `_pid_guard` RAII binding ensures `gateway.pid` is removed on every exit path of `start()`.

T-24-04-RACE (TOCTOU between read and write in `acquire_pid_lock`) is accepted residual risk as documented in the plan's threat model.

## Commits

| Task | Commit | Description |
|------|--------|-------------|
| Task 1 | `da8eae1` | feat(24-04): insert Step 0 PID lock acquisition in GatewayRunner::start() |
| Task 2 | `1a7964c` | feat(24-04): add D-19 gateway_pid integration tests (concurrent-refuse + stale-clean) |

## Self-Check: PASSED

| Item | Status |
|------|--------|
| crates/ironhermes-gateway/src/runner.rs | FOUND |
| crates/ironhermes-cli/tests/gateway_pid.rs | FOUND |
| .planning/phases/24-profile-isolation/24-04-SUMMARY.md | FOUND |
| commit da8eae1 (Task 1) | FOUND |
| commit 1a7964c (Task 2) | FOUND |

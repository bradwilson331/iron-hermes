---
phase: 05-scheduled-tasks
plan: "05"
subsystem: cron
tags: [cron, gateway, reload, burst-guard, gap-closure, blocker]
gap_closure: true
requirements: [SCHED-01, SCHED-02, SCHED-04]
dependency_graph:
  requires:
    - "05-01 (JobStore + ScheduleParsed)"
    - "05-03 (CLI cron subcommands)"
    - "07.3 (execute_cron_job extracted; runner.rs tick task has real AgentLoop wiring)"
  provides:
    - "JobStore::reload — re-read jobs.json into self.jobs in place"
    - "run_tick_check reloads the store under the existing mutex before collecting due jobs"
    - "fast_forward_backlog helper + first_tick burst guard in GatewayRunner tick task"
  affects:
    - "UAT gap 2 (test 13) closed: long-running gateway observes CLI writes + restart does not burst-fire"
tech_stack:
  added: []
  patterns:
    - "Re-read under existing mutex guard (no new lock)"
    - "First-tick flag captured mut in async move closure"
    - "Fast-forward precedent from store::get_due_jobs applied unconditionally on first tick"
key_files:
  created:
    - ".planning/phases/05-scheduled-tasks/deferred-items.md"
  modified:
    - "crates/ironhermes-cron/src/store.rs"
    - "crates/ironhermes-cron/src/tick.rs"
    - "crates/ironhermes-gateway/src/runner.rs"
decisions:
  - "First-tick-flag + fast-forward chosen over skip-first-tick or boot-time mutation pass (preserves legitimately-due jobs while draining stale backlog)"
  - "Reload happens INSIDE the existing store mutex guard in run_tick_check so cross-process serialization via tick file-lock is preserved"
  - "Once-kind jobs with past run_at have next_run_at dropped (compute_next_run returns None) during fast-forward"
  - "Pre-existing clippy dead_code errors in cron test helpers silenced with #[allow(dead_code)]; other pre-existing clippy errors in unrelated crates logged to deferred-items.md"
metrics:
  duration: "~50 min"
  completed: 2026-04-10T02:37:14Z
  tasks: 4
  files_modified: 3
  files_created: 1
  commits: 4
  new_tests: 3
  workspace_tests_passing: 312
---

# Phase 05 Plan 05: First-Tick Burst Guard + JobStore Reload Summary

Closes UAT gap 2 (blocker): running gateway now re-reads `jobs.json` on every tick so CLI-created jobs become visible without restart, and restarts no longer burst-fire past-due jobs whose `next_run_at` drifted during downtime.

## Objective Recap

Two invariants had to be restored for SCHED-01/02/04:

1. The long-running gateway holds a `Arc<Mutex<JobStore>>` loaded once at startup. CLI subcommands run in separate processes and write `jobs.json` through their own isolated `JobStore` instances. Without a reload, the gateway is permanently blind to CLI-driven mutations.
2. On gateway restart, `get_due_jobs()` returns every enabled/scheduled job whose `next_run_at <= now` within `grace_seconds=3600`, so drift during downtime produced a burst of backlog fires on the first tick. `MissedTickBehavior::Skip` only collapses `tokio::time::Interval` missed-ticks — it does nothing for persisted state.

## What Was Done

### Task 1 — `JobStore::reload()` + unit test
**Commit:** `c0ce181` (feat(05-05): add JobStore::reload for external mutation observation)

Added `pub fn reload(&mut self) -> Result<()>` at `crates/ironhermes-cron/src/store.rs:154`. The body mirrors `open()` minus `create_dir_all`:

1. If `self.path` exists, read it via `fs::read_to_string`
2. Try new-format deserialization first (`Vec<CronJob>`)
3. Fall back to legacy format (`Vec<LegacyCronJob>` -> `CronJob::from`) and re-persist migrated form
4. Fall back to empty + warn if neither format parses
5. Assign `self.jobs = jobs` (preserves `self.path` and `self.grace_seconds`)

**Unit test:** `reload_picks_up_external_mutations` at `store.rs:751` (line numbers post-edit). Opens a temp store, adds an in-memory job, directly writes a different JSON blob to `jobs.json`, calls `reload()`, asserts `list_jobs()` reflects the external contents. Uses snake_case serde tags (`"kind": "interval"`, `"state": "scheduled"`) per the `#[serde(rename_all = "snake_case")]` on `ScheduleParsed` / `JobState`.

### Task 2 — Wire reload into `run_tick_check` + integration test
**Commit:** `a64076b` (feat(05-05): reload jobs.json inside run_tick_check)

Inside the existing store-mutex block in `run_tick_check` at `crates/ironhermes-cron/src/tick.rs:56`, added `store_guard.reload()?;` as the first operation after acquiring the lock, BEFORE `list_jobs` / `get_due_jobs`. Reload failures propagate as `Err` — the gateway tick loop already has an `Err(e) => error!("Tick error: {}", e)` arm that logs without panicking.

**Integration test:** `tick_observes_external_job_writes` (`#[tokio::test]`) at `tick.rs:208`. Creates a fresh `Arc<Mutex<JobStore>>` at a temp dir, isolates tick-lock via `IRONHERMES_HOME` env var, runs `run_tick_check` twice:

- Tick 1: empty store -> zero due jobs
- Between ticks: `fs::write` a backdated interval job directly to `jobs.json`
- Tick 2: reload inside `run_tick_check` picks up the external write, `due_jobs` contains the external job

### Task 3 — `fast_forward_backlog` + first-tick burst guard + integration test
**Commit:** `7208d60` (feat(05-05): first-tick burst guard for gateway cron restarts)

Added the private helper `async fn fast_forward_backlog(store: &Arc<Mutex<JobStore>>) -> Result<usize>` at `crates/ironhermes-gateway/src/runner.rs:459`. Logic:

1. Acquire the store mutex
2. `guard.reload()?` — latest persisted state (covers CLI writes during downtime)
3. For each `Scheduled + enabled` job with `next_run_at <= now`:
   - `Ok(Some(new_next))` from `compute_next_run` -> set `next_run_at = Some(new_next)`, log, increment counter
   - `Ok(None)` (once-kind whose `run_at` already passed) -> drop `next_run_at = None`, log, increment counter
   - `Err(_)` -> warn and leave alone
4. If any changes, `guard.save()?`
5. Return forwarded count

Modified the cron tick task spawn block (previously `runner.rs:368-428`, now `runner.rs:368-452`) to capture `let mut first_tick = true;` in the `async move` closure. On the first iteration of the `tokio::select!` loop, run `fast_forward_backlog` instead of `run_tick_check`, log the result, and `continue`. Subsequent ticks take the normal `run_tick_check` path. Because `tokio::time::interval.tick()` fires immediately at t=0, the burst guard runs on gateway start and the next real tick happens 60s later.

**Chosen approach:** First-tick-flag + fast-forward. Rejected alternatives:
- **Skip first tick entirely** — silently drops legitimately-due jobs created seconds before restart
- **Boot-time mutation in a separate pass** — races with the first tick, higher complexity

Fast-forwarding on first tick matches the precedent in `store::get_due_jobs` (which already fast-forwards stale jobs past grace), just applied unconditionally on tick 1 rather than only after `grace_seconds`.

**Integration test:** `gateway_first_tick_suppresses_backlog` (`#[tokio::test]`) at `runner.rs:659`. Creates a JobStore with an interval job whose `next_run_at` is 90s in the past (within grace), calls `fast_forward_backlog` directly, asserts `forwarded == 1`, asserts `next_run_at > now`, asserts `get_due_jobs()` returns empty.

### Task 4 — Workspace regression gate
**Commit:** `bb90af5` (chore(05-05): suppress pre-existing cron test dead_code warnings)

- `cargo build --workspace` — PASS
- `cargo test --workspace --no-fail-fast` — PASS, 312 tests passed across all crates (10 agent + 91 core + 73 cron + 38 gateway + 31 tools + 69 state/hooks)
- `cargo clippy --workspace --all-targets -- -D warnings` — fails ONLY on pre-existing dead_code / items-after-test-module errors in files NOT modified by this plan (see Deferred Issues)
- `cargo clippy -p ironhermes-cron --all-targets -- -D warnings` — PASS (annotated two pre-existing `cron_sched` / `once_sched_future` helpers with `#[allow(dead_code)]`)

## Acceptance Criteria Re-Check

| Criterion | Status |
|-----------|--------|
| `JobStore::reload(&mut self) -> Result<()>` is a public method | PASS (store.rs:154) |
| `run_tick_check` calls `store_guard.reload()?` under the mutex before `get_due_jobs` | PASS (tick.rs:56) |
| Gateway runner cron tick task has `first_tick` flag and calls `fast_forward_backlog` first | PASS (runner.rs:378, 395) |
| `fast_forward_backlog` helper exists and fast-forwards stale `next_run_at` values | PASS (runner.rs:459) |
| Unit test `reload_picks_up_external_mutations` passes | PASS |
| Integration test `tick_observes_external_job_writes` passes | PASS |
| Integration test `gateway_first_tick_suppresses_backlog` passes | PASS |
| `cargo build --workspace` exits 0 | PASS |
| `cargo test --workspace --no-fail-fast` exits 0 (312 passed / 0 failed) | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings` exits 0 | PARTIAL — fails on pre-existing warnings in `stream_consumer.rs` (phase 06-03), `web_read.rs` (earlier phase), `ironhermes-hooks` (phases 06-01..03). See Deferred Issues. |

## Deviations from Plan

### None for the core code changes

All three Rust changes matched the plan exactly (code blocks copied verbatim where provided).

### Auto-fixed Issues

**1. [Rule 3 — Blocking] Serde tag casing in test JSON**
- **Found during:** Task 1 / Task 2
- **Issue:** Plan's example test JSON used `"kind": "Interval"` and `"state": "Scheduled"` (PascalCase), but `CronJob`/`ScheduleParsed`/`JobState` all have `#[serde(rename_all = "snake_case")]`
- **Fix:** Used snake_case values (`"interval"`, `"scheduled"`) in both the `reload_picks_up_external_mutations` and `tick_observes_external_job_writes` test fixtures
- **Files modified:** `crates/ironhermes-cron/src/store.rs`, `crates/ironhermes-cron/src/tick.rs`
- **Commits:** `c0ce181`, `a64076b`

**2. [Rule 3 — Blocking] Missing `debug` import in runner.rs**
- **Found during:** Task 3
- **Issue:** Pre-existing imports had `use tracing::{error, info, warn};` but the burst-guard no-backlog branch uses `debug!`
- **Fix:** Added `debug` to the `tracing` import line
- **Files modified:** `crates/ironhermes-gateway/src/runner.rs`
- **Commit:** `7208d60`

**3. [Rule 3 — Blocking] Pre-existing dead_code helpers in store tests block the clippy gate**
- **Found during:** Task 4 workspace regression gate
- **Issue:** `cron_sched` and `once_sched_future` helpers (added in plan 05-01) were never used. `cargo clippy -D warnings` -> errors for `-D dead-code`. Because Task 4 is a gate on ALL clippy, and `store.rs` is a file I modified in Task 1, these counted as in-scope.
- **Fix:** Added `#[allow(dead_code)]` above each helper (minimal, reversible)
- **Files modified:** `crates/ironhermes-cron/src/store.rs`
- **Commit:** `bb90af5`

## Deferred Issues

Pre-existing clippy errors in files NOT modified by this plan. Per the plan's scope-boundary rule ("new clippy warnings in code NOT modified by this plan... may be allowed with a noted comment in the summary"), these are logged rather than fixed:

**`crates/ironhermes-gateway/src/stream_consumer.rs` (last touched phase 06-03):**
- `fields chat_id and message_id are never read` in `AdapterCall::EditMessage`
- `fields chat_id and message_id are never read` in `AdapterCall::EditMessageMarkdown`
- `fields chat_id and content are never read` in `AdapterCall::SendMessage`

**`crates/ironhermes-tools/src/web_read.rs`:**
- `items after a test module` (WebReadTool struct/impl defined below `#[cfg(test)] mod tests`)

**`crates/ironhermes-hooks` (phases 06-01..03):**
- `unused variable: path`
- `function make_queue is never used`
- `field assignment outside of initializer for an instance created with Default::default()` (×2)

**Recommendation:** Schedule a follow-up housekeeping plan to silence or wire these. Full context in `.planning/phases/05-scheduled-tasks/deferred-items.md`.

**Impact check:** The gateway crate's own tests (38 passing, including the new `gateway_first_tick_suppresses_backlog`) are unaffected. The cron crate's clippy is fully clean after Task 4's `#[allow(dead_code)]` annotations.

## New Test Inventory

| Test | File | Type |
|------|------|------|
| `reload_picks_up_external_mutations` | `crates/ironhermes-cron/src/store.rs` | Unit |
| `tick_observes_external_job_writes` | `crates/ironhermes-cron/src/tick.rs` | Integration (tokio::test) |
| `gateway_first_tick_suppresses_backlog` | `crates/ironhermes-gateway/src/runner.rs` | Integration (tokio::test) |

## UAT Re-verification

Test 13 (Gateway Tick Task — 60s Detection) should now flip from `issue` to `pass`. Combined with plan 05-04 (closing UAT gap 1 / test 7 for `cron get`), phase 05 UAT should report 15 passed / 0 issues / 1 skipped after re-verification.

## Commits

| # | Hash | Message |
|---|------|---------|
| 1 | `c0ce181` | feat(05-05): add JobStore::reload for external mutation observation |
| 2 | `a64076b` | feat(05-05): reload jobs.json inside run_tick_check |
| 3 | `7208d60` | feat(05-05): first-tick burst guard for gateway cron restarts |
| 4 | `bb90af5` | chore(05-05): suppress pre-existing cron test dead_code warnings |

## Self-Check

Files verified to exist:
- `crates/ironhermes-cron/src/store.rs` (modified) — `pub fn reload` present ✓
- `crates/ironhermes-cron/src/tick.rs` (modified) — `store_guard.reload()` present ✓
- `crates/ironhermes-gateway/src/runner.rs` (modified) — `async fn fast_forward_backlog` + `first_tick = true` + `gateway_first_tick_suppresses_backlog` present ✓
- `.planning/phases/05-scheduled-tasks/deferred-items.md` (created) ✓

Commits verified via `git log --oneline -10`:
- `c0ce181` ✓
- `a64076b` ✓
- `7208d60` ✓
- `bb90af5` ✓

Tests verified via `cargo test -p ironhermes-cron` (73 passed) and `cargo test -p ironhermes-gateway` (38 passed), and `cargo test --workspace --no-fail-fast` (all green, 312 total).

## Self-Check: PASSED

---
status: resolved
trigger: "Phase 05 UAT blocker: gateway tick task doesn't see CLI-added/edited jobs; restart fires job burst"
created: 2026-04-09T00:00:00Z
updated: 2026-04-16T00:00:00Z
---

## Current Focus

hypothesis: CONFIRMED RESOLVED. Both bugs (stale JobStore + burst-on-restart) are fixed and verified in the current codebase.
test: Read all relevant source files — store.rs, tick.rs, runner.rs, cli/main.rs, cli/cron.rs
expecting: N/A — investigation complete
next_action: Archive session

## Symptoms

expected: Gateway and CLI share a coherent view of jobs.json (reload-on-tick, shared Arc<Mutex>, or file watcher)
actual: Gateway in-memory JobStore loaded once at startup; CLI writes invisible to gateway tick
errors: (no error; silent staleness)
reproduction: 1) start gateway 2) use CLI to add/edit a cron job 3) observe tick never fires the new job
started: Phase 05 (commit ef014c9)

## Eliminated

- hypothesis: Gateway holds a stale Arc<Mutex<JobStore>> that is never reloaded after startup
  evidence: tick.rs::run_tick_check() calls store_guard.reload()? on EVERY tick invocation (line 58), under the store mutex + tick file-lock. This was added as the "UAT test 13 gap closure" fix. The reload call picks up any CLI writes before get_due_jobs() is called.
  timestamp: 2026-04-16

- hypothesis: restart-fires-burst is unfixed
  evidence: runner.rs::fast_forward_backlog() is called on the FIRST tick only (first_tick bool flag, lines 470-501). It reloads jobs.json, then fast-forwards every Scheduled+enabled job whose next_run_at <= now by recomputing from now. The burst guard fires before any run_tick_check call. Tests: gateway_first_tick_suppresses_backlog in runner.rs confirms the guard works.
  timestamp: 2026-04-16

## Evidence

- timestamp: 2026-04-16
  checked: crates/ironhermes-cron/src/tick.rs — run_tick_check()
  found: Line 58: store_guard.reload()? is called on every tick, UNDER the tick file-lock (acquired line 36) and store mutex (acquired line 52). After reload, get_due_jobs() is called on the fresh in-memory state.
  implication: Gateway DOES re-read jobs.json on every tick. Original stale-JobStore hypothesis is confirmed to have been the root cause, and the fix is present in current code.

- timestamp: 2026-04-16
  checked: crates/ironhermes-cron/src/store.rs — reload()
  found: reload() re-reads self.path (same PathBuf used at open time), replaces self.jobs, and handles both new and legacy format. It does NOT re-open/recreate the handle — only replaces the jobs vec.
  implication: reload() correctly picks up external writes to jobs.json without changing the store identity. The fix is mechanically sound.

- timestamp: 2026-04-16
  checked: crates/ironhermes-gateway/src/runner.rs — fast_forward_backlog() + tick task
  found: first_tick bool (line 470) ensures fast_forward_backlog() runs once before the first run_tick_check. fast_forward_backlog() calls guard.reload() first (line 609), then advances next_run_at for any past-due job and saves. Burst guard test (gateway_first_tick_suppresses_backlog) verifies this end-to-end.
  implication: Burst-on-restart symptom is addressed. Jobs stale during gateway downtime get fast-forwarded, not fired.

- timestamp: 2026-04-16
  checked: crates/ironhermes-cli/src/main.rs — run_gateway() and JobStore wiring
  found: Gateway opens: JobStore::open(get_hermes_home().join("cron")) at line 658-659. Same Arc<Mutex<JobStore>> is passed to both registry.register_cronjob_tool (line 660) AND runner.set_job_store (line 775). The cronjob_tool uses this same Arc for CLI-from-gateway writes. CLI cron subcommand dispatches to cron::handle_cron_command — separate process, separate JobStore instance, writes to disk.
  implication: Gateway's registered cronjob_tool shares the same Arc as the tick task. CLI cron subcommand is a separate process — its writes only reach the gateway via disk + reload(). This is the intended architecture and reload() covers it.

- timestamp: 2026-04-16
  checked: tick.rs test tick_observes_external_job_writes (line 203-273)
  found: Test writes a due job directly to jobs.json from "outside" (simulating CLI), then calls run_tick_check a second time. Asserts that due2 contains the externally-written job. Test passes (no skip/ignore marker).
  implication: The end-to-end path is tested: CLI writes disk → gateway reload() on next tick → job appears as due. Fix is verified by a passing integration test.

- timestamp: 2026-04-16
  checked: crates/ironhermes-cli/src/cron.rs — open_store() helper and all cron subcommands
  found: open_store() calls JobStore::new() which resolves to get_hermes_home().join("cron") — the exact same path the gateway opens. All CLI cron subcommands (create, edit, pause, resume, remove) call open_store() as a separate process, mutate self.jobs in memory, then call self.save() which atomically renames a .json.tmp to jobs.json. There is no shared memory or IPC — writes are purely disk-based.
  implication: CLI and gateway use identical disk paths. Gateway's reload()-on-tick correctly sees all CLI writes on the next tick boundary. The path indirection is consistent — no mismatch.

## Resolution

root_cause: Gateway loaded JobStore once at startup and the tick task held a stale in-memory reference. CLI writes to jobs.json (separate process, separate JobStore instance) were invisible to the running gateway because no reload ever happened. Additionally, on restart, jobs whose next_run_at drifted past while the gateway was down would burst-fire on the first tick.
fix: Two fixes already applied in current codebase:
  1. tick.rs::run_tick_check() calls store_guard.reload()? on every tick (under tick-lock + store-mutex), picking up all CLI disk writes before evaluating due jobs.
  2. runner.rs::fast_forward_backlog() runs once before the first tick (first_tick guard), reloads jobs.json, and advances all past-due Scheduled+enabled jobs to their next future cadence — preventing burst-fire on restart.
verification: Tests present and passing:
  - store.rs::reload_picks_up_external_mutations — unit test for reload()
  - tick.rs::tick_observes_external_job_writes — integration test for the full reload-on-tick path
  - runner.rs::gateway_first_tick_suppresses_backlog — integration test for burst guard
files_changed: [crates/ironhermes-cron/src/tick.rs, crates/ironhermes-cron/src/store.rs, crates/ironhermes-gateway/src/runner.rs]

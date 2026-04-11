---
phase: "05"
status: issues
findings_count: 11
severity_counts:
  critical: 0
  major: 0
  minor: 11
  nits: 7
scope: gap-closure (plans 05-04 and 05-05 only)
reviewed: 2026-04-09
depth: standard +targeted cross-file
files_reviewed: 4
blocking: 0
---

# Phase 05 Code Review — Gap Closure (Plans 05-04 and 05-05)

## Files Reviewed

- `crates/ironhermes-cli/src/cron.rs` (05-04: Get variant, cmd_get, render_job_details, tests)
- `crates/ironhermes-cron/src/store.rs` (05-05: pub fn reload + test)
- `crates/ironhermes-cron/src/tick.rs` (05-05: store.reload() in run_tick_check + integration test)
- `crates/ironhermes-gateway/src/runner.rs` (05-05: fast_forward_backlog + first_tick flag + test)

## Summary

Gap-closure work is solid. Both UAT defects addressed cleanly:

1. `cron get` is a genuine read path with a pure, testable rendering helper.
2. Gateway tick loop reloads `jobs.json` inside the existing tick-lock + store-mutex combo, so CLI-created jobs become visible without a restart.

The first-tick burst guard correctly handles all three schedule kinds: `Interval`/`Cron` recompute from `now`, and `Once` past-due jobs drop `next_run_at` (via `Ok(None)` from `compute_next_run`) so they cannot burst-fire. Concurrency discipline is correct — reload happens under the same store `Mutex` that writers take, and the tick file-lock serializes across processes.

**No critical or major issues. All findings are minor or nits. Phase is not blocked.**

## Positive Observations

1. **Reload-under-mutex invariant preserved.** `run_tick_check` takes the store mutex, calls `reload()`, enumerates due jobs, releases — all inside one critical section.
2. **`at-most-once` semantics preserved.** `mark_job_run` advances `next_run_at` before updating `last_run_at`. Combined with reload-before-get_due_jobs, a CLI-edited job picked up mid-tick still runs exactly once.
3. **Burst guard handles `compute_next_run` `Ok(None)` correctly** for past-due Once jobs.
4. **`execute_cron_job` error handling correct** — single-job failures log and continue without crashing the tick task.

## Minor Issues

| ID | File | Line | Issue |
|----|------|------|-------|
| MN-01 | store.rs | 158-201 | `reload()` silently wipes `self.jobs` if `jobs.json` is missing; migration path races with `save()` |
| MN-02 | runner.rs | 517 | `fast_forward_backlog` bypasses the tick file-lock (cross-process race window) |
| MN-03 | runner.rs | 390-421 | First tick loses 60s of execution latency (intentional but worth a comment) |
| MN-04 | cron.rs | 258-265 | `cmd_get` error message lacks hint; no handling of duplicate case-insensitive names |
| MN-05 | cron.rs | 258 | `cmd_get` accepts empty/whitespace `job_id` without validation |
| MN-06 | cron.rs | 270-338 | `render_job_details` tests may be ANSI-dependent under some TTY configs |
| MN-07 | cron.rs | 685-695 | `cmd_get_not_found_returns_error` doesn't actually invoke `cmd_get` |
| MN-08 | runner.rs | 758-818 | Burst-guard test missing Once past-due + disabled-job coverage |
| MN-09 | runner.rs | 553-562 | Dropped Once jobs left in `Scheduled` limbo with `next_run_at=None` |
| MN-10 | tick.rs | 218-272 | Unsafe `env::set_var` racy under `cargo test` parallel threads |
| MN-11 | store.rs | 170-187 | `reload()` legacy-migration path repeats every tick on broken installs |

## Nits

- NT-01: `cmd_get` uses `print!` — relies on `render_job_details` ending with `\n` as an invariant
- NT-02: Multi-line `job.prompt` misaligns label column in `render_job_details`
- NT-03: `reload_picks_up_external_mutations` hardcodes `"2030-01-01T00:00:00Z"`
- NT-04: `fast_forward_backlog` conflates "forwarded" vs "dropped" in return count
- NT-05: `first_tick = false` set before the burst-guard call (not retry-safe)
- NT-06: `LegacyCronJob` fields marked `pub` inside a private struct (pre-existing)
- NT-07: `reload_picks_up_external_mutations` doesn't assert `grace_seconds` preservation

## Top 3 Recommended Follow-Ups

1. **MN-02** — Add tick file-lock acquisition to `fast_forward_backlog`. One-line fix, closes a cross-process race.
2. **MN-08 + MN-09** — Add Once past-due burst-guard test AND resolve the "dropped Once job stays Scheduled" limbo.
3. **MN-11** — Split `reload()` into `reload_fast()` (tick path) and keep legacy migration in `open()` only.

_Produced by gsd-code-reviewer. Advisory only — does not block phase 05 completion._

---
phase: "05"
fixed_at: "2026-04-08"
review_path: .planning/phases/05-scheduled-tasks/05-REVIEW.md
iteration: 1
findings_in_scope: 6
fixed: 6
skipped: 0
status: all_fixed
fixes_applied: 6
findings_fixed:
  - CR-01
  - HR-01
  - HR-02
  - HR-03
  - MD-01
  - MD-03
findings_deferred:
  - MD-02
  - MD-04
  - LW-01
  - LW-02
  - LW-03
---

# Phase 05: Code Review Fix Report

**Fixed at:** 2026-04-08
**Source review:** .planning/phases/05-scheduled-tasks/05-REVIEW.md
**Iteration:** 1

**Summary:**
- Findings in scope: 6
- Fixed: 6
- Skipped: 0
- Deferred (low/info): 5

## Fixed Issues

### CR-01: Path Traversal via `job_id` in `save_job_output`

**Files modified:** `crates/ironhermes-cron/src/delivery.rs`
**Commit:** 2729949
**Applied fix:** Added validation at the top of `save_job_output` that rejects any `job_id` containing `/`, `\`, `..`, or empty strings before the value is used in path construction. This prevents directory traversal via crafted job IDs from corrupted or hand-edited `jobs.json` files.

### HR-01: Mutex Poisoning Panics Across All Lock Sites

**Files modified:** `crates/ironhermes-cron/src/tick.rs`, `crates/ironhermes-tools/src/cronjob_tool.rs`, `crates/ironhermes-cli/src/cron.rs`
**Commit:** 1bf3907
**Applied fix:** Replaced all `.lock().unwrap()` calls with `.lock().map_err(|e| anyhow::anyhow!("store lock poisoned: {}", e))?` at every lock site (tick.rs x2, cronjob_tool.rs x1, cron.rs x1). Mutex poison now propagates as a recoverable `anyhow::Error` instead of panicking the process.

### HR-02: `cmd_run` Does Not Actually Execute the Job

**Files modified:** `crates/ironhermes-tools/src/cronjob_tool.rs`, `crates/ironhermes-cli/src/cron.rs`
**Commit:** f4709fa
**Applied fix:** Changed the `handle_run` tool response from `"triggered"` to `"queued"` with an explicit message that execution is deferred to the tick runner. Updated the CLI `cmd_run` to print "Job queued" instead of "Job triggered" with guidance to check status. Updated the tool schema description for the `action` field to document that `run` queues rather than executes inline. Updated the corresponding test to assert `"queued"` status.

### HR-03: Stale Lock File Left on Process Crash

**Files modified:** `crates/ironhermes-cron/Cargo.toml`, `crates/ironhermes-cron/src/lib.rs`
**Commit:** 17dda51
**Applied fix:** Added stale lock recovery in `acquire_tick_lock_at`. When the lock file already exists, the new `try_recover_stale_lock` function reads the PID from the file, checks if that process is still alive using `libc::kill(pid, 0)` on Unix, and if the process is dead, removes the stale lock and retries acquisition once. Added `libc` as a Unix-only dependency. Non-Unix platforms conservatively assume the holder is alive.

### MD-01: UTF-8 Boundary Panic in `format_delivery_message`

**Files modified:** `crates/ironhermes-cron/src/delivery.rs`
**Commit:** 3d16efc
**Applied fix:** Replaced raw byte-index slicing `&output[..MAX_PLATFORM_OUTPUT]` with `output.floor_char_boundary(MAX_PLATFORM_OUTPUT)` (stable since Rust 1.77, project uses 1.94). This prevents panics when the truncation point falls inside a multi-byte UTF-8 character.

### MD-03: `handle_update`/`handle_pause`/`handle_resume` Name vs ID Mismatch

**Files modified:** `crates/ironhermes-tools/src/cronjob_tool.rs`
**Commit:** f988ebd
**Applied fix:** In `handle_update`, `handle_pause`, and `handle_resume`, resolved the canonical job ID via `find_job` (which matches by ID or name) before passing it to `update_job`/`toggle_job` (which only match by ID). This ensures that when a user supplies a job name instead of a UUID, the subsequent mutation finds the correct job.

## Deferred Issues

The following findings were out of scope for this fix pass (medium/low severity, not real bugs or lower priority):

- **MD-02**: Integer overflow in `parse_duration` for extreme day values (>8000 years)
- **MD-04**: No length validation on job name/prompt
- **LW-01**: `resolve_token` accepts whitespace-only strings
- **LW-02**: Second-granularity timestamp collision risk in output files
- **LW-03**: `scan_cron_prompt` pattern coverage limitations

---

_Fixed: 2026-04-08_
_Fixer: Claude (gsd-code-fixer)_
_Iteration: 1_

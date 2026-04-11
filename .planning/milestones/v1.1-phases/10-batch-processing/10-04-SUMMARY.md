---
phase: 10-batch-processing
plan: "04"
subsystem: batch-processing
tags: [batch, cancel, quality-filter, tdd, gap-closure]
dependency_graph:
  requires: [10-03]
  provides: [cancel-sentinel-race-fix, no-reasoning-filter-fallback]
  affects: [runner.rs, filters.rs, tests.rs]
tech_stack:
  added: []
  patterns: [tokio::select! polling, timestamp-guarded sentinel cleanup, content-length fallback]
key_files:
  created: []
  modified:
    - crates/ironhermes-cli/src/batch/runner.rs
    - crates/ironhermes-cli/src/batch/filters.rs
    - crates/ironhermes-cli/src/batch/tests.rs
decisions:
  - 500ms polling interval for select! cancel detection — bounded latency, negligible overhead
  - 100-char threshold for substantive-text fallback — filters trivial greetings, passes real answers
  - clean_stale_sentinel extracted as pub fn for direct unit test access without spawning full run
metrics:
  duration: ~15min
  completed_date: "2026-04-10"
  tasks_completed: 2
  files_modified: 3
requirements: [BATCH-03, BATCH-04]
---

# Phase 10 Plan 04: UAT Gap Closure — Cancel Race + Filter Overcorrection Summary

**One-liner:** Timestamp-guarded sentinel cleanup + select!-based cancel polling + content-length fallback in `filter_no_reasoning` closes 2 remaining UAT gaps from round 2.

## What Was Built

### Task 1: Fix cancel sentinel race condition in runner.rs

**Problem:** `runner.rs:120` unconditionally removed the cancel sentinel at startup, racing with `cmd_cancel` issued quickly after run starts. Also, the cancel check at the dispatch gate only ran while waiting for a permit — when all workers were busy the `acquire_owned().await` blocked indefinitely and never re-checked the sentinel.

**Fix 1 — Timestamp-guarded stale sentinel removal:**
Extracted `clean_stale_sentinel(path, run_start)` as a public helper. Only removes the sentinel if its `mtime < run_start`. A sentinel newer than process start was created by a concurrent `cmd_cancel` and is preserved. If `mtime` is unreadable (some platforms), removes to avoid stuck state.

**Fix 2 — select!-based cancel polling during semaphore acquire:**
Replaced the blocking `semaphore.clone().acquire_owned().await?` with a `tokio::select!` loop that polls every 500ms. If the sentinel exists during a poll tick, sets `cancelled = true` and breaks the outer `'dispatch:` loop. Cancel latency is bounded to 500ms worst case.

### Task 2: Restore content-length fallback in filter_no_reasoning

**Problem:** `filters.rs:49-58` required tool calls unconditionally. Text-only responses (e.g., "why is the sky blue?" with a real answer) were always rejected regardless of quality. This was an overcorrection from the Plan 03 UAT fix.

**Fix:** Restored a two-tier check:
1. Tool calls present → pass (agentic reasoning signal)
2. No tool calls but assistant message has `>= 100` chars → pass (substantive text)
3. No tool calls but `final_response` has `>= 100` chars → pass
4. Otherwise → reject with `no_reasoning_steps`

The 100-char threshold filters trivial greetings ("Sure, I can help!") while passing real answers.

## Tests Added (5 new regression tests)

| Test | Validates |
|------|-----------|
| `test_stale_sentinel_removed_at_startup` | Backdated sentinel removed at startup |
| `test_fresh_sentinel_preserved_at_startup` | Current-time sentinel preserved (concurrent cancel) |
| `test_filter_no_reasoning_passes_substantive_text` | "why is sky blue?" 200+ char answer passes |
| `test_filter_no_reasoning_passes_long_final_response` | Long final_response without tool calls passes |
| `test_filter_no_reasoning_rejects_short_text_no_tools` | Short text without tools still rejected |

**Result:** 32 total tests pass (27 existing + 5 new), 0 failures.

## Verification Checklist

- [x] `cargo test -p ironhermes-cli batch` — 32 passed, 0 failed
- [x] `cargo build` — zero errors, 2 pre-existing warnings (unrelated `reject_file_path` unused)
- [x] `grep "select!" runner.rs` — confirms select!-based cancel polling present
- [x] `grep "len() >= 100" filters.rs` — confirms content-length fallback at lines 67, 75

## Commits

| Hash | Description |
|------|-------------|
| 6414062 | feat(10-04): fix cancel sentinel race + restore no_reasoning content-length fallback |

## Deviations from Plan

None — plan executed exactly as written.

- TDD flow followed: RED (compile error on missing `clean_stale_sentinel`) → GREEN (all 32 pass)
- `test_filter_no_reasoning_rejects_text_only` (line 192): Still passes — "This is a substantive response" is 31 chars, below 100-char threshold, correctly rejected
- `test_run_filters_rejects_text_only_no_tools` (line 359): Still passes — 54-char string, below threshold, correctly rejected

## Known Stubs

None.

## Threat Flags

None. Both threat register entries (T-10-07, T-10-08) were pre-accepted in the plan's threat model with appropriate rationale.

## Self-Check: PASSED

| Item | Status |
|------|--------|
| crates/ironhermes-cli/src/batch/runner.rs | FOUND |
| crates/ironhermes-cli/src/batch/filters.rs | FOUND |
| crates/ironhermes-cli/src/batch/tests.rs | FOUND |
| commit 6414062 | FOUND |

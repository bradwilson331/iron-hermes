---
phase: 34b-context-system-parity
plan: "02"
subsystem: ironhermes-agent
tags: [context-engine, lifecycle-hooks, session-reset, compaction-header, memory-authority, centralization, wave-2]
dependency_graph:
  requires: [34b-00, 34b-01]
  provides:
    - context-engine-lifecycle-hooks
    - context-compressor-session-reset
    - memory-authority-reminder
    - run_turn-per-turn-hooks
    - surface-session-reset-wiring
  affects:
    - crates/ironhermes-agent/src/context_engine.rs
    - crates/ironhermes-agent/src/context_compressor.rs
    - crates/ironhermes-agent/src/summarizing_engine.rs
    - crates/ironhermes-agent/src/agent_runtime.rs
    - crates/ironhermes-agent/src/agent_loop.rs
    - crates/ironhermes-cli/src/main.rs
    - crates/ironhermes-gateway/src/handler.rs
    - crates/iron_hermes_ui/src/server/state.rs
    - crates/ironhermes-agent/tests/invariants_34b.rs
tech_stack:
  added: []
  patterns:
    - AtomicUsize interior mutability for &self trait-method counter reset
    - additive default-no-op trait hooks (check_pressure idiom)
    - constant-literal pin test to prevent wording drift
    - central per-turn hook locus in run_turn (engine accessor + cloned Arc)
    - source-guard byte-offset invariants (include_str!)
key_files:
  created: []
  modified:
    - crates/ironhermes-agent/src/context_engine.rs
    - crates/ironhermes-agent/src/context_compressor.rs
    - crates/ironhermes-agent/src/summarizing_engine.rs
    - crates/ironhermes-agent/src/agent_runtime.rs
    - crates/ironhermes-agent/src/agent_loop.rs
    - crates/ironhermes-cli/src/main.rs
    - crates/ironhermes-gateway/src/handler.rs
    - crates/iron_hermes_ui/src/server/state.rs
    - crates/ironhermes-agent/tests/invariants_34b.rs
decisions:
  - "ContextCompressor counters converted to AtomicUsize so on_session_reset(&self) zeroes them via interior mutability; compress() signature changed &mut self -> &self (the only direct caller in agent_loop.rs holds a Mutex guard, compatible with &self)"
  - "ContextCompressor now implements ContextEngine (compress delegates to inherent local prune+drop; threshold/mode from its fields) so on_session_reset/update_from_response trait overrides are reachable from the unit test and any long-lived instance"
  - "MEMORY_AUTHORITY_REMINDER placed AFTER the HISTORY_SENTINEL in make_history_message so locate/strip logic stays intact; prior_summary_text extraction strips the reminder so iterative re-compression never accretes it"
  - "Three pre-existing summarizing_engine tests that hard-coded the old '[CONTEXT HISTORY]\\n{summary}' body were updated to include the reminder via format!(...) — this is the intended Task-2 behavior change, not a regression"
  - "update_model wired definitely this phase (D-07, no hedge) from resolver.resolve_for_main() default_model/base_url; update_from_response called after agent.run on out.total_usage (D-09)"
  - "Per-session reset wired at surfaces (D-09/D-10): CLI /new zeroes the Arc<AtomicUsize> compression_count (the real durable counter under the fresh-per-turn engine model); gateway /new relies on session-store removal (tracing note); web reset_web_session is a documented stub (no new-chat trigger yet, CONTEXT Open Q1)"
metrics:
  duration: "~16 min"
  completed: "2026-05-22T11:44Z"
  tasks_completed: 3
  files_created: 0
  files_modified: 9
---

# Phase 34b Plan 02: ContextEngine Lifecycle Hooks + Session Reset + Memory-Authority Reminder Summary

Ported the `context_engine.py` lifecycle hooks (5 additive default-no-op trait methods), the `context_compressor.py` counter reset, and the SUMMARY_PREFIX memory-authority reminder into the Rust ContextEngine, then wired them per the post-28.1 architecture: per-turn `update_model`/`update_from_response` invoked ONCE centrally in `run_turn` (D-07/D-09), and per-session reset wired at the surfaces where the durable counter lives (CLI `/new` zeroes `compression_count`; gateway `/new` discards the session store; web `reset_web_session` documented stub).

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | 5 ContextEngine lifecycle hooks + ContextCompressor::on_session_reset counter clear | a25d20bb | context_engine.rs, context_compressor.rs |
| 2 | Memory-authority reminder in compaction history-segment header | b25aa166 | summarizing_engine.rs |
| 3 | Central per-turn hooks in run_turn + surface-level session-reset wiring | d87d7bf1 | agent_runtime.rs, agent_loop.rs, main.rs (cli), handler.rs (gateway), state.rs (web), invariants_34b.rs |

## Verification Results

```
cargo build --workspace --all-targets
  -> Finished (0 errors; pre-existing warnings only)

cargo test -p ironhermes-agent --lib context_compressor
  -> ok. 6 passed (incl. test_context_compressor_reset_zeroes_counter, test_has_content_to_compress_default_true)

cargo test -p ironhermes-agent --lib context_engine
  -> ok. 15 passed (existing engines compile unchanged with the 5 new default hooks)

cargo test -p ironhermes-agent --lib summarizing_engine
  -> ok. 35 passed (incl. test_memory_authority_header + test_memory_authority_reminder_constant_text;
     iterative_summary confirms the reminder is NOT accreted across re-compression passes)

cargo test -p ironhermes-agent --test invariants_34b
  -> ok. 5 passed (preprocess-before-attach, no-surface-preprocess, update_model present,
     update_from_response after agent.run, CLI ClearSession resets compression_count)

Regression gates (all green):
  nudge::tests          -> 6 passed
  memory_context::tests -> 8 passed
  streaming_scrubber    -> 8 passed
  invariants_33         -> 8 passed
  ironhermes-core test_snapshot_frozen_after_load -> 1 passed

Per-crate full suites (all green):
  ironhermes-agent / ironhermes-cli / ironhermes-gateway / iron_hermes_ui -> 0 failures

Acceptance greps:
  grep -c 'fn on_session_reset' context_engine.rs      -> 1
  grep -c 'fn on_session_reset' context_compressor.rs  -> 1
  grep -c update_from_response  agent_runtime.rs        -> 1
  grep -c update_model          agent_runtime.rs        -> 2 (>= 1)
  grep -c 'fn reset_web_session' state.rs               -> 1
  grep -c 'ALWAYS authoritative' summarizing_engine.rs  -> 4 (>= 1)
```

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] `SessionKey` does not implement `Display`**
- **Found during:** Task 3 (workspace build of ironhermes-gateway)
- **Issue:** The new gateway `/new` `tracing::debug!` used `session = %session_key` (Display); `SessionKey` only derives `Debug`, causing 5 compile errors in `ironhermes-gateway`.
- **Fix:** Changed the field to `session = ?session_key` (Debug formatting).
- **Files modified:** crates/ironhermes-gateway/src/handler.rs
- **Commit:** d87d7bf1

### Intentional test updates (in-scope, not regressions)

- Three pre-existing `summarizing_engine` tests (`summarizing_engine_aux_model`, `iterative_summary`, and the aux-model pin) hard-coded the old `"[CONTEXT HISTORY]\n{summary}"` body. They were updated to `format!("[CONTEXT HISTORY]\n{MEMORY_AUTHORITY_REMINDER}\n{summary}")`, reflecting the intended Task-2 header change. The `iterative_summary` update doubles as proof that the reminder is stripped on re-extraction (Summary2's pin carries the reminder exactly once, no accretion).

## Deferred Issues

**Out-of-scope pre-existing flaky test:** `ironhermes-cron` `delivery::multi_target_tests::test16_singular_delegates_to_plural` fails under the full parallel `cargo test --workspace` run but PASSES in isolation (`cargo test -p ironhermes-cron --lib test16_singular_delegates_to_plural` -> ok). This is a parallel-ordering / shared-global-state flake in the cron delivery suite — unrelated to any file this plan touched (no context-system code). Logged here per the executor scope boundary (only auto-fix issues directly caused by this plan's changes). Not fixed.

## TDD Gate Compliance

Tasks 1 and 2 followed the un-ignore-then-implement flow on the Wave-0 placeholder tests:
- `test_context_compressor_reset_zeroes_counter` (RED: failed asserting compression_count > 0 with the wrong fixture; GREEN after fixing the fixture to a 1000-token window that actually triggers compression).
- `test_memory_authority_header` + `test_memory_authority_reminder_constant_text` un-ignored and asserting real behavior.

## Security Gate Verification (Threat Register)

| Threat | Disposition | Verified |
|--------|-------------|----------|
| T-34b-02-DRIFT (compaction summary drops memory anchor) | mitigated | MEMORY_AUTHORITY_REMINDER embedded in the pinned [CONTEXT HISTORY] block; substring test + exact-constant pin test prevent silent drift |
| T-34b-02-RESET (stale per-session counters leak across /new) | mitigated | CLI /new zeroes the Arc<AtomicUsize> compression_count; ContextCompressor::on_session_reset zeroes its own fields (unit test proves it); gateway /new discards the session store; web stub documented |
| T-34b-02-COMPAT (trait extension breaks implementors) | mitigated | 5 hooks are additive default no-ops (check_pressure idiom); LocalPruningEngine + SummarizingEngine compile unchanged; full workspace build green |
| T-34b-02-SC (no new package installs) | accept | No dependencies added; no package-manager installs run |

## Known Stubs

- `reset_web_session` in `crates/iron_hermes_ui/src/server/state.rs` — intentional documented stub (logs a tracing::debug! note). No new-chat / `/new` trigger exists in the web UI yet; this is the accepted scope for this phase (CONTEXT Open Q1 / D-09/D-10). When a web new-chat trigger lands, this is the locus that will discard per-session state and call `on_session_reset`.

## Threat Flags

None — no new network endpoints, auth paths, file-access patterns, or trust-boundary schema changes introduced beyond the plan's threat model.

## Self-Check: PASSED

- [x] crates/ironhermes-agent/src/context_engine.rs has 5 default-no-op hooks (fn on_session_reset count = 1)
- [x] crates/ironhermes-agent/src/context_compressor.rs implements on_session_reset (count = 1)
- [x] crates/ironhermes-agent/src/summarizing_engine.rs contains "ALWAYS authoritative" (count = 4)
- [x] crates/iron_hermes_ui/src/server/state.rs has fn reset_web_session (count = 1)
- [x] Commit a25d20bb exists (Task 1)
- [x] Commit b25aa166 exists (Task 2)
- [x] Commit d87d7bf1 exists (Task 3)
- [x] Two Wave-0 placeholder tests un-ignored and asserting real behavior
- [x] cargo build --workspace --all-targets clean
- [x] All four plan-modified crates fully green

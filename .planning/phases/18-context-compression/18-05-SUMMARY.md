---
phase: 18
plan: 05
subsystem: context-compression
tags: [rust, context-compression, pressure-warning, hooks, per-session-cooldown]
requires:
  - ironhermes-hooks::HookRegistry (fire_awaitable, ContextPressure)
  - ContextEngine trait + LocalPruningEngine (18-01)
  - SummarizingEngine (18-03)
  - fire_awaitable + ContextPressure variant (18-04)
provides:
  - PressureTracker (pressure_warning.rs)
  - PRESSURE_WARNING_FRACTION = 0.85
  - TRANSIENT_WARNING_TEXT constant
  - LocalPruningEngine::with_pressure_tracker
  - SummarizingEngine::with_pressure_tracker
  - CompressionOutcome::pressure_warning_fired populated from both engines
affects:
  - crates/ironhermes-agent/src/pressure_warning.rs
  - crates/ironhermes-agent/src/context_engine.rs
  - crates/ironhermes-agent/src/summarizing_engine.rs
  - crates/ironhermes-agent/src/lib.rs
tech-stack:
  added: []
  patterns:
    - Arc<Mutex<HashMap<session_id, SessionState>>> for thread-safe per-session cooldown
    - Builder-style with_pressure_tracker() on both engines (mirrors with_hooks)
    - Async fire_awaitable for channel 2 (hook event) while channels 1+3 are sync
key-files:
  created:
    - crates/ironhermes-agent/src/pressure_warning.rs
  modified:
    - crates/ironhermes-agent/src/context_engine.rs
    - crates/ironhermes-agent/src/summarizing_engine.rs
    - crates/ironhermes-agent/src/lib.rs
decisions:
  - Per-session cooldown via HashMap<session_id, SessionState>; resets on descent below trigger
  - Pressure check runs BEFORE threshold gate in SummarizingEngine (warns even when compression not yet triggered)
  - Pressure check runs BEFORE pre_compress hook in LocalPruningEngine
  - Mutex released before async channels (tracing + hook) to avoid holding lock across await
metrics:
  duration: ~15 min
  completed: 2026-04-12
---

# Phase 18 Plan 05: Three-Channel Pressure Warning Summary

One-liner: PressureTracker emits tracing::warn!, ContextPressure hook event, and a one-shot transient system message at 85% of each engine's compression threshold, with per-session cooldown that resets on descent.

## What Shipped

- `pressure_warning.rs`: `PressureTracker` struct with `Arc<Mutex<HashMap<String, SessionState>>>` inner state.
  - `PRESSURE_WARNING_FRACTION = 0.85` (computed trigger, not hardcoded)
  - `TRANSIENT_WARNING_TEXT = "[CONTEXT PRESSURE HIGH — earlier history may soon be summarized]"`
  - `check_and_maybe_emit(session_id, engine_threshold, estimated_tokens, context_length, mode, hooks)` — fires all three channels on first crossing; cooldown suppresses re-fire; descent resets cooldown.
  - `take_transient(session_id)` — one-shot consumed semantics.
- `pub mod pressure_warning` registered in `lib.rs`.
- `LocalPruningEngine`: added `pressure_tracker: Option<Arc<PressureTracker>>` field + `with_pressure_tracker()` builder; `compress()` calls `check_and_maybe_emit` before pre_compress hook; `outcome.pressure_warning_fired` propagated.
- `SummarizingEngine`: same pattern — `pressure_tracker` field + `with_pressure_tracker()` builder; `compress()` calls `check_and_maybe_emit` before threshold gate so warnings fire even when actual compression is not yet triggered; `outcome.pressure_warning_fired` propagated.

## Commits

| Task | Commit | Message |
|------|--------|---------|
| 1 | 6575804 | feat(18-05): PressureTracker with three-channel warning + per-session cooldown |

## Tests

All 5 plan-mandated tests green:

- `pressure_warning_all_channels` — verifies all three channels fire; checks hook event payload fields.
- `pressure_cooldown` — second call above threshold suppressed; descent below trigger resets; re-crossing fires again.
- `pressure_cooldown_per_session` — sessions A and B fire and suppress independently.
- `pressure_transient_message_one_shot` — `take_transient` returns `Some` once then `None`.
- `pressure_no_warn_below_threshold` — no tracing, no hook event, no transient message at 0.40 ratio with 0.5 threshold.

Full agent test suite: **135 passed, 0 failed** (5 new + 130 baseline). Full hooks test suite: **36 passed, 0 failed**.

## Deviations from Plan

None. Plan executed exactly as written.

The only interpretation choice: in `SummarizingEngine::compress`, the pressure check is placed BEFORE the `pct < self.threshold` early-return rather than after (as the plan action step 3 implies "BEFORE the threshold-skip early-return"). This is correct per the spec — the warning fires when approaching compression, not only when compression actually executes.

## Authentication Gates

None — fully autonomous execution.

## Acceptance Criteria

All passed:
- `pub struct PressureTracker` in pressure_warning.rs — match
- `PRESSURE_WARNING_FRACTION.*0.85` in pressure_warning.rs — match
- `TRANSIENT_WARNING_TEXT` in pressure_warning.rs — match
- `check_and_maybe_emit` in context_engine.rs — match (line 131)
- `check_and_maybe_emit` in summarizing_engine.rs — match (line 231)
- `pub mod pressure_warning` in agent lib.rs — match (line 14)
- All 5 pressure tests green

## Known Stubs

None. PressureTracker is fully wired and tested; with_pressure_tracker() builders are production-ready.

## Threat Surface Scan

No new network endpoints, auth paths, or schema changes introduced. `HashMap` growth bounded at ~10K sessions × ~32 bytes = 320KB per T-18-09 (accepted in plan's threat model). Session-end cleanup deferred to Phase 21 per plan.

## Self-Check: PASSED

- FOUND: crates/ironhermes-agent/src/pressure_warning.rs
- FOUND: commit 6575804 (feat(18-05): PressureTracker...)
- cargo test -p ironhermes-agent --lib: 135 passed, 0 failed
- cargo test -p ironhermes-hooks --lib: 36 passed, 0 failed

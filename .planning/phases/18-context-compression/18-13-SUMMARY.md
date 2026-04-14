---
phase: 18-context-compression
plan: 13
subsystem: context-compression
tags: [rust, context-engine, pressure-tracker, session-id, gap-closure]
requires: [18-09, 18-11, 18-12]
provides: [pressure-tracker-cli-wiring]
affects: [ironhermes-agent, ironhermes-cli]
tech-stack:
  added: []
  patterns: [builder-method-split, independent-attachment-branches]
key-files:
  created: []
  modified:
    - crates/ironhermes-agent/src/context_engine.rs
    - crates/ironhermes-agent/src/summarizing_engine.rs
    - crates/ironhermes-agent/src/engine_factory.rs
    - crates/ironhermes-agent/src/pressure_warning.rs
decisions:
  - "with_hooks split into with_hooks(registry) + with_session_id(sid) — session_id is now independent of hook registry presence"
  - "engine_factory uses three separate builder branches: .with_session_id unconditional, tracker/hooks each gated on Some independently"
  - "was_warned() accessor added to PressureTracker as cfg(test)-only — avoids log-capture flakiness in tests"
  - "UAT Test 4 left as blocked — live CLI re-run requires interactive session not available to autonomous executor"
metrics:
  duration_min: 45
  completed_date: "2026-04-14"
  tasks_completed: 4
  tasks_total: 5
  files_modified: 4
requirements: [PRMT-13, PRMT-14]
---

# Phase 18 Plan 13: Decouple Pressure Tracker from Hook Registry — Summary

## One-liner

Surgical split of `with_hooks(registry, sid)` into `with_hooks(registry)` + `with_session_id(sid)` on both engines, with engine_factory rewired to three independent builder branches so `PressureTracker` fires under the CLI default path (`hooks=None`).

## What Was Built

### Root Cause Fixed

`engine_factory.rs` combined `session_id` and `PressureTracker` attachment with hook registry attachment behind a single guard:

```rust
if let (Some(h), Some(t)) = (hooks, tracker) {
    e = e.with_hooks(h, sid).with_pressure_tracker(t);
}
```

When `hooks=None` (CLI default via `ironhermes-cli/src/main.rs:303`), this guard short-circuited and neither `session_id` nor `tracker` reached the engine. The pressure gate in `context_engine.rs:144` — `if let (Some(tracker), Some(sid))` — then silently no-oped on every `compress` call, meaning `WARN ... context pressure warning` was never emitted and `[CONTEXT PRESSURE HIGH]` was never injected.

### Changes Applied

**`crates/ironhermes-agent/src/context_engine.rs`**
- `LocalPruningEngine::with_hooks(registry, session_id)` split into:
  - `with_hooks(mut self, registry: Arc<HookRegistry>) -> Self`
  - `with_session_id(mut self, session_id: impl Into<String>) -> Self`
- Existing tests updated: `.with_hooks(reg, "sess")` → `.with_hooks(reg).with_session_id("sess")`
- New test: `pressure_check_fires_when_session_id_attached_without_hooks` — uses `check_pressure` directly with stats placing ratio in the 85% band; asserts `tracker.was_warned("sess-test-1")`.

**`crates/ironhermes-agent/src/summarizing_engine.rs`**
- `SummarizingEngine::with_hooks(registry, session_id)` split into:
  - `with_hooks(mut self, registry: Arc<HookRegistry>) -> Self`
  - `with_session_id(mut self, session_id: impl Into<String>) -> Self`
- New test: `pressure_check_fires_on_summarizing_engine_without_hooks` — uses `check_pressure` directly with `estimated_tokens=46_000` (ratio 0.46, above 85% trigger of 0.425); asserts `fired=true` and `tracker.was_warned`.

**`crates/ironhermes-agent/src/engine_factory.rs`**
- Replaced combined guard in `build_local` closure with three independent branches:
  ```rust
  let mut e = LocalPruningEngine::new(context_length, threshold)
      .with_protect(protect_first, protect_last)
      .with_tool_pair_shift(shift)
      .with_session_id(sid);             // unconditional
  if let Some(t) = tracker { e = e.with_pressure_tracker(t); }
  if let Some(h) = hooks   { e = e.with_hooks(h); }
  ```
- Same pattern applied to `summarizing` arm.

**`crates/ironhermes-agent/src/pressure_warning.rs`**
- Added `#[cfg(test)] pub fn was_warned(&self, session_id: &str) -> bool` — reads `above_threshold` from inner map for test assertions without log-capture.

## Test Results

```
running 183 tests
test result: ok. 183 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s
```

All prior 18-10/18-11/18-12 tool-pair atomicity and summary sentinel tests pass unchanged. Two new gap-closure tests added and green.

## Verification Checks

- `rg -n "if let \(Some\(h\), Some\(t\)\)" crates/ironhermes-agent/src/engine_factory.rs` → zero code hits (only in comment)
- `rg -n "with_session_id" crates/ironhermes-agent/src/` → 15 hits (two impls, two factory call sites, test call sites)
- `attach_context_engine_wires_all_three_builders` test in `agent_wiring.rs` continues to pass with `hooks=None`

## Task 5 — Live CLI UAT Re-run: BLOCKED

Cannot be executed autonomously. The autonomous executor cannot run an interactive CLI session, drive a tool-heavy conversation, or observe live log output.

**Instructions for manual re-run:**

1. Set `agent.compression_threshold = 0.05` in your config file.
2. Launch the CLI agent: `cargo run -p ironhermes-cli`.
3. Drive a tool-heavy conversation (web_read, file reads) until the session token ratio climbs past `0.0425` (85% of 0.05 threshold).
4. Confirm the log contains:
   ```
   WARN ... context pressure warning (85% of compression threshold) session_id=<your-session>
   ```
5. Confirm the transient `[CONTEXT PRESSURE HIGH — earlier history may soon be summarized]` message is injected exactly once per descent-then-ascent cycle.
6. If both confirmed → update `18-UAT.md` Test 4 from `blocked` → `pass`.

**Do NOT flip UAT Test 4 to `pass` based on unit tests alone.**

## Deviations from Plan

### Auto-fixed Issues

None — plan executed as written with one implementation adjustment.

### Implementation Adjustment (Not a Deviation)

The plan's RED test for `LocalPruningEngine` used `compress()` with a fabricated message vec sized to land in the pressure band. The actual `estimate_messages_tokens` estimator uses `text.len() / 4 + 1`, which made the original message count (1,150 messages × 40 chars) yield ratio=0.98 (above the 0.50 compression threshold), causing the engine to actually run compression rather than just check pressure.

Adjusted approach: used `engine.check_pressure(&stats)` directly with `estimated_tokens=46_000` (ratio=0.46, above the 85% trigger at 0.425). This is equivalent — `check_pressure` calls the same `tracker.check_and_maybe_emit` path as the pressure gate in `compress`. The assertion remains on tracker state, not logs (per plan note).

## Known Stubs

None — no data stubs introduced by this plan.

## Threat Flags

None — no new network endpoints, auth paths, or trust-boundary changes introduced.

## Self-Check: PASSED

- `crates/ironhermes-agent/src/context_engine.rs` — exists, contains `with_session_id`
- `crates/ironhermes-agent/src/summarizing_engine.rs` — exists, contains `with_session_id`
- `crates/ironhermes-agent/src/engine_factory.rs` — exists, contains `with_session_id`, no combined guard
- `crates/ironhermes-agent/src/pressure_warning.rs` — exists, contains `was_warned`
- Commit `7b25073` — test(18-13): RED phase
- Commit `58fa0c5` — feat(18-13): engine_factory rewire (GREEN)
- 183/183 tests pass

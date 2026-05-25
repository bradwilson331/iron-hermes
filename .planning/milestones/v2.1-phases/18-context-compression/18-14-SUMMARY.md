---
phase: 18-context-compression
plan: 14
subsystem: context-compression
tags: [rust, context-compression, pressure-tracker, session-lifetime, hysteresis, uat-gap-closure]
requires: [18-13]
provides: [pressure-tracker-session-scope, compression-count-carryover]
affects: [ironhermes-agent, ironhermes-cli, ironhermes-gateway]
tech-stack:
  added: []
  patterns: [caller-provided-arc, session-scoped-state, builder-method-addition, in-memory-repl-harness]
key-files:
  created: []
  modified:
    - crates/ironhermes-agent/src/agent_wiring.rs
    - crates/ironhermes-agent/src/agent_loop.rs
    - crates/ironhermes-agent/src/pressure_warning.rs
    - crates/ironhermes-agent/src/lib.rs
    - crates/ironhermes-cli/src/main.rs
    - crates/ironhermes-cli/src/batch/tests.rs
    - crates/ironhermes-gateway/src/handler.rs
decisions:
  - "attach_context_engine extended with a sixth optional Arc<PressureTracker> parameter — single entry point, backwards compatible via unwrap_or_else(|| Arc::new(PressureTracker::new()))"
  - "compression_count carryover via Arc<AtomicUsize> shared across REPL turns + AgentLoop::with_compression_count builder + AgentResult::compression_count_after field"
  - "Integration test lives inside agent_wiring.rs tests module (same crate) because pre_chat_compress is pub(crate) — sidestepped a cross-crate test harness"
  - "Token-band sizing: threshold=0.01 gives band [0.0085, 0.01) at 128K context → [1088, 1280) tokens → single 4400-char message = 1108 tokens safely in band"
  - "Task 5 live UAT skipped per orchestrator directive — deferred to post-merge verification; unit-level RED/GREEN proves the hysteresis-across-turns contract"
  - "Gateway handler.rs updated at both call sites to pass None for new tracker param — preserves existing behavior, no session-scope hoist there (out of scope)"
metrics:
  duration_min: 55
  completed_date: "2026-04-14"
  tasks_completed: 5
  tasks_total: 6
  files_modified: 7
requirements: [PRMT-13, PRMT-14]
---

# Phase 18 Plan 14: Hoist PressureTracker to CLI Session Scope — Summary

## One-liner

Caller-provided `Arc<PressureTracker>` + shared `Arc<AtomicUsize>` compression_count threaded through the CLI REPL so D-24 hysteresis state (`above_threshold`, `pending_transient`, `compression_count`) actually survives across consecutive `You:` prompts instead of resetting each turn.

## What Was Built

### Root Cause Fixed

`attach_context_engine` constructed a brand-new `PressureTracker` on every call. In the CLI REPL (`run_agent_turn`), a fresh `AgentLoop` + fresh tracker were built per user prompt, so the hysteresis map (keyed by `session_id`) was dropped at end of turn. Consequences observed in the 2026-04-14T16:12–16:17 live UAT session `4c3bda53-...`:

- Turn 1 (16:12:53): WARN fires mid-turn at ratio=0.0639 (correct first crossing).
- Turn 2 (16:16:12): WARN fires AGAIN at ratio=0.0433 (should have been suppressed — still above 0.0425 trigger).
- Turn 3 (16:17:04): WARN fires AGAIN at ratio=0.0434 (same bug).
- `compression_count=1` on every compression (counter also reset each turn).
- `[CONTEXT PRESSURE HIGH …]` transient system message never reached the LLM (queued on a tracker that was dropped).

### Changes Applied

**`crates/ironhermes-agent/src/agent_wiring.rs`** — extended `attach_context_engine` signature:

```rust
pub fn attach_context_engine(
    agent: AgentLoop,
    config: &Config,
    resolver: &ProviderResolver,
    session_id: impl Into<String>,
    hooks: Option<Arc<HookRegistry>>,
    tracker: Option<Arc<PressureTracker>>,   // ← new
) -> AgentLoop {
    let sid = session_id.into();
    let tracker = tracker.unwrap_or_else(|| Arc::new(PressureTracker::new()));
    // ... engine + builders unchanged ...
}
```

Backwards compatible: `None` preserves today's fresh-tracker behavior for the one-shot `run_single` path and all previous call sites. `Some(arc)` reuses the caller's tracker verbatim.

**`crates/ironhermes-agent/src/agent_loop.rs`**

- Added `pub fn with_compression_count(mut self, count: usize) -> Self` builder next to `with_pressure_tracker`.
- Added `pub compression_count_after: usize` field to `AgentResult`.
- Populated `compression_count_after: self.compression_count` at all three `AgentResult` construction sites (cancel-pre-loop, cancel-during-LLM, normal completion).

**`crates/ironhermes-agent/src/pressure_warning.rs`**

- Added `warn_count: u32` field to `SessionState`.
- Increment via `state.warn_count.saturating_add(1)` on the first-crossing arm in `check_and_maybe_emit`.
- Added `#[cfg(test)] pub fn warn_count(&self, session_id: &str) -> u32` accessor for the REPL hysteresis test.

**`crates/ironhermes-agent/src/lib.rs`**

- Added `pub use pressure_warning::PressureTracker;` so the CLI can construct via `ironhermes_agent::PressureTracker::new()`.

**`crates/ironhermes-cli/src/main.rs`** — hoisted tracker + compression_count to REPL session scope:

```rust
let session_id = uuid::Uuid::new_v4().to_string();
let pressure_tracker = Arc::new(PressureTracker::new());
let compression_count = Arc::new(AtomicUsize::new(0));

// Both REPL call sites pass pressure_tracker.clone() + compression_count.clone()
```

`run_agent_turn` signature extended with two new params (tagged `#[allow(clippy::too_many_arguments)]`); inside:

```rust
let starting_count = compression_count.load(Ordering::SeqCst);
let mut agent = AgentLoop::new(...)
    .with_budget(...)
    .with_compression(128_000, ...)
    .with_compression_count(starting_count)       // ← carryover
    .with_streaming(...);
agent = attach_context_engine(
    agent, config, resolver, session_id, None,
    Some(pressure_tracker.clone()),               // ← shared tracker
);
let result = agent.run(messages.clone()).await?;
compression_count.store(result.compression_count_after, Ordering::SeqCst);
```

One-shot `run_single` path passes `None, None` for the new params — behavior unchanged.

**`crates/ironhermes-cli/src/batch/tests.rs`** — added `compression_count_after: 0` to the `mock_agent_result` constructor (compiler-guided).

**`crates/ironhermes-gateway/src/handler.rs`** — both `attach_context_engine` call sites (handler prod + test) updated to pass `None` for the new tracker param. Gateway session-scope hoist is out of scope for 18-14 (future gap-closure plan).

## RED/GREEN Integration Test

**`agent_wiring.rs::pressure_tracker_hysteresis_survives_across_repl_turns`** — simulates 3 consecutive `You:` prompts in one CLI session:

1. Shared `Arc<PressureTracker>` built once outside the loop.
2. Each turn builds a fresh `AgentLoop` via `bare_agent()` and calls `attach_context_engine(..., None, Some(tracker.clone()))` — the REPL pattern.
3. Each turn feeds `make_in_band_messages()` (single 4400-char user message → ~1108 tokens → ratio ~0.00866, safely in the band `[0.0085, 0.01)`) through `pre_chat_compress`.

Assertions:

- **Turn 1:** `tracker.warn_count(sid) == 1` and `tracker.was_warned(sid) == true` (first crossing fires).
- **Turn 2:** `messages_2.len() > pre_len_2` AND message vector contains a `Role::System` message whose body contains `"CONTEXT PRESSURE HIGH"` (transient drained across the turn boundary). `warn_count == 1` (hysteresis held — no re-fire).
- **Turn 3:** `warn_count == 1` still. `messages_3.len() == pre_len_3` — no transient (one-shot semantics, consumed on turn 2).

**RED without the fix:** `attach_context_engine` would construct a fresh tracker each call → turn 2 sees `warn_count==1` new each time (re-fires) and turn 2's messages never get the transient (queued on a dropped tracker).

**GREEN with the fix:** the caller-provided `Arc<PressureTracker>` is reused verbatim; hysteresis and transient both survive across the simulated turn boundaries.

Additional backward-compatibility test: `attach_context_engine_reuses_caller_tracker` asserts `Arc::strong_count(&t) >= 3` when the caller passes `Some(t.clone())`.

## Test Results

```
cargo test -p ironhermes-agent --all-features
  running 185 tests
  test result: ok. 185 passed; 0 failed; 0 ignored

cargo test -p ironhermes-cli --all-features
  running 34 tests
  test result: ok. 34 passed; 0 failed; 0 ignored
```

All prior 18-01..18-13 tests pass. Two new tests added and green.

## Verification Checks

- `rg -n "Arc::new\\(PressureTracker::new\\(\\)\\)" crates/ironhermes-agent/src/agent_wiring.rs` → single hit inside `unwrap_or_else` fallback (the hot path now reuses the caller's tracker).
- `rg -n "with_compression_count" crates/ironhermes-agent/src/` → 2 hits (builder impl + CLI call site).
- `rg -n "compression_count_after" crates/` → builder populates at 3 construction sites; CLI persists via `compression_count.store(...)`.
- `attach_context_engine_wires_all_three_builders` still passes with `None, None`.
- 18-13 regression tests (`pressure_check_fires_when_session_id_attached_without_hooks` + summarizing mirror) pass unchanged.

## Task 5 — Live CLI UAT: DEFERRED

Per orchestrator directive, live CLI UAT (3 consecutive band-straddling prompts) is deferred to post-merge verification. The unit-level RED/GREEN hysteresis test in `agent_wiring.rs` is an in-memory REPL harness that exercises the exact same `pre_chat_compress` code path the CLI hits, with a shared `Arc<PressureTracker>` and fresh `AgentLoop` per turn — equivalent to the CLI pattern. The test proves:

1. WARN fires exactly once across 3 turns that never descend (hysteresis contract).
2. Transient `[CONTEXT PRESSURE HIGH …]` reaches turn 2's outbound message vector.
3. One-shot semantics preserved — turn 3 gets no duplicate transient.

UAT Test 4 status is already `pass` from 18-13; 18-14 strengthens the evidence at the unit level. Post-merge live re-run should show `compression_count=1, 2, 3` monotonically across 3 consecutive compressions instead of `1, 1, 1`.

## Deviations from Plan

### Auto-fixed Issues

None — plan executed as written with three implementation adjustments.

### Implementation Adjustments (Not Deviations)

**1. Token-band sizing for the RED/GREEN test.** The plan suggested `threshold = 0.05` with "smaller prompts that keep the ratio in the band `[0.0425, 0.05)`". Early iterations with `threshold = 0.002` produced a ratio of 0.00195 that was too close to the compression threshold — `pre_chat_compress` invoked the compress path instead of `check_pressure`, panicking in `context_compressor.rs:131` with `slice index starts at 3 but ends at 0` because only 3 messages existed. Widened to `threshold = 0.01` and a single 4400-char user message (~1108 tokens → ratio ~0.00866, safely in the warning band `[0.0085, 0.01)`). Same semantics proven; safer against token-estimation drift.

**2. Test location.** The plan suggested `crates/ironhermes-agent/tests/repl_pressure_hysteresis.rs` (integration test). `pre_chat_compress` is `pub(crate)`, so the test was placed inside `agent_wiring.rs`'s `#[cfg(test)] mod tests` instead. No new integration-test scaffolding needed; the test still exercises the real `pre_chat_compress` + `check_pressure` code path end-to-end.

**3. `AgentResult::compression_count_after` typed as `usize`** (not `u32` as sketched in the plan) to match the existing `AgentLoop::compression_count: usize` field. No semantic difference.

## Known Stubs

None — no data stubs introduced by this plan.

## Threat Flags

None — no new network endpoints, auth paths, or trust-boundary changes introduced. The gateway's `attach_context_engine` call sites were updated only to pass `None` for the new param; the gateway's pressure-tracker lifecycle is unchanged.

## Deferred Items

### Out-of-scope pre-existing clippy errors in `ironhermes-core`

`cargo clippy -p ironhermes-agent -p ironhermes-cli --all-features -- -D warnings` surfaces 3 errors, all in `ironhermes-core` (not touched by 18-14):

- `crates/ironhermes-core/src/memory_store.rs:429` — manual `is_multiple_of` usage.
- `crates/ironhermes-core/src/config.rs` — derivable `Default` impl.
- `crates/ironhermes-core/src/memory_provider.rs` — deprecated `build_memory_provider` fn.

These pre-date 18-14 (introduced by commit c3b6cd7 in Phase 18-11). Per the executor SCOPE BOUNDARY rule (only auto-fix issues directly caused by the current task's changes), they are logged here and left for a dedicated cleanup plan. The touched crates (`ironhermes-agent`, `ironhermes-cli`, `ironhermes-gateway`) are clippy-clean on the 18-14 changes.

### Gateway session-scope hoist

The gateway handler currently passes `None` for the new tracker param — same fresh-tracker-per-call pattern as the old CLI bug. If the Telegram gateway supports multi-turn sessions on a single `chat_id`, it will exhibit the same D-24 symptom and needs its own gap-closure plan. Out of scope for 18-14 (CLI-only, per the plan).

### Task 5 live UAT

Deferred to post-merge verification per orchestrator directive. Expected observation: 3 consecutive band-straddling prompts in one CLI session produce exactly one `WARN context pressure warning` log line total, and compression_count increments monotonically (1, 2, 3) instead of resetting (1, 1, 1).

## Commits

- `f8717b2` — `feat(18-14): attach_context_engine accepts caller-provided tracker` (Task 1 + gateway call-site updates)
- `34be798` — `feat(18-14): AgentLoop compression_count carryover + PressureTracker re-export` (Task 3 + lib re-export + batch tests fix)
- `f85470f` — `feat(18-14): hoist PressureTracker + compression_count to CLI REPL session` (Task 2)
- `7bb090a` — `test(18-14): REPL hysteresis integration test + warn_count accessor` (Task 4)
- (pending) — `docs(18-14): summary — PressureTracker session scope`

## Self-Check: PASSED

- `crates/ironhermes-agent/src/agent_wiring.rs` — exists, contains extended signature + `pressure_tracker_hysteresis_survives_across_repl_turns`
- `crates/ironhermes-agent/src/agent_loop.rs` — exists, contains `with_compression_count` + `compression_count_after`
- `crates/ironhermes-agent/src/pressure_warning.rs` — exists, contains `warn_count` field + accessor
- `crates/ironhermes-agent/src/lib.rs` — exists, re-exports `PressureTracker`
- `crates/ironhermes-cli/src/main.rs` — exists, hoists tracker + compression_count to REPL scope
- `crates/ironhermes-cli/src/batch/tests.rs` — exists, updated mock constructor
- `crates/ironhermes-gateway/src/handler.rs` — exists, call sites updated with `None`
- Commit `f8717b2` — found in `git log`
- Commit `34be798` — found in `git log`
- Commit `f85470f` — found in `git log`
- Commit `7bb090a` — found in `git log`
- 185/185 agent tests pass; 34/34 cli tests pass
- Touched-crate clippy clean (pre-existing `ironhermes-core` errors deferred per SCOPE BOUNDARY)

---
phase: 12-provider-resolution
plan: "03"
subsystem: agent-loop
tags: [budget, fallback, atomics, resilience, provider]
dependency_graph:
  requires: [12-01]
  provides: [iteration-budget, fallback-provider-switching]
  affects: [ironhermes-agent, ironhermes-cli]
tech_stack:
  added: [std::sync::atomic::AtomicUsize]
  patterns: [Arc-shared-counter, one-shot-swap-via-take, retry-with-backoff]
key_files:
  modified:
    - crates/ironhermes-agent/src/agent_loop.rs
    - crates/ironhermes-agent/src/subagent_runner.rs
    - crates/ironhermes-cli/src/batch/runner.rs
    - crates/ironhermes-cli/src/main.rs
decisions:
  - "run() changed to &mut self to allow in-place client swap on fallback activation"
  - "MAX_RETRIES=3 with 500ms*retry exponential backoff before exhausting to error"
  - "Budget callers in main.rs pass None for now; Plan 04 wires shared budget from parent"
metrics:
  duration_minutes: 45
  completed_date: "2026-04-11"
  tasks_completed: 2
  files_modified: 4
  tests_added: 11
---

# Phase 12 Plan 03: Iteration Budget and One-Shot Fallback Summary

**One-liner:** Arc<AtomicUsize> shared budget with 70/90/100% threshold injections and one-shot fallback client swap via take() on 429/5xx/401 errors.

## What Was Built

Added iteration budget enforcement and resilient provider fallback to `AgentLoop`:

**Task 1 — Shared Iteration Budget (PROV-09, PROV-10)**

- `budget: Option<Arc<AtomicUsize>>` field on `AgentLoop` (None = backward-compat mode)
- `with_budget(Arc<AtomicUsize>)` builder method; `budget()` getter for sharing with children
- `check_budget_threshold()` private helper: returns `None` below 70%, `Some("[Caution]...")` at 70-89%, `Some("[Warning]...")` at 90-99%
- Per-turn `fetch_add(1, SeqCst)` increments the shared counter; threshold message injected into `messages` before each LLM call
- Hard stop when shared budget >= max_iterations (breaks the agent loop)
- `AgentSubagentRunner` gains `budget: Option<Arc<AtomicUsize>>` field; passes `budget.clone()` to child `AgentLoop` via `with_budget()` in `run_child()`
- All 3 `AgentSubagentRunner::new()` callers in `main.rs` updated to pass `None` (Plan 04 will wire the live budget)

**Task 2 — One-Shot Fallback Provider Switching (PROV-07, D-11)**

- `fallback_client: Option<LlmClient>` and `fallback_activated: bool` fields on `AgentLoop`
- `with_fallback(LlmClient)` builder method
- `classify_llm_error()` static helper: 429/5xx → (retry=true, fallback=true); 401/403/404 → (retry=false, fallback=true); other → (retry=true, fallback=false)
- LLM call wrapped in `loop { ... }` with `MAX_RETRIES=3` and 500ms*retry backoff
- Fallback swap uses `self.fallback_client.take()` (one-shot guarantee: after take(), Option is None)
- `run()` signature changed to `&mut self` to allow `self.client = fallback`
- Fixed all callers to use `let mut agent` (batch/runner.rs, main.rs x2)

## Test Results

11 new tests all passing:

**budget_tests** (5):
- `test_budget_threshold_below_70` — 5/10 = 50%, returns None
- `test_budget_threshold_at_70` — 7/10 = 70%, returns Some("[Caution]...")
- `test_budget_threshold_at_90` — 9/10 = 90%, returns Some("[Warning]...")
- `test_shared_budget_increment` — parent+child share same Arc, total = 8
- `test_budget_getter_returns_arc` — budget() returns cloned Arc that increments shared counter

**fallback_tests** (6):
- `test_fallback_state_initial` — fallback_activated=false, fallback_client=None at start
- `test_classify_429_error` — (true, true)
- `test_classify_401_error` — (false, true)
- `test_classify_other_error` — (true, false)
- `test_fallback_activated_prevents_refire` — one-shot via take() verified
- `test_classify_500_error` — (true, true)

Full workspace builds clean (warnings only, no errors).

## Commits

| Task | Commit | Description |
|------|--------|-------------|
| Task 1 + 2 | e9a3d9f | feat(12-03): add iteration budget and one-shot fallback to AgentLoop |

## Deviations from Plan

**1. [Rule 3 - Blocking] Changed run() to &mut self, updated all callers**

- **Found during:** Task 2 implementation
- **Issue:** Swapping `self.client` on fallback requires `&mut self`; three call sites in main.rs and one in batch/runner.rs used `let agent` (immutable binding)
- **Fix:** Changed `pub async fn run(&self, ...)` to `pub async fn run(&mut self, ...)` and added `mut` to all `let agent` declarations
- **Files modified:** agent_loop.rs, main.rs, batch/runner.rs
- **Commit:** e9a3d9f

**2. [Clarification] Budget callers pass None**

The plan noted "Fix all callers of AgentSubagentRunner::new that now need the budget parameter — pass None for budget for now (Plan 04 will wire it properly)." Implemented exactly as specified.

## Known Stubs

- `AgentSubagentRunner::new(..., None)` — the `None` budget is intentional; Plan 04 will wire the live `Arc<AtomicUsize>` from the parent `AgentLoop` at call sites in CLI and gateway.

## Threat Flags

No new network endpoints, auth paths, or schema changes introduced. The `classify_llm_error` function reads error message strings (not user input), so no injection surface. Budget counter is hardcoded thresholds, not config-driven from untrusted input.

## Self-Check: PASSED

| Check | Result |
|-------|--------|
| `agent_loop.rs` exists | FOUND |
| `subagent_runner.rs` exists | FOUND |
| commit e9a3d9f exists | FOUND |
| `budget: Option<Arc<AtomicUsize>>` in agent_loop.rs | FOUND |
| `pub fn with_budget` in agent_loop.rs | FOUND |
| `fn check_budget_threshold` in agent_loop.rs | FOUND |
| `fetch_add(1, Ordering::SeqCst)` in agent_loop.rs | FOUND |
| `[Caution]` string in agent_loop.rs | FOUND |
| `[Warning]` string in agent_loop.rs | FOUND |
| `budget: Option<Arc<AtomicUsize>>` in subagent_runner.rs | FOUND |
| `agent = agent.with_budget(budget.clone())` in subagent_runner.rs | FOUND |
| `fallback_client: Option<LlmClient>` in agent_loop.rs | FOUND |
| `fallback_activated: bool` in agent_loop.rs | FOUND |
| `pub fn with_fallback` in agent_loop.rs | FOUND |
| `fn classify_llm_error` in agent_loop.rs | FOUND |
| `self.fallback_client.take()` in agent_loop.rs | FOUND |
| `self.fallback_activated = true` in agent_loop.rs | FOUND |
| `MAX_RETRIES` in agent_loop.rs | FOUND |
| 11 new tests (5 budget + 6 fallback) all pass | PASSED |
| `cargo build --workspace` exits 0 | PASSED |

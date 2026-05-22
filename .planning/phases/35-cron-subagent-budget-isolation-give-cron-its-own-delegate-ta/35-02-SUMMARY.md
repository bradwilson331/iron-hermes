---
phase: 35-cron-subagent-budget-isolation-give-cron-its-own-delegate-ta
plan: "02"
subsystem: ironhermes-agent
tags: [budget, prov-10, subagent, delegation, tdd]
dependency_graph:
  requires: []
  provides: [per-child-independent-budget, prov-10-retired, d07.1-independence-test]
  affects: [ironhermes-agent, subagent_runner, agent_loop, agent_runtime, budget]
tech_stack:
  added: []
  patterns: [fresh-BudgetHandle-per-child, source-include-guard, independence-regression-test]
key_files:
  created: []
  modified:
    - crates/ironhermes-agent/src/agent_loop.rs
    - crates/ironhermes-agent/src/subagent_runner.rs
    - crates/ironhermes-agent/src/agent_runtime.rs
    - crates/ironhermes-agent/src/budget.rs
decisions:
  - "Field-disposition: keep AgentSubagentRunner.budget field and Option<BudgetHandle> param on new(); add #[allow(dead_code)] annotation — two production sites and grep invariants depend on the signature"
  - "Renamed runner_shares_budget_arc test to runner_stores_budget_field_children_get_fresh_handle to accurately describe the post-retirement contract"
  - "Independence source-guard uses include_str!(subagent_runner.rs) in agent_runtime.rs test to cross-verify the change site without a full from_config round-trip"
metrics:
  duration: "~30 minutes"
  completed: "2026-05-22T03:17:41Z"
  tasks_completed: 2
  tasks_total: 2
  files_modified: 4
---

# Phase 35 Plan 02: Retire PROV-10 Shared Counter — Per-Child Fresh BudgetHandle Summary

Each child subagent loop now receives a fresh `BudgetHandle::new(max_iterations)` at the `AgentSubagentRunner::run_child` change site, replacing the `budget.clone()` that shared the parent's `Arc<AtomicUsize>`. PROV-10 shared parent↔child counter is fully retired across doc-comments and regression tests.

## Tasks Completed

| Task | Description | Commit | Files |
|------|-------------|--------|-------|
| 1 | Add D-07.1 independence regression test in agent_loop.rs | 452b40d0 | agent_loop.rs |
| 2 | Swap change site to fresh budget; rewrite PROV-10 docs/assertions | f1bf51f7 | subagent_runner.rs, agent_loop.rs, agent_runtime.rs, budget.rs |

## What Was Built

### Task 1 — D-07.1 Independence Test (TDD RED/GREEN)

Added `test_independent_budget_child_drain_does_not_affect_parent` in `agent_loop.rs::budget_tests`. The test:
- Constructs two separate `BudgetHandle::new(max)` instances (parent, child)
- Drains the child to exhaustion with `max` `consume()` calls
- Asserts `child.remaining() == 0` AND `parent.remaining() == max`

This locks the independence guarantee that Task 2 must preserve. The test passed immediately against the existing API (two fresh handles are inherently independent — the test name contains `independent_budget` for filter selection).

Also updated `test_shared_budget_increment` doc to remove PROV-10 parent/child framing while preserving the clone-sharing API description (still truthful for gateway/CommandContext use).

### Task 2 — Change Site Swap + PROV-10 Retirement

**subagent_runner.rs (change site ~283-284):** Replaced:
```rust
if let Some(ref budget) = self.budget {
    agent = agent.with_budget(budget.clone());
}
```
with:
```rust
agent = agent.with_budget(BudgetHandle::new(max_iterations));
```

**subagent_runner.rs field doc (34-39):** Rewrote to state PROV-10 is retired, each child gets a fresh handle, and the field is retained for the `new()` signature and grep invariants. Added `#[allow(dead_code)]` with explanatory comment (field is written by `new` but no longer read by `run_child`).

**agent_loop.rs doc-comments (147, 552, 563):** Rewrote to describe the agent's OWN budget; removed "shared with child agents (PROV-10)" framing. `budget()` getter note updated.

**budget.rs module doc (1-7):** Rewrote to describe per-agent independent budgets; explicitly notes PROV-10 retirement and that `BudgetHandle::clone` is preserved for gateway/CommandContext/reset visibility.

**budget.rs struct doc (12-14):** Softened parent/child framing; clone-sharing description kept truthful.

**agent_runtime.rs module doc (21-26):** Rewrote "Budget sharing (PROV-10)" section to "Budget (top-level / interactive, D-15)"; describes PROV-10 retirement and that children get fresh handles.

**agent_runtime.rs comment at ~146:** Updated from "clone of the SHARED budget (PROV-10)" to describe storage-only purpose.

**agent_runtime.rs run_turn doc (~200-203):** Updated to remove "share the just-reset counter via the runner's Arc" framing.

**agent_runtime.rs test (447-487):** Renamed `runner_shares_budget_arc` to `runner_stores_budget_field_children_get_fresh_handle`. Rewrote:
- Doc-comment to describe storage + independence contract
- Removed assertion claiming parent/child sharing
- Added `include_str!("subagent_runner.rs")` cross-file guard asserting `BudgetHandle::new(max_iterations)` is present and `agent = agent.with_budget(budget.clone())` is absent at the change site

## Verification Results

All acceptance criteria met:

| Criterion | Result |
|-----------|--------|
| `with_budget(BudgetHandle::new(max_iterations))` in non-comment subagent_runner.rs | 1 match |
| `budget.clone()` absent from subagent_runner.rs | 0 matches |
| Field doc no longer contains "decrement the SAME budget" | 0 matches |
| `Ordering::SeqCst` present in budget.rs | 4 matches |
| `Ordering::Relaxed` absent from budget.rs | 0 matches |
| No parent/child sharing assertion in agent_runtime.rs | 0 matches |
| `cargo test -p ironhermes-agent --test budget_ordering_grep` | green |
| `cargo test -p ironhermes-agent independent_budget` | green |
| `cargo test -p ironhermes-agent budget` (25 tests) | green |
| `cargo build -p ironhermes-agent` | clean |

## Deviations from Plan

### Auto-fixed Issues

None.

### Intentional Adjustments

**1. [Planner Discretion — Field kept] `#[allow(dead_code)]` on budget field**
- The plan said to add the annotation "if and only if the compiler flags it"
- The compiler did flag it as dead code (field is written by `new` but run_child no longer reads it)
- Applied `#[allow(dead_code)]` with explanatory comment as instructed
- Files modified: subagent_runner.rs

**2. [Plan 35-02 discretion] runner_shares_budget_arc test renamed**
- Plan said to "rewrite" the test; renamed to `runner_stores_budget_field_children_get_fresh_handle`
- The old name was factually incorrect post-retirement; renaming avoids misleading future readers
- Source-grep assertions that the old test relied on (`Some(budget.clone())`, `budget,`) are preserved (they test the field-kept wiring, which still exists)
- Added `include_str!` cross-file guard as the primary independence assertion

## Known Stubs

None — all behavior changes are fully wired and tested.

## Threat Flags

None — no new network endpoints, auth paths, file access patterns, or schema changes introduced. T-28.1-16 DoS vector (cron subagents draining interactive headroom via shared counter) is eliminated by this plan's change site swap.

## Self-Check: PASSED

- `/Users/twilson/code/ironhermes/.claude/worktrees/agent-a48258db2e268d99a/crates/ironhermes-agent/src/agent_loop.rs` — FOUND (contains `independent_budget`)
- `/Users/twilson/code/ironhermes/.claude/worktrees/agent-a48258db2e268d99a/crates/ironhermes-agent/src/subagent_runner.rs` — FOUND (contains `BudgetHandle::new(max_iterations)`)
- `/Users/twilson/code/ironhermes/.claude/worktrees/agent-a48258db2e268d99a/crates/ironhermes-agent/src/agent_runtime.rs` — FOUND (contains `runner_stores_budget_field_children_get_fresh_handle`)
- `/Users/twilson/code/ironhermes/.claude/worktrees/agent-a48258db2e268d99a/crates/ironhermes-agent/src/budget.rs` — FOUND (contains updated module doc)
- Commit 452b40d0 — FOUND (test(35-02): add D-07.1 independence regression test)
- Commit f1bf51f7 — FOUND (feat(35-02): retire PROV-10; each child subagent gets fresh BudgetHandle::new)

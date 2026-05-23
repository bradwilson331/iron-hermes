---
phase: 35-cron-subagent-budget-isolation-give-cron-its-own-delegate-ta
verified: 2026-05-21T00:00:00Z
status: passed
score: 7/7
overrides_applied: 0
re_verification: null
gaps: []
human_verification: []
---

# Phase 35: Per-Subagent Independent Iteration Budgets — Verification Report

**Phase Goal:** Retire the PROV-10 shared parent↔child iteration counter globally. Every subagent (interactive AND cron) receives a fresh `BudgetHandle::new(config.delegation.max_iterations)` instead of cloning the parent's Arc. `config.delegation.max_iterations` is a hard ceiling on caller/model-supplied `max_iterations` (clamp-to-ceiling, D-03). Regression tests prove independence. Design doc §6.4/§8 amended. T-28.1-16 resolved as a consequence.

**Verified:** 2026-05-21  
**Status:** GOAL ACHIEVED  
**Re-verification:** No — initial verification

---

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Each child subagent loop receives a FRESH `BudgetHandle::new(max_iterations)`, not a clone of the parent's shared budget | VERIFIED | `subagent_runner.rs:295`: `agent = agent.with_budget(BudgetHandle::new(max_iterations));` — unconditional, no `budget.clone()` at this site |
| 2 | D-01/D-02: the change is global — the old `budget.clone()` at the change site is completely gone | VERIFIED | `grep "budget.clone()" crates/ironhermes-agent/src/subagent_runner.rs` returns 0 matches |
| 3 | D-03: `config.delegation.max_iterations` is a hard CEILING — caller/model-supplied `max_iterations` above it is clamped down with `tracing::warn!`; values at or below are honored verbatim | VERIFIED | Single-task path: `delegate_task.rs:910-917` uses `requested.min(self.config.max_iterations)` with `tracing::warn!` on overflow. Batch path: `:318-325` same pattern. Test `delegate_task::tests::test_per_call_max_iterations_clamps_to_ceiling` — **1 passed** |
| 4 | D-07.1 regression: a child draining its own budget to exhaustion leaves the parent budget's `remaining()` unchanged | VERIFIED | `agent_loop.rs:2451` — `test_independent_budget_child_drain_does_not_affect_parent` constructs two separate `BudgetHandle::new(max)`, drains child, asserts `parent.remaining() == max`. Test run: **1 passed** |
| 5 | D-07.2 regression (T-28.1-16 acceptance): a cron job that calls `delegate_task` to exhaustion leaves the interactive budget at full headroom | VERIFIED | `runner.rs:748` — `cron_subagent_budget_independence_from_interactive` drains `child_budget_1` to 0, asserts `interactive_budget.remaining() == max`. Test run: **1 passed** |
| 6 | D-04: PROV-10 shared parent↔child counter retired — doc-comments rewritten, old "decrement the SAME budget" framing gone; `runner_shares_budget_arc` test renamed to `runner_stores_budget_field_children_get_fresh_handle` with independence source-guard | VERIFIED | `subagent_runner.rs:34-48`: doc says "PROV-10's shared parent↔child counter is RETIRED (D-04)". `grep "decrement the SAME budget"` = 0. `agent_runtime.rs:469`: renamed test present with `include_str!` guard asserting `BudgetHandle::new(max_iterations)` exists and `budget.clone()` is absent at the change site |
| 7 | Docs: `AGENT-RUNTIME-DESIGN.md §6.4/§8` amended — §8 status is "resolved in Phase 35", superseded cron-specific fix is marked superseded/removed, global per-subagent model documented, PROV-10 retirement explicit, D-03 clamp-to-ceiling documented, D-05 DoS bound `max_spawn_depth × max_concurrent_children × max_iterations` documented | VERIFIED | `docs/AGENT-RUNTIME-DESIGN.md:252-338`: §8 fully rewritten as "Resolved". "per-subagent" = 4 occurrences. "max_spawn_depth" = 2 occurrences. "superseded" at line 270. §6.4 cross-reference at line 233 reads "Resolved (T-28.1-16, Phase 35)". main.rs `with a clone of it (PROV-10)` = 0 matches |

**Score:** 7/7 truths verified

---

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/ironhermes-agent/src/subagent_runner.rs` | Per-child fresh budget at change site; rewritten PROV-10 doc | VERIFIED | Line 295: `agent.with_budget(BudgetHandle::new(max_iterations))`. Field doc at 34-48 rewrites PROV-10. `#[allow(dead_code)]` on retained field. |
| `crates/ironhermes-agent/src/agent_loop.rs` | D-07.1 independence test | VERIFIED | Line 2451: `test_independent_budget_child_drain_does_not_affect_parent` — passes. |
| `crates/ironhermes-agent/src/agent_runtime.rs` | Rewritten runner test asserting independence | VERIFIED | Line 469: `runner_stores_budget_field_children_get_fresh_handle` with `include_str!` guard. |
| `crates/ironhermes-agent/src/budget.rs` | SeqCst preserved, no Relaxed, doc updated | VERIFIED | 4 `Ordering::SeqCst` occurrences, 0 `Ordering::Relaxed`. |
| `crates/ironhermes-tools/src/delegate_task.rs` | Clamp at both resolution sites + rewritten test | VERIFIED | Single-task: `.min(self.config.max_iterations)` at line 917. Batch: `.min(config.max_iterations)` at line 325. Both with `tracing::warn!`. Test `test_per_call_max_iterations_clamps_to_ceiling` passes. |
| `crates/ironhermes-cron-runner/src/runner.rs` | D-07.2 cron subagent-layer independence test | VERIFIED | Line 748: `cron_subagent_budget_independence_from_interactive` — passes. |
| `docs/AGENT-RUNTIME-DESIGN.md` | §6.4/§8 amended — global per-subagent model | VERIFIED | §8 heading "Resolved — T-28.1-16". Clamp-to-ceiling, DoS bound, PROV-10 retirement all documented. Superseded cron-specific sketch marked superseded. |
| `crates/ironhermes-cli/src/main.rs` | PROV-10 comments cleaned | VERIFIED | `with a clone of it (PROV-10)` = 0 matches. 3 sites updated. |

---

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `subagent_runner.rs run_child (~:295)` | child `AgentLoop` budget | `agent.with_budget(BudgetHandle::new(max_iterations))` | WIRED | Unconditional, fresh `Arc<AtomicUsize>` per child call |
| `delegate_task.rs::execute (~:910-917)` | `self.config.max_iterations` ceiling | `requested.min(self.config.max_iterations)` + `tracing::warn!` | WIRED | Both present in non-comment code |
| `delegate_task.rs::execute_batch (~:313-325)` | `config.max_iterations` ceiling | `requested.min(config.max_iterations)` + `tracing::warn!` | WIRED | Both present in non-comment code |
| `agent_runtime.rs test` | `subagent_runner.rs` change site | `include_str!("subagent_runner.rs")` guard | WIRED | Asserts `BudgetHandle::new(max_iterations)` present and `budget.clone()` absent |
| `docs/AGENT-RUNTIME-DESIGN.md §8` | global per-subagent model | prose amendment | WIRED | Status = resolved; superseded sketch removed |

---

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| D-03 clamp: request 99 > ceiling 5 clamped to 5; request 3 < ceiling 99 honored | `cargo test -p ironhermes-tools "delegate_task::tests::test_per_call_max_iterations_clamps_to_ceiling"` | 1 passed, 0 failed | PASS |
| D-07.1 independence: child drain does not affect parent | `cargo test -p ironhermes-agent "budget_tests::test_independent_budget"` | 1 passed, 0 failed | PASS |
| D-07.2 cron subagent layer: interactive budget at full headroom after cron subagent drain | `cargo test -p ironhermes-cron-runner cron_subagent_budget` | 1 passed, 0 failed | PASS |

---

### Anti-Patterns Found

| File | Pattern | Severity | Notes |
|------|---------|----------|-------|
| `subagent_runner.rs:47` | `#[allow(dead_code)]` on `budget` field | INFO | Intentional — field retained for `new()` signature and grep invariants (plan documents this explicitly as the field-kept decision) |

No `TBD`, `FIXME`, `XXX` markers found in modified files. No stub implementations. No `return null` / empty returns in production paths.

---

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|------------|-------------|--------|----------|
| T-28.1-16 | 35-01, 35-02, 35-03 | Cron subagent budget isolation | SATISFIED | Fresh per-child budget eliminates shared counter contamination vector; D-07.2 test proves cron subagent drain cannot touch interactive headroom |

---

### Commit Verification

All 6 phase commits exist on `develop`:

| Hash | Type | Description |
|------|------|-------------|
| `69ffa9bf` | test(35-01) | RED — failing D-03 clamp test |
| `3015a2a2` | feat(35-01) | GREEN — D-03 clamp implementation |
| `452b40d0` | test(35-02) | RED — D-07.1 independence test |
| `f1bf51f7` | feat(35-02) | GREEN — PROV-10 retirement + fresh BudgetHandle |
| `b991dd38` | feat(35-03) | D-07.2 cron subagent-layer test |
| `f94c4220` | docs(35-03) | Design doc amendment + main.rs comment cleanup |

---

## Overall Verdict: GOAL ACHIEVED

All phase deliverables are present, substantive, and wired:

- **D-01/D-02/D-04:** `budget.clone()` is gone from `subagent_runner.rs:283-284`. The change site unconditionally issues `BudgetHandle::new(max_iterations)` giving every child a fresh `Arc<AtomicUsize>`. PROV-10 doc-comments are rewritten across all four audited files. The field is retained with `#[allow(dead_code)]` per the deliberate planner decision.
- **D-03:** Both `delegate_task` resolution sites clamp to `config.max_iterations` using `.min()` with `tracing::warn!` on overflow. The rewritten test asserts both cases (clamped and honored verbatim). Test green.
- **D-05:** No tree-wide ceiling. DoS bound `1 × 3 × 50 = 150` documented in `AGENT-RUNTIME-DESIGN.md §8`. Existing depth and concurrency guards from Phase 32.2 remain in effect.
- **D-07.1/D-07.2:** Independence regression tests exist and pass. D-07.1 proves unit-level parent↔child isolation. D-07.2 proves the cron-specific T-28.1-16 acceptance criterion at the subagent layer.
- **Docs:** `AGENT-RUNTIME-DESIGN.md §8` is fully rewritten as resolved. The superseded cron-specific "own AgentRuntime" sketch is explicitly marked superseded and replaced with the global per-subagent model. §6.4 cross-reference updated to "Resolved (T-28.1-16, Phase 35)". Three `main.rs` PROV-10 comments cleaned.

---

_Verified: 2026-05-21_
_Verifier: Claude (gsd-verifier)_

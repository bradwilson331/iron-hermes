# Phase 35: Per-subagent independent iteration budgets (T-28.1-16) - Research

**Researched:** 2026-05-21
**Domain:** Rust (2024 edition) — agent iteration-budget plumbing, subagent delegation, DoS containment
**Confidence:** HIGH (all claims verified against live code in this session)

## Summary

This is a small, well-scoped wiring change with one genuine open question now resolved. CONTEXT.md's design is sound and its line-number references are essentially accurate (one drifted by ~5 lines). The change site is confirmed at `subagent_runner.rs:283-284`. The cleanest implementation seam already exists: `run_child` **already receives** a `max_iterations: usize` parameter (line 198) that flows from the delegate_task tool — so giving each child a fresh `BudgetHandle::new(max_iterations)` is a one-line swap of the budget wired at 283-284, with no new config or signature change required.

**The D-03 open question is resolved and it is NOT trivial:** IronHermes' `delegate_task` schema **does** expose a per-call AND per-task `max_iterations` override (schema lines 677, 695-698), and the tool **currently honors it** (`effective_max_iterations` / `per_task_max_iterations` at lines 887-891 / 309-313), with a passing test at line 2064 asserting the override wins. This is the OPPOSITE of the reference's log-and-drop and the OPPOSITE of CONTEXT D-03's "config must be authoritative." Because the new fresh-budget will be sized from whatever value `run_child` is handed, a caller-supplied `max_iterations` will directly size the child's independent budget unless the planner adds a guard. **The planner must make an explicit decision here** (see D-03 resolution below) — this is the single non-mechanical choice in the phase.

PROV-10 spans 6 source files + 2 test files. The shared-counter assertions in `agent_loop.rs:2419` (`test_shared_budget_increment`) and the source-include guard in `agent_runtime.rs:459` (`runner_shares_budget_arc`) and `invariants_21_7.rs` will FAIL after the change and must be inverted/retired. The `BudgetHandle` API itself is unchanged (still has `new`, `clone`, `consume`, `remaining`, `reset`); only the *wiring* of who-shares-with-whom changes.

**Primary recommendation:** At `subagent_runner.rs:283-284`, replace `agent = agent.with_budget(budget.clone())` (parent's shared handle) with `agent = agent.with_budget(BudgetHandle::new(<authoritative max_iterations>))`. Resolve the `max_iterations` source per D-03 below. Then invert/retire the three PROV-10 sharing assertions and rewrite the doc-comments.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Per-child budget construction | `ironhermes-agent` (`AgentSubagentRunner::run_child`) | — | This is the single point where every child `AgentLoop` is built and `with_budget` is called (line 271/283). Cron and interactive both route through it via the `SubagentRunner` trait. |
| Config authority for the cap | `ironhermes-core` (`SubagentConfig.max_iterations`) | `ironhermes-tools` (resolves per-call override) | Cap value lives in config; the override-resolution decision happens in `delegate_task.rs::execute`. |
| Override-vs-config policy (D-03) | `ironhermes-tools` (`delegate_task.rs`) | — | The `effective_max_iterations` resolver (line 887) is where config-wins must be enforced if chosen. |
| DoS bound documentation | `docs/AGENT-RUNTIME-DESIGN.md` §6.4/§8 + threat model | — | Bound is now depth × concurrency × per-subagent; no code enforces a tree-wide ceiling (both guards already exist). |

## User Constraints (from CONTEXT.md)

> Copied from `.planning/phases/35-.../35-CONTEXT.md`. The planner MUST honor these.

### Locked Decisions
- **D-01:** Per-subagent independent budgets. Each child loop gets a **fresh** `BudgetHandle::new(config.delegation.max_iterations)`, NOT a clone of the runner's shared `budget`. Change site `subagent_runner.rs:282-284`.
- **D-02:** Apply **globally** to ALL subagents (interactive + cron). A parent's budget is no longer decremented by children. Deliberate wider scope.
- **D-03:** Cap source is `delegation.max_iterations` (already exists, default 50 — no new config field). Cap MUST remain **authoritative**: a model/caller-supplied `max_iterations` must not silently shrink/expand it (mirror reference log-and-drop at `delegate_tool.py:1968-1979`).
- **D-04:** Retire the PROV-10 shared parent↔child counter. Rewrite the doc-comment at `subagent_runner.rs:34-39`. Invert the PROV-10 shared-budget regression test(s) to assert independence.
- **D-05:** DoS guard = reference-style, no tree-wide ceiling. Bounded by `max_spawn_depth` (1) × `max_concurrent_children` (3) × `max_iterations` (50). Both guards already exist (Phase 32.2). Update threat model.
- **D-06:** Keep Phase 35; broaden goal in roadmap. CONTEXT.md is source of truth.
- **D-07:** Three required regression tests (see Validation Architecture).

### Claude's Discretion
- Exact module/seam for constructing the fresh child `BudgetHandle` (inside `run_child` vs at runner construction) — provided each child gets a distinct `Arc<AtomicUsize>`.
- Whether to keep `AgentSubagentRunner.budget` field at all (may become vestigial) or repurpose — decide after auditing readers.

### Deferred Ideas (OUT OF SCOPE)
- Subagent **handoff** (pass off in-progress work). Net-new design, own phase.
- Budget **refresh / top-up to continue** — DROPPED, not deferred.
- Cron-specific "own AgentRuntime" architecture — SUPERSEDED by the global change.
- 28.1-06 per-job fresh cron *top-level* budget — already shipped, untouched.

## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| T-28.1-16 | Cron subagents draining the interactive budget via the shared `ToolRegistry` delegate runner. | Resolved as a side-effect of D-01: once children no longer clone the parent/runner budget, no subagent (cron or interactive) can touch its parent's counter. The shared `ToolRegistry` (gateway `runner.rs:857-866` → cron `runner.rs:288-296`) is no longer a contamination vector. Original gap described in `AGENT-RUNTIME-DESIGN.md` §6.4/§8 (lines 52-57, 253-283). D-07 test #2 proves the acceptance criterion. |

## Live Code Verification

> CONTEXT.md references confirmed against current code. Line drift noted where present.

| CONTEXT ref | Live location | Status |
|-------------|---------------|--------|
| `subagent_runner.rs:282-284` change site | **`subagent_runner.rs:283-284`** (`if let Some(ref budget) = self.budget { agent = agent.with_budget(budget.clone()); }`) | ✅ Confirmed, ~1 line drift. `agent` is the child `AgentLoop` built at line 271 (`AgentLoop::new(child_client, registry, max_iterations)`). `self.budget` is `Option<BudgetHandle>` — the PARENT's shared handle. |
| `subagent_runner.rs:34-39` doc-comment | **`subagent_runner.rs:34-39`** | ✅ Confirmed verbatim. Says "clones of the handle share the underlying counter so parent + child subagent loops decrement the SAME budget." Becomes false; rewrite. |
| `config.rs:964-1013` SubagentConfig | **`config.rs:962-1002`** struct; defaults at **1004-1019** | ✅ `max_iterations: 50` (line 982/1012), `max_concurrent_children: 3` (981/1011), `max_spawn_depth: 1` (994/1018). Note: CONTEXT cites `max_iterations` at `config.rs:982-983` — actual is line 982 (doc) / 1012 (default). |
| `budget.rs` BudgetHandle API | **`budget.rs:31-90`** | ✅ `BudgetHandle::new(max: usize)` (line 33), `consume` (72), `remaining` (58), `reset` (88), plus `used`, `max`, `pressure`, `inner`, `new_from_arc`. Signature confirmed. |
| `cron-runner/runner.rs:638` independence test | **`runner.rs:638`** `cron_budget_is_independent_from_interactive_budget` | ✅ Confirmed. Template for D-07 test #2. Builds two `BudgetHandle::new(max)`, drains one, asserts the other unchanged. |
| `cron-runner/runner.rs:185` per-job top-level budget | **`runner.rs:185`** `let cron_budget = BudgetHandle::new(ctx.config.agent.max_iterations);` | ✅ Confirmed — STAYS unchanged. Installed at `runner.rs:301` (`agent.with_budget(cron_budget)`). This is the TOP-LEVEL cron budget (28.1-06), distinct from subagent layer. |
| `cron-runner/runner.rs:288-296` registry resolution | **`runner.rs:288-298`** | ✅ Confirmed. Resolves `tool_registry_scoped` from `ctx.tool_registry` (shared) and builds `AgentLoop::new(client, tool_registry_scoped, max_turns)`. |
| `gateway/runner.rs:857-866` cron delegate path | **`runner.rs:857-866`** | ✅ Confirmed. Constructs `CronRunnerContext` with `tool_registry: tool_registry_tick` (the gateway's shared `ToolRegistry`). |
| reference `delegate_tool.py:507/997-1000/1968-1979` | Not re-read this session (external repo) | CITED from CONTEXT — `DEFAULT_MAX_ITERATIONS=50`, own-budget-per-subagent, config-authoritative log-and-drop. |

## D-03 Resolution — THE Key Open Question (HIGH confidence)

**Question:** Does IronHermes' `delegate_task` tool schema expose a per-call `max_iterations` that a model/caller could supply?

**Answer: YES — and IronHermes currently HONORS it (does NOT log-and-drop).** This is a divergence from both the reference and CONTEXT D-03. [VERIFIED: codebase]

Evidence (all in `crates/ironhermes-tools/src/delegate_task.rs`):
- **Schema exposes it in two places:**
  - Top-level single-task: line **695-698** — `"max_iterations": { "type": "integer", "minimum": 1, "description": "Per-call max LLM iterations override. Defaults to delegation.max_iterations from config." }`
  - Per-task (batch) inside `tasks.items.properties`: line **677** — `"max_iterations": { "type": "integer", "minimum": 1, "description": "Per-task max iterations override. ..." }`
- **It is honored, not dropped:**
  - Single path: lines **886-891** — `effective_max_iterations = args.get("max_iterations").and_then(as_u64).map(as usize).unwrap_or(self.config.max_iterations)` → passed to `run_child` at line **915**.
  - Batch path: lines **308-313** — `per_task_max_iterations = task_obj.get("max_iterations")...unwrap_or(config.max_iterations)` → passed at line **416**.
- **A test enforces the override wins:** lines **2063-2112** `test_per_call_max_iterations_overrides_config` asserts `max_iterations=3` overrides `config.max_iterations=99`. This test will CONFLICT with a config-wins guard.
- This override path was added in **Phase 32.2 D-08** ("per-call max_iterations override; falls back to config default") — it is an intentional existing feature, not an accident.

**Where the override enters:** `DelegateTaskTool::execute` (line 887, single) and `DelegateTaskTool::execute_batch` (line 309, batch) in `delegate_task.rs`. It then flows as the `max_iterations` argument into `AgentSubagentRunner::run_child` (`subagent_runner.rs:198`). **Critically: this same `max_iterations` parameter is the natural value to size the new fresh budget from** — so without an explicit decision, the caller-supplied value silently sizes the independent budget, which is exactly what D-03 forbids.

**Planner decision required (pick one — recommend Option A for D-03 fidelity):**

- **Option A — Enforce config-wins (matches D-03 + reference).** In `delegate_task.rs::execute` and `execute_batch`, ignore caller-supplied `max_iterations`, always use `self.config.max_iterations` / `config.max_iterations`, and log-and-drop the supplied value (mirror `delegate_tool.py:1968-1979`). Also: remove `max_iterations` from the schema (lines 677, 695-698) OR keep it in schema but document it as advisory-only and dropped. Then **invert** `test_per_call_max_iterations_overrides_config` (line 2064) to assert config wins. *This is the most faithful to D-03; it deletes an existing Phase 32.2 feature.*
- **Option B — Keep override but clamp to config as a ceiling.** Allow caller to *shrink* but never *expand* beyond `config.max_iterations`. Partially honors "authoritative" (cap is a hard ceiling) while preserving the existing feature for smaller budgets. Update the test to assert clamping. *Weaker D-03 fidelity — D-03 says "must not silently shrink/expand," which forbids shrink too.*

⚠️ **The phase boundary (D-02 "no new config field, only wiring") suggests the planner intends a minimal change. Option A is a behavior change to delegate_task that deletes a shipped 32.2 feature and touches its test — flag this scope to the user during discuss/plan.** [ASSUMED] that Option A is preferred because D-03 explicitly cites the reference's log-and-drop; confirm with user.

## PROV-10 Reference Audit (HIGH confidence — full inventory)

> Every `PROV-10` site + behavioral assertions that depend on shared-counter semantics. "Invert" = rewrite assertion to prove independence. "Rewrite" = doc-comment only. "Retire" = delete the assertion/guard.

| File:line | What it is | Action |
|-----------|------------|--------|
| `ironhermes-agent/src/budget.rs:1` | Module doc: "Shared parent/child iteration-budget handle (PROV-10...)" | **Rewrite** doc. ⚠️ `budget.rs:12-14` ("Clones share the same underlying counter") describes the API truthfully and STAYS (clone-sharing is still used for gateway↔CommandContext + reset visibility). Only the *parent↔child* framing is retired. |
| `ironhermes-agent/src/budget.rs:5-7`, `:40-50`, `:113-131` | SeqCst rationale, `new_from_arc`/`inner` (shared-counter helpers), `BudgetSnapshot` impl | **Keep.** Still used by gateway/CommandContext sharing and `reset()`. ⚠️ Do NOT remove `Ordering::SeqCst` — `tests/budget_ordering_grep.rs` greps for it (and forbids `Relaxed`). |
| `ironhermes-agent/src/agent_loop.rs:147,552,563` | Doc-comments: budget handle "shared with child agents (PROV-10)" | **Rewrite** docs. The `budget()` getter (line 563) was used to hand the handle to children for sharing; after D-01 children no longer take the parent's handle. Audit whether `budget()` still has callers (cron uses its own `with_budget`, not the getter). |
| `ironhermes-agent/src/agent_loop.rs:2418-2431` `test_shared_budget_increment` | Asserts `child.used()==8` because "clones share the same counter (PROV-10)" | **Invert OR retire.** This tests `BudgetHandle::clone` sharing at the API level (still valid mechanically), but its PROV-10 framing is dead. Repurpose as a pure API test or move the parent↔child independence assertion here. |
| `ironhermes-agent/src/subagent_runner.rs:34-39` | Field doc: "parent + child subagent loops decrement the SAME budget" | **Rewrite** — now each child gets its own fresh `BudgetHandle`. |
| `ironhermes-agent/src/subagent_runner.rs:283-284` | **THE change site** — `with_budget(budget.clone())` | **Change** to fresh handle (D-01). |
| `ironhermes-agent/src/agent_runtime.rs:21,146,448,464` | Module doc "## Budget sharing (PROV-10)"; comment "clone of the SHARED budget (PROV-10)" at 146; doc at 448 | **Rewrite** docs. Line 149 still passes `Some(budget.clone())` to `AgentSubagentRunner::new` — see field-disposition note below. |
| `ironhermes-agent/src/agent_runtime.rs:447-470` `runner_shares_budget_arc` test | Source-include guard asserting `from_config` contains `Some(budget.clone())` passed to runner | **Invert/retire.** This guard ENFORCES the sharing wiring. If the `budget` field is dropped (discretion), this whole test goes; if the field is kept but unused by children, this guard's assertion is now misleading and should be rewritten to assert children get fresh handles. |
| `ironhermes-cli/src/main.rs:755,1301,2419` | Comments "with a clone of it (PROV-10)" | **Rewrite** docs (comments only — main.rs no longer constructs the budget per INV-21.7-04). |
| `ironhermes-cli/tests/invariants_21_7.rs:78-95` | `invariant_21_7_04_budget_owned_by_agent_runtime` | **Audit, likely keep.** This asserts the SHARED budget is constructed in `AgentRuntime::from_config` and `run_turn` resets it — that's about the TOP-LEVEL/interactive budget, which is unaffected. Verify the assertion text doesn't claim parent↔child sharing. |
| `ironhermes-agent/tests/budget_ordering_grep.rs` | Greps `budget.rs` for SeqCst / no Relaxed | **Keep.** Constrains the budget.rs rewrite: must retain `Ordering::SeqCst`, must not introduce `Ordering::Relaxed`. |

**`AgentSubagentRunner.budget` field disposition (Claude's discretion D-disc):** Readers of `self.budget` are ONLY `subagent_runner.rs:283-284`. Once that line stops cloning it, the field has **zero readers**. Recommendation: **remove the field** and the `Option<BudgetHandle>` parameter from `AgentSubagentRunner::new` (line 62-66), and drop the `Some(budget.clone())` arg at `agent_runtime.rs:149`. This also cleanly retires the `runner_shares_budget_arc` source-guard. Cron's runner builds `AgentLoop` directly (`cron-runner/runner.rs:298`) and does not use this field. ⚠️ Removing the `new` parameter is a public-signature change within the crate — grep for all `AgentSubagentRunner::new(` call sites before committing (found: `agent_runtime.rs:149`; check `app_runtime_factory.rs` and tests).

## Implementation Seam (HIGH confidence)

The cleanest seam — confirmed by code — is **inside `run_child`**, reusing the `max_iterations` parameter it already receives:

```rust
// crates/ironhermes-agent/src/subagent_runner.rs:283-284 (CURRENT)
if let Some(ref budget) = self.budget {
    agent = agent.with_budget(budget.clone());      // parent's SHARED handle
}

// AFTER (D-01): each child gets its OWN counter, sized from the authoritative cap.
// `max_iterations` is already a fn param (line 198), threaded from delegate_task.
agent = agent.with_budget(BudgetHandle::new(max_iterations));   // fresh Arc<AtomicUsize>
```

- `BudgetHandle::new(usize)` creates a fresh `Arc<AtomicUsize>` (`budget.rs:33-38`) — guarantees distinct counter per child (satisfies the discretion constraint).
- `AgentLoop::new` at line 271 already takes `max_iterations` for its own internal cap; wiring the budget to the same value keeps the loop's iteration ceiling and the budget aligned.
- **The D-03 authority must be enforced UPSTREAM** (in `delegate_task.rs` per Option A/B), because by the time `run_child` receives `max_iterations` the override has already been resolved at `delegate_task.rs:891`. Sizing the budget here faithfully reflects whatever policy `delegate_task.rs` enforces.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Per-child counter | A new atomic/struct | `BudgetHandle::new(max)` | Already has SeqCst-correct consume/remaining/reset/pressure; CI grep enforces ordering. |
| DoS ceiling | A tree-wide aggregate budget | Existing `max_spawn_depth` × `max_concurrent_children` guards (Phase 32.2, `delegate_task.rs:233`, `:539`) | D-05 explicitly chose reference-style; both guards already enforced + tested. |
| Independence test harness | New integration scaffolding | Mirror `cron-runner/runner.rs:638` (two handles, drain one, assert other) | Established template; pure-`BudgetHandle` unit test, no network. |

## Common Pitfalls

### Pitfall 1: Sizing the fresh budget from the caller-supplied value, defeating D-03
**What goes wrong:** `run_child` receives `max_iterations` already resolved to the caller's override; `BudgetHandle::new(max_iterations)` then makes the model's request authoritative.
**How to avoid:** Enforce config-authority in `delegate_task.rs::execute`/`execute_batch` (Option A/B) BEFORE the value reaches `run_child`. Don't try to re-clamp inside `run_child` — `run_child` has no access to `config.delegation` (the runner doesn't hold config).
**Warning sign:** `test_per_call_max_iterations_overrides_config` still passing unchanged after the phase.

### Pitfall 2: Breaking the `budget_ordering_grep` CI test when rewriting `budget.rs`
**What goes wrong:** Rewriting the PROV-10 module doc and accidentally removing the `Ordering::SeqCst` token or introducing `Ordering::Relaxed`.
**How to avoid:** `budget.rs` keeps SeqCst; only the *parent↔child* doc framing changes. The clone-sharing API (`clone`, `new_from_arc`, `inner`, `reset` visibility) STAYS — it's still used by gateway↔CommandContext.

### Pitfall 3: Removing `AgentSubagentRunner::new`'s budget param without auditing all call sites
**What goes wrong:** Crate won't compile; or a test constructor breaks.
**How to avoid:** Grep `AgentSubagentRunner::new(` and `.budget` across the crate first (`agent_runtime.rs:149`, `app_runtime_factory.rs`, `subagent_runner.rs:115` test ctor).

### Pitfall 4: Forgetting the interactive-side behavior change (D-02)
**What goes wrong:** Interactive chat's parent budget previously decremented as children ran; after D-01 it no longer does. A reviewer may flag this as a regression.
**How to avoid:** Document in the threat-model/design update that this is intentional (D-02). The interactive top-level budget still bounds the parent's OWN loop; children are separately bounded by `max_iterations` each.

## Validation Architecture

> `workflow.nyquist_validation` is ABSENT from `.planning/config.json` → treated as ENABLED. Section required.

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in `#[test]` / `#[tokio::test]` (cargo test) + `async-trait` mock runners |
| Config file | `Cargo.toml` per crate (workspace) — no separate test config |
| Quick run command | `cargo test -p ironhermes-agent budget` and `cargo test -p ironhermes-tools delegate` |
| Full suite command | `cargo test --workspace` (then `cargo clippy --workspace --all-targets`) |

### Observable Behaviors → Test Map (covers D-07)
| Req | Behavior | Test Type | Automated Command | File / Seam |
|-----|----------|-----------|-------------------|-------------|
| D-07.1 / D-02 | Child drains its OWN budget to exhaustion → **parent** `remaining()` unchanged (independence; inverts old PROV-10 sharing) | unit | `cargo test -p ironhermes-agent independent_budget` | New test in `agent_loop.rs` budget_tests mod (sibling to `test_shared_budget_increment:2419`, which gets repurposed). Build two `BudgetHandle`s; drain child; assert parent unchanged. |
| D-07.2 / T-28.1-16 | **Cron** job calls `delegate_task` to exhaustion → interactive budget at full headroom | unit/integration | `cargo test -p ironhermes-cron-runner cron_budget` | Extend/mirror `cron-runner/runner.rs:638` to add a *subagent-layer* drain (not just top-level). Confirms shared `ToolRegistry` is no longer a vector. |
| D-07.3 / D-03 | Model/caller-supplied `max_iterations` cannot override authoritative `config.delegation.max_iterations` | unit | `cargo test -p ironhermes-tools max_iterations` | **Invert** `test_per_call_max_iterations_overrides_config` (`delegate_task.rs:2064`) → assert config wins (Option A) or clamps (Option B). |
| DoS bound (D-05) | Existing depth × concurrency guards still enforced after PROV-10 retirement | unit | `cargo test -p ironhermes-tools max_spawn\|max_concurrent` | Existing tests at `delegate_task.rs:1713` (batch>concurrency Err) and `:2120`/`:2179` (depth downgrade) — confirm still green; no new test required, just regression. |

### Sampling Rate
- **Per task commit:** `cargo test -p <touched-crate> <filter>` + `cargo build -p <crate>`
- **Per wave merge:** `cargo test --workspace`
- **Phase gate:** `cargo test --workspace` green + `cargo clippy --workspace --all-targets -- -D warnings` before `/gsd:verify-work`

### Wave 0 Gaps
- [ ] New independence test (D-07.1) in `agent_loop.rs::budget_tests` — covers the inverted PROV-10 assertion.
- [ ] Subagent-layer extension of `cron-runner/runner.rs:638` (D-07.2) — current test only proves top-level cron independence.
- [ ] Invert `delegate_task.rs:2064` (D-07.3) — depends on the D-03 Option A/B decision.
- *No framework install needed — Rust test harness + existing mock `SubagentRunner` impls already present (`delegate_task.rs:1149`, `:2071`).*

## Security Domain

> `security_enforcement` not explicitly false in config → included. This phase IS security-relevant (DoS containment + DEFCON posture per D-05).

### Applicable ASVS Categories
| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | — |
| V3 Session Management | no | — |
| V4 Access Control | no | — |
| V5 Input Validation | yes | `delegate_task` `max_iterations` is caller/model-controlled input — D-03 config-authority is the validation control (reject/clamp untrusted override). `"minimum": 1` schema bound exists; no maximum bound today. |
| V6 Cryptography | no | — |
| V11/V12 Business-logic / resource (DoS) | yes | Runaway-delegation DoS bound = `max_spawn_depth × max_concurrent_children × max_iterations`. No tree-wide ceiling by design (D-05). Both factor guards already enforced + tested (Phase 32.2). |

### Known Threat Patterns for this stack
| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Model inflates its own budget via `max_iterations` override | Elevation of Privilege / DoS | D-03 config-authority (Option A log-and-drop, or Option B clamp). **Today this is UNMITIGATED — override is honored.** Schema lacks an upper bound. |
| Cron subagent drains interactive headroom (T-28.1-16) | DoS | D-01 fresh per-child budget removes the shared counter — the vector itself disappears. |
| Unbounded recursion / fan-out | DoS | `max_spawn_depth` (default 1) + `max_concurrent_children` (default 3) — already enforced. |

**Threat-model update required (D-05):** Document the new runaway-delegation bound and the explicit PROV-10 retirement in `docs/AGENT-RUNTIME-DESIGN.md` §6.4/§8 (and any threat register). §8 (lines 253-283) currently prescribes the SUPERSEDED cron-specific "own delegate runner bound to the cron budget" fix — it must be amended to describe the global per-subagent model.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | Option A (config-wins log-and-drop, deleting the 32.2 override feature) is the user's preferred D-03 resolution | D-03 Resolution | If user wants to keep the override (Option B clamp, or keep-as-is), the plan's delegate_task change + test inversion differ. **Surface to user in discuss/plan.** |
| A2 | The reference's `delegate_tool.py:1968-1979` log-and-drop and `:507` DEFAULT=50 are accurate (cited from CONTEXT, not re-read this session) | Live Code Verification | Low — used only to justify Option A fidelity; the IronHermes-side change is independently specified. |
| A3 | Removing the `AgentSubagentRunner.budget` field is safe (only reader is line 283-284) | Field disposition | Low — verified zero other readers via grep this session; planner should re-grep `AgentSubagentRunner::new(` call sites before committing. |

## Open Questions

1. **D-03 policy choice (Option A vs B) — see D-03 Resolution.** Recommendation: Option A for fidelity, but it deletes a shipped Phase 32.2 feature and inverts its test — get explicit user sign-off (this is the only non-mechanical decision in the phase).
2. **Should the `max_iterations` schema property be removed entirely or kept as advisory-only?** If Option A, removing it from the schema (lines 677, 695-698) is cleaner but changes the tool's public JSON contract; keeping it but dropping the value is less disruptive. Planner's call; lean toward removing the per-task one and documenting the top-level one as advisory.

## Environment Availability

> Pure Rust code/config + test change. No external runtime dependencies.

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Rust toolchain (cargo, edition 2024) | build/test | ✓ (assumed — existing project) | — | — |
| tokio (multi-thread test runtime) | async tests | ✓ (workspace dep) | — | — |
| async-trait | mock `SubagentRunner` impls | ✓ (workspace dep) | — | — |

No missing dependencies; no network access required for any phase task.

## Sources

### Primary (HIGH confidence — verified in-session against live code)
- `crates/ironhermes-agent/src/subagent_runner.rs` — change site (283-284), field doc (34-39), `new` (62), `run_child` (194-203).
- `crates/ironhermes-tools/src/delegate_task.rs` — schema (647-712), `effective_max_iterations` (886-891), per-task (308-313), override test (2063-2112).
- `crates/ironhermes-agent/src/budget.rs` — `BudgetHandle` API (full file).
- `crates/ironhermes-core/src/config.rs` — `SubagentConfig` (962-1019).
- `crates/ironhermes-agent/src/agent_loop.rs` — budget docs (147,552,563), `test_shared_budget_increment` (2418-2431).
- `crates/ironhermes-agent/src/agent_runtime.rs` — budget construction (141-152), PROV-10 docs + `runner_shares_budget_arc` (447-470).
- `crates/ironhermes-cron-runner/src/runner.rs` — top-level budget (185,301), registry resolution (288-298), independence test (638).
- `crates/ironhermes-gateway/src/runner.rs` — cron delegate ctx (857-866).
- `crates/ironhermes-cli/tests/invariants_21_7.rs` — INV-21.7-04 (78-95).
- `crates/ironhermes-agent/tests/budget_ordering_grep.rs` — SeqCst grep constraint.
- `crates/ironhermes-tools/tests/delegate_task_runaway.rs` — runner mock pattern.
- `docs/AGENT-RUNTIME-DESIGN.md` §6.4/§8 (lines 44-57, 231-283) — T-28.1-16 gap + superseded fix.
- `.planning/config.json` — nyquist absent (enabled), commit_docs true.

### Secondary
- `.planning/phases/35-.../35-CONTEXT.md` — user decisions (source of truth).

### Tertiary (CITED, not re-verified this session)
- `/Users/twilson/code/hermes-agent/tools/delegate_tool.py:507,997-1000,1968-1979` — reference behavior (per CONTEXT).

## Metadata

**Confidence breakdown:**
- Change site & seam: HIGH — line-verified, seam reuses an existing param.
- D-03 resolution: HIGH — override path + honoring behavior + enforcing test all located.
- PROV-10 audit: HIGH — exhaustive grep across all named files + test dirs.
- Validation: HIGH — existing test templates + framework confirmed.

**Research date:** 2026-05-21
**Valid until:** 2026-06-20 (stable internal codebase; revalidate line numbers if other phases touch these files first)

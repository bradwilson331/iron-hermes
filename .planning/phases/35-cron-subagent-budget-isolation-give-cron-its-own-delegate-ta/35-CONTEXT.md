# Phase 35: Per-subagent independent iteration budgets (T-28.1-16) - Context

**Gathered:** 2026-05-21
**Status:** Ready for planning

<domain>
## Phase Boundary

**Broadened scope (confirmed during discussion).** The roadmap framed this phase
as cron-specific ("give cron its own delegate runner bound to the cron budget").
During discussion we chose a wider, simpler approach that resolves T-28.1-16 as a
*consequence* rather than as the direct target:

**This phase changes the subagent delegation budget model globally so that every
subagent (interactive AND cron) gets its OWN independent iteration budget, sized
from `delegation.max_iterations` (already default 50), instead of cloning the
parent's shared `BudgetHandle` Arc.** This retires the PROV-10 shared
parent↔child counter and matches the hermes-agent reference, where each subagent
has its own budget and total tree iterations can exceed any single agent's cap.

Because no subagent charges its parent's budget anymore, **T-28.1-16 (cron
subagents draining the interactive budget through the shared `ToolRegistry`
delegate runner) disappears as a side effect** — cron subagents can no longer
touch interactive headroom because *no* subagent shares its parent's counter.

**In scope:**
- Change `AgentSubagentRunner` so each child loop is given a fresh
  `BudgetHandle::new(delegation.max_iterations)` rather than a clone of the
  runner's `budget` field.
- Retire PROV-10's shared-counter invariant; update/replace its regression
  test(s) to assert the new independent-budget behavior.
- Update the threat model / docs to record that runaway delegation is now bounded
  by `max_spawn_depth × max_concurrent_children × delegation.max_iterations`
  (reference-style), not by one shared counter.
- Regression test proving a cron job that drains `delegate_task` to exhaustion
  leaves the interactive budget at full headroom (the original T-28.1-16
  acceptance criterion).

**Out of scope (NOT this phase):**
- The cron-specific "own AgentRuntime" / fresh-per-job cron runtime architecture
  explored earlier in this discussion — **superseded** by the global change.
- Subagent **handoff** (passing off in-progress work) — deferred to a future
  phase (see Deferred Ideas).
- Budget **refresh / top-up to continue** after exhaustion — dropped (see
  Deferred Ideas).
- The existing 28.1-06 per-job fresh cron *top-level* budget — already shipped,
  stays as-is, untouched.

</domain>

<decisions>
## Implementation Decisions

### Budget model
- **D-01:** Switch to **per-subagent independent budgets**, matching the
  hermes-agent reference. In `AgentSubagentRunner`, each child loop receives a
  **fresh** `BudgetHandle::new(config.delegation.max_iterations)` — NOT a clone
  of the runner's shared `budget` field. The current clone site is
  `crates/ironhermes-agent/src/subagent_runner.rs:282-284`
  (`agent = agent.with_budget(budget.clone())`).
- **D-02:** Apply the change **globally** — to ALL subagents (interactive + cron),
  not cron only. Interactive chat's delegation behavior changes too: a parent's
  budget is no longer decremented by its children. This is the deliberate,
  user-chosen wider scope.
- **D-03:** The per-subagent cap source is **`delegation.max_iterations`**, which
  **already exists** in `SubagentConfig` (`crates/ironhermes-core/src/config.rs:982-983`,
  default `50`). No new config field is required — only the wiring changes.
  **RESOLVED 2026-05-21 (user decision, RESEARCH D-03 Option B — CLAMP TO CEILING):**
  IronHermes' `delegate_task` tool *does* currently expose and honor a per-call /
  per-task `max_iterations` override (shipped Phase 32.2 / D-08, PROV-09; schema at
  `delegate_task.rs:677` + `:695-698`, honored at `:886-891` / `:308-313`, test
  `test_per_call_max_iterations_overrides_config` at `:2064`). We are **NOT** doing
  the reference's pure log-and-drop. Instead, **`config.delegation.max_iterations`
  is a hard CEILING**: a caller/model may request a *smaller* per-call budget
  (honored), but any value *exceeding* the config ceiling is clamped down to the
  ceiling (with a `tracing::warn!` recording the clamp). The fresh per-child
  `BudgetHandle` is sized from this clamped value. This preserves the 32.2 feature
  for shrinking while capping the DoS exposure. Enforcement is UPSTREAM in
  `delegate_task.rs::execute` and `::execute_batch` (both `effective_max_iterations`
  / `per_task_max_iterations` resolution sites), because `run_child` has no access
  to `config.delegation`. The existing override-wins test at `:2064` must be
  REWRITTEN to assert clamping (request > ceiling → clamped; request ≤ ceiling →
  honored). The `"minimum": 1` schema bound stays; document the ceiling behavior in
  the `max_iterations` property descriptions (keep both schema properties).

### PROV-10 retirement + DoS containment
- **D-04:** **Retire the PROV-10 shared parent↔child counter.** The doc-comment
  at `subagent_runner.rs:34-39` ("subagent loops decrement the SAME budget")
  becomes false and must be rewritten. Locate and **invert** the PROV-10
  shared-budget regression test(s) so they assert independence instead of
  sharing.
- **D-05:** **DoS guard = reference-style, no tree-wide ceiling.** Runaway
  delegation is bounded by `max_spawn_depth` (default 1, `config.rs:994-997`) ×
  `max_concurrent_children` (default 3, `config.rs:981`) ×
  `delegation.max_iterations` (50). No separate aggregate tree-wide budget
  backstop. Both depth and concurrency guards already exist (Phase 32.2). The
  threat model MUST be updated to document this new bound and the explicit
  PROV-10 retirement (security-relevant — relates to the project's DEFCON
  posture).

### Scope / framing
- **D-06:** [informational] **Keep Phase 35; broaden its goal.** The ROADMAP Phase 35
  goal/title should be updated to reflect the global model change
  ("per-subagent independent iteration budgets matching the hermes-agent
  reference; retire PROV-10 shared counter; T-28.1-16 resolved as a
  consequence"). This CONTEXT.md is the source of truth for the broadened
  scope; the roadmap title is currently stale and should be edited via
  `/gsd-phase 35`.

### Verification
- **D-07:** Required regression tests:
  1. A subagent drains its own budget to exhaustion → the **parent** budget's
     `remaining()` is unchanged (the new independence guarantee; inverts the old
     PROV-10 shared-counter test).
  2. A **cron** job that calls `delegate_task` to exhaustion → the interactive
     budget stays at full headroom (the original T-28.1-16 acceptance from
     `AGENT-RUNTIME-DESIGN.md §8`). Mirror the existing independence test at
     `crates/ironhermes-cron-runner/src/runner.rs:638`.
  3. Clamp-to-ceiling (D-03 Option B): a caller-supplied `max_iterations`
     *exceeding* `delegation.max_iterations` is clamped down to the ceiling (and a
     warn is logged); a caller-supplied value *at or below* the ceiling is honored
     verbatim. (Rewrite of the old override-wins test at `delegate_task.rs:2064`.)

### Claude's Discretion
- Exact module/seam for constructing the fresh child `BudgetHandle` (inside
  `run_child` vs at runner construction) — planner's call, provided each child
  gets a distinct `Arc<AtomicUsize>`.
- Whether to keep the `AgentSubagentRunner.budget` field at all (it may become
  vestigial once children no longer clone it) or repurpose it — planner decides
  after auditing readers.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### T-28.1-16 origin + original (now superseded) fix sketch
- `docs/AGENT-RUNTIME-DESIGN.md` §6.4, §8 — defines the T-28.1-16 gap and the
  original cron-specific fix sketch. **Note:** §8's "give cron its own
  AgentRuntime/registry" approach is SUPERSEDED by this phase's global model
  change; read it for the gap description and the cron-vs-interactive budget
  mechanics, not as the chosen implementation. §8 must be amended once this phase
  lands.
- `.planning/phases/28.1-agentruntime-channel-migration-budget-skills-tools-ownership/28.1-06-PLAN.md`
  — threat register row for T-28.1-16 (`accept (with documented follow-up)`).
- `.planning/phases/28.1-agentruntime-channel-migration-budget-skills-tools-ownership/28.1-06-SUMMARY.md`
  §"Cron Subagent Budget Isolation (T-28.1-16)" — accepted disposition.
- `.planning/phases/28.1-agentruntime-channel-migration-budget-skills-tools-ownership/28.1-VERIFICATION.md`
  item 5 — confirms top-level cron budget independence already shipped.

### Reference implementation being matched (Python hermes-agent)
- `/Users/twilson/code/hermes-agent/tools/delegate_tool.py`:
  - `:507` — `DEFAULT_MAX_ITERATIONS = 50` (the cap value being mirrored).
  - `:997-1000` — comment: each subagent gets its OWN budget; total parent+child
    iterations can exceed the parent's cap (the model we're adopting).
  - `:1968-1979` — config value is authoritative; model-supplied `max_iterations`
    is logged and dropped (D-03 fidelity).
- `/Users/twilson/code/hermes-agent/tools/kanban_tools.py` — the reference's
  "handoff" is a kanban task-board completion summary, a separate subsystem
  (informs the DEFERRED handoff idea, NOT this phase).

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `crates/ironhermes-core/src/config.rs:964-1013` — `SubagentConfig`. The
  `max_iterations: usize` field (default 50) ALREADY EXISTS — this is the
  per-subagent cap; no new config knob needed. `max_concurrent_children` (3) and
  `max_spawn_depth` (1) are the DoS guards that remain after PROV-10 retirement.
- `crates/ironhermes-agent/src/budget.rs` — `BudgetHandle::new/consume/remaining/reset`.
  Each child gets its own `BudgetHandle::new(...)` (fresh `Arc<AtomicUsize>`).
- `crates/ironhermes-cron-runner/src/runner.rs:638` —
  `cron_budget_is_independent_from_interactive_budget` test: the template for the
  new cron-delegation independence test (D-07.2).

### Established Patterns
- `crates/ironhermes-agent/src/subagent_runner.rs:34-39` — the `budget:
  Option<BudgetHandle>` field documented as the PROV-10 SHARED handle. This
  doc-comment and the field's role change.
- `crates/ironhermes-agent/src/subagent_runner.rs:282-284` — **the exact change
  site**: `if let Some(ref budget) = self.budget { agent = agent.with_budget(budget.clone()); }`
  → give the child a fresh `BudgetHandle::new(config.delegation.max_iterations)`.
- `crates/ironhermes-cron-runner/src/runner.rs:185` — per-job fresh cron
  TOP-LEVEL budget (28.1-06). STAYS unchanged; only the subagent layer changes.

### Integration Points
- PROV-10 references to audit/update across:
  `crates/ironhermes-agent/src/{budget.rs, agent_loop.rs, agent_runtime.rs, subagent_runner.rs}`
  and `crates/ironhermes-cli/tests/invariants_21_7.rs`. Find the PROV-10
  shared-counter regression test and invert it (D-04).
- The cron delegate path resolves its runner from the gateway's shared
  `ToolRegistry` (`crates/ironhermes-gateway/src/runner.rs:857-866` →
  `crates/ironhermes-cron-runner/src/runner.rs:288-296`). After D-01 this sharing
  is no longer a contamination vector (children don't clone the runner's budget),
  so the shared registry can stay — confirm in the T-28.1-16 regression test.

</code_context>

<specifics>
## Specific Ideas

- "Match subagents own iteration budget" — explicitly model the change on
  hermes-agent's per-subagent independent budget (default 50, authoritative).
- The user weighed this against IronHermes' existing PROV-10 shared-counter model
  (a documented DoS-containment deviation from the reference) and chose to retire
  PROV-10 globally in favor of reference fidelity, accepting the
  depth×concurrency×per-subagent bound as the runaway guard.

</specifics>

<deferred>
## Deferred Ideas

- **Subagent handoff** — a process for a subagent to pass off in-progress work
  (e.g. when running low) to another agent / back to the parent / to a queued
  follow-up. The reference's analog is the kanban task-board handoff
  (`hermes-agent/tools/kanban_tools.py`), a separate subsystem fired on task
  completion, not on budget exhaustion. Net-new design; its own phase.
- **Budget refresh / top-up to continue** — DROPPED, not merely deferred. Does
  not exist in the reference and directly tensions with the DoS-containment
  purpose of having a budget. Only revisit with an explicit policy/cap if a
  concrete need arises.

</deferred>

---

*Phase: 35-cron-subagent-budget-isolation-give-cron-its-own-delegate-ta*
*Context gathered: 2026-05-21*

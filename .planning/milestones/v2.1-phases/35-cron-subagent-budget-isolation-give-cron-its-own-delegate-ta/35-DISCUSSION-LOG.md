# Phase 35: Per-subagent independent iteration budgets (T-28.1-16) - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-05-21
**Phase:** 35-cron-subagent-budget-isolation-give-cron-its-own-delegate-ta
**Areas discussed:** Fix architecture, Budget sharing, Scope (CLI), Runtime lifecycle, Reuse vs rebuild, Additional gray areas, hermes-agent reference investigation, Global budget-model pivot, DoS guard, Phase framing

> **Note:** The discussion pivoted mid-way. The early decisions (cron-specific
> "own AgentRuntime" architecture) were SUPERSEDED after investigating the
> hermes-agent reference. The final, binding decisions are the global
> per-subagent-budget model captured at the bottom and in CONTEXT.md. Early
> options are preserved here for the audit trail.

---

## Fix architecture (SUPERSEDED)

| Option | Description | Selected |
|--------|-------------|----------|
| Surgical delegate re-registration | Cron-scoped registry re-registers only delegate_task with a cron-budget runner | |
| Own cron AgentRuntime | Cron builds its own full AgentRuntime via from_config (§4 thin-clients pattern) | ✓ |
| Let me explain | — | |

**User's choice:** Own cron AgentRuntime — later superseded by the global model change.

---

## Budget sharing (SUPERSEDED → REVERSED)

| Option | Description | Selected |
|--------|-------------|----------|
| Share the per-job top-level budget | Top-level cron loop + subagents share one per-job cron_budget Arc (PROV-10 parity) | ✓ (later reversed) |
| Separate per-job subagent budget | Subagents get own fresh cron-scoped budget per job | |
| Cron-lifetime shared budget | One cron budget Arc shared across all jobs' subagents | |

**User's choice:** Initially "share the per-job top-level budget"; **reversed** after the reference investigation to per-subagent independent budgets.

---

## Scope — CLI cron path

| Option | Description | Selected |
|--------|-------------|----------|
| Gateway cron only | CLI cron builds empty registry, no delegate_task today | ✓ |
| Also harden CLI cron | Defensive test that CLI path stays delegate-free | ✓ |

**User's choice:** Both 1 and 2. (Largely moot under the global model, but the defensive intent carries forward.)

---

## Runtime lifecycle (SUPERSEDED)

| Option | Description | Selected |
|--------|-------------|----------|
| Fresh per job | New cron AgentRuntime per tick | ✓ |
| Once at startup + per-turn overrides | One cron runtime; extend run_turn for model/toolset | |

**User's choice:** Fresh per job — superseded by the global model change.

---

## Reuse vs rebuild (SUPERSEDED)

| Option | Description | Selected |
|--------|-------------|----------|
| Reuse shared durables, fresh budget+runner only | Lighter cron-specific constructor | ✓ |
| Full from_config per job | Pay rebuild cost each tick | |

**User's choice:** Reuse shared durables — superseded by the global model change.

---

## hermes-agent reference investigation

User asked how the reference handles subagent budgets and handoff. Findings:
- Each subagent gets its OWN budget capped at `delegation.max_iterations` (default 50); no shared parent↔child counter; total tree iterations can exceed any single cap (`tools/delegate_tool.py:507,997-1000,1968-1979`).
- "Handoff" = kanban task-board completion summary (`tools/kanban_tools.py`) + cross-platform session handoff (`hermes_state.py`); neither tied to budget exhaustion.
- No budget refresh/top-up mechanism exists.

This prompted the user to change direction.

---

## Global budget-model pivot (BINDING)

| Option | Description | Selected |
|--------|-------------|----------|
| Cron only | Cron subagents own budget; interactive PROV-10 unchanged | |
| Global — match reference everywhere | All subagents get own budget; retire PROV-10 | ✓ |
| Let me explain | — | |

| Option | Description | Selected |
|--------|-------------|----------|
| New delegation.max_iterations (default 50) | Dedicated authoritative knob | ✓ |
| Reuse agent.max_iterations | Size from existing 90 cap | |
| You decide | — | |

**User's choice:** Global change + per-subagent cap from `delegation.max_iterations` (default 50). (Discovered during write-up that this config field already exists.)

---

## DoS guard (BINDING)

| Option | Description | Selected |
|--------|-------------|----------|
| Reference-style: depth + concurrency + per-subagent cap | No tree-wide ceiling | ✓ |
| Keep an aggregate tree-wide backstop too | PROV-10-style hard upper bound retained | |
| You decide | — | |

**User's choice:** Reference-style — runaway bounded by depth × concurrency × per-subagent cap.

---

## Phase framing (BINDING)

| Option | Description | Selected |
|--------|-------------|----------|
| Keep Phase 35, broaden its goal | Update roadmap goal/title to the global change | ✓ |
| Keep Phase 35 narrow, note global change in CONTEXT | Leave title; record in CONTEXT | |
| Let me explain | — | |

**User's choice:** Keep Phase 35, broaden its goal.

---

## Handoff / budget-refresh disposition (BINDING)

User direction: "match subagents own iteration budget, defer the handoff and drop the budget refresh."
- Handoff → deferred to a future phase.
- Budget refresh → dropped entirely.

## Claude's Discretion

- Exact seam for constructing the fresh child `BudgetHandle` (in `run_child` vs runner construction).
- Whether the `AgentSubagentRunner.budget` field becomes vestigial after the change.

## Deferred Ideas

- Subagent handoff (pass off in-progress work) — future phase; reference analog is kanban handoff.
- Budget refresh / top-up to continue — dropped (tensions with DoS containment).

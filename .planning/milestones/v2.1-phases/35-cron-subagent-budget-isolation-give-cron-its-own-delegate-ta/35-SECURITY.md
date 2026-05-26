---
phase: 35
slug: cron-subagent-budget-isolation-give-cron-its-own-delegate-ta
status: verified
threats_open: 0
asvs_level: 1
created: 2026-05-21
---

# Phase 35 — Security

> Per-phase security contract: threat register, accepted risks, and audit trail.

---

## Trust Boundaries

| Boundary | Description | Data Crossing |
|----------|-------------|---------------|
| model/caller → `delegate_task` tool args | The `max_iterations` JSON arg is model-controlled untrusted input that sizes a child agent's iteration budget. | Untrusted integer (iteration count) |
| parent agent loop ↔ child subagent loop | Iteration budget accounting crosses here. Previously a child shared (and could drain) the parent's counter (PROV-10); now each side owns a distinct counter. | Iteration budget state |
| cron tick ↔ interactive runtime (shared `ToolRegistry` delegate runner) | Cron-spawned subagents reach the interactive delegate runner through the shared registry; the shared parent/child counter was the contamination vector. | Delegated task + budget state |

---

## Threat Register

| Threat ID | Category | Component | Disposition | Mitigation | Status |
|-----------|----------|-----------|-------------|------------|--------|
| T-35-01 | Elevation of Privilege / DoS | `delegate_task.rs::execute` / `execute_batch` `max_iterations` override | mitigate | Clamp-to-ceiling: `requested.min(config.max_iterations)` with `tracing::warn!` on overflow. Verified at `delegate_task.rs:325` (batch) and `:917` (single); test `test_per_call_max_iterations_clamps_to_ceiling` passing. | closed |
| T-28.1-16 | DoS | Cron subagent draining interactive headroom via shared `ToolRegistry` delegate runner | mitigate | Fresh per-child `BudgetHandle::new(max_iterations)` at `subagent_runner.rs:295` — shared `budget.clone()` removed (grep: 0 matches), so the contamination vector ceases to exist. Proven by D-07.2 cron-layer test `cron_subagent_budget_independence_from_interactive` (`runner.rs:748`, passing) and D-07.1 `test_independent_budget_child_drain_does_not_affect_parent` (`agent_loop.rs:2451`, passing). | closed |
| T-35-02 | DoS | Unbounded runaway delegation after removing the shared parent↔child counter | accept | No tree-wide ceiling by design (D-05). Runaway is bounded by `max_spawn_depth (1) × max_concurrent_children (3) × max_iterations (50) = 150`. Both factor guards already enforced + tested (Phase 32.2). Documented at `AGENT-RUNTIME-DESIGN.md:306`. See Accepted Risks Log. | closed |
| T-35-03 | DoS | Runaway-bound documentation drift | mitigate | `docs/AGENT-RUNTIME-DESIGN.md` §6.4/§8 amended to record the `max_spawn_depth × max_concurrent_children × max_iterations` bound and PROV-10 retirement (D-05), preventing reintroduction of a shared counter in future phases. | closed |
| T-35-SC | Tampering | npm/pip/cargo installs (supply chain) | n/a | No package installs in this phase — pure Rust source, test, and Markdown/comment edits; zero new dependencies (35-RESEARCH §Environment Availability). | closed |

*Status: open · closed*
*Disposition: mitigate (implementation required) · accept (documented risk) · transfer (third-party)*

---

## Accepted Risks Log

| Risk ID | Threat Ref | Rationale | Accepted By | Date |
|---------|------------|-----------|-------------|------|
| AR-35-01 | T-35-02 | Deliberate design decision D-05: no tree-wide iteration ceiling. Per-subagent independent budgets are the reference model (matches `hermes-agent` delegate_tool.py). Runaway remains bounded by the product `max_spawn_depth × max_concurrent_children × max_iterations` (1 × 3 × 50 = 150), with both factor guards enforced and tested in Phase 32.2. A tree-wide counter would reintroduce the PROV-10 contamination vector this phase removed. | bradwilson331@gmail.com | 2026-05-21 |

*Accepted risks do not resurface in future audit runs.*

---

## Security Audit Trail

| Audit Date | Threats Total | Closed | Open | Run By |
|------------|---------------|--------|------|--------|
| 2026-05-21 | 5 | 5 | 0 | gsd:secure-phase (plan-time register, mitigations cross-verified vs merged source + 35-VERIFICATION.md) |

---

## Sign-Off

- [x] All threats have a disposition (mitigate / accept / transfer)
- [x] Accepted risks documented in Accepted Risks Log
- [x] `threats_open: 0` confirmed
- [x] `status: verified` set in frontmatter

**Approval:** verified 2026-05-21

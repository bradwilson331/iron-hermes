---
phase: 35-cron-subagent-budget-isolation-give-cron-its-own-delegate-ta
plan: "01"
subsystem: ironhermes-tools / delegate_task
tags: [security, subagent, budget, iteration-cap, dos-prevention]
dependency_graph:
  requires: []
  provides: [D-03-clamp-to-ceiling, T-35-01-mitigated]
  affects: [delegate_task-execute, delegate_task-execute_batch]
tech_stack:
  added: []
  patterns: [clamp-via-min-with-overflow-warn, upstream-enforcement-before-run_child]
key_files:
  modified:
    - crates/ironhermes-tools/src/delegate_task.rs
decisions:
  - "D-03 Option B enforced upstream in delegate_task.rs (not in run_child): single-task path uses requested.min(self.config.max_iterations); batch path uses requested.min(config.max_iterations)"
  - "tracing::warn! emitted with structured fields (requested_max_iterations, ceiling) only when clamping fires — not on absent or within-ceiling requests"
  - "Refactored from if-else to .min() form to satisfy acceptance grep (non-comment .min(config.max_iterations) and .min(self.config.max_iterations))"
  - "TDD: RED commit (test_per_call_max_iterations_clamps_to_ceiling — case a fails) then GREEN commit (clamp implementation)"
metrics:
  duration_seconds: 270
  completed_date: "2026-05-22"
  tasks_completed: 1
  files_modified: 1
---

# Phase 35 Plan 01: Clamp delegate_task max_iterations to config ceiling (D-03 Option B) Summary

**One-liner:** Enforce `config.delegation.max_iterations` as a hard ceiling on caller/model-supplied `max_iterations` in both `delegate_task` resolution sites via `.min()` + `tracing::warn!` on overflow.

## What Was Built

Implemented D-03 Option B (clamp-to-ceiling) at both `max_iterations` override resolution sites in `crates/ironhermes-tools/src/delegate_task.rs`:

**Single-task path (`execute`, ~886-905):**
- A caller/model-supplied `max_iterations` above `self.config.max_iterations` is clamped to `requested.min(self.config.max_iterations)`
- A `tracing::warn!` fires with `requested_max_iterations` and `ceiling` fields when clamping occurs
- Values at or below the ceiling are honored verbatim (no warn)
- Absent `max_iterations` falls back to `self.config.max_iterations` (unchanged behavior)

**Batch path (`execute_batch`, ~308-328):**
- Identical clamp via `requested.min(config.max_iterations)` with `tracing::warn!` on overflow
- Values at or below honored verbatim; absent falls back to `config.max_iterations`

**Schema updates (both `max_iterations` properties):**
- Per-task schema property description updated to document ceiling semantics (D-03 Option B)
- Top-level schema property description updated likewise
- Both properties retain `"minimum": 1` — schema-shape tests stay green

**Test rewrite:**
- `test_per_call_max_iterations_overrides_config` renamed to `test_per_call_max_iterations_clamps_to_ceiling`
- Case (a): config ceiling=5, request 99 → `run_child` receives 5 (clamped)
- Case (b): config ceiling=99, request 3 → `run_child` receives 3 (honored verbatim)
- Reuses existing `IterCapture` mock that captures the value reaching `run_child`

## Acceptance Criteria Status

| Criterion | Status |
|-----------|--------|
| `min(self.config.max_iterations)` in non-comment code | PASS (1 occurrence) |
| `min(config.max_iterations)` in non-comment code (batch) | PASS (1 occurrence) |
| `tracing::warn!` count increased by 2 at clamp sites | PASS (6 → 8) |
| Both `max_iterations` schema properties with `"minimum": 1` retained | PASS (6 occurrences) |
| Test asserting request>ceiling clamped AND request<=ceiling honored | PASS |
| `cargo test -p ironhermes-tools max_iterations` exits 0 | PASS (2/2) |
| `cargo build -p ironhermes-tools` exits 0 | PASS |
| DoS-guard regressions: `test_batch_oversize_returns_err` | PASS |
| DoS-guard regressions: `test_orchestrator_at_max_depth_downgrades` | PASS |
| Full lib test suite: 357/357 pass | PASS |

## Commits

| Hash | Type | Description |
|------|------|-------------|
| `69ffa9bf` | test(35-01) | Add failing test for D-03 clamp-to-ceiling semantics (RED) |
| `3015a2a2` | feat(35-01) | Clamp delegate_task max_iterations to config ceiling (D-03 Option B) (GREEN) |

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Refactor] Replaced if-else with `.min()` to satisfy acceptance grep**
- **Found during:** Acceptance criteria check after GREEN implementation
- **Issue:** Initial GREEN implementation used if-else (ceiling/requested branching) which correctly clamped but did not produce `.min(self.config.max_iterations)` or `.min(config.max_iterations)` in non-comment source — causing the acceptance grep to return 0
- **Fix:** Refactored both sites to compute `requested.min(self.config.max_iterations)` / `requested.min(config.max_iterations)` after the conditional warn, satisfying both the behavioral requirement and the acceptance grep
- **Files modified:** `crates/ironhermes-tools/src/delegate_task.rs`
- **Commit:** `3015a2a2` (included in same GREEN commit)

None beyond the above refactor (the if-else → .min() form was a within-task refinement before the GREEN commit).

## TDD Gate Compliance

- RED gate: `test(35-01)` commit `69ffa9bf` — test existed and failed before implementation (case a: 99 passed through instead of being clamped to 5)
- GREEN gate: `feat(35-01)` commit `3015a2a2` — both cases pass after clamp implementation
- REFACTOR gate: Not needed — implementation was clean after the .min() form adjustment within GREEN

## Threat Surface Scan

No new network endpoints, auth paths, file access patterns, or schema changes at trust boundaries were introduced. This plan closes T-35-01 (Elevation of Privilege / DoS via unbounded `max_iterations` override) by adding upstream enforcement in `delegate_task.rs`. The threat was previously UNMITIGATED; it is now MITIGATED.

## Known Stubs

None. The clamp logic is fully wired — both resolution sites enforce the ceiling and the regression test captures the value reaching `run_child`.

## Self-Check: PASSED

- `crates/ironhermes-tools/src/delegate_task.rs` exists and contains both clamp sites
- Commit `69ffa9bf` exists (RED)
- Commit `3015a2a2` exists (GREEN)
- 357/357 lib tests pass

---
phase: 22-cli-feature-parity
plan: 02
subsystem: cli
tags: [hook-registry, jsonl, webhooks, retry-queue, event-lifecycle, observability]

# Dependency graph
requires:
  - phase: 22-cli-feature-parity
    plan: 01
    provides: tool registration parity, hooks_config in scope for both CLI paths
  - phase: 18-context-compression
    provides: HooksConfig, attach_context_engine with hooks parameter
provides:
  - HookRegistry construction with JSONL + webhook listeners in both run_chat and run_single
  - hook_registry wired into AgentLoop via .with_hook_registry() in run_agent_turn and run_single
  - hook_registry passed to attach_context_engine (replacing None) in both CLI paths
  - Retry queue drain on CLI startup in both paths
  - Static-grep regression tests locking all tool + hook wiring calls
affects: [hooks, cli, observability]

# Tech tracking
tech-stack:
  added: []
  patterns: [mirror-gateway-hook-registry-pattern, static-grep-regression-tests]

key-files:
  created:
    - crates/ironhermes-cli/tests/cli_tool_parity.rs
  modified:
    - crates/ironhermes-cli/src/main.rs

key-decisions:
  - "HookRegistry construction block identical across run_chat, run_single, and run_gateway for maintainability"
  - "Used .context() for RetryQueue init error in CLI (vs .expect() in gateway) for graceful error propagation"
  - "Brace-balanced function extraction in tests (matching run_chat_invariants.rs style) for robustness"

patterns-established:
  - "All three entry points (run_single, run_chat, run_gateway) construct HookRegistry identically: new, JSONL listener, webhook listeners, Arc wrap, drain"
  - "Static-grep tests in cli_tool_parity.rs lock both tool registration AND hook wiring calls against accidental removal"

requirements-completed: [CLI-01, D-05, D-06, D-07, D-09]

# Metrics
duration: 5min
completed: 2026-04-17
---

# Phase 22 Plan 02: CLI Feature Parity - HookRegistry Wiring Summary

**Wired HookRegistry with JSONL event logging, webhook listeners, and retry queue drain into both CLI paths (run_chat and run_single), closing the event lifecycle gap with run_gateway**

## Performance

- **Duration:** 5 min
- **Started:** 2026-04-17T20:35:28Z
- **Completed:** 2026-04-17T20:40:42Z
- **Tasks:** 2
- **Files modified:** 2

## Accomplishments
- Both CLI paths (run_chat and run_single) now construct HookRegistry with JSONL listener (D-06) and webhook listeners (D-07), matching run_gateway exactly
- AgentLoop receives hook_registry via .with_hook_registry() in both run_agent_turn and run_single, enabling lifecycle event firing (D-05)
- attach_context_engine receives Some(hook_registry) instead of None in both CLI paths, enabling context compression events to fire (D-09)
- Persistent retry queue drained on CLI startup in both paths, ensuring stale webhook events are purged (T-22-07 mitigation)
- 4 static-grep regression tests lock all wiring calls against accidental removal

## Task Commits

Each task was committed atomically:

1. **Task 1: Wire HookRegistry into run_chat and run_agent_turn** - `9eb6c37` (feat)
2. **Task 2: Wire HookRegistry into run_single and add static-grep regression tests** - `b312022` (feat)

## Files Created/Modified
- `crates/ironhermes-cli/src/main.rs` - Added HookRegistry construction, JSONL/webhook listeners, retry queue drain in run_chat and run_single; added hook_registry parameter to run_agent_turn; wired .with_hook_registry() and Some(hook_registry) into AgentLoop and attach_context_engine
- `crates/ironhermes-cli/tests/cli_tool_parity.rs` - 4 static-grep regression tests: tool wiring parity, hook_registry in run_agent_turn, attach_context_engine receives hook_registry (not None), active_skills sharing

## Decisions Made
- Mirrored gateway HookRegistry construction block identically for consistency (same order: new, JSONL, webhooks, Arc wrap, drain)
- Used `.context()` for RetryQueue initialization error in CLI paths (vs `.expect()` in gateway) for graceful error propagation through Result
- Adopted brace-balanced function extraction in tests (matching existing run_chat_invariants.rs style) instead of the simpler next-fn-marker approach from the plan, for robustness against edge cases

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered

None

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness
- Phase 22 (CLI feature parity) is now complete: Plan 01 delivered tool registration parity, Plan 02 delivered event hook lifecycle parity
- All CLI paths (run_chat, run_single) now have full gateway parity for tools, guardrails, and hooks
- 30 CLI tests passing (6 invariants + 10 skills + 4 tool parity + 4 chat memory + 6 others)
- Full workspace test suite passes (931+ tests, 0 failures)

## Self-Check: PASSED

- All files exist on disk
- All commit hashes found in git log

---
*Phase: 22-cli-feature-parity*
*Completed: 2026-04-17*

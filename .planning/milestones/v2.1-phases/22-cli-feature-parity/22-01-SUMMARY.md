---
phase: 22-cli-feature-parity
plan: 01
subsystem: cli
tags: [tool-registry, cron, skills, execute-code, guardrails, sandbox, blocklist]

# Dependency graph
requires:
  - phase: 19-skills
    provides: skills_tool, execute_code_tool_with_active_skills, credential_dir
  - phase: 18-context-compression
    provides: HooksConfig, BlocklistGuardrail, error_detail
provides:
  - Full tool registration parity in run_chat and run_single matching run_gateway
  - Shared active_skills Arc between skills_tool and execute_code in CLI paths
  - RPC dispatch registry with safe subset (file, web, memory) for CLI sandbox
  - BlocklistGuardrail and error_detail enforcement in CLI paths
affects: [22-02-PLAN, hooks, guardrails, cli]

# Tech tracking
tech-stack:
  added: []
  patterns: [mirror-gateway-registration-pattern, shared-active-skills-arc, separate-arc-wrap]

key-files:
  created: []
  modified:
    - crates/ironhermes-cli/src/main.rs

key-decisions:
  - "Mirrored run_gateway registration order exactly for consistency and maintainability"
  - "Separated Arc::new(registry) into explicit statement in run_single for clarity and pattern consistency"
  - "hooks_config kept in scope for Plan 02 HookRegistry construction"

patterns-established:
  - "All three entry points (run_single, run_chat, run_gateway) follow identical tool registration order: memory, delegate_task, cron, skills, rpc_registry, execute_code, guardrails, Arc wrap"

requirements-completed: [CLI-01, D-02, D-03, D-04, D-08]

# Metrics
duration: 3min
completed: 2026-04-17
---

# Phase 22 Plan 01: CLI Feature Parity - Tool Registration Summary

**Wired cron_tool, skills_tool, execute_code_tool (with shared active_skills Arc), and BlocklistGuardrail into both run_chat and run_single, closing the tool surface gap with run_gateway**

## Performance

- **Duration:** 3 min
- **Started:** 2026-04-17T20:29:57Z
- **Completed:** 2026-04-17T20:33:07Z
- **Tasks:** 2
- **Files modified:** 1

## Accomplishments
- Both CLI paths (run_chat and run_single) now register the full tool surface matching run_gateway
- Shared active_skills Arc ensures skill-declared env vars pass through to execute_code sandbox correctly
- RPC dispatch registry restricted to safe subset (file tools, web tools, memory tool) per D-04 -- no terminal or execute_code in sandbox
- BlocklistGuardrail enforced in CLI paths, preventing blocked tool execution
- error_detail level from HooksConfig applied to CLI tool registry

## Task Commits

Each task was committed atomically:

1. **Task 1: Add tool registration parity to run_chat** - `230fdfe` (feat)
2. **Task 2: Add tool registration parity to run_single** - `75f631b` (feat)

## Files Created/Modified
- `crates/ironhermes-cli/src/main.rs` - Added cron_tool, skills_tool, execute_code_tool, and guardrail registration to both run_chat and run_single functions

## Decisions Made
- Mirrored run_gateway registration order exactly (cron, skills, rpc_registry, execute_code, guardrails) for consistency and ease of future maintenance
- Separated `Arc::new(registry)` into an explicit statement in run_single (was previously inline in AgentLoop::new) for pattern consistency with run_chat and run_gateway
- Kept `hooks_config` variable in scope after guardrail registration so Plan 02 can use it for HookRegistry construction without re-loading

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered

None

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness
- hooks_config is in scope in both run_chat and run_single, ready for Plan 02's HookRegistry wiring
- All tool registrations verified with cargo check and cargo test (10 tests pass)
- Both CLI paths now have full gateway parity for tools; Plan 02 will add event hook lifecycle parity

## Self-Check: PASSED

- All files exist on disk
- All commit hashes found in git log

---
*Phase: 22-cli-feature-parity*
*Completed: 2026-04-17*

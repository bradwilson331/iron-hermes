---
phase: 08-code-execution
plan: 02
subsystem: exec
tags: [execute-code-tool, tool-trait, rpc-dispatch, registry-wiring, integration-tests]

# Dependency graph
requires:
  - phase: 08-code-execution
    plan: 01
    provides: "ironhermes-exec crate with Sandbox, RpcServer, ToolDispatch trait"
provides:
  - "ExecuteCodeTool implementing Tool trait in ironhermes-tools"
  - "RegistryDispatch adapter bridging ToolRegistry to ToolDispatch"
  - "register_execute_code_tool method on ToolRegistry"
  - "execute_code wired into CLI gateway with RPC-safe tool subset"
affects: [08-code-execution plan 03 (integration/verification)]

# Tech tracking
tech-stack:
  added: []
  patterns: [registry-dispatch-adapter, rpc-safe-tool-subset, defense-in-depth-recursion-prevention]

key-files:
  created:
    - crates/ironhermes-tools/src/execute_code.rs
  modified:
    - crates/ironhermes-tools/Cargo.toml
    - crates/ironhermes-tools/src/lib.rs
    - crates/ironhermes-tools/src/registry.rs
    - crates/ironhermes-core/src/lib.rs
    - crates/ironhermes-cli/src/main.rs

key-decisions:
  - "RegistryDispatch adapter lives in ironhermes-tools to avoid circular crate dependency"
  - "Separate RPC registry built with only D-07 safe tools - structurally excludes terminal and execute_code"
  - "ExecConfig re-exported from ironhermes-core crate root for ergonomic access"

patterns-established:
  - "RPC-safe registry pattern: build separate ToolRegistry with only sandbox-allowed tools, wrap in Arc, pass to ExecuteCodeTool"
  - "RegistryDispatch adapter: thin wrapper implementing ToolDispatch trait by delegating to ToolRegistry::dispatch"

requirements-completed: [EXEC-01, EXEC-02, EXEC-03, EXEC-04]

# Metrics
duration: 4min
completed: 2026-04-10
---

# Phase 8 Plan 02: ExecuteCodeTool + Registry Wiring Summary

**ExecuteCodeTool implementing Tool trait with RegistryDispatch adapter, RPC-safe tool subset, and 5 integration tests proving end-to-end execution with Python RPC tool calls**

## Performance

- **Duration:** 4 min
- **Started:** 2026-04-10T14:55:45Z
- **Completed:** 2026-04-10T14:59:47Z
- **Tasks:** 2
- **Files modified:** 6

## Accomplishments
- Created ExecuteCodeTool implementing the Tool trait with name "execute_code", toolset "code"
- Built RegistryDispatch adapter that bridges ToolRegistry to the ToolDispatch trait (avoids circular crate deps)
- Tool result format: [stdout]/[stderr]/[exit_code] sections with [timed out] prefix on timeout
- Added register_execute_code_tool method to ToolRegistry for clean registration API
- Wired execute_code into CLI gateway (run_gateway) with separate RPC-safe registry containing only D-07 tools
- RPC registry includes: read_file, write_file, patch, search_files, web_search, web_read, memory (7 tools)
- RPC registry excludes: terminal, execute_code (defense-in-depth against sandbox escape and recursion)
- 5 integration tests: basic execution, RPC tool calls from Python, timeout formatting, result formatting, missing param error
- Re-exported ExecConfig from ironhermes-core crate root

## Task Commits

Each task was committed atomically:

1. **Task 1: Create ExecuteCodeTool + RegistryDispatch adapter + register method** - `bc8f565` (feat)
2. **Task 2: Wire execute_code into CLI and gateway registries + integration tests** - `04c3085` (feat)

## Files Created/Modified
- `crates/ironhermes-tools/src/execute_code.rs` - ExecuteCodeTool, RegistryDispatch adapter, 5 tests (225 lines)
- `crates/ironhermes-tools/Cargo.toml` - Added ironhermes-exec dependency
- `crates/ironhermes-tools/src/lib.rs` - Added pub mod execute_code
- `crates/ironhermes-tools/src/registry.rs` - Added register_execute_code_tool method
- `crates/ironhermes-core/src/lib.rs` - Re-exported ExecConfig from crate root
- `crates/ironhermes-cli/src/main.rs` - Wired execute_code in run_gateway with RPC-safe registry

## Decisions Made
- RegistryDispatch adapter lives in ironhermes-tools (not ironhermes-exec) to avoid circular crate dependency -- ironhermes-exec defines ToolDispatch trait, ironhermes-tools implements it
- Separate RPC registry built with only D-07 safe tools -- structurally prevents terminal and execute_code from being called via sandbox RPC, even if ALLOWED_TOOLS check in rpc_server.rs were bypassed (T-08-09 mitigation)
- ExecConfig re-exported from ironhermes-core crate root for ergonomic use across crates
- Gateway gets execute_code automatically since CLI builds the registry and passes Arc to GatewayRunner

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] ExecConfig not re-exported from ironhermes-core**
- **Found during:** Task 1
- **Issue:** `ironhermes_core::ExecConfig` was not accessible because ExecConfig was only exported from the config module, not re-exported from the crate root
- **Fix:** Added `ExecConfig` to the `pub use config::{Config, ExecConfig}` line in ironhermes-core/src/lib.rs
- **Files modified:** crates/ironhermes-core/src/lib.rs
- **Committed in:** bc8f565

---

**Total deviations:** 1 auto-fixed (1 blocking)
**Impact on plan:** Trivial -- single re-export line. All planned functionality delivered.

## Threat Surface

T-08-09 (Elevation of Privilege via RPC registry) mitigated: RPC registry is built with only 7 D-07 safe tools. `terminal` and `execute_code` are structurally absent from the RPC registry -- they cannot be called even if the ALLOWED_TOOLS allowlist in rpc_server.rs were bypassed. This provides defense-in-depth alongside Plan 01's server-side allowlist.

T-08-10 (Recursive execute_code) mitigated: execute_code is not registered in the RPC registry, AND is excluded from ALLOWED_TOOLS. Double barrier prevents recursion.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- ExecuteCodeTool fully wired and tested
- All EXEC requirements (EXEC-01 through EXEC-04) satisfied across Plans 01 and 02
- Ready for Plan 03 integration verification if applicable

---
## Self-Check: PASSED

All 6 created/modified files verified present. Both task commits verified in git log (bc8f565, 04c3085).

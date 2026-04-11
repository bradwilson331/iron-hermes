---
phase: 08-code-execution
plan: 01
subsystem: exec
tags: [sandbox, python, json-rpc, unix-domain-socket, tokio, process-isolation]

# Dependency graph
requires:
  - phase: 07-skills-system
    provides: "SkillsConfig pattern in config.rs used as template for ExecConfig"
provides:
  - "ironhermes-exec crate with Sandbox, RpcServer, SandboxConfig, ToolDispatch trait"
  - "ExecConfig in ironhermes-core config with python_path, timeout, limits"
  - "hermes_tools.py Python helper module for RPC tool access"
  - "JSON-RPC 2.0 server over UDS with call limiting and tool allowlist"
affects: [08-code-execution plan 02 (ExecuteCodeTool), 08-code-execution plan 03 (integration)]

# Tech tracking
tech-stack:
  added: [tempfile]
  patterns: [sandbox-process-isolation, uds-json-rpc, embedded-python-helper]

key-files:
  created:
    - crates/ironhermes-exec/Cargo.toml
    - crates/ironhermes-exec/src/lib.rs
    - crates/ironhermes-exec/src/sandbox.rs
    - crates/ironhermes-exec/src/rpc_server.rs
    - crates/ironhermes-exec/src/hermes_tools.py
  modified:
    - Cargo.toml
    - crates/ironhermes-core/src/config.rs

key-decisions:
  - "ToolDispatch trait in ironhermes-exec decouples from ironhermes-tools, avoiding circular dependency"
  - "RpcServer implemented in Task 2 alongside Sandbox since they are tightly coupled (sandbox spawns RPC server)"
  - "SandboxConfig separate from ExecConfig to allow runtime override without config dependency"

patterns-established:
  - "Sandbox pattern: tempdir lifecycle, env_clear + allowlist, concurrent stdout/stderr drain, timeout + kill_on_drop"
  - "UDS RPC pattern: single-connection server, newline-delimited JSON-RPC 2.0, AtomicU32 call counter"
  - "Embedded Python helper via include_str! written to tempdir at execution time"

requirements-completed: [EXEC-01, EXEC-02, EXEC-03, EXEC-04]

# Metrics
duration: 3min
completed: 2026-04-10
---

# Phase 8 Plan 01: Exec Crate Foundation Summary

**ironhermes-exec crate with Python sandbox (env-stripped, timeout-enforced), JSON-RPC 2.0 UDS server with tool allowlist and call limiting, and bundled hermes_tools.py helper**

## Performance

- **Duration:** 3 min
- **Started:** 2026-04-10T14:43:18Z
- **Completed:** 2026-04-10T14:46:49Z
- **Tasks:** 3
- **Files modified:** 7

## Accomplishments
- Created ironhermes-exec crate with Sandbox, RpcServer, ToolDispatch trait, and SandboxConfig
- Sandbox spawns Python with env_clear() + 6 allowlisted vars, enforces timeout via SIGKILL, truncates stdout at configurable limit
- JSON-RPC 2.0 server over UDS dispatches only ALLOWED_TOOLS (7 tools), enforces call limit with AtomicU32 counter
- hermes_tools.py provides synchronous Python functions (read_file, write_file, web_search, etc.) with HermesRpcError/HermesCallLimitError exceptions
- ExecConfig added to ironhermes-core config with python_path, timeout_secs, max_rpc_calls, max_output_bytes
- 10 tests passing: 6 sandbox (simple exec, env stripping, timeout, truncation, stderr, nonzero exit) + 4 RPC integration (tool call, call limit, error handling, unknown method)

## Task Commits

Each task was committed atomically:

1. **Task 1: Create ironhermes-exec crate skeleton + ExecConfig** - `2944c95` (feat)
2. **Task 2: Implement Sandbox + RpcServer** - `89ae02f` (feat)
3. **Task 3: Implement hermes_tools.py + RPC integration tests** - `b59c2f2` (feat)

## Files Created/Modified
- `Cargo.toml` - Added ironhermes-exec to workspace members
- `crates/ironhermes-exec/Cargo.toml` - Crate manifest with tokio, serde, tempfile deps
- `crates/ironhermes-exec/src/lib.rs` - Crate root with ToolDispatch trait, SandboxConfig, HERMES_TOOLS_PY constant
- `crates/ironhermes-exec/src/sandbox.rs` - Sandbox struct: process spawning, env stripping, timeout, output truncation (320 lines)
- `crates/ironhermes-exec/src/rpc_server.rs` - UDS JSON-RPC server with ALLOWED_TOOLS allowlist and call counter (296 lines)
- `crates/ironhermes-exec/src/hermes_tools.py` - Python helper module with tool functions and error classes (112 lines)
- `crates/ironhermes-core/src/config.rs` - Added ExecConfig struct with defaults and tests

## Decisions Made
- ToolDispatch trait in ironhermes-exec decouples from ironhermes-tools, avoiding circular crate dependency. ExecuteCodeTool (Plan 02) will implement this trait.
- RpcServer implemented alongside Sandbox (Task 2) since they are structurally coupled -- Sandbox spawns the RPC server task. Tests for RPC added in Task 3.
- SandboxConfig is a runtime struct separate from ExecConfig (serde config) to allow callers to override values without depending on ironhermes-core.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] RpcServer implemented in Task 2 instead of Task 3**
- **Found during:** Task 2
- **Issue:** Sandbox::run() needs to spawn RpcServer, so the real RpcServer implementation was required for sandbox tests to work
- **Fix:** Implemented full RpcServer in Task 2 alongside Sandbox. Task 3 focused on hermes_tools.py and integration tests.
- **Files modified:** crates/ironhermes-exec/src/rpc_server.rs
- **Verification:** All 10 tests pass
- **Committed in:** 89ae02f

---

**Total deviations:** 1 auto-fixed (1 blocking)
**Impact on plan:** Minor task boundary shift. All planned functionality delivered. No scope creep.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- ironhermes-exec crate is complete and ready for Plan 02 (ExecuteCodeTool wrapper in ironhermes-tools)
- ToolDispatch trait ready to be implemented by ExecuteCodeTool with Arc<ToolRegistry>
- All sandbox, RPC, and Python helper infrastructure tested and working

---
## Self-Check: PASSED

All 6 created files verified present. All 3 task commits verified in git log.

---
*Phase: 08-code-execution*
*Completed: 2026-04-10*

---
phase: 08-code-execution
plan: 03
subsystem: exec-sandbox
tags: [gap-closure, sandbox, security, env-stripping, process-management]
dependency_graph:
  requires: [08-02]
  provides: [sandbox-env-stripping, kill-strategy, stderr-cap, duration-tracking, tool-calls-counter]
  affects: [08-04]
tech_stack:
  added: [libc]
  patterns: [pattern-based-env-exclusion, process-group-management, sigterm-sigkill-escalation]
key_files:
  created: []
  modified:
    - crates/ironhermes-exec/src/hermes_tools.py
    - crates/ironhermes-exec/src/sandbox.rs
    - crates/ironhermes-exec/src/lib.rs
    - crates/ironhermes-exec/src/rpc_server.rs
    - crates/ironhermes-exec/Cargo.toml
    - crates/ironhermes-core/src/config.rs
    - crates/ironhermes-tools/src/execute_code.rs
decisions:
  - Used pattern-based env exclusion over allowlist for forward compatibility
  - Kept CommandExt import with allow(unused_imports) since tokio re-exports it
metrics:
  duration: 316s
  completed: 2026-04-11T03:35:25Z
  tasks: 3/3
  files: 7
---

# Phase 8 Plan 03: Spec-Alignment Gap Closure Summary

Pattern-based env stripping, process group kill strategy (SIGTERM->5s->SIGKILL), 10KB stderr cap, duration/call_count wiring, and Python function signature alignment with spec D-20..D-37.

## Commits

| Task | Commit | Description |
|------|--------|-------------|
| 1 | 1ed4709 | Update hermes_tools.py function signatures (patch, web_search, search_files, web_read) |
| 2 | eb26f06 | Structural changes: SandboxConfig, SandboxResult, RpcServer, ExecConfig, libc dep |
| 3 | 9e176eb | Env stripping, kill strategy, stderr cap, duration + call_count wiring |

## Changes Made

### Task 1: Python Function Signatures (D-20..D-24)
- `patch(path, old_string, new_string, replace_all=False)` — replaces old `patch(path, diff)`
- `web_search(query, limit=10)` — adds limit parameter
- `search_files(pattern, path=".", file_glob=None, limit=None)` — adds file_glob and limit
- `web_read(urls)` — accepts single URL string or list of URLs

### Task 2: Structural Changes
- **SandboxConfig**: Added `max_stderr_bytes: usize` (default 10,240)
- **SandboxResult**: Added `tool_calls_made: u32` and `duration_seconds: f64`
- **RpcServer::new()**: Now accepts shared `Arc<AtomicU32>` call_count parameter
- **ExecConfig**: Added `max_stderr_bytes: usize` (default 10,240)
- **Cargo.toml**: Added `libc = "0.2"` dependency
- Updated all test helpers and ironhermes-tools SandboxConfig construction

### Task 3: Sandbox::run() Rewrite
- **Env stripping (D-35..D-37)**: Pattern-based exclusion strips vars containing KEY/TOKEN/SECRET/PASSWORD/CREDENTIAL/PASSWD/AUTH. Safe system vars (PATH, HOME, LANG, SHELL, USER, etc.) and XDG_* always pass through.
- **Kill strategy (D-31..D-34)**: Child runs in own process group via `setpgid(0,0)`. On timeout: SIGTERM to process group, 5s grace, SIGKILL to process group.
- **Stderr cap (D-28..D-30)**: `maybe_truncate_stderr()` caps at `max_stderr_bytes` with "[stderr truncated at 10KB]" notice.
- **Duration + call_count (D-25)**: `Instant::now()` before spawn, `elapsed().as_secs_f64()` in result. Shared `Arc<AtomicU32>` counter read after execution.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Fixed missing max_stderr_bytes in ironhermes-tools**
- **Found during:** Task 2 verification
- **Issue:** `crates/ironhermes-tools/src/execute_code.rs` constructs SandboxConfig without the new field
- **Fix:** Added `max_stderr_bytes: self.config.max_stderr_bytes` to the construction
- **Files modified:** crates/ironhermes-tools/src/execute_code.rs
- **Commit:** 9e176eb

**2. [Rule 1 - Bug] Fixed unsafe env var operations in Rust 2024 edition**
- **Found during:** Task 3 test compilation
- **Issue:** `std::env::set_var`/`remove_var` require unsafe blocks in Rust 2024 edition
- **Fix:** Wrapped test env operations in unsafe blocks with safety comment
- **Files modified:** crates/ironhermes-exec/src/sandbox.rs
- **Commit:** 9e176eb

## Verification

- `cargo check -p ironhermes-exec -p ironhermes-core -p ironhermes-tools` — passes with no errors or warnings
- `cargo test -p ironhermes-exec -- --test-threads=1` — 12 tests pass
- All spec gaps D-20..D-37 addressed

## Test Results

```
test rpc_server::tests::test_call_limit ... ok
test rpc_server::tests::test_rpc_error_handling ... ok
test rpc_server::tests::test_rpc_tool_call ... ok
test rpc_server::tests::test_unknown_method ... ok
test sandbox::tests::test_duration_seconds_populated ... ok
test sandbox::tests::test_env_stripping ... ok
test sandbox::tests::test_execute_simple_script ... ok
test sandbox::tests::test_nonzero_exit ... ok
test sandbox::tests::test_output_truncation ... ok
test sandbox::tests::test_stderr_captured ... ok
test sandbox::tests::test_stderr_truncation ... ok
test sandbox::tests::test_timeout_kills_process ... ok
test result: ok. 12 passed; 0 failed; 0 ignored
```

---
phase: 08-code-execution
plan: 04
subsystem: exec-response-format
tags: [gap-closure, json-response, cancellation, user-interruption]
dependency_graph:
  requires: [08-03]
  provides: [json-response-format, cancellation-token-support, interrupted-status]
  affects: []
tech_stack:
  added: [tokio-util (ironhermes-exec)]
  patterns: [tokio-select-three-way-race, structured-json-tool-response, cancellation-token-propagation]
key_files:
  created: []
  modified:
    - crates/ironhermes-exec/src/sandbox.rs
    - crates/ironhermes-exec/src/lib.rs
    - crates/ironhermes-exec/src/rpc_server.rs
    - crates/ironhermes-exec/Cargo.toml
    - crates/ironhermes-tools/src/execute_code.rs
    - crates/ironhermes-tools/src/registry.rs
    - crates/ironhermes-tools/Cargo.toml
decisions:
  - Used tokio::select! three-way race (completion vs timeout vs cancellation) instead of nested timeouts
  - CancellationToken wired as Option to maintain backward compat (None = no cancellation)
  - Gateway CancellationToken creation deferred to Phase 8 follow-up (TODO comment in place)
metrics:
  duration: 234s
  completed: 2026-04-11T03:41:16Z
  tasks: 2/2
  files: 7
---

# Phase 8 Plan 04: Response Format + User Interruption Summary

Structured JSON response from ExecuteCodeTool (status/output/stderr/exit_code/tool_calls_made/duration_seconds) and CancellationToken support in Sandbox::run for user interruption (D-25..D-27, D-38..D-40).

## Commits

| Task | Commit | Description |
|------|--------|-------------|
| 1 | ed264a8 | Add CancellationToken support to Sandbox::run |
| 2 | ecd25e6 | JSON response format and cancellation wiring in ExecuteCodeTool |

## Changes Made

### Task 1: CancellationToken in Sandbox::run (D-38, D-39)
- Added `interrupted: bool` field to `SandboxResult`
- Changed `Sandbox::run` signature to accept `Option<CancellationToken>` as 4th parameter
- Replaced `tokio::time::timeout` with `tokio::select!` three-way race:
  - Child completion (normal path)
  - Timeout (SIGTERM -> 5s -> SIGKILL, existing behavior)
  - Cancellation (SIGKILL immediately, no grace period)
- On interruption: returns `stderr = "[execution interrupted -- user sent a new message]"`, `interrupted = true`
- Added `tokio-util` dependency to ironhermes-exec
- Re-exported `CancellationToken` from `ironhermes_exec` crate root
- Updated all callers in sandbox.rs, rpc_server.rs tests to pass `None`

### Task 2: JSON Response Format (D-25..D-27, D-40)
- Removed old text format (`[stdout]`, `[stderr]`, `[exit_code: N]`)
- Returns structured JSON: `{"status", "output", "exit_code", "tool_calls_made", "duration_seconds"}`
- `stderr` field included only when non-empty
- Status values: `"success"` (exit 0), `"error"` (non-zero), `"timeout"`, `"interrupted"`
- Added `cancel_token: Option<CancellationToken>` field to `ExecuteCodeTool`
- Updated constructor to 3-arg signature; all call sites pass `None` for now
- All 5 tests rewritten to parse JSON and validate structure
- Added `tokio-util` dependency to ironhermes-tools

## Deviations from Plan

None - plan executed exactly as written.

## Verification

- `cargo test -p ironhermes-exec -- --test-threads=1` -- 12 tests pass
- `cargo test -p ironhermes-tools -- execute_code --test-threads=1` -- 6 tests pass
- `cargo check --workspace` -- compiles with no errors (2 pre-existing warnings in ironhermes-cli)
- No remaining `[stdout]`/`[stderr]`/`[exit_code:]` text format in execute_code.rs

## Self-Check: PASSED

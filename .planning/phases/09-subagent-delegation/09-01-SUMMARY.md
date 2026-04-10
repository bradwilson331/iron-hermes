---
phase: 09-subagent-delegation
plan: 01
subsystem: tools/agent
tags: [delegation, isolation, subagent, tool-registry]
dependency_graph:
  requires: []
  provides: [DelegateTaskTool, SubagentConfig, SubagentRunner-trait, TerminalTool-CWD, MemoryTool-read-only]
  affects: [ironhermes-tools, ironhermes-core]
tech_stack:
  added: [SubagentRunner-trait-abstraction]
  patterns: [trait-based-dependency-inversion, semaphore-concurrency-control, tempdir-isolation]
key_files:
  created:
    - crates/ironhermes-tools/src/delegate_task.rs
  modified:
    - crates/ironhermes-core/src/config.rs
    - crates/ironhermes-core/src/lib.rs
    - crates/ironhermes-tools/src/terminal.rs
    - crates/ironhermes-tools/src/memory_tool.rs
    - crates/ironhermes-tools/src/registry.rs
    - crates/ironhermes-tools/src/lib.rs
    - crates/ironhermes-tools/Cargo.toml
decisions:
  - id: D-CIRCULAR-DEP
    summary: "Used SubagentRunner trait in ironhermes-tools instead of importing AgentLoop from ironhermes-agent to avoid circular crate dependency. Mirrors ToolDispatch pattern from execute_code."
metrics:
  duration: 333s
  completed: "2026-04-10T17:37:03Z"
  tasks_completed: 2
  tasks_total: 2
  tests_added: 31
  files_changed: 8
---

# Phase 09 Plan 01: DelegateTaskTool Core Summary

DelegateTaskTool with SubagentRunner trait abstraction, child registry allowlist filtering, semaphore concurrency control, tempdir CWD isolation, and read-only memory mode

## What Was Built

### Task 1: SubagentConfig, TerminalTool CWD, MemoryTool read-only (b4ada62)

- **SubagentConfig** added to `config.rs` with defaults: `timeout_secs=300`, `max_subagents=3`, `max_iterations=10`. Exported from `ironhermes-core`. Backward-compatible via `serde(default)`.
- **TerminalTool** converted from unit struct to struct with `cwd: Option<PathBuf>`. Added `new()` (no CWD, existing behavior) and `with_cwd(PathBuf)` (isolated directory). `execute()` conditionally sets `cmd.current_dir()`.
- **MemoryTool** gained `read_only: bool` field. `new()` preserves existing behavior (`read_only: false`). `new_read_only()` blocks `add`, `replace`, `remove` actions with descriptive error message.
- **tempfile** moved from dev-dependency to runtime dependency for TempDir usage in delegate_task.

### Task 2: DelegateTaskTool (3a8fb07)

- **SubagentRunner trait** defined in `delegate_task.rs` to break the circular dependency between `ironhermes-tools` and `ironhermes-agent`. Mirrors the `ToolDispatch` pattern from `execute_code.rs`.
- **build_child_registry()** constructs filtered ToolRegistry from allowlist:
  - `delegate_task` silently stripped (AGENT-05: no recursion)
  - `skills`, `execute_code`, `cronjob` silently stripped
  - `terminal` gets isolated temp CWD (AGENT-04)
  - `memory` gets read-only mode (D-12)
  - Unknown tool names cause immediate error (D-04)
- **DelegateTaskTool** implements Tool trait with:
  - Schema: required `task` string, optional `allowed_tools` array
  - Semaphore-based concurrency limiting (D-14, D-16)
  - TempDir creation for child CWD isolation (D-10, D-13)
  - `tokio::time::timeout` wrapping child execution (D-08)
  - Falls back to `DEFAULT_SAFE_TOOLS` when no allowlist provided (D-02)
- **register_delegate_task_tool()** added to ToolRegistry for wiring in CLI/gateway (Plan 02).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Circular dependency between ironhermes-tools and ironhermes-agent**
- **Found during:** Task 2
- **Issue:** Plan noted that `ironhermes-agent` depends on `ironhermes-tools`, so `DelegateTaskTool` in `ironhermes-tools` cannot import `AgentLoop` from `ironhermes-agent`.
- **Fix:** Defined `SubagentRunner` async trait in `ironhermes-tools/src/delegate_task.rs` (following the `ToolDispatch` pattern from `execute_code.rs`). `ironhermes-agent` will implement this trait in Plan 02.
- **Files modified:** `crates/ironhermes-tools/src/delegate_task.rs`
- **Commit:** 3a8fb07

## Verification

- `cargo test -p ironhermes-core config::tests` -- 11 passed (3 new SubagentConfig tests)
- `cargo test -p ironhermes-tools terminal::tests` -- 3 passed (all new)
- `cargo test -p ironhermes-tools memory_tool::tests` -- 9 passed (3 new read-only tests)
- `cargo test -p ironhermes-tools delegate_task` -- 18 passed (all new)
- `cargo test --workspace --lib` -- 103 passed, 0 failed

## Threat Mitigations Verified

| Threat ID | Mitigation | Verified |
|-----------|-----------|----------|
| T-09-01 | delegate_task excluded from child registry | test_build_child_registry_strips_delegate_task |
| T-09-02 | Semaphore + timeout concurrency control | test_delegate_task_timeout |
| T-09-03 | MemoryTool read-only blocks writes | test_read_only_blocks_add, test_read_only_blocks_remove |
| T-09-04 | TerminalTool isolated CWD | test_terminal_with_cwd, test_build_child_registry_terminal_gets_cwd |
| T-09-06 | Skills tool stripped from child | test_build_child_registry_strips_skills |
| T-09-07 | execute_code stripped from child | test_build_child_registry_strips_execute_code |

## Self-Check: PASSED

All 8 files verified present. Both commits (b4ada62, 3a8fb07) verified in git log.

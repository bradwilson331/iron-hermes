---
phase: 09-subagent-delegation
plan: 04
subsystem: agent-delegation
tags: [cancellation, detach, progress, cli-tree-view]
dependency_graph:
  requires: [09-03]
  provides: [cancellation-propagation, detach-flag, cli-progress-display]
  affects: [agent_loop, delegate_task, subagent_runner, cli-main]
tech_stack:
  added: [tokio-util/CancellationToken in ironhermes-agent]
  patterns: [child_token hierarchy, tokio::select cancellation, progress callback routing]
key_files:
  created: []
  modified:
    - crates/ironhermes-agent/Cargo.toml
    - crates/ironhermes-agent/src/agent_loop.rs
    - crates/ironhermes-agent/src/subagent_runner.rs
    - crates/ironhermes-tools/src/delegate_task.rs
    - crates/ironhermes-tools/src/registry.rs
    - crates/ironhermes-cli/Cargo.toml
    - crates/ironhermes-cli/src/main.rs
decisions:
  - "CancellationToken as Option<CancellationToken> field for backward compat (None = no cancellation)"
  - "ChildToolProgressCallback type alias with explicit 'static bound to work with async_trait"
  - "SubagentProgressCallback uses Arc<dyn Fn> for cloneability across spawned batch tasks"
  - "Gateway gets CancellationToken but no progress callback (D-20: tracing::info only)"
metrics:
  duration_seconds: 1104
  completed: "2026-04-11T05:37:14Z"
  tasks_completed: 2
  tasks_total: 2
  files_modified: 7
---

# Phase 09 Plan 04: CancellationToken Propagation, Detach Flag, CLI Tree-View Summary

CancellationToken hierarchy from parent to child agents with detach opt-out, plus CLI tree-view progress display for subagent tool calls using colored stderr output.

## Commits

| Task | Commit  | Description                                                        |
| ---- | ------- | ------------------------------------------------------------------ |
| 1    | 9808b0e | Add CancellationToken propagation and detach flag for subagents    |
| 2    | 8988765 | Add CLI tree-view progress display and gateway progress batching   |

## Task Details

### Task 1: CancellationToken support in AgentLoop and SubagentRunner

- Added `cancel_token: Option<CancellationToken>` field to `AgentLoop` struct
- Added `with_cancellation_token()` builder method
- Cancellation checked at two points in `run()`:
  1. Top of iteration loop (before LLM call) via `token.is_cancelled()`
  2. During LLM call via `tokio::select!` with `token.cancelled()`
- Both paths return `AgentResult` with `finished_naturally: false` and `final_response: "Cancelled by parent"`
- Updated `SubagentRunner::run_child` trait to accept `cancel_token: Option<CancellationToken>`
- Added `parent_cancel_token: Option<CancellationToken>` field to `DelegateTaskTool`
- Added `detach` boolean to tool schema (D-22): when true, child gets None cancel token (survives parent interrupt)
- When detach=false (default), child gets `parent_token.child_token()` (D-21): cancelling parent cascades to child
- Batch mode applies same detach logic per task from top-level detach parameter
- `AgentSubagentRunner` forwards cancel_token to `AgentLoop::with_cancellation_token()`
- CLI main.rs creates `CancellationToken` for chat and gateway modes, passes to `register_delegate_task_tool`
- Added `tokio-util` dependency to `ironhermes-agent` and `ironhermes-cli` Cargo.toml
- Updated `register_delegate_task_tool` signature to accept `Option<CancellationToken>`
- All 9 test MockRunner implementations updated with new `cancel_token` parameter
- 2 new tests: `test_agent_loop_with_cancellation_token_sets_token`, `test_agent_loop_run_returns_early_when_cancelled_before_first_iteration`

### Task 2: CLI tree-view progress display and gateway progress batching

- Added `SubagentProgress` enum with `Started`, `ToolCall`, `Completed` variants (D-19)
- Added `SubagentProgressCallback` type: `Arc<dyn Fn(usize, SubagentProgress) + Send + Sync>`
- Added `ChildToolProgressCallback` type with explicit `'static` bound for async_trait compatibility
- Added `progress_callback: Option<SubagentProgressCallback>` field to `DelegateTaskTool`
- Added `with_progress_callback()` builder method
- Single-task `execute()` emits Started (with 50-char truncated task summary per T-09-16), ToolCall per tool, Completed
- Batch `execute_batch()` emits per-child Started/ToolCall/Completed with correct child index
- Per-child `ChildToolProgressCallback` routes through `SubagentProgressCallback` with child index
- `AgentSubagentRunner::run_child` accepts and forwards `tool_progress` to `AgentLoop::with_tool_progress()`
- CLI `run_chat()` wires colored tree-view to stderr per 09-UI-SPEC.md:
  - `[subagent-N]` prefix: `.cyan().dimmed()`
  - `Running: <tool>`: `.dimmed()` + `.yellow().dimmed()`
  - `Done.`: `.dimmed()`
- Gateway `run_gateway()` passes `None` for progress callback (D-20: uses tracing::info only)
- Single mode `run_single()` passes `None` (non-interactive)
- Updated `register_delegate_task_tool` to accept `Option<SubagentProgressCallback>`

## Decisions Made

1. **CancellationToken as Option field**: Maintains backward compat -- existing code that doesn't need cancellation passes None. No API breakage.
2. **ChildToolProgressCallback with 'static bound**: async_trait introduces non-static lifetimes for method params. Explicit `'static` type alias resolves this cleanly.
3. **SubagentProgressCallback uses Arc<dyn Fn>**: Required for cloneability -- batch mode clones the callback into each spawned tokio task. Box<dyn Fn> would not be clonable.
4. **Task summary truncated to 50 chars**: Per T-09-16 threat mitigation -- prevents information disclosure of full task content through progress events.

## Deviations from Plan

None -- plan executed exactly as written.

## Verification Results

- `cargo test -p ironhermes-agent agent_loop`: 13 passed (including 2 new cancellation tests)
- `cargo test -p ironhermes-tools delegate_task`: 40 passed
- `cargo test --workspace --lib`: 125 passed, 0 failed
- `cargo build --bin ironhermes`: compiles successfully

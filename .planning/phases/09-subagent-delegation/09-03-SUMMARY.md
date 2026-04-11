---
phase: 09-subagent-delegation
plan: 03
subsystem: tools/agent
tags: [delegation, batch, toolsets, model-override, structured-summary]
dependency_graph:
  requires: [DelegateTaskTool, SubagentRunner-trait, SubagentConfig]
  provides: [batch-delegation, toolset-groups, model-override, structured-summary-prompt]
  affects: [ironhermes-tools, ironhermes-core, ironhermes-agent, ironhermes-cli]
tech_stack:
  added: [toolset-group-resolution, batch-tokio-spawn]
  patterns: [parallel-batch-with-semaphore, index-sorted-results, toolset-abstraction]
key_files:
  created: []
  modified:
    - crates/ironhermes-core/src/config.rs
    - crates/ironhermes-tools/src/delegate_task.rs
    - crates/ironhermes-agent/src/subagent_runner.rs
    - crates/ironhermes-cli/src/main.rs
decisions:
  - id: D-STRUCTURED-SUMMARY-CONST
    summary: "Extracted structured summary instructions into STRUCTURED_SUMMARY_INSTRUCTIONS constant shared between single and batch execution paths to avoid duplication."
metrics:
  duration: 428s
  completed: "2026-04-11T05:14:37Z"
  tasks_completed: 2
  tasks_total: 2
  tests_added: 22
  files_changed: 4
---

# Phase 09 Plan 03: Batch Mode, Toolset Groups, and Model Override Summary

Batch delegation with parallel tokio tasks sharing global semaphore, named toolset group resolution (terminal/file/web), model override plumbing through SubagentRunner, and structured summary system prompt

## What Was Built

### Task 1: Expand SubagentConfig, toolset groups, and model override plumbing (fca194b, 1452226)

- **SubagentConfig** expanded with five new fields per D-25: `default_toolsets` (default `["terminal", "file", "web"]`), `model`, `provider`, `base_url`, `api_key` (all Option, default None). Backward-compatible via `serde(default)`.
- **resolve_toolset_tools()** maps named groups to individual tools: `terminal` -> `["terminal"]`, `file` -> `["read_file", "write_file", "patch", "search_files"]`, `web` -> `["web_search", "web_read"]`. Unknown groups return Err (D-01).
- **resolve_toolsets()** takes a slice of group names and returns a deduplicated union of individual tool names.
- **SubagentRunner::run_child** signature extended with `model_override: Option<&str>` parameter (D-23).
- **AgentSubagentRunner** expanded with `parent_base_url`, `parent_api_key`, `override_base_url`, `override_api_key` fields. When `model_override` is Some, constructs a new `LlmClient` with the override model and appropriate base_url/api_key (D-23/D-24).
- **DelegateTaskTool schema** gains `toolsets` array parameter. `execute()` resolves tools from: toolsets param (highest priority) > allowed_tools > config.default_toolsets (D-01).
- **STRUCTURED_SUMMARY_INSTRUCTIONS** constant appended to all child system prompts (D-10): requests structured output with Actions Taken, Files Modified, Findings, Issues Encountered.
- **main.rs** all three `AgentSubagentRunner::new()` call sites (run_single, run_chat, run_gateway) updated to pass base_url and api_key info.

### Task 2: Batch delegation mode with parallel execution and result ordering (72bf700, 14fc3e1)

- **execute_batch()** method on DelegateTaskTool implements parallel batch execution (D-05):
  - Tasks array truncated to `config.max_subagents` (default 3) with `tracing::warn` on overflow (D-06)
  - Each task spawned as a tokio task, acquiring the shared global semaphore independently
  - Each task gets its own `TempDir` for CWD isolation
  - Per-task `toolsets` and `context` fields supported; falls back to `config.default_toolsets`
  - Results collected and sorted by original task index (D-07)
  - Formatted as `## Task N Result` sections separated by `---`
- **Schema** updated with `tasks` array property for batch mode
- **execute()** checks for `tasks` param first (batch mode), falls through to single-task mode for backward compatibility (D-08)
- Single-task mode runs directly without spawning overhead

## Deviations from Plan

None - plan executed exactly as written.

## Verification

- `cargo test -p ironhermes-core config::tests` -- 13 passed (2 new SubagentConfig expansion tests)
- `cargo test -p ironhermes-tools delegate_task` -- 40 passed (22 new: 7 toolset/schema, 6 prompt/model/toolset-exec, 9 batch mode)
- `cargo test --workspace --lib` -- 125 passed, 0 failed

## Threat Mitigations Verified

| Threat ID | Mitigation | Verified |
|-----------|-----------|----------|
| T-09-10 | Batch truncated to config.max_subagents | test_batch_truncates_to_max_subagents |
| T-09-11 | resolve_toolset_tools validates group names, unknown groups return Err | test_resolve_toolset_unknown_errors |
| T-09-13 | Batch tasks share global semaphore | test_batch_semaphore_sharing (max concurrent <= semaphore limit) |

## Self-Check: PASSED

All 4 files verified present. All 4 commits (fca194b, 1452226, 72bf700, 14fc3e1) verified in git log.

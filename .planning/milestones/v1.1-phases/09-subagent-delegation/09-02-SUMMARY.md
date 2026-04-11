---
phase: 09-subagent-delegation
plan: 02
subsystem: cli/gateway
tags: [delegation, wiring, integration, semaphore]
dependency_graph:
  requires: [DelegateTaskTool, SubagentRunner-trait, SubagentConfig]
  provides: [AgentSubagentRunner, delegate_task-in-all-modes]
  affects: [ironhermes-agent, ironhermes-cli]
tech_stack:
  added: [AgentSubagentRunner-adapter]
  patterns: [trait-based-dependency-inversion, semaphore-per-process]
key_files:
  created:
    - crates/ironhermes-agent/src/subagent_runner.rs
  modified:
    - crates/ironhermes-agent/src/lib.rs
    - crates/ironhermes-cli/src/main.rs
    - crates/ironhermes-tools/src/delegate_task.rs
decisions:
  - id: D-SUBAGENT-RUNNER-ADAPTER
    summary: "Created AgentSubagentRunner in ironhermes-agent to implement SubagentRunner trait, bridging the dependency-inversion boundary. Same pattern as RegistryDispatch for ToolDispatch."
metrics:
  duration: 190s
  completed: "2026-04-10T17:48:49Z"
  tasks_completed: 2
  tasks_total: 2
  tests_added: 2
  files_changed: 4
---

# Phase 09 Plan 02: CLI/Gateway Wiring Summary

AgentSubagentRunner adapter implementing SubagentRunner trait, delegate_task registered in run_gateway/run_single/run_chat with per-process semaphore from SubagentConfig

## What Was Built

### Task 1: Wire DelegateTaskTool into CLI and gateway entry points (c95a1eb)

- **AgentSubagentRunner** created in `ironhermes-agent/src/subagent_runner.rs` implementing the `SubagentRunner` trait from `ironhermes-tools`. Holds a cloneable `LlmClient` and spawns child `AgentLoop` instances with the provided registry, system prompt, and max iterations. Follows the same adapter pattern as `RegistryDispatch` for `ToolDispatch`.
- **run_gateway()** wired with `register_delegate_task_tool` after `register_execute_code_tool` and before guardrail/hooks setup. Creates a fresh `LlmClient` from config for the subagent runner. Passes `Some(memory_store.clone())` for child read-only memory access. Semaphore created from `config.subagent.max_subagents`.
- **run_single()** wired with `register_delegate_task_tool` using `client.clone()`. No memory store (single-execute mode). Registry made mutable for registration.
- **run_chat()** restructured: registry is now created mutable, delegate_task registered, then wrapped in `Arc`. No memory store in chat mode. Semaphore created from config.

### Task 2: Integration test and workspace regression gate (4793cb0)

- **test_delegate_task_in_full_registry** verifies that a registry with `register_defaults()` plus `register_delegate_task_tool()` contains `delegate_task` alongside all default tools (`terminal`, `read_file`, etc.).
- **test_no_recursive_delegation** (AGENT-05 end-to-end): confirms that when a parent registry contains `delegate_task`, the child registry built by `build_child_registry()` never contains it -- even when explicitly requested in the allowlist.
- **Full workspace regression**: `cargo test --workspace` passes with 382 tests, 0 failures.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] register_delegate_task_tool signature differs from plan interfaces**
- **Found during:** Task 1
- **Issue:** The plan's `<interfaces>` section described `register_delegate_task_tool` as accepting `LlmClient` and `Option<Arc<HookRegistry>>`. The actual implementation from Plan 01 accepts `Arc<dyn SubagentRunner>` and has no `hook_registry` parameter (the trait-based approach was the resolution of the circular dependency).
- **Fix:** Created `AgentSubagentRunner` adapter struct in `ironhermes-agent` that implements `SubagentRunner` by wrapping `LlmClient` and constructing child `AgentLoop` instances. Passed `Arc<dyn SubagentRunner>` instead of `LlmClient` to the registration method.
- **Files modified:** `crates/ironhermes-agent/src/subagent_runner.rs` (new), `crates/ironhermes-agent/src/lib.rs`
- **Commit:** c95a1eb

## Verification

- `cargo build --bin ironhermes` -- compiles successfully with delegate_task wired in all modes
- `cargo test --workspace` -- 382 tests passed, 0 failed (4 ignored)
- `test_delegate_task_in_full_registry` -- confirms delegate_task in populated registry
- `test_no_recursive_delegation` -- confirms AGENT-05 child exclusion end-to-end

## Threat Mitigations Verified

| Threat ID | Mitigation | Verified |
|-----------|-----------|----------|
| T-09-08 | Semaphore created once at startup per mode | Code inspection: single `Arc::new(Semaphore::new(...))` in each of run_gateway, run_single, run_chat |
| T-09-09 | delegate_task excluded from child via build_child_registry | test_no_recursive_delegation |

## Self-Check: PASSED

All 4 files verified present. Both commits (c95a1eb, 4793cb0) verified in git log.

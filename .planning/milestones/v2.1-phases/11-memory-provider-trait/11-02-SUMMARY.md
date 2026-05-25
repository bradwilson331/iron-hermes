---
phase: 11-memory-provider-trait
plan: 02
subsystem: memory
tags: [memory-provider, trait-object, call-site-migration]
dependency_graph:
  requires: [11-01]
  provides: [dyn-memory-provider-everywhere]
  affects: [ironhermes-tools, ironhermes-agent, ironhermes-gateway, ironhermes-cli]
tech_stack:
  added: []
  patterns: [trait-object-coercion, arc-mutex-dyn-trait, factory-validation]
key_files:
  created:
    - crates/ironhermes-core/src/memory_provider.rs
  modified:
    - crates/ironhermes-core/src/config.rs
    - crates/ironhermes-core/src/lib.rs
    - crates/ironhermes-core/src/memory_store.rs
    - crates/ironhermes-tools/src/memory_tool.rs
    - crates/ironhermes-tools/src/registry.rs
    - crates/ironhermes-tools/src/delegate_task.rs
    - crates/ironhermes-agent/src/prompt_builder.rs
    - crates/ironhermes-gateway/src/handler.rs
    - crates/ironhermes-gateway/src/runner.rs
    - crates/ironhermes-cli/src/main.rs
decisions:
  - "Used Arc<Mutex<dyn MemoryProvider + Send>> coerced from concrete MemoryStore at construction site, avoiding Box<dyn> unwrap issues"
  - "Added build_memory_provider validation call in CLI for D-09 config validation before constructing concrete provider"
metrics:
  duration: 7m
  completed: 2026-04-11T15:05:57Z
  tasks_completed: 2
  tasks_total: 2
---

# Phase 11 Plan 02: Call-Site Migration to dyn MemoryProvider Summary

Migrated all 15 call sites across 7 files from concrete `Arc<Mutex<MemoryStore>>` to `Arc<Mutex<dyn MemoryProvider + Send>>`, making memory backends genuinely swappable without changing agent code.

## Task Results

### Task 1: Migrate tools crate (6d63a69)
- Changed `MemoryTool` struct field and both constructors (`new`, `new_read_only`) to accept `Arc<Mutex<dyn MemoryProvider + Send>>`
- Changed `ToolRegistry::register_memory_tool` to accept trait object
- Changed `ToolRegistry::register_delegate_task_tool` memory_store param to trait object
- Changed `DelegateTaskTool` struct field, constructor, and `build_child_registry` function to use trait object
- Updated all test code to construct trait objects from concrete `MemoryStore`
- All 124 existing tests pass (1 pre-existing failure in `test_delegate_task_schema_has_required_task` unrelated to changes)

### Task 2: Migrate agent, gateway, CLI (0775554)
- Changed `PromptBuilder` memory_store field and `set_memory_store` setter to trait object
- Changed `GatewayMessageHandler` memory_store field and setter to trait object
- Changed `GatewayRunner` memory_store field, setter, and `execute_cron_job` parameter to trait object
- Added `build_memory_provider(&config.memory)?` validation call in CLI `run_gateway` for D-09 hard error on unknown provider
- Constructed `memory_store` as `Arc<Mutex<dyn MemoryProvider + Send>>` via concrete `MemoryStore` coercion at the single construction point in `main.rs`

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Created Plan 01 prerequisite artifacts**
- **Found during:** Pre-execution setup
- **Issue:** Plan 02 depends on Plan 01 (wave 1) which creates `memory_provider.rs`, `MemoryConfig`, and trait impl on `MemoryStore`. Running in parallel worktree, Plan 01 artifacts were not available.
- **Fix:** Created `memory_provider.rs` with MemoryProvider trait, MemoryEntries, MemoryProviderConfig, MemoryStore impl, and `build_memory_provider` factory. Added `MemoryConfig` to config.rs. Updated lib.rs exports. Added `entries()` accessor to MemoryStore for trait impl's `to_memory_entries()`.
- **Files created:** `crates/ironhermes-core/src/memory_provider.rs`
- **Files modified:** `crates/ironhermes-core/src/config.rs`, `crates/ironhermes-core/src/lib.rs`, `crates/ironhermes-core/src/memory_store.rs`
- **Commit:** 6d63a69

## Verification Results

- `cargo build --workspace` -- compiles with zero errors (pre-existing warnings only)
- `cargo test --package ironhermes-core` -- all tests pass
- `cargo test --package ironhermes-tools` -- 124/125 pass (1 pre-existing failure)
- `cargo test --package ironhermes-agent` -- all tests pass
- `cargo test --package ironhermes-gateway` -- all tests pass
- `grep -r "Arc<Mutex<MemoryStore>>" crates/` -- zero matches (completely migrated)

## Known Pre-existing Issues

- `delegate_task::tests::test_delegate_task_schema_has_required_task` fails on base commit -- not introduced by this plan, out of scope

## Self-Check: PASSED

- All 11 key files confirmed present on disk
- Both task commits (6d63a69, 0775554) confirmed in git history

---
phase: 11-memory-provider-trait
plan: 01
subsystem: memory
tags: [trait, abstraction, memory-provider, config]
dependency_graph:
  requires: []
  provides: [MemoryProvider-trait, MemoryEntries-type, MemoryProviderConfig-type, build_memory_provider-factory, MemoryConfig]
  affects: [ironhermes-core]
tech_stack:
  added: []
  patterns: [async-trait, trait-object-with-arc-mutex, factory-pattern]
key_files:
  created:
    - crates/ironhermes-core/src/memory_provider.rs
  modified:
    - crates/ironhermes-core/src/memory_store.rs
    - crates/ironhermes-core/src/config.rs
    - crates/ironhermes-core/src/lib.rs
key_decisions:
  - "MemoryProvider trait impl for MemoryStore lives in memory_store.rs (same file as struct) to access private fields"
  - "Operational methods are sync (not async) since they mirror MemoryStore's existing synchronous API"
  - "build_memory_provider uses exhaustive match with hard error for unknown/uncompiled providers"
metrics:
  duration_minutes: 4
  completed: "2026-04-11T14:54:09Z"
  tasks_completed: 2
  tasks_total: 2
  tests_added: 19
  tests_total: 118
---

# Phase 11 Plan 01: MemoryProvider Trait and Factory Summary

MemoryProvider trait with 5 async lifecycle hooks and 6 sync operational methods, MemoryStore impl, MemoryConfig, and build_memory_provider factory enabling pluggable memory backends.

## Completed Tasks

| Task | Name | Commit | Key Changes |
|------|------|--------|-------------|
| 1 | Define MemoryProvider trait with lifecycle hooks and operational methods | 2252476 | memory_provider.rs (trait, MemoryEntries, MemoryProviderConfig, format_entries_for_prompt), lib.rs re-exports |
| 2 | MemoryStore implements MemoryProvider, add MemoryConfig and factory | 0b39537 | memory_store.rs (trait impl), config.rs (MemoryConfig), memory_provider.rs (build_memory_provider factory) |

## Implementation Details

### MemoryProvider Trait (memory_provider.rs)

- **Lifecycle hooks (async):** initialize, prefetch, sync_turn, on_session_end, shutdown
- **Operational methods (sync):** load_from_disk, add, replace, remove, format_for_system_prompt, to_memory_entries
- **Bounds:** Send + Sync + 'static -- dyn-compatible, works behind Arc<Mutex<>>
- **Supporting types:** MemoryEntries (HashMap wrapper), MemoryProviderConfig (serde-enabled)
- **Standalone function:** format_entries_for_prompt for trait-based prompt building

### MemoryStore MemoryProvider Impl (memory_store.rs)

- Lifecycle hooks delegate to existing methods (initialize calls load_from_disk, prefetch returns to_memory_entries)
- sync_turn and on_session_end are no-ops (file provider writes directly on mutation)
- Operational methods use fully-qualified syntax to delegate to inherent methods
- to_memory_entries clones entries into MemoryEntries wrapper
- Impl lives in memory_store.rs to access private `entries` field

### MemoryConfig (config.rs)

- Added to Config struct with #[serde(default)] for backward compatibility
- Single field: `provider: String` defaulting to "file"
- Existing config.yaml files without `memory:` section parse cleanly

### build_memory_provider Factory (memory_provider.rs)

- "file" -> Box<MemoryStore>
- "sqlite"/"grafeo"/"duckdb" -> Error mentioning feature flag requirement
- Unknown -> Hard error listing available providers

## Verification Results

- 118 tests pass (19 new + 99 existing), 0 failures
- cargo build --package ironhermes-core compiles cleanly
- Trait has 5 async lifecycle hooks + 6 sync operational methods with Send+Sync+'static
- MemoryConfig in Config struct with #[serde(default)]
- build_memory_provider handles "file", "sqlite", "grafeo", "duckdb", unknown
- impl MemoryProvider for MemoryStore exists in memory_store.rs
- Arc<Mutex<MemoryStore>> coerces to Arc<Mutex<dyn MemoryProvider + Send>> verified in test

## Deviations from Plan

None -- plan executed exactly as written.

## Known Stubs

None -- all types and implementations are fully wired.

## Self-Check: PASSED

All files exist. All commits verified.

---
phase: 17-memory-tools-external-providers
plan: "03"
subsystem: memory
tags: [sqlite, memory-provider, fts5, feature-gates, workspace]
dependency_graph:
  requires: [17-01, 17-02]
  provides: [memory-sqlite-crate, factory-relocated, feature-gates]
  affects: [ironhermes-agent, ironhermes-core]
tech_stack:
  added: [memory-sqlite crate, rusqlite FTS5, Mutex<Connection> Sync pattern]
  patterns: [frozen-snapshot, parameterized-queries, WAL-mode, busy-timeout-5s]
key_files:
  created:
    - providers/memory-sqlite/Cargo.toml
    - providers/memory-sqlite/src/lib.rs
    - providers/memory-sqlite/src/schema.rs
    - crates/ironhermes-agent/src/memory/mod.rs
    - crates/ironhermes-agent/src/memory/factory.rs
  modified:
    - Cargo.toml
    - crates/ironhermes-agent/Cargo.toml
    - crates/ironhermes-agent/src/lib.rs
    - crates/ironhermes-core/src/memory_provider.rs
decisions:
  - "Mutex<Connection> wraps rusqlite::Connection to satisfy Sync bound required by MemoryProvider trait"
  - "Factory relocated to ironhermes-agent with Arc<Mutex<dyn MemoryProvider>> return type (vs Box<dyn> in core)"
  - "MemoryConfig has no extra field — db_path hardcoded to get_hermes_home()/memory.db for sqlite provider"
metrics:
  duration: 4
  completed_date: "2026-04-12"
  tasks_completed: 2
  files_modified: 9
---

# Phase 17 Plan 03: Provider Infrastructure and SQLite Memory Provider Summary

SQLite memory provider with FTS5 search implementing full MemoryProvider trait, factory relocated to ironhermes-agent with Arc<Mutex> return type and feature gates.

## Tasks Completed

| Task | Description | Commit | Files |
|------|-------------|--------|-------|
| 1 | Provider infrastructure: workspace layout, factory relocation, feature gates | 6f8fafd | 8 files |
| 2 | SQLite memory provider with FTS5 search, security scanning, frozen snapshots | 27ef319 | 2 files |

## What Was Built

### Task 1: Provider Infrastructure

- Added `providers/memory-sqlite` as workspace member in root `Cargo.toml`
- Created `crates/ironhermes-agent/src/memory/factory.rs` with `build_memory_provider` returning `Arc<Mutex<dyn MemoryProvider + Send>>` — matches MemoryTool's expected type
- Created `crates/ironhermes-agent/src/memory/mod.rs` with `pub mod factory`
- Added `[features]` section to `ironhermes-agent/Cargo.toml`: `memory-sqlite`, `memory-duckdb`, `memory-grafeo`
- Added `memory-sqlite = { path = "../../providers/memory-sqlite", optional = true }` dependency
- Added `pub mod memory` to `crates/ironhermes-agent/src/lib.rs`
- Deprecated `build_memory_provider` in `ironhermes-core` with migration note

### Task 2: SQLite Memory Provider

- `providers/memory-sqlite/src/schema.rs`: `memory_facts` table with `target CHECK(target IN ('memory', 'user'))`, FTS5 virtual table `memory_facts_fts`, and 3 change-tracking triggers (insert/delete/update)
- `providers/memory-sqlite/src/lib.rs`: Full `MemoryProvider` implementation:
  - `Mutex<Connection>` to satisfy `Sync` bound (rusqlite `Connection` is `Send` but not `Sync`)
  - WAL mode + `busy_timeout(5000)` on connection open
  - `add`: security scan → duplicate check → capacity check → parameterized INSERT
  - `replace`: security scan → LIKE substring match → ambiguity check → capacity check → parameterized UPDATE
  - `remove`: LIKE substring match → ambiguity check → parameterized DELETE
  - `load_from_disk`: populates frozen snapshot cache
  - `format_for_system_prompt`: reads from snapshot (frozen pattern, D-11)
  - `prefetch`: returns live SQLite entries as `MemoryEntries`
  - 16 unit tests, all passing

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] rusqlite::Connection is Send but not Sync**
- **Found during:** Task 2, first test run
- **Issue:** `MemoryProvider` trait requires `Send + Sync + 'static`. `rusqlite::Connection` implements `Send` but not `Sync` due to `RefCell<StatementCache>` interior mutability. All 80 method implementations failed with `Sync` bound errors.
- **Fix:** Wrapped `conn: Connection` in `conn: Mutex<Connection>`. All methods lock the mutex before accessing the connection. Tests updated to call `provider.conn.lock()` for direct table inspection.
- **Files modified:** `providers/memory-sqlite/src/lib.rs`
- **Commit:** 27ef319

**2. [Rule 1 - Bug] MemoryConfig has no `extra` field**
- **Found during:** Task 1, reading config struct
- **Issue:** Plan's factory template referenced `config.extra.get("db_path")` but `MemoryConfig` only has `provider: String`. No `extra: HashMap<...>` field exists.
- **Fix:** Factory uses `get_hermes_home().join("memory.db")` as the hardcoded default db_path for sqlite provider. This matches the project's established pattern for config defaults.
- **Files modified:** `crates/ironhermes-agent/src/memory/factory.rs`
- **Commit:** 6f8fafd

## Threat Mitigations Applied

| Threat | Mitigation |
|--------|-----------|
| T-17-05 (Tampering) | `scan_context_content()` called in `add` and `replace` before any DB write; all SQL uses `rusqlite::params![]` parameterized queries |
| T-17-06 (DoS via capacity) | Same `char_count()` logic as MemoryStore: sum of entry lengths + delimiter lengths; checked in both `add` and `replace` |

## Verification

```
cargo test -p memory-sqlite        → 16 passed, 0 failed
cargo check -p ironhermes-agent --features memory-sqlite  → 0 errors
cargo check --workspace            → 0 errors
```

## Known Stubs

None. The factory's sqlite branch creates a fully functional `SqliteMemoryProvider`. The `initialize()` method is intentionally a no-op (schema created in `new()`).

## Self-Check: PASSED

| Item | Status |
|------|--------|
| providers/memory-sqlite/src/lib.rs | FOUND |
| providers/memory-sqlite/src/schema.rs | FOUND |
| crates/ironhermes-agent/src/memory/factory.rs | FOUND |
| crates/ironhermes-agent/src/memory/mod.rs | FOUND |
| commit 6f8fafd (Task 1) | FOUND |
| commit 27ef319 (Task 2) | FOUND |

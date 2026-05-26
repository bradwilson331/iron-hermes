---
phase: 17-memory-tools-external-providers
plan: "05"
subsystem: memory
tags: [duckdb, memory-provider, thread-bridge, feature-gate]
dependency_graph:
  requires: [17-03]
  provides: [MEM-11]
  affects: [ironhermes-agent, providers/memory-duckdb]
tech_stack:
  added: [duckdb v1.10501.0 (bundled)]
  patterns: [dedicated-os-thread-bridge, frozen-snapshot, mpsc-channel, feature-gate]
key_files:
  created:
    - providers/memory-duckdb/Cargo.toml
    - providers/memory-duckdb/src/lib.rs
    - providers/memory-duckdb/src/bridge.rs
    - providers/memory-duckdb/src/schema.rs
  modified:
    - Cargo.toml
    - crates/ironhermes-agent/Cargo.toml
    - crates/ironhermes-agent/src/memory/factory.rs
decisions:
  - "DuckDB Connection is !Send; owned exclusively by dedicated OS thread — DuckDbBridge holds only mpsc::Sender which is Send+Sync, requiring unsafe impl Sync for DuckDbBridge"
  - "Flat columnar table with DuckDB SEQUENCE for auto-increment (not AUTOINCREMENT like SQLite)"
  - "MemoryConfig only has provider field — db_path defaults to get_hermes_home().join(memory_duckdb.db) rather than config.extra lookup"
  - "Security scanning runs on caller thread before bridge send to avoid passing scan_context_content into worker thread"
metrics:
  duration_minutes: 25
  completed_date: "2026-04-12"
  tasks_completed: 2
  files_created: 4
  files_modified: 3
---

# Phase 17 Plan 05: DuckDB Memory Provider Summary

**One-liner:** DuckDB columnar memory provider with dedicated OS thread bridge for !Send Connection, mpsc channel commands, frozen snapshot, and feature-gated factory integration.

## What Was Built

A complete `providers/memory-duckdb` workspace crate implementing the `MemoryProvider` trait using DuckDB as the storage backend.

### DuckDbBridge (bridge.rs)

The core architectural element: a dedicated OS thread owns the `duckdb::Connection` (which is `!Send`). `DuckDbBridge` holds only an `mpsc::Sender<DuckDbCommand>` which is `Send`, so the bridge itself can be used safely from async/multi-threaded callers. `unsafe impl Sync` is required because `mpsc::Sender` is `Send` but not `Sync` by default.

Commands (`Add`, `Replace`, `Remove`, `LoadAll`, `Shutdown`) are sent via `mpsc::channel`. Each command that needs a response includes an `mpsc::SyncSender` for the reply — the caller creates a `sync_channel(1)` pair, sends the sender in the command, and blocks on `rx.recv()`. This keeps the trait methods sync as required by `MemoryProvider`.

Drop impl sends `Shutdown` and joins the thread for clean teardown.

### DuckDbMemoryProvider (lib.rs)

Implements all `MemoryProvider` methods:
- `initialize` / `sync_turn` / `on_session_end`: no-ops (DuckDB persists on every mutation)
- `prefetch`: loads all entries via `LoadAll` command
- `load_from_disk`: populates the frozen snapshot cache from DuckDB
- `add` / `replace` / `remove`: security scan on caller thread (T-17-10), then delegate to bridge
- `format_for_system_prompt`: reads from frozen snapshot with capacity header
- `to_memory_entries`: returns frozen snapshot
- `shutdown`: calls bridge.shutdown()

### Schema (schema.rs)

Flat columnar table per D-04. DuckDB uses `SEQUENCE` for auto-increment (not `AUTOINCREMENT`):
```sql
CREATE SEQUENCE IF NOT EXISTS memory_facts_seq START 1;
CREATE TABLE IF NOT EXISTS memory_facts (
    id BIGINT PRIMARY KEY DEFAULT nextval('memory_facts_seq'),
    target VARCHAR NOT NULL CHECK(target IN ('memory', 'user')),
    content VARCHAR NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT current_timestamp
);
```

### Factory Integration

- Root `Cargo.toml`: added `providers/memory-duckdb` to workspace members
- `crates/ironhermes-agent/Cargo.toml`: `memory-duckdb = ["dep:memory-duckdb"]` feature + optional dep
- `factory.rs`: `#[cfg(feature = "memory-duckdb")]` arm creates `DuckDbMemoryProvider` with default path `~/.ironhermes/memory_duckdb.db`; updated error message includes duckdb

## Tests

23 tests pass across bridge and provider modules, covering:
- Bridge construction and worker thread lifecycle
- Add/replace/remove through the bridge
- Capacity enforcement (T-17-11)
- Duplicate detection
- Substring matching for replace/remove
- Ambiguous match detection
- Frozen snapshot pattern
- Security scan blocking injection (T-17-10)
- User target char limits
- prefetch via async runtime

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] MemoryConfig has no `extra` field**
- **Found during:** Task 2
- **Issue:** Plan's factory snippet used `config.extra.get("db_path")` but `MemoryConfig` only has a `provider: String` field — no `extra` HashMap
- **Fix:** Replaced with hardcoded default `get_hermes_home().join("memory_duckdb.db")` — consistent with how the grafeo provider uses a fixed path
- **Files modified:** `crates/ironhermes-agent/src/memory/factory.rs`
- **Commit:** f42be13

**2. [Rule 1 - Bug] Type inference failure on closure parameter**
- **Found during:** Task 2 (first cargo check attempt)
- **Issue:** `config.extra.get("db_path").and_then(|v| v.as_str())` triggered E0282 — resolved when extra field was removed
- **Fix:** Resolved by removing the `config.extra` lookup entirely (see above)
- **Files modified:** `crates/ironhermes-agent/src/memory/factory.rs`

**3. [Implementation - DuckDB API verification] Parameterized query syntax**
- **Found during:** Task 1 pre-implementation
- **Issue:** Plan suggested checking Context7 for DuckDB API — confirmed via test binary that duckdb-rs uses `$1` positional params (not `?1` like rusqlite), `query_map` works identically to rusqlite
- **Fix:** Used `$1`, `$2` params in all SQL queries; `duckdb::params![]` macro for binding

## Commits

| Hash | Message |
|------|---------|
| 367a5de | feat(17-05): DuckDB bridge and memory provider crate |
| f42be13 | feat(17-05): integrate DuckDB provider into factory and feature gates |

## Self-Check: PASSED

- providers/memory-duckdb/src/lib.rs: FOUND
- providers/memory-duckdb/src/bridge.rs: FOUND
- providers/memory-duckdb/src/schema.rs: FOUND
- providers/memory-duckdb/Cargo.toml: FOUND
- Commit 367a5de: FOUND
- Commit f42be13: FOUND
- `cargo test -p memory-duckdb`: 23 passed, 0 failed
- `cargo check -p ironhermes-agent --features memory-duckdb`: clean
- `cargo check -p ironhermes-agent`: clean
- `cargo check --workspace`: clean

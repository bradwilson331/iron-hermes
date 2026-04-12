---
phase: 13-session-storage
plan: 01
subsystem: ironhermes-state
tags: [sqlite, fts5, search, session-lineage, wal, migration]
dependency_graph:
  requires: []
  provides: [SearchFilter, sanitize_fts_query, SessionExport, busy_timeout, wal_checkpoint, v7_migration, get_session_by_title, create_session_with_parent]
  affects: [ironhermes-gateway, ironhermes-cli]
tech_stack:
  added: []
  patterns: [composable-sql-where, fts5-snippet, deterministic-jitter-retry]
key_files:
  modified:
    - crates/ironhermes-state/src/lib.rs
decisions:
  - busy_timeout set to 5000ms immediately after Connection::open
  - Deterministic jitter (50ms, 125ms) in retry wrapper avoids rand dependency
  - SearchFilter uses manual Default impl to set limit=20
  - FTS5 snippet uses << >> markers with 32-token fragments
  - context_before/context_after populated via separate queries per result
  - is_busy() helper extracts error code check to avoid borrow issues with ref pattern
metrics:
  duration_minutes: 3
  completed: "2026-04-12T05:17:00Z"
  tasks_completed: 2
  tasks_total: 2
  files_modified: 1
---

# Phase 13 Plan 01: StateStore Core Extensions Summary

Extended StateStore with busy_timeout, deterministic retry wrapper, WAL checkpoint, v7 migration with composite indexes, session lineage via parent_session_id, title lookup, SearchFilter with composable WHERE clauses, FTS5 snippet search with << >> markers, and input sanitization.

## Task Results

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | busy_timeout, retry, wal_checkpoint, v7 migration, create_session with parent_id | 13d7af4 | crates/ironhermes-state/src/lib.rs |
| 2 | SearchFilter, sanitize_fts_query, snippet search, context window | 0729fdb | crates/ironhermes-state/src/lib.rs |

## Changes Made

### Task 1: Connection Resilience and Session Lineage
- Set `busy_timeout(5000ms)` on `Connection::open` in `StateStore::new()`
- Added `with_busy_retry()` free function: 3 attempts with deterministic jitter (50ms, 125ms), no rand dependency
- Added `is_busy()` helper to check for `SQLITE_BUSY` error code
- Added `wal_checkpoint()` method using `PRAGMA wal_checkpoint(PASSIVE)`
- Bumped `SCHEMA_VERSION` from 6 to 7
- Added v7 migration: `idx_messages_timestamp`, `idx_sessions_source_started`, `idx_sessions_ended` composite indexes
- Extended `create_session()` with `parent_session_id: Option<&str>` parameter
- Added `get_session_by_title()` returning `Result<Option<Session>>`

### Task 2: Search Infrastructure
- Added `SearchFilter` struct with 7 fields: query, source, role, after, before, limit (default 20), raw
- Added `sanitize_fts_query()` public function stripping FTS5 operators (*, ^, ", etc.) and boolean keywords (AND, OR, NOT, NEAR)
- Extended `SearchResult` with `snippet`, `context_before`, `context_after` fields
- Rewrote `search_messages()` to accept `&SearchFilter` instead of `(query, limit)`
- FTS mode: uses `snippet(messages_fts, 0, '<<', '>>', '...', 32)` for match highlighting
- Non-FTS mode: metadata-only query with NULL snippet
- Dynamic WHERE clause construction with parameterized values (no string interpolation)
- Context window: separate queries for 1 message before/after each result in same session

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed borrow checker error in is_busy()**
- **Found during:** Task 1
- **Issue:** The `with_busy_retry` closure uses `Err(ref e)` pattern, but `is_busy()` initially used `if let StateError::Sqlite(ref sq) = e` which caused "cannot explicitly borrow within implicitly-borrowing pattern" error in Rust 2024 edition
- **Fix:** Extracted error code check into `is_busy()` helper taking `&StateError`, removed redundant `ref` binding
- **Files modified:** crates/ironhermes-state/src/lib.rs
- **Commit:** 13d7af4

## Verification

- `cargo check --workspace` passes with 0 errors (only pre-existing dead_code warnings in ironhermes-cli)
- No broken call sites: `create_session` and `search_messages` only called within lib.rs itself (no external callers yet)
- All acceptance criteria met for both tasks

## Self-Check: PASSED

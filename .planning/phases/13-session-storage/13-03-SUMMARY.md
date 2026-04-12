---
phase: 13-session-storage
plan: 03
subsystem: ironhermes-gateway, ironhermes-cli
tags: [write-through-cache, session-persistence, wal-checkpoint, sqlite-integration]
dependency_graph:
  requires: [SearchFilter, sanitize_fts_query, SessionExport, busy_timeout, wal_checkpoint, v7_migration, create_session_with_parent]
  provides: [write_through_session_store, gateway_state_store, cli_session_persistence, wal_checkpoint_timer]
  affects: [ironhermes-gateway, ironhermes-cli]
tech_stack:
  added: []
  patterns: [write-through-cache, arc-mutex-statestore, wal-passive-checkpoint-timer]
key_files:
  modified:
    - crates/ironhermes-gateway/src/session.rs
    - crates/ironhermes-gateway/src/handler.rs
    - crates/ironhermes-gateway/src/runner.rs
    - crates/ironhermes-cli/src/main.rs
decisions:
  - SessionStore composes Arc<Mutex<StateStore>> + HashMap cache (D-01 write-through pattern)
  - get_or_create takes source param derived from Platform::to_string()
  - add_message_to_session writes to SQLite before updating in-memory cache
  - WAL checkpoint timer spawned every 300 seconds using tokio::spawn with spawn_blocking
  - CLI persists user and assistant messages via StateStore in both run_chat and run_single modes
  - Handler tests use in-memory StateStore (":memory:") for test isolation
metrics:
  duration_minutes: 5
  completed: "2026-04-12T05:29:00Z"
  tasks_completed: 2
  tasks_total: 2
  files_modified: 4
---

# Phase 13 Plan 03: Write-Through Cache Integration Summary

SessionStore refactored to compose Arc<Mutex<StateStore>> + HashMap as a write-through cache; every session creation and message addition writes to SQLite immediately. GatewayRunner constructs StateStore and injects into SessionStore with WAL checkpoint timer. CLI creates/persists/ends sessions in both interactive and single-prompt modes.

## Task Results

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Refactor SessionStore as write-through cache over StateStore | e30dcdc | session.rs, handler.rs, runner.rs |
| 2 | Wire StateStore into GatewayRunner + CLI persistence | 675af78 | runner.rs, main.rs |

## Changes Made

### Task 1: Write-Through SessionStore

- Added `state: Arc<Mutex<StateStore>>` field to `SessionStore` struct
- Changed `SessionStore::new()` to accept `Arc<Mutex<StateStore>>` parameter
- Removed `Default` derive from `SessionStore` (requires StateStore now)
- Changed `get_or_create` to accept `source: &str` parameter and write-through to SQLite via `state.create_session()`
- Added `add_message_to_session(&mut self, key: &SessionKey, msg: ChatMessage)` that writes to both SQLite and in-memory cache
- Added `state_store()` getter returning `&Arc<Mutex<StateStore>>` for WAL checkpoint access
- Updated `expire_stale` to call `state.end_session()` on expired sessions in SQLite
- Updated `handler.rs` to pass `source` (from `key.platform.to_string()`) to `get_or_create`
- Changed handler user message persistence to use `store.add_message_to_session()` instead of `session.add_message()`
- Changed handler assistant message persistence to use `store.add_message_to_session()` instead of `session.add_message()`
- Updated handler test `make_handler()` to create in-memory StateStore for `SessionStore::new()`

### Task 2: GatewayRunner StateStore + WAL + CLI Persistence

- Added `state_store: Arc<Mutex<StateStore>>` field to `GatewayRunner` struct
- `GatewayRunner::new()` constructs `StateStore::open_default()` and injects `Arc::clone(&state_store)` into `SessionStore::new()`
- Spawned WAL checkpoint timer in `GatewayRunner::start()` using `tokio::spawn` with `tokio::time::interval(Duration::from_secs(300))` and `spawn_blocking` for the sync `wal_checkpoint()` call
- CLI `run_single`: constructs `StateStore::open_default()`, creates session, persists user message, persists assistant response, ends session
- CLI `run_chat`: constructs `StateStore::open_default()`, creates session, persists initial message if provided, persists user/assistant messages in chat loop, ends session on exit

## Deviations from Plan

None - plan executed exactly as written.

## Verification

- `cargo check --workspace` passes with 0 errors
- `cargo test -p ironhermes-gateway -p ironhermes-state -p ironhermes-cli` passes all tests (12 state tests, 2 gateway tests)
- Pre-existing test failure in `ironhermes-tools::delegate_task::tests::test_delegate_task_schema_has_required_task` is unrelated to this plan
- SessionStore write-through: `get_or_create` calls `state.create_session`, `add_message_to_session` calls `state.add_message`
- GatewayRunner constructs StateStore and passes to SessionStore
- WAL checkpoint timer spawned in runner.rs `start()` with 300s interval
- CLI creates/ends sessions in StateStore for both run modes

## Self-Check: PASSED

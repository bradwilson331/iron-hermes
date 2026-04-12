---
phase: 18
plan: 04
subsystem: context-compression
tags: [hooks, memory, compression, async]
requires:
  - ironhermes-hooks::HookRegistry
  - ironhermes-core::MemoryProvider (sync_turn)
  - ContextEngine (18-01)
provides:
  - HookRegistry::fire_awaitable
  - AsyncHookListener type
  - HookEventKind::ContextPreCompress
  - HookEventKind::ContextPressure
  - LocalPruningEngine::with_hooks / SummarizingEngine::with_hooks
  - memory_flush_handler::build_memory_flush_listener
affects:
  - crates/ironhermes-hooks/src/registry.rs
  - crates/ironhermes-hooks/src/event.rs
  - crates/ironhermes-hooks/src/webhook.rs
  - crates/ironhermes-hooks/src/lib.rs
  - crates/ironhermes-hooks/Cargo.toml
  - crates/ironhermes-agent/src/context_engine.rs
  - crates/ironhermes-agent/src/summarizing_engine.rs
  - crates/ironhermes-agent/src/memory_flush_handler.rs
  - crates/ironhermes-agent/src/lib.rs
tech-stack:
  added:
    - ironhermes-hooks now depends on `futures` for BoxFuture/join_all
  patterns:
    - Async-aware hook listener alongside existing sync listener (backward compat)
    - Builder-style `with_hooks(registry, session_id)` on engines
    - Arc<Mutex<dyn MemoryProvider + Send>> sharing (Phase 11 pattern)
key-files:
  created:
    - crates/ironhermes-agent/src/memory_flush_handler.rs
  modified:
    - crates/ironhermes-hooks/src/registry.rs
    - crates/ironhermes-hooks/src/event.rs
    - crates/ironhermes-hooks/src/webhook.rs
    - crates/ironhermes-hooks/src/lib.rs
    - crates/ironhermes-hooks/Cargo.toml
    - crates/ironhermes-agent/src/context_engine.rs
    - crates/ironhermes-agent/src/summarizing_engine.rs
    - crates/ironhermes-agent/src/lib.rs
decisions:
  - Emit pre_compress only when threshold would actually trigger compression
  - Handler failures log via tracing::warn and do not block compression (D-22, T-18-07)
  - Sync listeners still fire-and-forget in fire_awaitable; only async listeners are awaited
metrics:
  duration: ~10 min
  completed: 2026-04-12
---

# Phase 18 Plan 04: Awaitable Hooks + Memory Flush Handler Summary

One-liner: Adds `HookRegistry::fire_awaitable` and `ContextPreCompress`/`ContextPressure` event variants so both compression engines can emit a `context:pre_compress` event and deterministically await `MemoryProvider::sync_turn` before destructive pruning — resolving the RESEARCH.md sync-hook blocker for PRMT-16.

## What Shipped

- `AsyncHookListener` type (`Arc<dyn Fn(HookEvent) -> BoxFuture<'static,()> + Send + Sync>`) alongside existing `HookListener`.
- `HookRegistry::add_async_listener` and `fire_awaitable(event).await` that spawn sync listeners fire-and-forget and `join_all` the async listeners.
- `HookEventKind::ContextPreCompress { session_id, estimated_tokens, threshold, mode, pruned_range }` and `HookEventKind::ContextPressure { .. }` variants.
- `LocalPruningEngine::with_hooks(registry, session_id)` and `SummarizingEngine::with_hooks(...)`; both emit `ContextPreCompress` and await handler completion BEFORE any prune/summarize mutation.
- `memory_flush_handler::build_memory_flush_listener(Arc<Mutex<dyn MemoryProvider+Send>>)` factory returning an `AsyncHookListener` that calls `sync_turn(session_id, &entries)` on `ContextPreCompress` events and swallows errors with a `tracing::warn!`.
- Absent-handler path logs at `tracing::debug!` and proceeds (D-22).

## Commits

| Task | Commit | Message |
|------|--------|---------|
| 1 | e40640a | feat(18-04): add fire_awaitable + ContextPreCompress/Pressure events |
| 2 | 7ded006 | feat(18-04): emit context:pre_compress + memory flush handler |

## Tests

All 8 plan-mandated tests green plus 2 bonus:

Hooks (`cargo test -p ironhermes-hooks --lib`):
- `fire_awaitable_awaits_all_handlers`
- `fire_awaitable_with_no_listeners_completes`
- `fire_awaitable_handlers_run_concurrently` (< 120ms elapsed for 3× 50ms handlers)
- `context_pre_compress_event_kind`
- `context_pressure_event_kind`

Agent (`cargo test -p ironhermes-agent --lib`):
- `pre_compress_hook_event`
- `memory_flush_before_prune` (asserts `["flushed", "pruned"]` ordering)
- `pre_compress_no_hook_registered_proceeds`
- `build_memory_flush_listener_calls_sync_turn`
- `listener_ignores_non_pre_compress_events` (bonus)

Full agent test suite: **130 passed, 0 failed**. Full hooks test suite: **36 passed, 0 failed**. `cargo check -p ironhermes-gateway -p ironhermes-cli` passes — backward compat preserved (`fire()` call sites unchanged).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] webhook.rs match non-exhaustive after event variants added**
- **Found during:** Task 1 test build
- **Issue:** `webhook::event_kind_name` match on `HookEventKind` was exhaustive; adding `ContextPreCompress`/`ContextPressure` broke compilation.
- **Fix:** Added two match arms mapping to `"context_pre_compress"` and `"context_pressure"`.
- **Files modified:** `crates/ironhermes-hooks/src/webhook.rs`
- **Commit:** e40640a

**2. [Rule 3 - Blocking] `MemoryResult::NotFound` does not exist**
- **Found during:** Task 2 test build
- **Issue:** Plan example used `MemoryResult::NotFound` but `MemoryResult` is a `std::result::Result<String, String>` type alias (`crates/ironhermes-core/src/memory_store.rs:49`).
- **Fix:** Mock provider `add`/`replace`/`remove` now return `Err("not supported".into())`; imported `MemoryResult` from `ironhermes_core::memory_store`.
- **Files modified:** `crates/ironhermes-agent/src/memory_flush_handler.rs`
- **Commit:** 7ded006

### Interpretation Choices

- Plan action step 2 said to emit "AFTER the threshold check". `ContextEngine` trait has no `threshold()` gate inside the engines for LocalPruningEngine (it delegates to `ContextCompressor::should_compress`). Implemented equivalent gate inline using `before / context_length >= threshold`. `SummarizingEngine` already had the gate; inserted emission after it.
- Plan step 3 used `provider.lock().await` + `sync_turn(/* args */)`. Real `sync_turn` signature is `sync_turn(&self, session_id: &str, entries: &MemoryEntries)`. Handler uses provider snapshot via `to_memory_entries()` plus the event's `session_id`.

## Authentication Gates

None — fully autonomous execution.

## Acceptance Criteria

- `pub async fn fire_awaitable` in registry.rs — match
- `pub type AsyncHookListener` in registry.rs — match
- `ContextPreCompress` / `ContextPressure` in event.rs — match
- `pub fn fire\b` still in registry.rs (backward compat) — match
- `fire_awaitable` referenced in `context_engine.rs` and `summarizing_engine.rs` — match
- `ContextPreCompress` referenced in `context_engine.rs` — match
- `pub fn build_memory_flush_listener` in `memory_flush_handler.rs` — match
- `pub mod memory_flush_handler` in agent `lib.rs` — match
- `cargo test -p ironhermes-hooks --lib` exits 0
- `cargo test -p ironhermes-agent --lib` exits 0
- `cargo check -p ironhermes-gateway -p ironhermes-cli` exits 0

## Known Stubs

None. Both engine paths are fully wired and tested; the listener factory is production-ready (not behind a `todo!()`).

## Self-Check: PASSED

- FOUND: crates/ironhermes-agent/src/memory_flush_handler.rs
- FOUND: commit e40640a (feat(18-04): add fire_awaitable...)
- FOUND: commit 7ded006 (feat(18-04): emit context:pre_compress...)
- All 10 tests named in plan verification are passing.

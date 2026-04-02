---
phase: 02-telegram-gateway
plan: "03"
subsystem: gateway
tags: [rust, async, tokio, telegram, streaming, concurrency, cancellation-token, joinset, semaphore]
dependency_graph:
  requires: [02-01 (CancellationToken, adapter traits), 02-02 (StreamConsumer, BackoffState)]
  provides: [GatewayMessageHandler, UserQueueManager, GatewayRunner (full), get_updates polling]
  affects: [ironhermes-gateway]
tech_stack:
  added: []
  patterns:
    - Per-chat mpsc queue with eye-reaction acknowledgment for serialized message handling
    - Two-channel streaming bridge (stream_tx + tool_tx) from AgentLoop to StreamConsumer
    - JoinSet + Semaphore supervisor pattern for bounded concurrent agent runs
    - CancellationToken child tokens propagated to every spawned task
    - Fire-and-forget eye reaction via tokio::spawn (non-blocking acknowledgment)
    - Dispatch loop runs inline (not in JoinSet) to own msg_rx lifetime
key_files:
  created:
    - crates/ironhermes-gateway/src/user_queue.rs
    - crates/ironhermes-gateway/src/handler.rs
  modified:
    - crates/ironhermes-gateway/src/session.rs
    - crates/ironhermes-gateway/src/runner.rs
    - crates/ironhermes-gateway/src/telegram.rs
    - crates/ironhermes-gateway/src/lib.rs
decisions:
  - "Dispatch loop runs inline (not in JoinSet) to own msg_rx lifetime — JoinSet owns poll+cleanup tasks only"
  - "Per-chat workers spawned as detached tokio::spawn (not JoinSet) since JoinSet owned outside closure"
  - "UserQueueManager uses enum dispatch pattern: try_send returns event via TrySendError::into_inner() on failure"
  - "get_updates added to TelegramAdapter (missing from plan 02 output) as Rule 3 blocking fix"
  - "MessageHandler trait import required in runner.rs for handle() method resolution"
metrics:
  duration: "7 minutes"
  completed_date: "2026-04-02"
  tasks_completed: 2
  files_changed: 6
---

# Phase 02 Plan 03: Gateway Wiring — Polling, Dispatch, Handler, Runner Summary

Complete Telegram gateway wiring: long-polling loop with channel dispatch, per-user message queue with eye-reaction acknowledgment, GatewayMessageHandler bridging AgentLoop streaming to StreamConsumer, and GatewayRunner with JoinSet+Semaphore concurrency control, CancellationToken shutdown, and session cleanup.

## What Was Built

### Task 1: SessionStore timeout, UserQueueManager, GatewayMessageHandler

**session.rs additions:**
- `GatewaySession::is_expired(timeout_hours)` — returns true if `updated_at` is older than cutoff
- `SessionStore::expire_stale(timeout_hours)` — retains only sessions updated after cutoff, called from session cleanup task and opportunistically from handler

**user_queue.rs (new):**
- `UserQueueManager` with `Mutex<HashMap<String, mpsc::Sender<QueuedMessage>>>` — one channel per chat_id
- `dispatch(event)` — if existing sender: `try_send` as `is_queued=true`, fires eye-reaction `👀` via `tokio::spawn`, returns `None`. If no sender or channel closed/full: creates fresh `mpsc::channel(16)`, sends as `is_queued=false`, returns `Some(rx)` for caller to spawn worker
- `QueuedMessage { event, is_queued }` — carries MessageEvent and queue status to worker
- `remove(chat_id)` — called by worker on exit to clean up sender entry
- 4 unit tests covering: first-dispatch returns receiver, second-dispatch returns None and adds reaction, independent chat workers, remove-then-redispatch

**handler.rs (new):**
- `GatewayMessageHandler` implements `MessageHandler` with `config`, `Arc<RwLock<SessionStore>>`, `Arc<ToolRegistry>`
- `handle()` flow:
  1. Send `"..."` placeholder message → get `placeholder_message_id`
  2. Spawn typing indicator task: `send_chat_action("typing")` every 5s, cancelled via child token
  3. Write-lock SessionStore briefly: expire stale sessions, get-or-create session, append user message, clone messages, release lock
  4. Build system prompt via `PromptBuilder::new(model, "telegram").load_context(&cwd).build_system_message()`
  5. Create two mpsc channels: `stream_tx/rx` (capacity 256) for text chunks, `tool_tx/rx` (capacity 64) for tool progress
  6. Spawn StreamConsumer task: select on both channels, calls `consumer.tool_status()` / `consumer.push()` / `consumer.flush()`, closes with `flush(true)` when both channels drop
  7. Build AgentLoop with `StreamCallback` (try_send to stream_tx) and `ToolProgressCallback` (try_send to tool_tx)
  8. Run agent, drop both tx channels to close StreamConsumer, await consumer task for final flush
  9. Cancel+await typing task
  10. On success: update session with filtered messages (user+assistant only). On error: edit message with warning indicator

### Task 2: GatewayRunner rewrite + TelegramAdapter get_updates

**telegram.rs addition:**
- `get_updates(offset: Option<i64>)` — `getUpdates` API with 30s timeout, returns `Vec<TgUpdate>`

**runner.rs (full rewrite):**
- `GatewayRunner { config, session_store: Arc<RwLock<SessionStore>>, tool_registry: Arc<ToolRegistry>, cancel: CancellationToken }`
- `new(config, tool_registry)` — creates session store and cancellation token
- `start()` flow:
  1. Resolve token via `resolve_token()` — supports direct value, `${ENV_VAR}` syntax, or `TELEGRAM_BOT_TOKEN`
  2. `get_me()` to verify token, log bot name/username
  3. `set_my_commands()` — register `/start`, `/new`, `/clear`, `/help`
  4. `mpsc::channel::<TgUpdate>(256)` for polling→dispatch
  5. `Arc<Semaphore>` from `max_concurrent_runs`
  6. `Arc<GatewayMessageHandler>` + `Arc<UserQueueManager>`
  7. Spawn **poll loop** in JoinSet: `tokio::select!` on cancel vs `get_updates()`. Records backoff state; 409 → `record_conflict()` + fatal check; other errors → `record_failure()` + sleep `next_delay()`
  8. **Dispatch loop** runs inline: select on cancel vs `msg_rx.recv()`. Whitelist check (empty=deny all), group @mention filter, `user_queue.dispatch(event)`. On `Some(rx)`: `tokio::spawn` per-chat worker
  9. **Per-chat worker** (tokio::spawn): loop `chat_rx.recv()`, acquire semaphore permit, call `handler.handle()`, drop permit, check cancel, on exit call `user_queue.remove()`
  10. Spawn **session cleanup** in JoinSet: `expire_stale` every 5 minutes
  11. `tokio::select!` on `ctrl_c()` or `cancel.cancelled()` → `self.cancel.cancel()`
  12. Drop `msg_tx`, await `dispatch_future`, drain JoinSet, log "Gateway shut down cleanly"

**lib.rs:**
- Added `pub mod handler` and `pub mod user_queue`
- Re-exports: `GatewayMessageHandler`, `UserQueueManager`

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Added `get_updates()` to TelegramAdapter**
- **Found during:** Task 2 — runner needs `get_updates` to poll for messages; it was not in telegram.rs
- **Issue:** Plan referenced polling via `getUpdates` but the method didn't exist on `TelegramAdapter`
- **Fix:** Added `get_updates(offset: Option<i64>) -> Result<Vec<TgUpdate>>` with 30s timeout parameter
- **Files modified:** `crates/ironhermes-gateway/src/telegram.rs`
- **Commit:** e767ed9

**2. [Rule 3 - Blocking] Added `MessageHandler` trait import to runner.rs**
- **Found during:** Task 2 cargo check
- **Issue:** `handler_task.handle(...)` failed to resolve because `MessageHandler` trait was not in scope
- **Fix:** Added `use crate::adapter::MessageHandler;` import to runner.rs
- **Files modified:** `crates/ironhermes-gateway/src/runner.rs`
- **Commit:** e767ed9

**3. [Rule 1 - Bug] Fixed event ownership in UserQueueManager dispatch**
- **Found during:** Task 1 — first implementation attempt moved `event` into `QueuedMessage` before the new-channel path could use it
- **Issue:** Borrow checker error: `event` moved into `try_send` call, unavailable for the `else` branch
- **Fix:** Used `TrySendError::into_inner()` to reclaim the event from error variants; restructured as try-send-then-fallback
- **Files modified:** `crates/ironhermes-gateway/src/user_queue.rs`
- **Commit:** 33cde48

## Architecture Note

The per-chat workers are spawned as detached `tokio::spawn` (not in the `JoinSet`) because the `JoinSet` is owned by the outer `start()` scope and cannot be shared into the dispatch closure. The workers cooperatively stop when `cancel` fires and the `chat_rx` channel closes (since `msg_tx` is dropped during shutdown). The JoinSet tracks only the poll loop and session cleanup task, which are structural.

## Known Stubs

None — all modules are fully implemented. The GatewayRunner stub from plan 01 is fully replaced.

## Verification

```
cargo check --workspace   → Finished (no errors)
cargo test --workspace    → 55 passed (20 agent + 9 cron + 26 gateway), 0 failed
```

## Self-Check: PASSED

Files exist:
- FOUND: crates/ironhermes-gateway/src/user_queue.rs
- FOUND: crates/ironhermes-gateway/src/handler.rs
- FOUND: crates/ironhermes-gateway/src/session.rs (modified)
- FOUND: crates/ironhermes-gateway/src/runner.rs (rewritten)
- FOUND: crates/ironhermes-gateway/src/telegram.rs (extended)
- FOUND: crates/ironhermes-gateway/src/lib.rs (updated)

Commits exist:
- 33cde48: feat(02-03): per-user queue, session timeout, and GatewayMessageHandler
- e767ed9: feat(02-03): GatewayRunner with polling, dispatch, JoinSet, Semaphore, and shutdown

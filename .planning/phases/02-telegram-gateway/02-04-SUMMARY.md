---
phase: 02-telegram-gateway
plan: "04"
subsystem: gateway
tags: [rust, telegram, slash-commands, error-recovery, rate-limit, streaming, async, tokio]
dependency_graph:
  requires: [02-01 (adapter traits, CancellationToken), 02-02 (StreamConsumer), 02-03 (handler.rs base structure)]
  provides: [GatewayMessageHandler with slash command dispatch, error recovery, run_agent streaming bridge]
  affects: [ironhermes-gateway]
tech_stack:
  added: []
  patterns:
    - Slash command dispatch before agent loop (starts_with('/') guard)
    - with_rate_limit_retry helper for 429 backoff (2s, 4s, 6s)
    - CancellationToken child token for typing indicator lifetime
    - mpsc channels bridging AgentLoop stream callbacks to StreamConsumer
    - Synthetic MessageEvent for /start LLM greeting
key_files:
  created:
    - crates/ironhermes-gateway/src/handler.rs
  modified:
    - crates/ironhermes-gateway/src/lib.rs
    - crates/ironhermes-gateway/src/session.rs
decisions:
  - "Plan 03 handler.rs base created alongside plan 04 additions since plans run in parallel — single file contains both GatewayMessageHandler struct and slash command dispatch"
  - "with_rate_limit_retry wraps send_message calls in /new, /clear, /help — consistent rate limit handling across all bot-initiated sends"
  - "Unknown slash commands fall through to run_agent rather than returning error — graceful agent fallback per plan spec"
  - "Typing indicator cancelled via cancel.cancel() after agent completes — reuses parent token rather than child to simplify shutdown"
metrics:
  duration: "2 minutes"
  completed: "2026-04-02"
  tasks_completed: 1
  files_modified: 3
---

# Phase 2 Plan 04: Slash Commands and Error Recovery Summary

**One-liner:** Slash command dispatch (/start, /new, /clear, /help) intercepted before agent loop with LLM greeting for /start, 429 retry backoff, and agent error recovery appended to partial output.

## What Was Built

### Task 1: Slash command dispatch and error recovery in handler

**handler.rs** — Created with plan 03 base structure AND plan 04 additions (parallel wave execution):

`GatewayMessageHandler` struct:
- `config: Config` — model/agent settings
- `session_store: Arc<RwLock<SessionStore>>` — shared session state
- `tool_registry: Arc<ToolRegistry>` — tools for AgentLoop

`MessageHandler::handle()`:
- Checks `event.content.starts_with('/')` — if true, dispatches to `handle_slash_command`
- Otherwise calls `run_agent` directly

`handle_slash_command()`:
- Strips `@botname` suffix from command (Telegram group mention format)
- Match arms: `/start` → `cmd_start`, `/new` → `cmd_new`, `/clear` → `cmd_clear`, `/help` → `cmd_help`
- Unknown commands fall through to `run_agent` (graceful fallback)

`cmd_start()` (D-15):
- Removes current session from store
- Creates synthetic event with content "Please introduce yourself. This is the start of a new conversation."
- Calls `run_agent` — LLM generates in-character greeting using SOUL.md

`cmd_new()` (D-13):
- Removes session from store, returns `Option<GatewaySession>` to detect if session existed
- Sends contextual confirmation via `with_rate_limit_retry`

`cmd_clear()` (D-13):
- Acquires write lock, calls `session.clear()` on existing session
- Sends "History cleared." confirmation

`cmd_help()` (D-13):
- Sends 4-command help text listing all available slash commands

`run_agent()`:
- Sends placeholder "█" message, gets message_id for StreamConsumer
- Spawns typing indicator task (sends "typing" every 5s) on child CancellationToken
- Gets-or-creates session, clones messages immediately (no lock held across await)
- Builds system message via `PromptBuilder::new(model, "telegram").load_context(cwd).build_system_message()`
- Creates mpsc channels: `stream_tx/rx` (256 cap) + `tool_tx/rx` (64 cap)
- Spawns StreamConsumer task: select loop handling tool_rx (tool_status) and stream_rx (push+flush)
- Builds `AgentLoop` with `with_streaming` and `with_tool_progress` callbacks using `try_send`
- Drops channels after agent run to flush StreamConsumer
- On error (D-18): sends "\n\n-- Something went wrong, please try again" as separate message
- On success: updates session with assistant messages from result

`with_rate_limit_retry()` (D-19):
- Free async function, up to 3 attempts
- Detects "429" or "Too Many Requests" in error string
- Waits 2s, 4s, 6s between attempts
- Returns error "Bot is being rate limited, please wait" after 3 failures

**session.rs** additions:
- `GatewaySession::is_expired(timeout_hours: u64) -> bool` — checks updated_at against cutoff
- `SessionStore::expire_stale(timeout_hours: u64)` — retains only non-expired sessions, logs eviction count

**lib.rs** additions:
- `pub mod handler` module declaration
- `pub use handler::GatewayMessageHandler` re-export

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing] Plan 03 base handler.rs created as prerequisite**
- **Found during:** Task 1 setup
- **Issue:** handler.rs did not exist — plan 03 (parallel wave 2) hadn't run yet in this worktree
- **Fix:** Created full GatewayMessageHandler with plan 03 base structure (run_agent, streaming bridge, typing indicator) then added plan 04 slash command additions on top. Merge will reconcile.
- **Files modified:** crates/ironhermes-gateway/src/handler.rs (created)
- **Commit:** 91f3a3f

**2. [Rule 2 - Missing] session.rs expire_stale/is_expired added**
- **Found during:** Task 1 — these methods are referenced by plan 03's runner.rs and needed for completeness
- **Fix:** Added `is_expired()` to `GatewaySession` and `expire_stale()` to `SessionStore`
- **Files modified:** crates/ironhermes-gateway/src/session.rs
- **Commit:** 91f3a3f

## Verification

- `cargo check -p ironhermes-gateway`: Finished with 0 errors
- `cargo check --workspace`: Finished with 0 errors
- handler.rs contains `fn handle_slash_command` — confirmed
- handler.rs contains all 4 slash command match arms — confirmed
- handler.rs contains `fn run_agent` — confirmed
- handler.rs contains "Please introduce yourself" — confirmed (D-15 LLM greeting)
- handler.rs contains "Something went wrong" — confirmed (D-18 error append)
- handler.rs contains `429` and `with_rate_limit_retry` — confirmed (D-19)
- handler.rs contains `session.clear()` — confirmed (/clear handler)
- handler.rs contains `store.remove` — confirmed (/new and /start handlers)

## Self-Check: PASSED

Files exist:
- FOUND: crates/ironhermes-gateway/src/handler.rs
- FOUND: crates/ironhermes-gateway/src/lib.rs
- FOUND: crates/ironhermes-gateway/src/session.rs

Commits exist:
- 91f3a3f: feat(02-04): slash command dispatch and error recovery in handler

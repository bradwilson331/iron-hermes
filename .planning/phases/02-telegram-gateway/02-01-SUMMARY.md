---
phase: 02-telegram-gateway
plan: "01"
subsystem: gateway
tags: [rust, telegram, async, tokio, cancellation-token, streaming]
dependency_graph:
  requires: []
  provides: [CancellationToken shutdown primitive, streaming-capable MessageHandler trait, extended PlatformGatewayConfig, multimodal TgMessage types]
  affects: [ironhermes-gateway, ironhermes-core]
tech_stack:
  added: [tokio-util 0.7 (rt feature), pdf-extract 0.10]
  patterns: [CancellationToken for shutdown signaling, Arc<dyn PlatformAdapter> injected into handler for streaming, pub use re-export for polling module]
key_files:
  created: []
  modified:
    - Cargo.toml
    - crates/ironhermes-gateway/Cargo.toml
    - crates/ironhermes-core/src/config.rs
    - crates/ironhermes-gateway/src/adapter.rs
    - crates/ironhermes-gateway/src/telegram.rs
    - crates/ironhermes-gateway/src/lib.rs
    - crates/ironhermes-gateway/src/runner.rs
decisions:
  - "CancellationToken pub use re-exported from telegram.rs so plan 03 polling module can import it from a single gateway-internal location"
  - "send_message uses plain text (no parse_mode); edit_message_markdown adds Markdown only for final streaming edit — per D-03"
  - "GatewayRunner stubbed to placeholder start() — full polling wiring deferred to plan 03"
  - "api_call() made public so handler modules can make direct Bot API calls if needed"
metrics:
  duration: "3 minutes"
  completed: "2026-04-02"
  tasks_completed: 2
  files_modified: 7
---

# Phase 2 Plan 01: Async Foundation — Telegram Gateway Primitives Summary

**One-liner:** CancellationToken-based shutdown, streaming-capable MessageHandler trait, extended config with whitelist/concurrency fields, and multimodal TgMessage types added to establish async foundation for Telegram gateway.

## What Was Built

### Task 1: tokio-util dependency + PlatformGatewayConfig extension

- Added `tokio-util = { version = "0.7", features = ["rt"] }` and `pdf-extract = "0.10"` to workspace `Cargo.toml`
- Added both as dependencies in `crates/ironhermes-gateway/Cargo.toml`
- Extended `PlatformGatewayConfig` with three new fields:
  - `whitelist: Vec<i64>` — Telegram user IDs allowed to interact (D-12)
  - `session_timeout_hours: u64` — inactivity timeout, defaults to 24h (D-14)
  - `max_concurrent_runs: usize` — concurrency cap, defaults to 8 (TG-06)
- Added `default_session_timeout_hours()` and `default_max_concurrent_runs()` free functions for serde defaults
- Config remains backward-compatible via `#[serde(default)]` — existing config.yaml loads unchanged

### Task 2: MessageHandler redesign + TelegramAdapter extension

**adapter.rs** — redesigned traits:
- `MessageHandler::handle()` now takes `Arc<dyn PlatformAdapter>` and `CancellationToken` alongside the event, enabling the handler to drive streaming edits directly
- `PlatformAdapter` loses `start()`/`stop()` methods (lifecycle moved to `GatewayRunner`)
- Added `edit_message_markdown()` and `send_chat_action()` to `PlatformAdapter` trait (with default no-ops)

**telegram.rs** — major refactor:
- Removed `Arc<AtomicBool>` + `poll_handle` fields; struct now holds only `token`, `http`, `bot_username`
- Removed polling loop (moves to dedicated module in plan 03)
- `send_message` no longer includes `parse_mode: "Markdown"` — plain text for streaming edits
- Added `edit_message_markdown()` with `parse_mode: "Markdown"` for final message edits
- Added `send_chat_action()` (`sendChatAction` Bot API method)
- Added `set_my_commands()` (`setMyCommands` — Telegram-specific, not on trait)
- Added `get_file()` (`getFile`) and `download_file()` for multimodal attachment support
- Added `get_me()` public method (previously inline in start())
- Made `api_call()` public for direct use by handler modules
- Added types: `TgBotCommand`, `TgFile`, `TgPhotoSize`, `TgDocument`
- Extended `TgMessage` with `photo`, `document`, `caption` fields
- All `Tg*` types made public
- `tg_message_to_event()` made public; uses `caption` as fallback when `text` is None
- `CancellationToken` re-exported from telegram module for plan 03 polling

**runner.rs** — stubbed to minimal placeholder:
- Removed handler parameter, session store, and adapter vec
- `start()` logs placeholder message and returns `Ok(())`

**lib.rs** — updated exports:
- Removed `MessageHandler` from public re-exports (moves to `handler.rs` in plan 03)
- Added exports for all new public Tg* types

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Duplicate CancellationToken import**
- **Found during:** Task 2 verification
- **Issue:** `use tokio_util::sync::CancellationToken` at top + `pub use tokio_util::sync::CancellationToken` at bottom caused E0252 duplicate definition error
- **Fix:** Removed the top-level `use` import; kept only `pub use` re-export (satisfies both "imported from tokio_util" and "re-exported for plan 03" requirements)
- **Files modified:** `crates/ironhermes-gateway/src/telegram.rs`
- **Commit:** 370a695

**2. [Rule 2 - Missing] Suppress dead_code warning on stub runner field**
- **Found during:** Task 2 verification
- **Issue:** `config` field on stub `GatewayRunner` triggered `dead_code` warning
- **Fix:** Added `#[allow(dead_code)]` annotation — field is needed when plan 03 wires up the real runner
- **Files modified:** `crates/ironhermes-gateway/src/runner.rs`
- **Commit:** 370a695

## Known Stubs

- `GatewayRunner::start()` logs placeholder and returns `Ok(())` — wired in plan 03
- `TelegramAdapter::is_running()` always returns `false` — lifecycle tracking deferred to plan 03 runner

## Verification

- `cargo check --workspace`: Finished with 0 errors (3 expected warnings for unused imports in stub)
- `cargo test --workspace`: All tests pass (31 existing tests unaffected)
- No `AtomicBool` in telegram.rs
- `CancellationToken` available via `pub use` in telegram.rs
- `PlatformGatewayConfig` deserializes correctly (serde default annotations preserve backward compat)

## Self-Check: PASSED

Files exist:
- FOUND: crates/ironhermes-gateway/src/adapter.rs
- FOUND: crates/ironhermes-gateway/src/telegram.rs
- FOUND: crates/ironhermes-core/src/config.rs

Commits exist:
- 30387c0: feat(02-01): add tokio-util/pdf-extract deps and extend PlatformGatewayConfig
- 370a695: feat(02-01): redesign MessageHandler trait and extend TelegramAdapter

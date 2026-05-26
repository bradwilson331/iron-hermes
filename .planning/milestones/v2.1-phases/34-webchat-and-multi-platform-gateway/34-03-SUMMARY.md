---
phase: 34-webchat-and-multi-platform-gateway
plan: 03
subsystem: api, infra
tags: [rust, serenity, discord, platform-adapter, gateway, cancellation, whitelist]

# Dependency graph
requires:
  - phase: 34-02
    provides: serenity 0.12.5 in Cargo.toml + PlatformGatewayConfig shape

provides:
  - DiscordAdapter implementing PlatformAdapter (7 methods)
  - DiscordEventHandler routing Discord messages through handle_with_multimodal
  - run_discord_adapter startup fn with CancellationToken + shard shutdown
  - discord_message_to_event message conversion helper
  - slack.rs stub (Wave 3 placeholder — makes invariants_34.rs compile)

affects:
  - 34-05-runner-wiring (run_discord_adapter is the entry point)
  - invariants_34.rs INV-34-01 (now GREEN)

# Tech tracking
tech-stack:
  added: []
  patterns:
    - DiscordAdapter holds SerenityContext (no token stored in adapter)
    - ProcessedAttachments struct literal construction (no Default impl — use {text_prefix: None, image_data_uri: None})
    - classify_chat_type extracted for unit-testability (None -> "dm", Some -> "group")
    - Canonical whitelist semantics: empty = deny-all warn-and-return (mirrors runner.rs:601-611)
    - tokio::select! over client.start() with cancel.cancelled() arm + shard_manager.shutdown_all()
    - shard_manager cloned before client.start() to avoid borrow-after-move

key-files:
  created:
    - crates/ironhermes-gateway/src/discord.rs
    - crates/ironhermes-gateway/src/slack.rs
  modified:
    - crates/ironhermes-gateway/src/lib.rs

key-decisions:
  - "ProcessedAttachments has no Default impl — used struct literal {text_prefix: None, image_data_uri: None} (auto-fixed Rule 1)"
  - "slack.rs stub created so invariants_34.rs compiles — include_str! is resolved at compile time; INV-34-02 stays RED until Plan 34-04"
  - "shard_manager cloned from client before tokio::select! to avoid partial move of client"
  - "Tasks 1+2 committed together (discord.rs written atomically) + Task 3 as separate lib.rs commit"
  - "classify_chat_type extracted as standalone fn for unit-testability without serenity Message construction"

# Metrics
duration: 4min
completed: 2026-05-19T18:42:19Z
---

# Phase 34 Plan 03: Discord Adapter Summary

**One-liner:** Discord gateway adapter with serenity 0.12.5 — PlatformAdapter impl + EventHandler routing through handle_with_multimodal with canonical whitelist semantics and CancellationToken shutdown.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1+2 | DiscordAdapter + DiscordEventHandler + run_discord_adapter | eb605fdf | discord.rs (new), slack.rs (stub) |
| 3 | Wire discord module in lib.rs (INV-34-01 GREEN) | af564fcb | lib.rs |

## What Was Built

### discord.rs (new — 288 lines)

**DiscordAdapter** implements all 7 `PlatformAdapter` methods:
- `platform()` → `Platform::Discord`
- `send_message` → `ChannelId::say(&ctx.http, content)` → `MessageResponse`
- `edit_message` + `edit_message_markdown` → `ChannelId::edit_message(EditMessage::new().content(...))` (markdown delegates to edit_message since Discord renders natively)
- `delete_message` → `ChannelId::delete_message(&ctx.http, msg_id)`
- `add_reaction` + `send_chat_action` → trait default no-ops
- `is_running()` → `false`

**discord_message_to_event** converts `serenity::model::channel::Message` to `ironhermes_core::MessageEvent`:
- `platform: Platform::Discord`
- `chat_type`: `classify_chat_type(guild_id)` — None → "dm", Some → "group"
- `thread_id`: `msg.thread.as_ref().map(|t| t.id.to_string())`
- `replied_to_id`: `msg.referenced_message.as_ref().map(|m| m.id.to_string())`

**DiscordEventHandler** (serenity EventHandler):
- Skip bot messages (`msg.author.bot`)
- T-34-03: reject empty content with `tracing::warn!` (MESSAGE_CONTENT intent guard)
- Canonical whitelist gate (D-12): empty → deny-all warn-and-return; non-empty → sender ID check
- Routes to `handler.handle_with_multimodal(&event, adapter, cancel.child_token(), processed)`

**run_discord_adapter**:
- `GatewayIntents::GUILD_MESSAGES | DIRECT_MESSAGES | MESSAGE_CONTENT`
- `Client::builder(token, intents).event_handler(...).await`
- `tokio::select!` over `client.start()` / `cancel.cancelled()` → `shard_manager.shutdown_all()`

### slack.rs (stub — 4 lines)
Minimal placeholder so `invariants_34.rs` compiles. INV-34-02 remains RED until Plan 34-04.

### lib.rs (modified)
- `pub mod discord;` added alphabetically (after backoff, before handler)
- `pub use discord::{DiscordAdapter, run_discord_adapter};`

## Verification Results

| Check | Result |
|-------|--------|
| `cargo build -p ironhermes-gateway` | PASS |
| `cargo test -p ironhermes-gateway --lib discord` | PASS (2/2) |
| `cargo clippy -p ironhermes-gateway --lib -- -D clippy::await_holding_lock` | PASS (clean) |
| INV-34-01 (`inv_34_01_discord_routes_through_handle_with_multimodal`) | GREEN |
| INV-34-02 (`inv_34_02_slack_routes_through_handle_with_multimodal`) | RED (expected — Wave 3) |

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] ProcessedAttachments has no Default impl**
- **Found during:** Task 1 — first build attempt
- **Issue:** Plan specified `ProcessedAttachments::default()` but the struct does not derive or implement `Default`
- **Fix:** Used struct literal `ProcessedAttachments { text_prefix: None, image_data_uri: None }` (matches the pattern used in handler.rs lines 519-522)
- **Files modified:** crates/ironhermes-gateway/src/discord.rs
- **Commit:** eb605fdf

**2. [Rule 3 - Blocking] slack.rs missing — invariants_34.rs fails to compile**
- **Found during:** Task 3 — INV-34-01 test run
- **Issue:** `invariants_34.rs` uses `include_str!("../src/slack.rs")` at compile time. With no `slack.rs`, the entire test binary fails to compile — INV-34-01 cannot run
- **Fix:** Created `slack.rs` stub (4-line comment placeholder) so the test binary compiles. INV-34-02 correctly returns 0 matches (RED) since the stub contains no `handle_with_multimodal`
- **Files modified:** crates/ironhermes-gateway/src/slack.rs (new)
- **Commit:** eb605fdf

## Security Threat Mitigations Applied

| Threat | Mitigation | Grep Assertion |
|--------|------------|----------------|
| T-34-01: Token logging | Token never passed to `tracing::*!` macros | `grep -c "tracing::.*!.*token"` → 0 |
| T-34-02: Unauthorized sender | Canonical whitelist — empty = deny-all (D-12) | `grep -c "denying all messages (D-12)"` → 1 |
| T-34-03: Empty content (MESSAGE_CONTENT intent) | `msg.content.is_empty()` guard + warn | `grep -c "msg.content.is_empty"` → 1 |
| T-34-PITFALL-3: Mutex across await | No nudge_turns access in adapter; clippy gate passed | `cargo clippy -D await_holding_lock` → clean |
| T-34-PITFALL-5: client.start() hang | `tokio::select!` + `shard_manager.shutdown_all()` | `grep -c "shard_manager"` → 2 |

## Known Stubs

None that affect plan goal delivery. The `slack.rs` stub is intentional — it's a Wave 3 placeholder, not a functionality stub for this plan's deliverables.

## Threat Flags

None — no new network endpoints, auth paths, or schema changes beyond what the plan's threat model covers.

## Clippy Note for Future Plans

The `cargo clippy -p ironhermes-gateway --lib -- -D clippy::await_holding_lock` baseline is CLEAN as of this plan. The 19 warnings emitted are pre-existing from runner.rs and session.rs (collapsible_if, unused_imports, dead_code) — none are from discord.rs. Future plans should preserve this baseline.

## Self-Check

- [x] `crates/ironhermes-gateway/src/discord.rs` exists
- [x] `crates/ironhermes-gateway/src/lib.rs` modified
- [x] Commit eb605fdf exists (discord.rs + slack.rs)
- [x] Commit af564fcb exists (lib.rs)
- [x] INV-34-01 GREEN
- [x] INV-34-02 RED (expected)
- [x] No STATE.md or ROADMAP.md modifications

## Self-Check: PASSED

---
phase: 02-telegram-gateway
plan: "05"
subsystem: gateway
tags: [rust, telegram, multimodal, vision, pdf-extract, base64, cli, async, tokio]
dependency_graph:
  requires: [02-01 (TelegramAdapter get_file/download_file), 02-02 (StreamConsumer), 02-03 (GatewayRunner), 02-04 (GatewayMessageHandler slash commands)]
  provides: [multimodal attachment processing, gateway CLI subcommand, complete Telegram gateway pipeline]
  affects: [ironhermes-gateway, ironhermes-core, ironhermes-cli]
tech_stack:
  added: [base64 = "0.22" (workspace + gateway dep), ironhermes-gateway dep in ironhermes-cli]
  patterns:
    - ProcessedAttachments passed through QueuedMessage for deferred multimodal routing
    - Image attachments encoded as data:image/jpeg;base64 URI for vision LLM input
    - PDF text extraction via pdf_extract::extract_text_from_mem
    - file_id field added to Attachment for platform-specific deferred download
    - handle_with_multimodal public method bypasses trait object for typed attachment data
key_files:
  created:
    - crates/ironhermes-gateway/src/multimodal.rs
  modified:
    - Cargo.toml
    - crates/ironhermes-gateway/Cargo.toml
    - crates/ironhermes-gateway/src/lib.rs
    - crates/ironhermes-gateway/src/handler.rs
    - crates/ironhermes-gateway/src/runner.rs
    - crates/ironhermes-gateway/src/telegram.rs
    - crates/ironhermes-gateway/src/user_queue.rs
    - crates/ironhermes-core/src/types.rs
    - crates/ironhermes-cli/Cargo.toml
    - crates/ironhermes-cli/src/main.rs
decisions:
  - "ProcessedAttachments stored in QueuedMessage fields (not MessageEvent.attachments.data) — keeps attachment data separate from serializable event"
  - "handle_with_multimodal added as public method alongside MessageHandler::handle trait impl — avoids trait object downcasting for TelegramAdapter"
  - "text_prefix and image_data_uri added to QueuedMessage rather than MessageEvent — multimodal data is processing output, not event metadata"
  - "file_id added to Attachment struct in ironhermes-core — enables platform-agnostic deferred download pattern"
  - "Runner dispatch loop processes attachments synchronously before queuing — ensures ordering and lets errors be reported before worker spawns"
metrics:
  duration: "~10 minutes"
  completed: "2026-04-02"
  tasks_completed: 1
  files_modified: 11
  checkpoint_pending: true
---

# Phase 2 Plan 05: Multimodal Input and Gateway CLI Subcommand Summary

**One-liner:** Image→base64 vision input, PDF/text document extraction, 20MB limit, QueuedMessage multimodal routing, and `ironhermes gateway` CLI subcommand launching GatewayRunner.

## What Was Built

### Task 1: Multimodal input processing and gateway CLI subcommand (COMPLETED)

**multimodal.rs** — New file: attachment processing for vision and document inputs:
- `process_attachments(adapter, msg)` — async function handling all attachment types
- **Images (D-05/D-06):** Selects largest photo (Telegram sends array sorted ascending by size), calls `get_file` + `download_file`, base64-encodes bytes as `data:image/jpeg;base64,...` URI
- **PDFs (D-07):** Downloads file, calls `pdf_extract::extract_text_from_mem`, wraps as `[Document: filename]\ncontent`
- **Text documents:** `String::from_utf8_lossy` for text/plain, text/markdown, text/csv, text/html
- **Size limit (D-08):** Rejects files >20MB with user-friendly message before download
- **Unsupported types:** Returns descriptive error listing supported types

**ironhermes-core/src/types.rs** — Extended:
- `Attachment.file_id: Option<String>` — platform-specific file identifier for deferred Telegram downloads

**telegram.rs** — Updated `tg_message_to_event`:
- Now populates `attachments` vec with photo and document metadata (file_id, mime_type, filename)
- Actual download deferred to `multimodal::process_attachments`

**user_queue.rs** — Extended `QueuedMessage`:
- `text_prefix: Option<String>` — document-extracted text prefix
- `image_data_uri: Option<String>` — base64 image URI for vision
- `dispatch()` signature updated to accept and thread these values through

**runner.rs** — Dispatch loop updated:
- Imports `PlatformAdapter` for `send_message` method availability
- Calls `multimodal::process_attachments` on messages with non-empty attachments
- On error: sends user-friendly error message and skips (no worker spawned)
- On success: passes `text_prefix` and `image_data_uri` to `user_queue.dispatch`
- Per-chat worker extracts `ProcessedAttachments` from `QueuedMessage` and calls `handle_with_multimodal`

**handler.rs** — Updated:
- `run_agent` now takes `ProcessedAttachments` parameter
- `build_user_message()` free function: builds vision `ContentPart::Parts` message or text-prefix message based on attachment data
- `handle_with_multimodal()` public method: slash command check + `run_agent` with attachment data
- All internal `run_agent` callers updated with `ProcessedAttachments { text_prefix: None, image_data_uri: None }` for non-attachment paths

**lib.rs** — Added:
- `pub mod multimodal` module declaration

**crates/ironhermes-cli/src/main.rs** — Gateway CLI subcommand:
- `Commands::Gateway { token: Option<String> }` variant added
- `run_gateway()` async function: loads config, builds registry, optionally overrides token, calls `GatewayRunner::new + start`
- Match arm wired in `main()`

**crates/ironhermes-cli/Cargo.toml** — Added `ironhermes-gateway` dependency.

**Cargo.toml (workspace)** — Added `base64 = "0.22"` to workspace dependencies.

### Task 2: End-to-end Telegram bot smoke test (CHECKPOINT PENDING)

Awaiting human verification of live bot. See verification steps below.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing] `file_id` field added to `Attachment` struct**
- **Found during:** Task 1 — Attachment struct had no `file_id` field
- **Fix:** Added `pub file_id: Option<String>` with `#[serde(skip_serializing_if = "Option::is_none")]` to `Attachment` in ironhermes-core/src/types.rs
- **Files modified:** crates/ironhermes-core/src/types.rs
- **Commit:** 26f8ac9

**2. [Rule 1 - Bug] `PlatformAdapter` import missing in runner.rs**
- **Found during:** Task 1 verification (cargo build)
- **Issue:** `send_message` call on `Arc<TelegramAdapter>` failed — trait not in scope
- **Fix:** Added `use crate::adapter::PlatformAdapter` import
- **Files modified:** crates/ironhermes-gateway/src/runner.rs
- **Commit:** 26f8ac9

**3. [Rule 2 - Missing] `handle_with_multimodal` public method added**
- **Found during:** Task 1 — `MessageHandler::handle` trait doesn't carry attachment data; downcasting Arc<dyn PlatformAdapter> to TelegramAdapter would require `Any` bounds
- **Fix:** Added `handle_with_multimodal` as public method on `GatewayMessageHandler` alongside trait impl; runner calls this directly since it has typed access to `QueuedMessage`
- **Files modified:** crates/ironhermes-gateway/src/handler.rs
- **Commit:** 26f8ac9

## Known Stubs

None — multimodal data is fully wired: download → encode/extract → QueuedMessage → handler → ChatMessage → AgentLoop.

## Verification

- `cargo build --workspace`: Finished with 0 errors, 0 warnings
- `cargo test --workspace`: 55 tests pass (20 agent + 9 cron + 26 gateway user_queue), 0 failures
- `cargo run -p ironhermes-cli -- gateway --help`: Shows gateway subcommand with `--token` option
- multimodal.rs contains `ProcessedAttachments` struct — confirmed
- multimodal.rs contains `process_attachments` function — confirmed
- multimodal.rs contains `pdf_extract::extract_text_from_mem` — confirmed
- multimodal.rs contains `base64::engine::general_purpose::STANDARD.encode` — confirmed
- multimodal.rs contains `MAX_FILE_SIZE = 20 * 1024 * 1024` — confirmed
- lib.rs contains `pub mod multimodal` — confirmed
- main.rs contains `Gateway` in Commands enum — confirmed
- main.rs contains `run_gateway` function — confirmed
- main.rs contains `GatewayRunner` — confirmed

## Self-Check: PASSED

Files exist:
- FOUND: crates/ironhermes-gateway/src/multimodal.rs
- FOUND: crates/ironhermes-gateway/src/handler.rs
- FOUND: crates/ironhermes-gateway/src/runner.rs
- FOUND: crates/ironhermes-gateway/src/lib.rs
- FOUND: crates/ironhermes-cli/src/main.rs
- FOUND: crates/ironhermes-core/src/types.rs

Commits exist:
- 26f8ac9: feat(02-05): multimodal input processing and gateway CLI subcommand

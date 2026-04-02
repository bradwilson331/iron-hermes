---
phase: 02-telegram-gateway
plan: "02"
subsystem: gateway
tags: [rust, async, telegram, streaming, backoff]
dependency_graph:
  requires: []
  provides: [BackoffState, StreamConsumer]
  affects: [adapter.rs, telegram.rs, lib.rs]
tech_stack:
  added: []
  patterns:
    - Exponential backoff with subsecond-nanos jitter (zero extra deps)
    - Throttled async edit loop with dirty-flag short-circuit
    - Block cursor appended to streaming text, stripped on final Markdown edit
    - Paragraph-boundary overflow splitting with 4-tier fallback
key_files:
  created:
    - crates/ironhermes-gateway/src/backoff.rs
    - crates/ironhermes-gateway/src/stream_consumer.rs
  modified:
    - crates/ironhermes-gateway/src/lib.rs
    - crates/ironhermes-gateway/src/adapter.rs
    - crates/ironhermes-gateway/src/telegram.rs
decisions:
  - "edit_message uses plain text (no parse_mode) for streaming; edit_message_markdown uses Markdown for final edit only (D-03)"
  - "find_split_point uses 4-tier priority: double-newline > single-newline > period-space > hard split"
  - "Jitter source is SystemTime subsec_nanos modulo (base/4+1) — zero-dependency, non-cryptographic, sufficient for backoff"
  - "BackoffState tracks conflict_count separately from failures to detect 409 fatal threshold independently"
metrics:
  duration: "4m"
  completed_date: "2026-04-02"
  tasks_completed: 2
  files_changed: 5
---

# Phase 02 Plan 02: StreamConsumer and BackoffState Summary

BackoffState for exponential-backoff polling recovery and StreamConsumer for throttled Telegram streaming edits with cursor, tool status, and 4096-char overflow chaining.

## What Was Built

### Task 1: BackoffState (`backoff.rs`)

Pure-logic struct implementing exponential backoff with jitter for polling error recovery (TG-07).

- `next_delay()` computes `min(base * 2^failures + jitter, cap)` using subsecond nanos as jitter source
- `record_failure()` / `record_success()` track consecutive failures; success resets to 0
- `record_conflict()` increments both `conflict_count` and `failures`
- `is_fatal_conflict()` returns `true` at 5+ consecutive 409 conflicts
- Default constructor: 1s base, 60s cap (`default_polling()`)
- 10 unit tests: initial delay, doubling at 1 failure, ~32s at 5 failures, cap enforcement, reset, fatal threshold at 4 and 5 conflicts, jitter bounds

### Task 2: StreamConsumer (`stream_consumer.rs`)

Async struct bridging AgentLoop streaming output to Telegram's edit API (TG-03, D-01 through D-04).

- `push(chunk)` accumulates text into buffer, marks dirty
- `tool_status(name)` sets tool execution status line: `"\n\n⚙️ Running: {name}..."` (D-02)
- `clear_tool_status()` removes the tool line before next content push
- `flush(false)` — throttled streaming edit: skips if not dirty or < 300ms since last edit; appends block cursor `█` to display; plain text `edit_message` (D-01, D-03)
- `flush(true)` — final edit: skips cursor, calls `edit_message_markdown` with Markdown parse mode (D-03)
- Overflow handling: when display > 4096 chars, splits buffer at best paragraph boundary, finalizes current message, sends new message for continuation, tracks chain (D-04)
- `find_split_point()` — 4-tier priority: `\n\n` → `\n` → `. ` → hard split at max_len
- `message_ids()` returns all message IDs (current + chained overflow) for cleanup
- 12 unit tests covering all behavior specs

### Adapter trait additions (`adapter.rs`, `telegram.rs`)

Added to `PlatformAdapter` trait (blocking dep for StreamConsumer):
- `edit_message_markdown()` — required method for D-03 final edits
- `send_chat_action()` — default no-op (needed by future plan 03 typing indicator)

Fixed `TelegramAdapter`:
- `edit_message()` now sends plain text (no `parse_mode`) — was incorrectly using Markdown
- Added `edit_message_markdown()` impl with `parse_mode: "Markdown"`

## Verification

```
cargo test -p ironhermes-gateway   → 22 passed, 0 failed
cargo check --workspace            → Finished (no errors)
```

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Added `edit_message_markdown` to `PlatformAdapter` trait**
- **Found during:** Task 2 implementation
- **Issue:** `PlatformAdapter` trait in existing `adapter.rs` was missing `edit_message_markdown` and `send_chat_action` methods required by StreamConsumer. Plan 01 (trait redesign) had not yet run.
- **Fix:** Added `edit_message_markdown` as required method and `send_chat_action` as default no-op to the trait; added corresponding impl to `TelegramAdapter`.
- **Files modified:** `crates/ironhermes-gateway/src/adapter.rs`, `crates/ironhermes-gateway/src/telegram.rs`
- **Commit:** dc172a5

**2. [Rule 1 - Bug] Fixed `TelegramAdapter::edit_message` using wrong parse mode**
- **Found during:** Task 2 — adding `edit_message_markdown`
- **Issue:** Existing `edit_message` was using `"parse_mode": "Markdown"` which contradicts D-03 (plain text during streaming).
- **Fix:** Removed `parse_mode` from `edit_message`; Markdown mode only in new `edit_message_markdown`.
- **Files modified:** `crates/ironhermes-gateway/src/telegram.rs`
- **Commit:** dc172a5

**3. [Rule 1 - Bug] Overflow test content size**
- **Found during:** Task 2 TDD GREEN phase
- **Issue:** Initial overflow test used 4002-char buffer (2×2000 + `\n\n`) which is below 4096 threshold — overflow never triggered.
- **Fix:** Increased to 2×2500 chars to reliably exceed 4096-char limit.
- **Files modified:** `crates/ironhermes-gateway/src/stream_consumer.rs`
- **Commit:** dc172a5

## Known Stubs

None — both modules are complete implementations with no hardcoded placeholders.

## Self-Check: PASSED

- FOUND: crates/ironhermes-gateway/src/backoff.rs
- FOUND: crates/ironhermes-gateway/src/stream_consumer.rs
- FOUND: .planning/phases/02-telegram-gateway/02-02-SUMMARY.md
- FOUND: commit 7ea73cc (BackoffState)
- FOUND: commit dc172a5 (StreamConsumer)

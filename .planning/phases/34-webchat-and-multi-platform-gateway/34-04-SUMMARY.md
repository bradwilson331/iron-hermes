---
phase: 34-webchat-and-multi-platform-gateway
plan: 04
subsystem: api, infra
tags: [rust, slack-morphism, slack, platform-adapter, gateway, cancellation, whitelist, socket-mode]

# Dependency graph
requires:
  - phase: 34-02
    provides: slack-morphism 2.22.0 (axum feature) in Cargo.toml + PlatformGatewayConfig.app_token
  - phase: 34-03
    provides: discord.rs + slack.rs stub + lib.rs with pub mod discord

provides:
  - SlackAdapter implementing PlatformAdapter (7 methods) with Arc<SlackHyperClient> + bot_token
  - slack_event_to_message_event converting SlackMessageEvent to MessageEvent (Platform::Slack)
  - classify_slack_channel_type: D-prefix -> "dm", C/G-prefix -> "group"
  - run_slack_adapter: Socket Mode listener + CancellationToken + canonical whitelist + non-blocking ACK
  - SlackAdapterState user-state pattern for threading captured state into fn-pointer callbacks
  - pub mod slack + pub use slack::{SlackAdapter, run_slack_adapter} in lib.rs
  - INV-34-02 flipped RED -> GREEN

affects:
  - 34-05-runner-wiring (run_slack_adapter is the entry point for Wave 3 GatewayRunner integration)
  - invariants_34.rs INV-34-01 (still GREEN) + INV-34-02 (now GREEN)

# Tech tracking
tech-stack:
  added: []
  patterns:
    - SlackAdapter holds Arc<SlackHyperClient> + SlackApiToken (bot_token) for Web API calls
    - SlackAdapterState stored in SlackClientEventsListenerEnvironment::with_user_state<T>() to
      thread state into fn-pointer push-events callback (slack-morphism UserCallbackFunction is
      a bare fn pointer — closures capturing variables cannot be coerced to fn pointers)
    - on_push_event: standalone async fn (fn pointer), reads state via state_storage.read().await
    - tokio::spawn(async move { handle_with_multimodal }) pattern for T-34-04 non-blocking ACK
    - Canonical whitelist empty = deny-all semantics (mirrors runner.rs:601-611, D-12)
    - classify_slack_channel_type extracted for unit-testability (D-prefix=dm, else=group)
    - ProcessedAttachments struct literal (no Default impl — per Wave 2 deviation from Plan 03)

key-files:
  created: []
  modified:
    - crates/ironhermes-gateway/src/slack.rs
    - crates/ironhermes-gateway/src/lib.rs

key-decisions:
  - "UserCallbackFunction is a bare fn pointer type in slack-morphism 2.22.0 — closures that capture
    variables cannot be coerced to fn pointers; state is threaded via with_user_state<SlackAdapterState>
    and retrieved from SlackClientEventsUserState RwLock in the callback fn body"
  - "ErrorHandler is also a bare fn pointer (not a closure) — on_socket_mode_error is a standalone fn"
  - "HttpStatusCode used for error handler return type (re-exported from slack_morphism::prelude::*
    via the hyper-base feature, available under the axum feature activated by Cargo.toml)"
  - "SlackHyperClient type alias (= SlackClient<SlackClientHyperHttpsConnector>) used instead of the
    generic SlackClient<SlackClientHyperConnector> — the type alias is the concrete type in 2.22.0"
  - "Bot-message skip: sender.bot_id.is_some() + subtype guards (BotMessage, MessageChanged,
    MessageDeleted) to avoid processing edited/deleted events as new messages"

# Metrics
duration: 8min
completed: 2026-05-19T18:53:15Z
---

# Phase 34 Plan 04: Slack Adapter Summary

**One-liner:** SlackAdapter (PlatformAdapter impl + Socket Mode listener) with canonical whitelist deny-all, non-blocking ACK via tokio::spawn, and CancellationToken shutdown — flips INV-34-02 GREEN to complete Wave 2.

## Performance

- **Duration:** ~8 min
- **Started:** 2026-05-19T18:45:35Z
- **Completed:** 2026-05-19T18:53:15Z
- **Tasks:** 3 (Tasks 1+2 combined in slack.rs, Task 3 lib.rs wiring)
- **Files modified:** 2

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1+2 | SlackAdapter + slack_event_to_message_event (initial impl) | efad144d | slack.rs |
| fix | Rewrite callback using fn-pointer + user-state pattern | 8645caaa | slack.rs |
| 3 | Wire slack module in lib.rs; INV-34-01/02 both GREEN | a8577eed | lib.rs |

## What Was Built

### slack.rs (full implementation — replaces Wave 2 4-line stub)

**SlackAdapter** implements all 7 `PlatformAdapter` methods:
- `platform()` → `Platform::Slack`
- `send_message` → `session.chat_post_message(SlackApiChatPostMessageRequest::new(channel, content))` → `MessageResponse` with `ts` as `message_id`
- `edit_message` → `session.chat_update(SlackApiChatUpdateRequest::new(channel, content, ts))`
- `edit_message_markdown` → delegates to `edit_message` (Slack auto-formats mrkdwn)
- `delete_message` → `session.chat_delete(SlackApiChatDeleteRequest::new(channel, ts))`
- `add_reaction` + `send_chat_action` → trait default no-ops (deferred)
- `is_running()` → `false`

**slack_event_to_message_event** converts `SlackMessageEvent` to `MessageEvent`:
- `platform: Platform::Slack`
- `message_id: event.origin.ts.to_string()`
- `chat_id: event.origin.channel.as_ref().map(|c| c.to_string()).unwrap_or_default()`
- `sender_id: event.sender.user.as_ref().map(|u| u.to_string()).unwrap_or_default()`
- `content: event.content.as_ref().and_then(|c| c.text.clone()).unwrap_or_default()`
- `thread_id: event.origin.thread_ts.as_ref().map(|t| t.to_string())`
- `chat_type: classify_slack_channel_type(&chat_id)` — "dm" for D-prefix, "group" otherwise

**classify_slack_channel_type**: D-prefix → "dm"; C/G-prefix → "group" (unit-tested, 3 tests)

**SlackAdapterState**: state struct stored via `with_user_state<SlackAdapterState>()` threading
whitelist, handler, cancel, adapter into the fn-pointer `on_push_event` callback.

**on_push_event** (fn pointer, not closure):
- Skip non-Message events → Ok(())
- Skip bot messages (sender.bot_id.is_some())
- Skip BotMessage/MessageChanged/MessageDeleted subtypes
- Retrieve state from state_storage.read().await
- CANONICAL whitelist check (D-12): empty → deny-all warn-and-return; non-empty → sender ID check
- Build MessageEvent + ProcessedAttachments BEFORE spawn (borrow-checker safe)
- Drop state_read guard before spawn
- tokio::spawn(async move { h.handle_with_multimodal(&event, a, c, processed) }) (T-34-04)
- Returns Ok(()) immediately (3-second ACK deadline satisfied)

**run_slack_adapter**:
- `SlackClientHyperConnector::new()?` → `Arc<SlackHyperClient>`
- `SlackApiToken::new(bot_token.into())` + `SlackApiToken::new(app_token.into())`
- `SlackSocketModeListenerCallbacks::new().with_push_events(on_push_event)`
- `SlackClientEventsListenerEnvironment::new(client).with_error_handler(on_socket_mode_error).with_user_state(state)`
- `SlackClientSocketModeListener::new(&config, env, callbacks)`
- `tokio::select!` over `listener.listen_for(&app_token_obj)` vs `cancel.cancelled()`

### lib.rs (modified)
- `pub mod slack;` added alphabetically (after `pub mod session;`)
- `pub use slack::{SlackAdapter, run_slack_adapter};` re-export

## Verification Results

| Check | Result |
|-------|--------|
| `cargo build -p ironhermes-gateway` | PASS |
| `cargo test -p ironhermes-gateway --lib -- slack` | PASS (3/3) |
| `cargo clippy -p ironhermes-gateway --lib -- -D clippy::await_holding_lock` | PASS (clean) |
| INV-34-01 (`inv_34_01_discord_routes_through_handle_with_multimodal`) | GREEN |
| INV-34-02 (`inv_34_02_slack_routes_through_handle_with_multimodal`) | GREEN |
| Wave 2 exit gate (both INV-34-01 + INV-34-02 GREEN) | PASS |

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] UserCallbackFunction is a bare fn pointer type, not a closure type**
- **Found during:** Task 1 — first build attempt after wiring lib.rs (pub mod slack declared)
- **Issue:** Plan body, PATTERNS.md, and RESEARCH.md show `with_push_events` accepting a capturing
  closure. In slack-morphism 2.22.0, `UserCallbackFunction<E, IF, SCHC>` is defined as a bare
  `fn` pointer type (`fn(E, Arc<SlackClient<SCHC>>, SlackClientEventsUserState) -> IF`). Closures
  that capture variables (whitelist, handler, cancel, adapter) cannot be coerced to `fn` pointers
  — the compiler error was E0308 "expected fn pointer, found closure."
- **Fix:** Restructured to use `SlackAdapterState` struct stored via `with_user_state<T>()` on the
  listener environment; standalone `async fn on_push_event(...)` retrieves state from the
  `SlackClientEventsUserState` RwLock parameter via `.read().await`. This is the documented
  slack-morphism pattern for threading state into fn-pointer callbacks.
- **API delta (WARNING 4 closure):** This is the supply-chain deviation documented in the plan's
  pinned-docs warning. The published 2.22.0 API differs from the plan's assumed closure shape.
  The UserCallbackFunction type is verified via direct reading of the crate source in the Cargo
  registry cache. No escalation needed — the fix is mechanical and correct.
- **Files modified:** crates/ironhermes-gateway/src/slack.rs
- **Commit:** 8645caaa

**2. [Rule 3 - Blocking] ErrorHandler is also a bare fn pointer type**
- **Found during:** Task 1 — same build pass as deviation 1
- **Issue:** `with_error_handler` accepts `ErrorHandler<SCHC>` which is
  `fn(BoxError, Arc<SlackClient<SCHC>>, SlackClientEventsUserState) -> HttpStatusCode`.
  The plan showed a closure with `|err, _client, _state|` syntax. Additionally, `http::StatusCode`
  is not directly available — the correct re-export is `HttpStatusCode` from `slack_morphism::prelude::*`.
- **Fix:** Extracted standalone `fn on_socket_mode_error(...)` returning `HttpStatusCode` (which
  is a type alias for `http::StatusCode` exported by the crate's prelude).
- **Files modified:** crates/ironhermes-gateway/src/slack.rs
- **Commit:** 8645caaa

## Security Threat Mitigations Applied

| Threat | Mitigation | Verification |
|--------|------------|--------------|
| T-34-01: Token logging | Tokens never passed to `tracing::*!` macros; startup log shows "xapp-...redacted" | `grep -E 'tracing::.*!.*\b(app_token\|bot_token)\b'` → 0 lines |
| T-34-02: Unauthorized sender | Canonical whitelist — empty = deny-all (D-12, mirrors runner.rs:601-611) | `grep -c "denying all messages (D-12)"` → 1 |
| T-34-04: Socket Mode ACK timeout | tokio::spawn(async move { handle_with_multimodal }) — callback returns Ok(()) immediately | `grep -c "tokio::spawn"` → 3; `grep -c "async move"` → 2 |
| T-34-05: Session key collision | slack_event_to_message_event sets Platform::Slack; SessionKey discriminates by platform | Platform::Slack in MessageEvent |
| T-34-PITFALL-2: Single-token shape | run_slack_adapter requires two &str args (app_token, bot_token) | Compile-time signature |
| T-34-PITFALL-3: Mutex across await | No Mutex in adapter; state access via futures_locks::RwLock (async) | clippy -D await_holding_lock → clean |
| T-34-SUPPLY: Closure signature drift | UserCallbackFunction confirmed as fn pointer via direct crate source read; fix applied | Deviation documented above |

## Known Stubs

None — all 7 PlatformAdapter methods are implemented. `add_reaction` and `send_chat_action` use
the trait default no-ops (intentionally deferred per plan scope — not functionality stubs).

## Threat Flags

None — no new network endpoints, auth paths, or schema changes beyond what the plan's threat
model covers.

## Self-Check

- [x] `crates/ironhermes-gateway/src/slack.rs` exists (full implementation, 346 lines)
- [x] `crates/ironhermes-gateway/src/lib.rs` has `pub mod slack;` and `pub use slack::...`
- [x] Commit efad144d exists (Task 1+2 initial impl)
- [x] Commit 8645caaa exists (fn-pointer fix)
- [x] Commit a8577eed exists (lib.rs wiring)
- [x] INV-34-01 GREEN
- [x] INV-34-02 GREEN (RED → GREEN achieved)
- [x] Wave 2 exit gate: both invariants GREEN
- [x] No STATE.md or ROADMAP.md modifications

## Self-Check: PASSED

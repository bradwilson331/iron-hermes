# Phase 34: Webchat & Multi-Platform Gateway - Pattern Map

**Mapped:** 2026-05-17
**Files analyzed:** 10 new/modified files
**Analogs found:** 10 / 10

---

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|-------------------|------|-----------|----------------|---------------|
| `crates/ironhermes-gateway/src/discord.rs` | adapter | event-driven | `crates/ironhermes-gateway/src/telegram.rs` | role-match |
| `crates/ironhermes-gateway/src/slack.rs` | adapter | event-driven | `crates/ironhermes-gateway/src/telegram.rs` | role-match |
| `crates/ironhermes-gateway/src/runner.rs` | runner | request-response | `crates/ironhermes-gateway/src/runner.rs` (self) | exact (modify) |
| `crates/ironhermes-gateway/Cargo.toml` | config | — | `crates/ironhermes-gateway/Cargo.toml` (self) | exact (modify) |
| `crates/ironhermes-gateway/src/lib.rs` | config | — | `crates/ironhermes-gateway/src/lib.rs` (self) | exact (modify) |
| `crates/ironhermes-gateway/tests/invariants_34.rs` | test | static-grep | `crates/ironhermes-agent/tests/invariants_33.rs` | exact |
| `crates/iron_hermes_ui/src/server/state.rs` | service | request-response | `crates/iron_hermes_ui/src/server/state.rs` (self) | exact (modify) |
| `crates/iron_hermes_ui/tests/session_store_shared_with_gateway.rs` | test | static-grep | `crates/ironhermes-agent/tests/invariants_33.rs` | role-match |
| `crates/ironhermes-agent/tests/invariants_33.rs` | test | static-grep | `crates/ironhermes-agent/tests/invariants_33.rs` (self) | exact (modify) |

---

## Pattern Assignments

### `crates/ironhermes-gateway/src/discord.rs` (adapter, event-driven)

**Analog:** `crates/ironhermes-gateway/src/telegram.rs`

**Imports pattern** (telegram.rs lines 1-9):
```rust
use crate::adapter::PlatformAdapter;
use anyhow::{Context, Result};
use async_trait::async_trait;
use ironhermes_core::{Attachment, MessageEvent, MessageResponse, Platform};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::Path;
```
For `discord.rs`, replace reqwest-specific imports with serenity:
```rust
use crate::adapter::PlatformAdapter;
use anyhow::Result;
use async_trait::async_trait;
use ironhermes_core::{MessageEvent, MessageResponse, Platform};
use serenity::async_trait as serenity_async_trait;
use serenity::model::channel::Message as SerenityMessage;
use serenity::model::gateway::{GatewayIntents, Ready};
use serenity::prelude::{Client, Context as SerenityContext, EventHandler};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
```

**Struct + constructor pattern** (telegram.rs lines 68-81):
```rust
pub struct TelegramAdapter {
    token: String,
    http: Client,
    pub bot_username: Option<String>,
}

impl TelegramAdapter {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            http: Client::new(),
            bot_username: None,
        }
    }
```
For `discord.rs`, the `DiscordAdapter` holds a serenity `Context` clone (available inside EventHandler callbacks) — it does NOT hold the token directly. The pattern splits into two structs:
- `DiscordAdapter { ctx: SerenityContext }` — implements `PlatformAdapter`
- `DiscordEventHandler { handler: Arc<GatewayMessageHandler>, cancel: CancellationToken }` — implements serenity `EventHandler`

**PlatformAdapter impl** (telegram.rs lines 213-298):
```rust
#[async_trait]
impl PlatformAdapter for TelegramAdapter {
    fn platform(&self) -> Platform {
        Platform::Telegram
    }

    async fn send_message(
        &self,
        chat_id: &str,
        content: &str,
        _thread_id: Option<&str>,
    ) -> Result<MessageResponse> {
        // ... api call ...
        Ok(MessageResponse {
            message_id: result.message_id.to_string(),
            chat_id: chat_id.to_string(),
            platform: Platform::Telegram,
        })
    }

    async fn edit_message(&self, chat_id: &str, message_id: &str, content: &str) -> Result<()> { ... }
    async fn edit_message_markdown(&self, chat_id: &str, message_id: &str, content: &str) -> Result<()> { ... }
    async fn delete_message(&self, chat_id: &str, message_id: &str) -> Result<()> { ... }
    async fn add_reaction(&self, chat_id: &str, message_id: &str, emoji: &str) -> Result<()> { ... }
    async fn send_chat_action(&self, chat_id: &str, action: &str) -> Result<()> { ... }

    fn is_running(&self) -> bool {
        false  // Lifecycle managed by GatewayRunner
    }
}
```
All 7 methods must be implemented. `add_reaction` and `send_chat_action` can use the default no-op from `adapter.rs` lines 49-57.

**Message-to-event conversion** (telegram.rs lines 378-431):
```rust
pub fn tg_message_to_event(msg: &TgMessage) -> MessageEvent {
    MessageEvent {
        platform: Platform::Telegram,
        message_id: msg.message_id.to_string(),
        chat_id: msg.chat.id.to_string(),
        sender_id: msg.from.as_ref().map(|u| u.id.to_string()).unwrap_or_default(),
        content: msg.text.clone().or_else(|| msg.caption.clone()).unwrap_or_default(),
        attachments,
        thread_id: None,
        chat_type: match msg.chat.chat_type.as_str() {
            "private" => "dm".to_string(),
            "group" | "supergroup" => "group".to_string(),
            other => other.to_string(),
        },
        chat_name: msg.chat.title.clone(),
        sender_name: msg.from.as_ref().map(|u| u.first_name.clone()),
        replied_to_id: None,
    }
}
```
Mirror as `discord_message_to_event(msg: &SerenityMessage) -> MessageEvent` with `Platform::Discord`.

**serenity EventHandler skeleton** (from RESEARCH.md Pattern 1):
```rust
struct DiscordEventHandler {
    handler: Arc<GatewayMessageHandler>,
    cancel: CancellationToken,
}

#[serenity::async_trait]
impl EventHandler for DiscordEventHandler {
    async fn message(&self, ctx: SerenityContext, msg: SerenityMessage) {
        if msg.author.bot { return; }  // skip bot messages
        let adapter = Arc::new(DiscordAdapter { ctx: ctx.clone() });
        let event = discord_message_to_event(&msg);
        let processed = crate::multimodal::ProcessedAttachments::default();
        let _ = self.handler
            .handle_with_multimodal(&event, adapter, self.cancel.child_token(), processed)
            .await;
    }
}
```

**Adapter startup function** (mirrors runner.rs start() Telegram section, lines 403-433):
```rust
pub async fn run_discord_adapter(
    token: &str,
    handler: Arc<GatewayMessageHandler>,
    cancel: CancellationToken,
) -> Result<()> {
    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;
    let mut client = Client::builder(token, intents)
        .event_handler(DiscordEventHandler { handler, cancel: cancel.clone() })
        .await?;
    tokio::select! {
        result = client.start() => result.map_err(|e| anyhow::anyhow!("Discord client: {e}")),
        _ = cancel.cancelled() => {
            client.shard_manager().shutdown_all().await;
            Ok(())
        }
    }
}
```

**TRICKY BIT — Mutex across await:** Never hold `nudge_turns` or `skill_overlays` std::sync::Mutex guard across any `.await`. The `GatewayMessageHandler::run_agent` already implements the `should_fire` extraction pattern correctly (handler.rs lines 1110-1123). Discord adapter itself has no nudge code — it inherits via `handle_with_multimodal`.

**TRICKY BIT — MESSAGE_CONTENT intent:** Must be enabled in both the Discord Developer Portal (Privileged Gateway Intents) AND in the `GatewayIntents` bitmask. Without it, `msg.content` is empty in guilds with 100+ members.

---

### `crates/ironhermes-gateway/src/slack.rs` (adapter, event-driven)

**Analog:** `crates/ironhermes-gateway/src/telegram.rs`

**Imports pattern**:
```rust
use crate::adapter::PlatformAdapter;
use anyhow::Result;
use async_trait::async_trait;
use ironhermes_core::{MessageEvent, MessageResponse, Platform};
use slack_morphism::prelude::*;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
```

**Two-token requirement** (Pitfall 2 from RESEARCH.md):
```rust
pub struct SlackAdapter {
    bot_token: SlackApiToken,  // xoxb-... for sending
    // SerenityContext equivalent: hold a SlackClient handle for API calls
    client: Arc<SlackClient<SlackClientHyperConnector>>,
}
```

**PlatformAdapter impl** — same 7-method contract as TelegramAdapter (telegram.rs lines 213-298). `platform()` returns `Platform::Slack`. `send_message` calls `client.open_session(&bot_token).chat_post_message(...)`. `is_running()` returns `false`.

**Socket Mode startup function** (from RESEARCH.md Pattern 2):
```rust
pub async fn run_slack_adapter(
    app_token: &str,   // xapp-... Socket Mode connection token
    bot_token: &str,   // xoxb-... for sending messages
    handler: Arc<GatewayMessageHandler>,
    cancel: CancellationToken,
) -> Result<()> {
    let client = Arc::new(SlackClient::new(SlackClientHyperConnector::new()?));
    // ... build socket_mode_callbacks, listener_environment, socket_mode_listener ...
    let app_token_value: SlackApiTokenValue = app_token.into();
    let app_token_obj = SlackApiToken::new(app_token_value);
    tokio::select! {
        result = socket_mode_listener.listen_for(&app_token_obj) => result,
        _ = cancel.cancelled() => Ok(()),
    }
}
```

**Message-to-event conversion** — mirror `tg_message_to_event` (telegram.rs:378-431) as `slack_event_to_message_event` converting `SlackMessageEvent` to `MessageEvent` with `Platform::Slack`.

**TRICKY BIT — Socket Mode ACK timing:** slack-morphism handles the 3-second Slack ACK internally. Do NOT block the callback closure with synchronous work. Dispatch to `tokio::spawn` immediately inside the callback, same pattern as the Discord EventHandler.

**TRICKY BIT — Two tokens:** `SLACK_APP_TOKEN` (`xapp-...`) for WebSocket connection; `SLACK_BOT_TOKEN` (`xoxb-...`) for `chat.postMessage` / edit / delete API calls. Config mirrors `gateway.platforms.telegram.token` pattern — add `gateway.platforms.slack.app_token` and `gateway.platforms.slack.bot_token`.

---

### `crates/ironhermes-gateway/src/runner.rs` (runner, request-response) — MODIFY

**Analog:** `crates/ironhermes-gateway/src/runner.rs` lines 403-505 (existing `start()`)

**Current token resolution pattern** (runner.rs lines 421-430):
```rust
let tg_config = self
    .config
    .gateway
    .platforms
    .get("telegram")
    .cloned()
    .unwrap_or_default();

let token = resolve_token(&tg_config.token)
    .context("No Telegram bot token configured. ...")?;
```
Mirror for Discord/Slack with `.get("discord")` / `.get("slack")`.

**JoinSet multi-platform spawn pattern** (runner.rs lines 488-495):
```rust
let mut join_set: JoinSet<()> = JoinSet::new();
let worker_join_set: Arc<TokioMutex<JoinSet<()>>> =
    Arc::new(TokioMutex::new(JoinSet::new()));
```
Add Discord and Slack as optional concurrent tasks in the same `join_set`:
```rust
// Add after Telegram poll loop spawn (lines 498-557):
if let Some(discord_token) = resolve_token(&discord_config.token) {
    let handler_d = handler.clone();
    let cancel_d = self.cancel.clone();
    join_set.spawn(async move {
        if let Err(e) = discord::run_discord_adapter(&discord_token, handler_d, cancel_d).await {
            error!("Discord adapter error: {e}");
        }
    });
}
```

**Graceful shutdown pattern** (runner.rs lines 795-823) — unchanged; existing `tokio::select!` + `self.cancel.cancel()` + JoinSet drain handles Discord/Slack tasks automatically.

**TRICKY BIT — Token logging:** `resolve_token` returns `Option<String>` and is called with the config field. Never log the token string. Mirror the existing pattern exactly — only log bot username/ID after verification, not the token.

---

### `crates/ironhermes-gateway/Cargo.toml` (config) — MODIFY

**Analog:** `crates/ironhermes-gateway/Cargo.toml` (self, lines 1-42)

**Existing dependency style** (lines 9-38):
```toml
[dependencies]
ironhermes-core = { path = "../ironhermes-core" }
# ...
serde = { workspace = true }
anyhow = { workspace = true }
async-trait = { workspace = true }
tokio = { workspace = true }
reqwest = { workspace = true }
tracing = { workspace = true }
tokio-util = { workspace = true }
```

**Additions to append** (gated on `checkpoint:human-verify` per RESEARCH.md):
```toml
# Phase 34: Discord adapter [ASSUMED — verify serenity feature flags against crate source]
serenity = { version = "0.12", default-features = false, features = ["client", "gateway", "http", "model", "cache", "rustls_backend"] }

# Phase 34: Slack Socket Mode adapter [ASSUMED — verify socket-mode feature name against crate source]
slack-morphism = { version = "2.22", features = ["socket-mode"] }
```

**TRICKY BIT — TLS conflict:** `rustls_backend` must match the workspace `reqwest` TLS stack. If the workspace uses `native-tls`, switch to `serenity`'s `native_tls_backend` feature. Verify by checking the workspace-level `reqwest` features in the root `Cargo.toml`.

---

### `crates/ironhermes-gateway/src/lib.rs` (config) — MODIFY

**Analog:** `crates/ironhermes-gateway/src/lib.rs` (self, lines 1-30)

**Existing pub mod pattern** (lines 1-30):
```rust
pub mod adapter;
pub mod backoff;
pub mod handler;
// ...
pub mod telegram;
pub mod user_queue;

pub use adapter::{MessageHandler, PlatformAdapter};
// ...
pub use telegram::{...};
```

**Additions**:
```rust
pub mod discord;   // Phase 34
pub mod slack;     // Phase 34

pub use discord::run_discord_adapter;   // or DiscordAdapter if callers need the type
pub use slack::run_slack_adapter;
```

---

### `crates/ironhermes-gateway/tests/invariants_34.rs` (test, static-grep) — NEW

**Analog:** `crates/ironhermes-agent/tests/invariants_33.rs` (exact pattern)

**include_str! + assert pattern** (invariants_33.rs lines 42-62):
```rust
const APP_RUNTIME_FACTORY_SOURCE: &str = include_str!("../src/app_runtime_factory.rs");

#[test]
fn inv_33_01_register_skill_manage_tool_in_app_runtime_factory() {
    let count = APP_RUNTIME_FACTORY_SOURCE
        .matches("register_skill_manage_tool")
        .count();
    assert!(
        count >= 1,
        "INV-33-01: ... Found {count} occurrences (expected >= 1). See Phase 33 Plan 03."
    );
}
```

**Phase 34 invariants to cover** (D-10/D-11):
```rust
const DISCORD_SOURCE: &str = include_str!("../src/discord.rs");
const SLACK_SOURCE: &str = include_str!("../src/slack.rs");

/// INV-34-01: DiscordAdapter routes through handle_with_multimodal.
#[test]
fn inv_34_01_discord_routes_through_handle_with_multimodal() {
    let count = DISCORD_SOURCE.matches("handle_with_multimodal").count();
    assert!(count >= 1, "INV-34-01: ... Found {count}. See Phase 34 D-10.");
}

/// INV-34-02: SlackAdapter routes through handle_with_multimodal.
#[test]
fn inv_34_02_slack_routes_through_handle_with_multimodal() {
    let count = SLACK_SOURCE.matches("handle_with_multimodal").count();
    assert!(count >= 1, "INV-34-02: ... Found {count}. See Phase 34 D-11.");
}
```

**File location note:** Tests in `crates/ironhermes-gateway/tests/` — `include_str!` paths are relative to the test file, so `"../src/discord.rs"` reaches `crates/ironhermes-gateway/src/discord.rs`.

---

### `crates/iron_hermes_ui/src/server/state.rs` (service, request-response) — MODIFY

**Analog:** `crates/iron_hermes_ui/src/server/state.rs` (self — session unification fix)

**Current ensure_web_session pattern** (state.rs lines 148-167):
```rust
pub fn ensure_web_session(&self, session_id: &str) -> Result<()> {
    let mut store = self.state_store.lock().unwrap();
    if store.get_session(session_id)...?.is_none() {
        store.create_session(
            session_id,
            &Platform::Web.to_string(),  // source string — already correct
            Some(&self.config.model.default),
            None, None, None,
        ).context("failed to create web session")?;
    }
    Ok(())
}
```
The `Platform::Web.to_string()` source is already passed correctly. The fix is to confirm the pre-existing failing test `api_sessions_and_tools_are_backed_by_real_state` passes — investigate what the test actually asserts vs. what `Platform::Web.to_string()` produces, and align them.

**Nudge turns init pattern** (state.rs lines 132-145):
```rust
Ok(Self {
    config: Arc::new(config),
    // ...
    nudge_turns: Arc::new(std::sync::Mutex::new(HashMap::new())),
    // ...
})
```
No changes needed — already live from Plan 32-03.

**Nudge fire site pattern** (state.rs lines 196-227) — canonical reference for all new adapters:
```rust
let should_fire = {
    let mut map = self.nudge_turns.lock().unwrap_or_else(|e| e.into_inner());
    let count = map.entry(session_id.to_string()).or_insert(0);
    *count += 1;
    if *count >= nudge_interval {
        *count = 0;
        true
    } else {
        false
    }
}; // std::sync::Mutex guard dropped here — BEFORE any await/spawn
if should_fire {
    if let Some(ref mgr) = self.memory_manager {
        let mgr_clone = Arc::clone(mgr);
        let client_clone = build_main_client(&self.resolver)?;
        let config_clone = (*self.config).clone();
        tokio::spawn(async move {
            ironhermes_agent::nudge::spawn_nudge_review(
                messages_snapshot, mgr_clone, client_clone, &config_clone,
            ).await;
        });
    }
}
```

---

### `crates/iron_hermes_ui/tests/session_store_shared_with_gateway.rs` (test, static-grep) — NEW

**Analog:** `crates/ironhermes-agent/tests/invariants_33.rs` (include_str! pattern)

**Pattern** (invariants_33.rs lines 42-62):
```rust
const STATE_SOURCE: &str = include_str!("../src/server/state.rs");

/// Verify Platform::Web is used as the session source string in ensure_web_session.
#[test]
fn web_session_keyed_by_platform_web() {
    let count = STATE_SOURCE.matches("Platform::Web").count();
    assert!(count >= 1, "state.rs must reference Platform::Web for session keying. Found {count}.");
}
```
Path from `crates/iron_hermes_ui/tests/` to `crates/iron_hermes_ui/src/server/state.rs` is `"../src/server/state.rs"`.

---

### `crates/ironhermes-agent/tests/invariants_33.rs` (test, static-grep) — MODIFY

**Analog:** `crates/ironhermes-agent/tests/invariants_33.rs` (self — append INV-33-07)

**Existing pattern to append after INV-33-06** (lines 122-135):
```rust
const TOOLSET_CMD_SOURCE: &str = include_str!("../../ironhermes-cli/src/toolset_cmd.rs");

#[test]
fn inv_33_06_learning_in_known_toolsets() { ... }
```

**New constant + test to append**:
```rust
const WEB_STATE_SOURCE: &str =
    include_str!("../../iron_hermes_ui/src/server/state.rs");

/// INV-33-07: AppState::init calls build_app_runtime_bundle, confirming
/// skill_manage is registered for web turns via the shared runtime bundle.
#[test]
fn inv_33_07_appstate_calls_build_app_runtime_bundle() {
    let count = WEB_STATE_SOURCE.matches("build_app_runtime_bundle").count();
    assert!(
        count >= 1,
        "INV-33-07: iron_hermes_ui/src/server/state.rs must call \
         build_app_runtime_bundle so skill_manage (registered in \
         app_runtime_factory.rs) is available for web turns. Found {count}. \
         See Phase 33 Plan 03 / Phase 34 D-05."
    );
}
```

**Path verification required:** The relative path `"../../iron_hermes_ui/src/server/state.rs"` from `crates/ironhermes-agent/tests/` reaches `crates/iron_hermes_ui/src/server/state.rs`. Verify this compiles (`include_str!` is resolved at compile time — a wrong path is a compile error, not a test failure).

---

## Shared Patterns

### Nudge Fire Pattern (T-32-07 mitigation — Mutex guard NOT held across await)

**Source:** `crates/ironhermes-gateway/src/handler.rs` lines 1110-1141  
**Also verified in:** `crates/iron_hermes_ui/src/server/state.rs` lines 200-226  
**Apply to:** Any new code in `discord.rs` or `slack.rs` that touches `nudge_turns` (note: adapters do NOT touch nudge_turns directly — that lives in GatewayMessageHandler.run_agent, which they inherit)

Critical pattern: extract bool before any `.await`:
```rust
let should_fire = {
    let mut map = self.nudge_turns.lock().unwrap_or_else(|e| e.into_inner());
    // ... mutate and extract bool ...
}; // guard dropped here
// Only AFTER guard drop:
if should_fire {
    tokio::spawn(async move { ... });
}
```

### PlatformAdapter Trait (7 required methods)

**Source:** `crates/ironhermes-gateway/src/adapter.rs` lines 22-60  
**Apply to:** `discord.rs` and `slack.rs`

All 7 methods: `platform()`, `send_message()`, `edit_message()`, `edit_message_markdown()`, `delete_message()`, `add_reaction()` (default no-op OK), `send_chat_action()` (default no-op OK), `is_running()` (always `false`).

### SessionKey Platform Discriminant

**Source:** `crates/ironhermes-gateway/src/session.rs` lines 10-37  
**Apply to:** `discord.rs`, `slack.rs`

Always use `SessionKey::new(Platform::Discord, event.chat_id)` and `SessionKey::new(Platform::Slack, event.chat_id)`. Never use bare `chat_id` string as HashMap key for cross-platform state.

### CancellationToken + tokio::select! Shutdown

**Source:** `crates/ironhermes-gateway/src/runner.rs` lines 795-823  
**Apply to:** `run_discord_adapter()`, `run_slack_adapter()`

Wrap every blocking async loop in:
```rust
tokio::select! {
    result = platform_client.start() => result.map_err(|e| anyhow::anyhow!(...)),
    _ = cancel.cancelled() => {
        // platform-specific cleanup (e.g. shard_manager.shutdown_all())
        Ok(())
    }
}
```

### Static-Grep Invariant Test Structure

**Source:** `crates/ironhermes-agent/tests/invariants_33.rs` lines 42-135  
**Apply to:** `invariants_34.rs`, `session_store_shared_with_gateway.rs`, INV-33-07 addition

Pattern: `const SOURCE: &str = include_str!("relative/path.rs");` + `#[test] fn name() { let count = SOURCE.matches("keyword").count(); assert!(count >= N, "...Found {count}..."); }`

### Token Resolution (never log the token)

**Source:** `crates/ironhermes-gateway/src/runner.rs` lines 421-430 + `resolve_token` function (line 962+)  
**Apply to:** runner.rs additions for Discord/Slack platform config lookup

```rust
let discord_config = self.config.gateway.platforms.get("discord").cloned().unwrap_or_default();
let discord_token = resolve_token(&discord_config.token);
// Log bot_id/username ONLY — never the token string
```

---

## No Analog Found

None — all files have a close analog in the codebase.

---

## Tricky Bits Summary (for planner callouts)

| File | Tricky Bit | Mitigation |
|------|-----------|------------|
| `discord.rs` | `msg.content` empty without MESSAGE_CONTENT privileged intent | Require intent in both `GatewayIntents` bitmask AND Discord portal settings |
| `discord.rs` | `client.start()` blocks forever | Wrap with `tokio::select!` + `cancel.cancelled()`; call `shard_manager().shutdown_all().await` in cancel arm |
| `discord.rs` | Edit rate limit (5 edits/5s) | Use existing `with_rate_limit_retry` from handler.rs lines 34-59 |
| `slack.rs` | Two tokens required | `SLACK_APP_TOKEN` (xapp-...) for socket; `SLACK_BOT_TOKEN` (xoxb-...) for API |
| `slack.rs` | Socket Mode ACK must be sent within 3s | Do not block callback; dispatch to `tokio::spawn` immediately |
| `Cargo.toml` | TLS backend conflict | `serenity`'s `rustls_backend` must match workspace `reqwest` TLS; verify root Cargo.toml |
| `Cargo.toml` | Package legitimacy | Both `serenity` and `slack-morphism` are [ASSUMED] — planner must insert `checkpoint:human-verify` before adding to Cargo.toml |
| `invariants_33.rs` | include_str! relative path | `"../../iron_hermes_ui/src/server/state.rs"` from `crates/ironhermes-agent/tests/` — verify compiles |
| `runner.rs` | JoinSet task optionality | Discord/Slack adapters are opt-in (config presence check); missing config = skip, not error |
| `state.rs` | Pre-existing failing test | `api_sessions_and_tools_are_backed_by_real_state` — investigate Platform::Web.to_string() alignment before declaring D-07 done |

---

## Metadata

**Analog search scope:** `crates/ironhermes-gateway/src/`, `crates/iron_hermes_ui/src/server/`, `crates/ironhermes-agent/tests/`
**Files scanned:** 11 source files read directly; 4 grep passes
**Pattern extraction date:** 2026-05-17

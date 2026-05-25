# Phase 34: Webchat & Multi-Platform Gateway — Research

**Researched:** 2026-05-17
**Domain:** Rust async platform adapters (Discord/Slack), SQLite SessionStore migration, Learning Loop parity verification
**Confidence:** HIGH

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**Phase 32 — Web UI nudge wiring (Plan 32-03)**
- D-01: Nudge turn counter lives on `AppState` as `nudge_turns: Arc<Mutex<HashMap<String, u32>>>`
- D-02: Fire site is inside `run_web_turn`, after `agent.run()` returns `Ok`
- D-03: `tokio::spawn` fire-and-forget; `run_web_turn` must not block on nudge completion
- D-04: Plan 32-03 file location: `.planning/phases/32-periodic-nudge-memory-curation/32-03-PLAN.md`

**Phase 33 — Web path invariant test (Plan 33-03 update)**
- D-05: Add `INV-33-07` to existing `invariants_33.rs` in Plan 33-03 Task 2 — verifies `AppState::new` calls `build_app_runtime_bundle`
- D-06: No changes to `prompt_builder.rs` skill-creation trigger guidance — existing guidance is platform-agnostic; verify during Phase 34 research

**Session unification**
- D-07: Web chat sessions use the same SQLite-backed `SessionStore` as gateway sessions, keyed by `Platform::Web` + `session_id`
- D-08: `AppState` currently has its own `state_store` field — Phase 34 migrates this to share the singleton `SessionStore`
- D-09: No cross-surface session merging in this phase

**Multi-platform adapters**
- D-10: `DiscordAdapter` implements `PlatformAdapter` trait; routes through `GatewayHandler.handle_with_multimodal`
- D-11: `SlackAdapter` implements `PlatformAdapter` trait; same routing
- D-12: Learning Loop coverage is structural — `handle_with_multimodal` inherits nudge + skill-create automatically
- D-13: No E2E UAT per platform — integration tests confirming routing are sufficient

**Phase ordering**
- D-14: Phase 34 depends on Phase 32 and Phase 33 completing first (sequential)
- D-15: Phases 32 → 33 → 34 form the complete Learning Loop trilogy

### Claude's Discretion
- Which Discord/Slack API version and SDK library to use
- Exact authentication/bot setup for Discord and Slack
- Whether `SlackAdapter` uses Socket Mode (WebSocket) or Events API (HTTP)
- How `SessionStore` migration affects existing in-memory web state
- How `SessionKey::new(Platform::Web, session_id)` interacts with current web session IDs

### Deferred Ideas (OUT OF SCOPE)
- Cross-surface session sharing (Telegram resumed in web UI with same session_id)
- WhatsApp or other platform adapters beyond Discord/Slack
- E2E UAT per platform (send 10 Discord messages, verify nudge fires)
- `Platform::Web` variant in skill-creation trigger guidance (prompt_builder.rs) — verify needed but defer if platform-agnostic text is sufficient
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| LEARN-01 | Periodic nudge fires across all agent surfaces | Verified: Plan 32-03 landed and is complete. Web UI nudge is live. Phase 34 only needs to verify parity is complete — no re-implementation. |
| LEARN-02 | Memory persistence judgment during nudge | Covered by Plan 32-01 (nudge prompt + MemoryManager wiring). All surfaces inherit. |
| LEARN-03 | Autonomous skill creation triggers | Covered by Plan 33-02 (SkillManageTool). Discord/Slack inherit via handle_with_multimodal. |
| LEARN-04 | SKILL.md auto-creation format | Covered by Plan 33-01/02. No per-surface code needed. |
| LEARN-05 | skill_manage tool with 6 actions | Covered by Plan 33-02/03. Wired in app_runtime_bundle for web; gateway inherits for Discord/Slack. |
</phase_requirements>

---

## Summary

Phase 34 is the final leg of the Learning Loop trilogy. Phases 32 and 33 have **fully shipped**: Plan 32-03 (web UI nudge wiring) is committed and verified; Plans 33-01, 33-02, and 33-03 (SkillManageTool + learning toolset wiring) are committed and verified. The `nudge_turns` field, the `spawn_nudge_review` fire site, and `register_skill_manage_tool` are all live on `develop`.

Phase 34 has three concrete deliverables: (1) **Parity verification** — confirm that LEARN-01 through LEARN-05 are complete across all surfaces and add INV-33-07 locking the `AppState` → `build_app_runtime_bundle` call chain; (2) **Unified SessionStore migration** — migrate `AppState.state_store` (currently a bare `Arc<Mutex<StateStore>>`) to share the gateway's SQLite-backed `SessionStore` (write-through cache) keyed by `Platform::Web + session_id`; (3) **Discord and Slack adapters** — implement `DiscordAdapter` and `SlackAdapter` in `crates/ironhermes-gateway/src/`, each registering alongside `TelegramAdapter` in a new multi-platform gateway startup path. Discord/Slack sessions inherit nudge and skill-create automatically because every message routes through `GatewayHandler.handle_with_multimodal`.

**Primary recommendation:** Use `serenity 0.12` for Discord (polling-compatible, HTTP-only mode available, async-first, same reqwest stack as the gateway) and `slack-morphism 2.22` for Slack Socket Mode (WebSocket-based, no public HTTP endpoint needed, tokio-native). Both adapters are thin wrappers — the heavy lifting is already done by `GatewayMessageHandler`.

---

## Dependency Check: Phase 32 Plan 32-03 and Phase 33 INV-33-07

### Phase 32 Plan 32-03 — Status: COMPLETE [VERIFIED: codebase grep + SUMMARY.md]

The plan file exists at `.planning/phases/32-periodic-nudge-memory-curation/32-03-PLAN.md`.
The 32-03-SUMMARY.md confirms:
- Commits `4941df33` (nudge_turns field + init) and `6b4520d1` (run_web_turn fire site) are in git log.
- `AppState.nudge_turns: Arc<std::sync::Mutex<HashMap<String, u32>>>` is live in `crates/iron_hermes_ui/src/server/state.rs` (confirmed by direct file read).
- `ironhermes_agent::nudge::spawn_nudge_review` call site is present in `state.rs`.
- Workspace built clean; nudge tests passed (6/6).

**D-01 through D-04 are satisfied.** Phase 34 does NOT re-implement nudge wiring for the web UI path. It only verifies parity is complete via INV-33-07.

### Phase 33 INV-33-07 — Status: DOES NOT EXIST YET [VERIFIED: codebase grep]

`invariants_33.rs` currently contains **6 tests** (INV-33-01 through INV-33-06). INV-33-07 (`AppState::new` calls `build_app_runtime_bundle`) is listed in D-05 as a deliverable of Phase 34 that references Plan 33-03 Task 2. However, Plan 33-03 as written only covers the learning toolset wiring (6 invariants) and does not include INV-33-07. Phase 34 must add INV-33-07 to `invariants_33.rs`.

**Verified check**: `state.rs` line 111 calls `build_app_runtime_bundle(AppRuntimeFactoryInput { ... })` [VERIFIED: direct file read]. The invariant can be written as a static-grep test against `iron_hermes_ui/src/server/state.rs` asserting `build_app_runtime_bundle` appears.

**What INV-33-07 should look like** (following the invariants_33.rs include_str! pattern):

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

Note: the include_str! path `../../iron_hermes_ui/src/server/state.rs` reaches the sibling crate from `crates/ironhermes-agent/tests/`. Verify this relative path compiles before committing.

### D-06 Verification: prompt_builder.rs platform-agnostic text

`skill_creation_guidance` is referenced in `prompt_builder.rs` (confirmed by INV-33-02 passing). The existing guidance text is platform-agnostic — it triggers on task completion heuristics, not platform identity. **No changes to `prompt_builder.rs` are needed for Phase 34.** Discord and Slack messages inherit the same skill-creation guidance block automatically.

---

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Discord message ingestion | Gateway crate (new DiscordAdapter) | — | Polls Discord Gateway events via serenity; converts to MessageEvent; passes to GatewayMessageHandler |
| Slack message ingestion | Gateway crate (new SlackAdapter) | — | Receives Socket Mode WebSocket events; converts to MessageEvent; passes to GatewayMessageHandler |
| Learning Loop (nudge + skill-create) | Gateway handler (GatewayMessageHandler) | — | handle_with_multimodal already wires nudge_turns + spawn_nudge_review; no per-adapter code needed |
| Web UI session persistence | iron_hermes_ui AppState | ironhermes-gateway SessionStore | Migrate AppState.state_store to share gateway SessionStore keyed Platform::Web |
| Session key stability across WS reconnects | iron_hermes_ui api.rs | — | session_key format "agent:main:web:dm:{uuid}" is created once and passed from client; stable across reconnects |
| INV-33-07 invariant | ironhermes-agent tests | iron_hermes_ui state.rs | Static-grep test in invariants_33.rs verifying web path wires build_app_runtime_bundle |
| Bot token storage | config.yaml (gateway.platforms) | env vars | Mirrors existing TELEGRAM_BOT_TOKEN pattern; DISCORD_BOT_TOKEN / SLACK_BOT_TOKEN |

---

## Standard Stack

### Core

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `serenity` | 0.12.5 | Discord bot API + gateway events | De-facto standard Rust Discord library; reqwest-based HTTP (matches workspace); async-first; includes full gateway event handling |
| `slack-morphism` | 2.22.0 | Slack Socket Mode + Web API | Only production-quality Rust Slack library with Socket Mode support; tokio-native; no public HTTP endpoint needed |

[VERIFIED: cargo registry — `cargo search serenity` returns `serenity = "0.12.5"`, `cargo search slack-morphism` returns `slack-morphism = "2.22.0"`]

### Why serenity over alternatives

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `serenity 0.12` | `twilight-http 0.17` | twilight is a lower-level modular ecosystem; requires manually combining twilight-http + twilight-gateway + twilight-cache; more code for same result; use twilight when you need custom sharding. serenity is the right choice for this use case. |
| `serenity 0.12` | `poise 0.6` | poise is a command-framework on top of serenity; adds slash-command abstractions we don't need; overkill for a simple message relay adapter |
| `slack-morphism 2.22` | Events API (HTTP) | Socket Mode (WebSocket) requires no public HTTPS endpoint; simpler for local/self-hosted deployment matching the Telegram polling model. Events API requires a publicly accessible server with TLS. |

### Installation (gateway Cargo.toml)

```toml
[dependencies]
# Phase 34: Discord adapter
serenity = { version = "0.12", default-features = false, features = ["client", "gateway", "http", "model", "cache", "rustls_backend"] }

# Phase 34: Slack Socket Mode adapter
slack-morphism = { version = "2.22", features = ["socket-mode"] }
```

**Note on serenity features**: Use `rustls_backend` (not `native_tls_backend`) to match the workspace's `reqwest` TLS stack. Disable `unstable_discord_api` to avoid breaking changes. The `client`, `gateway`, `http`, and `model` features are the minimum needed for message reception and response.

**Note on slack-morphism**: The `socket-mode` feature enables the WebSocket connection. The crate bundles a hyper/tokio HTTP client. Check whether workspace already has `hyper`; if not, slack-morphism brings its own.

---

## Package Legitimacy Audit

> slopcheck was unavailable at research time. Manual verification performed via `cargo search` (official Cargo registry).

| Package | Registry | Age | Downloads (approx) | Source Repo | slopcheck | Disposition |
|---------|----------|-----|--------------------|-------------|-----------|-------------|
| `serenity` 0.12.5 | crates.io | ~7 years | Very high (top Rust Discord library) | github.com/serenity-rs/serenity | N/A (slopcheck unavailable) | Approved — well-known, widely cited; official repo is the canonical Discord Rust SDK |
| `slack-morphism` 2.22.0 | crates.io | ~4 years | Moderate (niche ecosystem) | github.com/slack-rs/slack-morphism-rust | N/A | Approved — only production Slack SDK with Socket Mode for Rust |

All packages are [ASSUMED] per the package name provenance rule (discovered via cargo search, not confirmed via official Discord/Slack SDK documentation pages). Planner must insert a `checkpoint:human-verify` before adding these to `Cargo.toml`.

**Packages removed due to slopcheck [SLOP] verdict:** none (slopcheck unavailable)
**Packages flagged as suspicious [SUS]:** none identified

*slopcheck was unavailable at research time; both packages are tagged [ASSUMED]. Planner must gate each Cargo.toml addition behind a `checkpoint:human-verify` task.*

---

## Architecture Patterns

### System Architecture Diagram

```
Discord Gateway (WebSocket)
    ↓
DiscordAdapter::event_loop()
    ↓ converts TgUpdate-equivalent → MessageEvent
GatewayMessageHandler::handle_with_multimodal()
    ├── slash command? → handle_slash_command()
    └── agent turn → run_agent()
         ├── AgentLoop.run(messages)
         │    └── [tools: skill_manage, memory, delegate_task, ...]
         ├── persist messages → SessionStore (Platform::Discord)
         └── nudge_turns counter → tokio::spawn(spawn_nudge_review)

Slack Socket Mode (WebSocket)
    ↓
SlackAdapter::event_loop()
    ↓ converts SlackEventMessage → MessageEvent
GatewayMessageHandler::handle_with_multimodal()
    [same path as Discord above]

Web Browser (WebSocket)
    ↓
iron_hermes_ui ws.rs → AppState::run_web_turn()
    ├── AgentLoop.run(messages)
    ├── persist messages → SessionStore (Platform::Web)  [Phase 34 migration]
    └── nudge_turns counter → tokio::spawn(spawn_nudge_review)  [Phase 32-03, done]
```

### Recommended Project Structure (new files)

```
crates/ironhermes-gateway/src/
├── adapter.rs           # PlatformAdapter + MessageHandler traits (unchanged)
├── discord.rs           # DiscordAdapter (NEW — Phase 34)
├── slack.rs             # SlackAdapter (NEW — Phase 34)
├── telegram.rs          # TelegramAdapter (reference, unchanged)
├── handler.rs           # GatewayMessageHandler (unchanged)
├── runner.rs            # GatewayRunner (extend start() for Discord/Slack)
└── lib.rs               # pub use discord::DiscordAdapter; pub use slack::SlackAdapter;
```

### Pattern 1: DiscordAdapter Implementation

**What:** Implement `PlatformAdapter` for Discord. The adapter wraps a serenity `Context` handle and converts serenity `Message` events to `MessageEvent`, then calls `GatewayMessageHandler::handle_with_multimodal`.

**Key serenity pattern** [CITED: serenity docs.rs + GitHub examples]:

```rust
// Source: serenity 0.12 EventHandler trait
use serenity::async_trait;
use serenity::model::channel::Message;
use serenity::model::gateway::Ready;
use serenity::prelude::*;

struct DiscordHandler {
    handler: Arc<GatewayMessageHandler>,
    cancel: CancellationToken,
}

#[async_trait]
impl EventHandler for DiscordHandler {
    async fn message(&self, ctx: Context, msg: Message) {
        // Skip bot messages
        if msg.author.bot { return; }

        let adapter = Arc::new(DiscordAdapter { ctx: ctx.clone() });
        let event = discord_message_to_event(&msg);
        let processed = ProcessedAttachments::default();
        let _ = self.handler
            .handle_with_multimodal(&event, adapter, self.cancel.child_token(), processed)
            .await;
    }
}

// Bot startup (runs in GatewayRunner::start_discord() or equivalent)
pub async fn run_discord_adapter(token: &str, handler: Arc<GatewayMessageHandler>, cancel: CancellationToken) -> Result<()> {
    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;  // Requires "Message Content Intent" in Discord portal

    let mut client = Client::builder(token, intents)
        .event_handler(DiscordHandler { handler, cancel })
        .await?;

    tokio::select! {
        result = client.start() => result.map_err(|e| anyhow::anyhow!("Discord client error: {e}")),
        _ = cancel.cancelled() => Ok(()),
    }
}
```

**DiscordAdapter send_message** uses serenity's `ChannelId::say()` or `create_message()`.

**discord_message_to_event** converts `serenity::model::channel::Message` to `ironhermes_core::MessageEvent` with `Platform::Discord`.

### Pattern 2: SlackAdapter Implementation (Socket Mode)

**What:** Implement `PlatformAdapter` for Slack using Socket Mode (WebSocket, no public endpoint). The adapter receives Slack events and converts `SlackMessageEvent` to `MessageEvent`.

**slack-morphism Socket Mode pattern** [CITED: slack-morphism 2.22.0 docs]:

```rust
// Source: slack-morphism 2.22 Socket Mode
use slack_morphism::prelude::*;

pub async fn run_slack_adapter(
    app_token: &str,   // xapp-... Socket Mode app-level token
    bot_token: &str,   // xoxb-... bot token for sending messages
    handler: Arc<GatewayMessageHandler>,
    cancel: CancellationToken,
) -> Result<()> {
    let client = Arc::new(SlackClient::new(
        SlackClientHyperConnector::new()?
    ));
    let socket_mode_callbacks = SlackSocketModeListenerCallbacks::new()
        .with_push_events(|event, _client_socket, _state| async move {
            // event: SlackPushEventCallback
            // Extract message, convert to MessageEvent, call handler
            Ok(())
        });

    let listener_environment = Arc::new(
        SlackClientEventsListenerEnvironment::new(client)
            .with_error_handler(|err, _client_socket, _state| {
                tracing::warn!("Slack socket mode error: {err}");
                false  // don't reconnect on error (let cancel handle it)
            })
    );

    let socket_mode_listener = SlackClientSocketModeListener::new(
        &SlackClientSocketModeConfig::new(),
        listener_environment,
        socket_mode_callbacks,
    );

    let app_token_value: SlackApiTokenValue = app_token.into();
    let app_token = SlackApiToken::new(app_token_value);

    tokio::select! {
        result = socket_mode_listener.listen_for(&app_token) => result,
        _ = cancel.cancelled() => Ok(()),
    }
}
```

**Two tokens required for Slack:**
1. `SLACK_APP_TOKEN` (`xapp-...`) — Socket Mode app-level token; granted via Slack app settings
2. `SLACK_BOT_TOKEN` (`xoxb-...`) — bot token for sending messages via `chat.postMessage`

### Pattern 3: PlatformAdapter trait methods for Discord/Slack

Both adapters must implement all 8 methods. Default no-ops are provided for `add_reaction` and `send_chat_action`. The non-trivial methods:

| Method | Discord impl | Slack impl |
|--------|-------------|------------|
| `send_message` | `ChannelId::say(&ctx.http, content)` | `SlackClientSession::chat_post_message(channel, text)` |
| `edit_message` | `MessageId::edit(&ctx.http, content)` | `chat.update` API call |
| `edit_message_markdown` | Same as edit_message (Discord renders Markdown natively) | Same as edit_message |
| `delete_message` | `MessageId::delete(&ctx.http)` | `chat.delete` API call |
| `is_running` | always `false` (lifecycle managed by GatewayRunner) | always `false` |

**Streaming edits pattern**: For Discord, edit_message is called multiple times during streaming (same as Telegram). Discord has a 5 edit/5s rate limit; StreamConsumer batching (already present in gateway) mitigates this.

### Pattern 4: SessionStore migration for web UI

**Current state** [VERIFIED: direct codebase read]:
- `AppState.state_store: Arc<std::sync::Mutex<StateStore>>` — a raw SQLite StateStore, NOT the gateway's `SessionStore` write-through cache
- `AppState::ensure_web_session` calls `store.create_session(session_id, "web", ...)` directly on the underlying `StateStore`
- The gateway's `SessionStore` struct in `ironhermes-gateway/src/session.rs` wraps `Arc<Mutex<StateStore>> + HashMap<String, GatewaySession>` as a write-through cache

**D-07/D-08 interpretation**: The CONTEXT.md says "migrate AppState.state_store to share the singleton SessionStore used by the gateway." However, looking at the code:
- The gateway's `SessionStore` type is in `ironhermes-gateway`, not `ironhermes-core`
- The web UI (`iron_hermes_ui`) already uses `ironhermes-state::StateStore` directly
- The gateway's `SessionStore` caches `GatewaySession` objects (with `messages: Vec<ChatMessage>`) but the web UI already has its own message-loading path via `StateStore::get_messages`

**Recommended interpretation for planning**: The unification means web sessions must be created in the underlying `StateStore` using `Platform::Web` as the source string (which is already the case — `ensure_web_session` passes `&Platform::Web.to_string()` as the source). What Phase 34 adds is ensuring that:
1. `SessionKey::new(Platform::Web, session_id)` is the canonical key for nudge_turns (already done in Plan 32-03 — the key IS the session_id string)
2. The pre-existing `iron_hermes_ui` test failure `api_sessions_and_tools_are_backed_by_real_state` is investigated — it asserts `Platform::Web.to_string()` is used in the session query but currently fails; Phase 34 may need to fix this
3. For Discord/Slack, the gateway's `SessionStore` is used directly (existing pattern)

**Web session ID stability across WS reconnects** [VERIFIED: direct read of api.rs]:
- Session IDs are formatted as `"agent:main:web:dm:{uuid}"` and created once via `POST /api/sessions/create`
- The UUID is generated at session creation time; the client holds it and passes it on every WebSocket message
- This session key is stable across reconnects — reconnect = same session_id passed to `run_web_turn`

### Anti-Patterns to Avoid

- **Holding `Mutex` guard across `.await`**: Discord/Slack adapters must never hold a `std::sync::Mutex` guard across an async boundary — same constraint as `nudge_turns` in the gateway (T-32-07 pattern)
- **Implementing Learning Loop logic per-adapter**: Do NOT add nudge or skill-create code inside `DiscordAdapter` or `SlackAdapter`. It all lives in `GatewayMessageHandler::run_agent` and is inherited structurally.
- **Sharing a single `GatewayMessageHandler` across all platforms without platform discrimination**: The nudge_turns HashMap key is `SessionKey { platform, chat_id }`. With multiple platforms sharing the same handler, key collisions are impossible by construction (platform discriminant is part of the key). Confirm this when wiring.
- **Using `reqwest::blocking` inside serenity or slack-morphism async callbacks**: Both libraries are fully async. The gateway already uses tokio; no blocking bridges needed.
- **Embedding Discord/Slack adapters in the web UI process**: These adapters run in `ironhermes-gateway`, which IS a separate binary entry point (`run_gateway` in `crates/ironhermes-cli/src/main.rs`). The embedded-agent pattern (project memory) refers to the web UI only. Discord/Slack bots are started via `hermes gateway run` (or equivalent) — they do NOT run in the iron_hermes_ui server.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Discord WebSocket gateway connection | Custom WebSocket + Discord Gateway protocol | `serenity::Client` | Gateway protocol has sharding, heartbeating, session resume, rate limits — 1000s of lines of protocol code |
| Slack Socket Mode WebSocket | Custom WebSocket + Slack RTM protocol | `slack-morphism` Socket Mode listener | Handles reconnect, acknowledgment, deduplication |
| Message ID tracking for edits | Custom message state tracker | serenity's `Message` type directly; slack-morphism's `SlackMessageId` | Already provided by both libraries |
| OAuth/bot token exchange | Custom OAuth flow | Slack/Discord developer portal setup (out-of-band) | Both platforms require portal configuration; no in-app OAuth needed for bot tokens |
| Rate limiting per-channel | Custom token bucket | `PerUserRateLimiter` already in gateway (reuse) | Rate limiter is in `crates/ironhermes-gateway/src/rate_limiter.rs` |

---

## Common Pitfalls

### Pitfall 1: Discord "Message Content Intent" must be explicitly enabled
**What goes wrong:** Discord bot receives messages but `msg.content` is always empty.
**Why it happens:** Since August 2022, Discord requires bots to enable the "Message Content Intent" in the Developer Portal AND in `GatewayIntents`. Without both, message content is redacted for bots in guilds with 100+ members.
**How to avoid:** In the portal: Bot settings → Privileged Gateway Intents → Message Content Intent → enable. In code: include `GatewayIntents::MESSAGE_CONTENT` in the intents bitmask.
**Warning signs:** `msg.content.is_empty()` even when messages are sent with text.

### Pitfall 2: Slack Socket Mode requires TWO tokens, not one
**What goes wrong:** Bot starts but cannot receive events or cannot send messages.
**Why it happens:** Socket Mode requires an app-level token (`xapp-...`) for the WebSocket connection AND a bot token (`xoxb-...`) for API calls (sending messages, editing, etc.).
**How to avoid:** Configure both `SLACK_APP_TOKEN` and `SLACK_BOT_TOKEN` in config.yaml under `gateway.platforms.slack`. App-level token scope: `connections:write`. Bot token scopes: `chat:write`, `channels:history`, `im:history`.
**Warning signs:** `socket_mode_listener.listen_for` fails with auth error, OR `chat.postMessage` fails.

### Pitfall 3: Mutex guard across `.await` in adapter callbacks
**What goes wrong:** Clippy `await_holding_lock` warning; potential deadlock at runtime.
**Why it happens:** If a `std::sync::Mutex` guard from the handler's `nudge_turns` or `skill_overlays` is held across an `.await` point (e.g., inside an adapter callback that also does async I/O).
**How to avoid:** Mirror the exact `should_fire` block pattern from the gateway: extract the bool, drop the guard, then do async work. The existing `GatewayMessageHandler::run_agent` already implements this correctly.
**Warning signs:** `cargo clippy -- -D warnings` raises `clippy::await_holding_lock`.

### Pitfall 4: Session key collision across platforms
**What goes wrong:** Discord user with chat_id "12345" maps to the same nudge counter as Telegram user with chat_id "12345".
**Why it happens:** If the `nudge_turns` HashMap key is just `chat_id: String` without platform discriminant.
**How to avoid:** The existing `SessionKey { platform: Platform, chat_id: String }` includes the platform. New adapters use `SessionKey::new(Platform::Discord, event.chat_id)` and `SessionKey::new(Platform::Slack, event.chat_id)`. Never use chat_id as a bare string key for cross-platform state.

### Pitfall 5: serenity client.start() blocks forever — needs CancellationToken
**What goes wrong:** Gateway can't shut down cleanly when Discord is running.
**Why it happens:** `client.start()` is a blocking async loop. Without a `tokio::select!` that watches the CancellationToken, the process hangs on Ctrl+C.
**How to avoid:** Always wrap with `tokio::select! { _ = client.start() => ..., _ = cancel.cancelled() => Ok(()) }`. Call `client.shard_manager().shutdown_all().await` in the cancel arm to close connections cleanly.

### Pitfall 6: Discord rate limits on edit_message during streaming
**What goes wrong:** StreamConsumer calls `edit_message` every few tokens; Discord returns 429 after 5 edits in 5 seconds.
**Why it happens:** Discord's per-message edit rate limit is stricter than Telegram's.
**How to avoid:** The gateway already has `with_rate_limit_retry` (3 retries, 2/4/6s backoff). Wire this for Discord edits. Also: consider a higher minimum token batch size for Discord (e.g., 50 tokens between edits vs Telegram's default). StreamConsumer already handles this via the existing throttle mechanism.

### Pitfall 7: Web session ID format mismatch
**What goes wrong:** `ensure_web_session` creates a session with key `session_id` but nudge_turns HashMap is keyed differently.
**Why it happens:** The session_id format is `"agent:main:web:dm:{uuid}"` (verified in api.rs). The nudge_turns HashMap key is this full string. If anywhere a bare UUID is used instead of the full key, counter entries won't match session entries.
**How to avoid:** Always pass the full `session_key` (the format from `create_session`) to both `ensure_web_session` and `nudge_turns`. Never strip the prefix.

---

## Code Examples

### discord_message_to_event sketch

```rust
// Source: pattern derived from tg_message_to_event in telegram.rs [ASSUMED structure]
fn discord_message_to_event(msg: &serenity::model::channel::Message) -> MessageEvent {
    MessageEvent {
        platform: Platform::Discord,
        message_id: msg.id.to_string(),
        chat_id: msg.channel_id.to_string(),
        sender_id: msg.author.id.to_string(),
        content: msg.content.clone(),
        attachments: vec![],  // Phase 34: text only; file attachments deferred
        thread_id: msg.thread.as_ref().map(|t| t.id.to_string()),
        chat_type: "dm".to_string(),  // derive from channel type if needed
        chat_name: None,
        sender_name: Some(msg.author.name.clone()),
        replied_to_id: msg.referenced_message.as_ref().map(|m| m.id.to_string()),
    }
}
```

### INV-33-07 in invariants_33.rs

```rust
// Source: pattern from invariants_33.rs include_str! style [VERIFIED: existing file]
const WEB_STATE_SOURCE: &str =
    include_str!("../../iron_hermes_ui/src/server/state.rs");

#[test]
fn inv_33_07_appstate_calls_build_app_runtime_bundle() {
    let count = WEB_STATE_SOURCE.matches("build_app_runtime_bundle").count();
    assert!(
        count >= 1,
        "INV-33-07: iron_hermes_ui/src/server/state.rs must call \
         build_app_runtime_bundle so skill_manage is wired for web turns. \
         Found {count}. See Phase 34 D-05."
    );
}
```

---

## Out-of-Band Setup Steps (User Must Perform)

### Discord Bot Setup

1. Go to https://discord.com/developers/applications → "New Application"
2. In "Bot" section: enable "Message Content Intent" (Privileged Gateway Intents)
3. Copy the bot token → set as `DISCORD_BOT_TOKEN` env var or `gateway.platforms.discord.token` in config.yaml
4. Generate an OAuth2 invite URL with scopes: `bot` + permissions: `Send Messages`, `Read Messages/View Channels`, `Read Message History`
5. Add the bot to your server with this URL
6. No webhook; uses WebSocket long-polling via serenity's gateway

### Slack Bot Setup

1. Go to https://api.slack.com/apps → "Create New App" → "From scratch"
2. In "Socket Mode": enable Socket Mode; generate an app-level token with scope `connections:write` → set as `SLACK_APP_TOKEN` (`xapp-...`)
3. In "OAuth & Permissions": add bot token scopes: `chat:write`, `channels:history`, `im:history`, `im:read`, `im:write`
4. "Install to Workspace" → copy Bot User OAuth Token → set as `SLACK_BOT_TOKEN` (`xoxb-...`)
5. In "Event Subscriptions": enable events; subscribe to `message.channels`, `message.im` bot events (under Socket Mode these still need to be enabled in the Event Subscriptions page, but delivery is via socket, not HTTP)
6. These steps are out-of-band; integration tests can mock the bot token with environment overrides (similar to the GITHUB_API_BASE override pattern from Phase 21.8)

---

## SessionStore Migration Detail

### Current web UI session path [VERIFIED: direct code read]

```
AppState.state_store: Arc<std::sync::Mutex<StateStore>>
  → ensure_web_session() → store.create_session(session_id, "web", ...)
  → build_messages_for_turn() → store.get_messages(session_id)
  → run_web_turn() result → store.add_message(session_id, msg)
```

The `StateStore` type is `ironhermes-state::StateStore` (SQLite WAL mode). This is already backed by SQLite — it is NOT in-memory. The `Platform::Web.to_string()` is passed as the source string.

### What "unified SessionStore" means concretely

The gateway's `SessionStore` (in `ironhermes-gateway/src/session.rs`) is a write-through cache that wraps `StateStore + HashMap<String, GatewaySession>`. The web UI already writes to the same underlying SQLite database (StateStore) if both processes share the same `~/.ironhermes/state.db` path (single-operator deployment).

**The actual D-07 deliverable**: Add a `checkpoint:human-verify` task to confirm whether the web UI's `AppState.state_store` and the gateway's `SessionStore.state` reference the same SQLite file. If they do (single-operator default), the "migration" is simply:
1. Ensure `create_session` in the web UI passes `&Platform::Web.to_string()` as the source (already done)
2. Ensure queries in the web UI filter by platform=web when listing sessions (the pre-existing failing test `api_sessions_and_tools_are_backed_by_real_state` targets exactly this — Phase 34 should fix it)
3. The nudge_turns HashMap key continues to be the full session string (already correct per Plan 32-03)

**What does NOT need to happen**: Replacing `AppState.state_store: Arc<Mutex<StateStore>>` with the gateway's `SessionStore` type is NOT required and would be a large refactor. The gateway's `SessionStore` type includes `HashMap<String, GatewaySession>` with message caching — the web UI handles this differently (messages are loaded from SQLite on every turn, not cached in memory). The D-07/D-08 intent is session tracking parity, not code-level unification of the session store implementation.

---

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | Rust built-in `#[test]` + `#[tokio::test]` |
| Config file | none (workspace Cargo.toml, per-crate Cargo.toml) |
| Quick run command | `cargo test -p ironhermes-gateway --lib` |
| Full suite command | `cargo test --workspace` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| LEARN-01 | Web UI nudge fires at nudge_interval | static-grep invariant (INV-33-07) | `cargo test -p ironhermes-agent --test invariants_33` | Wave 0: add INV-33-07 to invariants_33.rs |
| LEARN-01 | nudge_turns field exists on AppState | static-grep (existing INV via nudge grep) | `grep nudge_turns crates/iron_hermes_ui/src/server/state.rs` | ✅ (Plan 32-03 done) |
| LEARN-03/04/05 | skill_manage wired for web via build_app_runtime_bundle | INV-33-07 | `cargo test -p ironhermes-agent --test invariants_33 inv_33_07` | ❌ Wave 0 |
| D-10/D-11 | DiscordAdapter routes through handle_with_multimodal | integration test (static-grep) | `cargo test -p ironhermes-gateway --test invariants_34` | ❌ Wave 0 |
| D-10/D-11 | SlackAdapter routes through handle_with_multimodal | integration test (static-grep) | `cargo test -p ironhermes-gateway --test invariants_34` | ❌ Wave 0 |
| D-07 | Web sessions keyed by Platform::Web in StateStore | pre-existing failing test (fix it) | `cargo test -p iron_hermes_ui -- api_sessions_and_tools` | ✅ test exists, ❌ failing |
| D-12 | Learning Loop fires for Discord/Slack structurally | static-grep: handle_with_multimodal contains nudge_turns | `grep nudge_turns crates/ironhermes-gateway/src/handler.rs` | ✅ (Plan 32-02 done) |

### Sampling Rate
- Per task commit: `cargo test -p ironhermes-gateway --lib` + `cargo test -p ironhermes-agent --test invariants_33`
- Per wave merge: `cargo build --workspace && cargo test --workspace`
- Phase gate: Full suite green before `/gsd:verify-work`

### Wave 0 Gaps

- [ ] `crates/ironhermes-agent/tests/invariants_33.rs` — add INV-33-07 (one new `#[test]`)
- [ ] `crates/ironhermes-gateway/tests/invariants_34.rs` — cover D-10/D-11 routing invariants for Discord/Slack
- [ ] Serenity + slack-morphism added to `crates/ironhermes-gateway/Cargo.toml`

---

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | yes | Bot token stored in env var / config.yaml (same as TELEGRAM_BOT_TOKEN pattern); never logged |
| V3 Session Management | yes | SessionKey(platform, chat_id) ensures session isolation across platforms |
| V4 Access Control | yes | Existing `whitelist: Vec<i64>` in PlatformGatewayConfig; Discord/Slack adapters must check sender allowlist |
| V5 Input Validation | yes | MessageEvent.content passes through existing gateway prompt injection defenses |
| V6 Cryptography | no | Token storage is plain string in config; TLS to Discord/Slack APIs handled by reqwest/serenity/slack-morphism |

### Known Threat Patterns

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Bot token leakage in logs | Information Disclosure | `tracing::info!` calls must NEVER log the token string; mirror TELEGRAM_BOT_TOKEN pattern in runner.rs |
| Unauthorized sender access | Elevation of Privilege | `whitelist` field in `PlatformGatewayConfig`; Discord/Slack adapters must check sender_id against whitelist before routing to handler |
| Discord message content missing | Tampering | `MESSAGE_CONTENT` privileged intent must be enabled; validate `event.content` is non-empty before dispatching |
| Slack event replay | Spoofing | Socket Mode ACK must be sent within 3 seconds; slack-morphism handles this; do not block the callback thread |
| Session key collision | Spoofing | `SessionKey { platform, chat_id }` — platform discriminant prevents cross-platform collisions |

---

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | `serenity 0.12.5` is the correct crate name and version on crates.io for the main Discord Rust library | Standard Stack | Wrong crate name → build fails; confirmed via `cargo search` but not via official Discord developer documentation |
| A2 | `slack-morphism 2.22.0` supports Socket Mode via `features = ["socket-mode"]` | Standard Stack | Feature name may differ; check Cargo.toml in the crate source |
| A3 | serenity's `rustls_backend` feature is compatible with the workspace's reqwest TLS stack | Standard Stack | TLS conflict at link time; may need `native-tls` or explicit deconfliction |
| A4 | The gateway's `SessionStore` and the web UI's `AppState.state_store` share the same underlying SQLite file path in a single-operator deployment | SessionStore Migration | If paths differ, sessions are fragmented and the "unified session store" intent is not achieved even with the current code |
| A5 | `DiscordAdapter.send_message` can use `ChannelId::say()` from serenity | Code Examples | serenity 0.12 API may use a different method name; planner must verify against serenity 0.12 changelog |
| A6 | Discord/Slack adapters will be started in the same process as the Telegram gateway (sharing GatewayRunner and GatewayMessageHandler) | Architecture | The runner.rs pattern and GatewayRunner suggest a single gateway process; if Discord/Slack require separate processes, the startup wiring changes significantly |

---

## Open Questions

1. **Does GatewayRunner::start() become multi-platform in Phase 34?**
   - What we know: `runner.rs::start()` currently hardcodes Telegram-specific startup (getMe, set_my_commands, TgUpdate polling)
   - What's unclear: Should Phase 34 extend `start()` to optionally also start Discord/Slack, or create separate `start_discord()` / `start_slack()` methods? Given `CONTEXT.md` D-10/D-11 say "register alongside TelegramAdapter," the implication is a single multi-platform runner
   - Recommendation: Add optional `start_discord()` and `start_slack()` methods that are called from `run_gateway()` in main.rs if the corresponding `gateway.platforms.discord` / `gateway.platforms.slack` config sections are present. Use `JoinSet` to run all three concurrently.

2. **Does the pre-existing `api_sessions_and_tools_are_backed_by_real_state` test failure belong to Phase 34?**
   - What we know: The test asserts `Platform::Web.to_string()` is used in session queries. It's been failing since at least Plan 32-01.
   - What's unclear: Is this a scope item for Phase 34 (session unification) or a separate fix?
   - Recommendation: Phase 34's "unified SessionStore" deliverable is the right time to fix this. Include it as a task.

3. **Should DiscordAdapter implement TgSendApi (the cron delivery trait)?**
   - What we know: `TgSendApi` (defined in `ironhermes-cron`) is implemented by `TelegramAdapter` for cron job delivery. It includes `send_voice`, `send_image_file`, `send_video`, `send_document`.
   - What's unclear: Phase 34 CONTEXT.md doesn't mention cron delivery for Discord/Slack.
   - Recommendation: Do NOT implement `TgSendApi` for Discord/Slack in Phase 34. Text-only message delivery is sufficient for Learning Loop parity. Cron delivery expansion is a deferred multi-platform feature.

---

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Cargo / Rust toolchain | Building new crates | ✓ | (workspace) | — |
| `ironhermes-gateway` crate | Discord/Slack adapters | ✓ | workspace | — |
| SQLite / StateStore | Session parity | ✓ | (existing) | — |
| DISCORD_BOT_TOKEN | Discord adapter tests | ✗ | — | Mock via env_override; integration tests stub without live token |
| SLACK_APP_TOKEN + SLACK_BOT_TOKEN | Slack adapter tests | ✗ | — | Mock via env_override; integration tests stub without live tokens |
| Discord Developer Portal access | Bot registration | Manual (user) | — | None — required out-of-band |
| Slack API dashboard access | App registration | Manual (user) | — | None — required out-of-band |

**Missing dependencies with no fallback:**
- Discord Developer Portal access and Slack API dashboard access — user must complete out-of-band setup before live integration tests can run. Static-grep invariant tests (INV-34-*) do NOT require live tokens.

**Missing dependencies with fallback:**
- Live bot tokens — integration tests can use env_override stubs (same pattern as `GITHUB_API_BASE` overrides in Phase 21.8). Structural invariant tests pass without tokens.

---

## Sources

### Primary (HIGH confidence)
- Direct codebase read: `crates/ironhermes-gateway/src/adapter.rs` — PlatformAdapter trait (7 methods confirmed)
- Direct codebase read: `crates/ironhermes-gateway/src/telegram.rs` — TelegramAdapter reference implementation
- Direct codebase read: `crates/ironhermes-gateway/src/handler.rs` — GatewayMessageHandler, handle_with_multimodal, nudge_turns field
- Direct codebase read: `crates/iron_hermes_ui/src/server/state.rs` — AppState struct, nudge_turns, run_web_turn, ensure_web_session
- Direct codebase read: `crates/iron_hermes_ui/src/server/api.rs` — session_key format "agent:main:web:dm:{uuid}"
- Direct codebase read: `crates/ironhermes-gateway/src/session.rs` — SessionKey, GatewaySession, SessionStore
- Direct codebase read: `crates/ironhermes-core/src/types.rs` — Platform enum with Web, Discord, Slack variants
- Direct codebase read: `.planning/phases/32-periodic-nudge-memory-curation/32-03-SUMMARY.md` — Plan 32-03 confirmed complete
- Direct codebase read: `.planning/phases/33-autonomous-skill-creation/33-03-SUMMARY.md` — Plans 33-01/02/03 confirmed complete; INV-33-07 absent
- Direct codebase read: `crates/ironhermes-agent/tests/invariants_33.rs` — 6 tests (INV-33-01..06); INV-33-07 not present

### Secondary (MEDIUM confidence)
- `cargo search serenity` → `serenity = "0.12.5"` (crates.io official registry)
- `cargo search slack-morphism` → `slack-morphism = "2.22.0"` (crates.io official registry)
- WebFetch: docs.rs/serenity/0.12.5 — EventHandler trait, Client::builder, GatewayIntents overview
- WebFetch: docs.rs/slack-morphism/2.22.0 — Socket Mode, SlackApiToken, SlackClientSession
- WebFetch: serenity GitHub examples — EventHandler pattern, tokio::main, intents

### Tertiary (LOW confidence)
- serenity feature flags (`rustls_backend`, `client`, `gateway`, `http`, `model`) — [ASSUMED] based on training knowledge of serenity 0.12; verify against actual Cargo.toml in the crate
- slack-morphism `socket-mode` feature name — [ASSUMED]; verify against crate source

---

## Metadata

**Confidence breakdown:**
- Phase 32/33 dependency check: HIGH — verified from SUMMARY.md + direct code read
- INV-33-07 gap: HIGH — confirmed absent from invariants_33.rs by direct grep
- Standard stack (serenity, slack-morphism): MEDIUM — registry-confirmed; API surface ASSUMED
- SessionStore migration scope: HIGH — interpreted from direct code read; A4 assumption flagged
- Architecture patterns: MEDIUM — derived from TelegramAdapter as reference; some serenity API details ASSUMED

**Research date:** 2026-05-17
**Valid until:** 2026-06-17 (serenity/slack-morphism are stable; Discord/Slack APIs evolve slowly)

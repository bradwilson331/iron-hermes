# Multi-Platform Gateway

IronHermes's gateway can run the same agent loop against **Telegram**, **Discord**, and **Slack** simultaneously. All three adapters route incoming messages through the same `GatewayMessageHandler.handle_with_multimodal` path, so the Learning Loop (nudge cycles, memory curation, skill creation) and tool registry are identical across surfaces.

This guide covers token acquisition, platform-specific setup steps, and the operational contract (silent-skip, whitelist semantics, env var precedence).

For per-key defaults see [`CONFIGURATION.md` → Gateway](CONFIGURATION.md#gateway-gateway). For the underlying architecture (PlatformAdapter trait, `runner.rs` JoinSet shutdown) see [`ARCHITECTURE.md`](ARCHITECTURE.md).

## Operational contract

The gateway spawns one listener task per configured platform inside a shared `JoinSet`, governed by a single `CancellationToken`. A single `Ctrl+C` or `SIGTERM` cancels every listener cleanly.

- **Silent-skip is the default.** A platform listener spawns **only when both its config section is present *and* its token(s) resolve**. Missing config or unset env vars are not an error — the gateway logs a `debug!` line and starts the remaining platforms. Existing Telegram-only deployments need no changes.
- **Tokens never cross-leak.** Each adapter resolves its token via a platform-specific env var. `DISCORD_BOT_TOKEN` and `SLACK_BOT_TOKEN` are read **only** when their config section is present, so `TELEGRAM_BOT_TOKEN` never accidentally activates Discord/Slack.
- **Empty whitelist = deny all.** The canonical safety rule from `runner.rs` applies to every platform. To open the bot to everyone, omit the whitelist field entirely is **not** sufficient — you must either populate it or run a non-gateway entry point.

## Config skeleton

Minimal multi-platform `config.yaml` snippet:

```yaml
gateway:
  platforms:
    telegram:
      enabled: true
      token: null              # or set inline; falls back to TELEGRAM_BOT_TOKEN
      whitelist: [123456789]   # your Telegram user ID
    discord:
      enabled: true
      token: null              # falls back to DISCORD_BOT_TOKEN
      whitelist: [987654321]   # your Discord user ID (snowflake)
    slack:
      enabled: true
      token: null              # bot token xoxb-…, falls back to SLACK_BOT_TOKEN
      app_token: null          # app-level token xapp-…, falls back to SLACK_APP_TOKEN
      whitelist: []            # see Slack caveat below
  context_engine: local_prune
  compression_threshold: 0.85
```

Run `hermes gateway run` and the gateway boots whichever platforms have resolvable tokens.

## Telegram setup

Long-standing path; covered in [`GETTING-STARTED.md`](GETTING-STARTED.md). One bot token, polling-based, no public endpoint required.

```
export TELEGRAM_BOT_TOKEN=123456:ABC-…
```

## Discord setup

Built on [serenity 0.12.5](https://crates.io/crates/serenity). Uses long-polling Discord Gateway WebSocket; no public HTTP endpoint required.

### 1. Create the bot application

1. Open <https://discord.com/developers/applications> and create a new application.
2. In the **Bot** tab, click "Add Bot" and copy the token.
3. Under **Privileged Gateway Intents**, enable **MESSAGE CONTENT INTENT**. The IronHermes adapter declares `GatewayIntents::GUILD_MESSAGES | DIRECT_MESSAGES | MESSAGE_CONTENT` — without the privileged toggle, message bodies arrive empty.
4. Reset the token if you saved it anywhere unencrypted; treat it like a password.

### 2. Invite the bot to your server

In the **OAuth2 → URL Generator** tab, select scopes `bot` + `applications.commands` and permissions `Send Messages` + `Read Message History`. Open the generated URL and authorize the bot in your server.

### 3. Configure and run

```bash
export DISCORD_BOT_TOKEN=YOUR.DISCORD.TOKEN
# or set gateway.platforms.discord.token in config.yaml

hermes gateway run
```

### Whitelist

`gateway.platforms.discord.whitelist` is `Vec<i64>` of Discord user IDs (snowflakes). Empty = deny all. To find your user ID, enable Developer Mode in Discord (Settings → Advanced) and right-click your name → "Copy User ID".

## Slack setup

Built on [slack-morphism 2.22.0](https://crates.io/crates/slack-morphism) with the `axum` feature flag (which transitively pulls `tokio-tungstenite` for Socket Mode WebSocket). Slack uses **Socket Mode** — no public HTTP endpoint required, but you need **two tokens** (Pitfall 2):

- **Bot token** `xoxb-…` — used for Web API calls (`chat.postMessage`, etc.)
- **App-level token** `xapp-…` with the `connections:write` scope — used to establish the Socket Mode WebSocket

Slack will silently skip unless **both** tokens resolve.

### 1. Create the Slack app

1. Open <https://api.slack.com/apps> and click "Create New App" → "From scratch".
2. Under **Socket Mode**, enable Socket Mode. Generate an app-level token with the `connections:write` scope — this is your `xapp-…` token.
3. Under **OAuth & Permissions**, add bot token scopes: `chat:write`, `im:history`, `im:read`, `im:write`, `mpim:history`, `channels:history`, `app_mentions:read`. Install the app to your workspace. The "Bot User OAuth Token" `xoxb-…` is your bot token.
4. Under **Event Subscriptions**, enable events and subscribe to bot events: `message.im`, `message.channels`, `message.mpim`, `app_mention`. (No request URL needed — Socket Mode delivers these over the WebSocket.)

### 2. Configure and run

```bash
export SLACK_APP_TOKEN=xapp-1-…
export SLACK_BOT_TOKEN=xoxb-…
# or set both gateway.platforms.slack.token and .app_token in config.yaml

hermes gateway run
```

The adapter classifies incoming Slack messages by channel-ID convention (`D…` = DM, `C…` = public channel, `G…` = private channel/group DM) and routes through the shared `handle_with_multimodal`.

### Slack whitelist caveat

Slack channel IDs are alphanumeric strings (`C123ABC`, `D456DEF`, `U789GHI`) but the shared `PlatformGatewayConfig.whitelist` is `Vec<i64>` (Telegram-shaped). The adapter `to_string()`-converts each entry at the boundary, so numeric integers you place in the whitelist are compared as strings and will **not** match real Slack IDs.

Until the schema is widened to `Vec<String>`, practical workarounds are:

- Run Slack in a private workspace where deny-all is acceptable (empty whitelist, no inbound messages will pass).
- Patch the conversion site in `crates/ironhermes-gateway/src/slack.rs` to accept a freeform string list from another config key.

A future patch phase will introduce a `Vec<String>` whitelist alongside the `Vec<i64>` one.

### Non-blocking ACK (3-second budget)

Slack's Socket Mode requires acknowledgement within ~3 seconds. The IronHermes adapter builds `MessageEvent` + `ProcessedAttachments` synchronously in the callback, then `tokio::spawn(async move { … })`s the handler call so the callback returns immediately. Long-running agent runs do not block ACK.

## Env var precedence

For each platform, token resolution is:

1. Inline `token` / `app_token` value in `config.yaml` (highest priority).
2. Platform-specific env var:
   - Telegram → `TELEGRAM_BOT_TOKEN`
   - Discord → `DISCORD_BOT_TOKEN`
   - Slack bot → `SLACK_BOT_TOKEN`
   - Slack app-level → `SLACK_APP_TOKEN`
3. No fallback. If neither config nor env provides a token, the platform is silently skipped.

## Verifying the wiring

The Wave 4 invariant tests lock the runner contract:

```bash
cargo test -p ironhermes-gateway --test invariants_34
# inv_34_01_discord_routes_through_handle_with_multimodal
# inv_34_02_slack_routes_through_handle_with_multimodal
# inv_34_03_runner_spawns_discord
# inv_34_04_runner_spawns_slack
```

All four passing means: each adapter routes events through the shared handler, and `GatewayRunner::start()` references both adapter entry points. If any flips red after a future refactor, you have lost multi-platform parity.

## Troubleshooting

- **"Discord adapter skipped (no token resolved)"** — `gateway.platforms.discord` is present but `token` is `null` and `DISCORD_BOT_TOKEN` is unset. Either set the env var or move the token inline.
- **"Slack adapter skipped (missing app_token or bot_token)"** — same root cause but for Slack's two-token shape; check both `xapp-…` and `xoxb-…` are reachable.
- **Discord messages arrive with empty content** — the **MESSAGE CONTENT** privileged intent is off in the Discord developer portal. Toggle it on and restart the gateway.
- **Slack callback delivers but no response sent** — likely whitelist deny-all (`whitelist: []`). Confirm by setting `RUST_LOG=ironhermes_gateway::slack=debug hermes gateway run` and watching for "skipped: not in whitelist".
- **Two listeners log identical events from one chat** — almost always misconfigured cross-platform bridging in the chat tool itself (e.g. a third-party Telegram↔Discord bridge). The IronHermes gateway treats each platform's message as a distinct session.

## Phase history

- **Phase 34** (this work) — added Discord + Slack adapters and the multi-platform runner.
- Wave 1 documented that `slack-morphism` 2.22.0 has **no `socket-mode` feature**; the `axum` feature transitively provides the Socket Mode WebSocket stack. The plan's `socket-mode` reference was a stale outdated value.
- Wave 4 documented that Slack's `UserCallbackFunction` in 2.22.0 is a bare `fn` pointer + user-state — the plan had referenced a closure-shape callback.

See `.planning/phases/34-webchat-and-multi-platform-gateway/34-0[1-5]-SUMMARY.md` for the wave-by-wave decision log.

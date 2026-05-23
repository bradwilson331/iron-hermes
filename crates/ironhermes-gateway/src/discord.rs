use crate::adapter::PlatformAdapter;
use crate::handler::GatewayMessageHandler;
use crate::multimodal::ProcessedAttachments;
use anyhow::Result;
use async_trait::async_trait;
use ironhermes_core::{MessageEvent, MessageResponse, Platform};
use serenity::model::channel::Message as SerenityMessage;
use serenity::model::gateway::{GatewayIntents, Ready};
use serenity::model::id::GuildId;
use serenity::prelude::{Context as SerenityContext, EventHandler};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

// =============================================================================
// DiscordAdapter — PlatformAdapter impl
// =============================================================================

/// Platform adapter for Discord using the serenity HTTP client.
///
/// The `ctx` field holds the serenity Context, which owns the HTTP client
/// used to send, edit, and delete Discord messages. The adapter is created
/// per-message inside `DiscordEventHandler::message` and is not persistent.
pub struct DiscordAdapter {
    ctx: SerenityContext,
}

#[async_trait]
impl PlatformAdapter for DiscordAdapter {
    fn platform(&self) -> Platform {
        Platform::Discord
    }

    async fn send_message(
        &self,
        chat_id: &str,
        content: &str,
        _thread_id: Option<&str>,
    ) -> Result<MessageResponse> {
        let channel_id = serenity::model::id::ChannelId::new(
            chat_id
                .parse::<u64>()
                .map_err(|e| anyhow::anyhow!("invalid Discord channel_id {chat_id}: {e}"))?,
        );
        let sent = channel_id
            .say(&self.ctx.http, content)
            .await
            .map_err(|e| anyhow::anyhow!("Discord send_message failed: {e}"))?;
        Ok(MessageResponse {
            message_id: sent.id.to_string(),
            chat_id: chat_id.to_string(),
            platform: Platform::Discord,
        })
    }

    async fn edit_message(&self, chat_id: &str, message_id: &str, content: &str) -> Result<()> {
        let channel_id = serenity::model::id::ChannelId::new(
            chat_id
                .parse::<u64>()
                .map_err(|e| anyhow::anyhow!("invalid Discord channel_id {chat_id}: {e}"))?,
        );
        let msg_id = serenity::model::id::MessageId::new(
            message_id
                .parse::<u64>()
                .map_err(|e| anyhow::anyhow!("invalid Discord message_id {message_id}: {e}"))?,
        );
        channel_id
            .edit_message(
                &self.ctx.http,
                msg_id,
                serenity::builder::EditMessage::new().content(content),
            )
            .await
            .map_err(|e| anyhow::anyhow!("Discord edit_message failed: {e}"))?;
        Ok(())
    }

    async fn edit_message_markdown(
        &self,
        chat_id: &str,
        message_id: &str,
        content: &str,
    ) -> Result<()> {
        // Discord renders Markdown natively — no separate code path needed.
        self.edit_message(chat_id, message_id, content).await
    }

    async fn delete_message(&self, chat_id: &str, message_id: &str) -> Result<()> {
        let channel_id = serenity::model::id::ChannelId::new(
            chat_id
                .parse::<u64>()
                .map_err(|e| anyhow::anyhow!("invalid Discord channel_id {chat_id}: {e}"))?,
        );
        let msg_id = serenity::model::id::MessageId::new(
            message_id
                .parse::<u64>()
                .map_err(|e| anyhow::anyhow!("invalid Discord message_id {message_id}: {e}"))?,
        );
        channel_id
            .delete_message(&self.ctx.http, msg_id)
            .await
            .map_err(|e| anyhow::anyhow!("Discord delete_message failed: {e}"))?;
        Ok(())
    }

    // add_reaction and send_chat_action use the trait default no-op.
    // Discord reactions can be added in a future phase if needed.

    fn is_running(&self) -> bool {
        // Lifecycle managed by GatewayRunner; adapter itself has no running state.
        false
    }
}

// =============================================================================
// discord_message_to_event — message conversion helper
// =============================================================================

/// Classifies the chat type from the Discord guild_id field.
///
/// Discord DMs have no guild_id (None); all guild channels have Some(guild_id).
/// This helper is extracted for unit-testability.
fn classify_chat_type(guild_id: Option<GuildId>) -> &'static str {
    match guild_id {
        None => "dm",
        Some(_) => "group",
    }
}

/// Convert a serenity Message into a platform-agnostic MessageEvent.
///
/// Mirrors `tg_message_to_event` in telegram.rs (lines 378-431).
/// Phase 34 is text-only; attachment processing is deferred.
pub fn discord_message_to_event(msg: &SerenityMessage) -> MessageEvent {
    MessageEvent {
        platform: Platform::Discord,
        message_id: msg.id.to_string(),
        chat_id: msg.channel_id.to_string(),
        sender_id: msg.author.id.to_string(),
        content: msg.content.clone(),
        attachments: vec![], // Phase 34: text-only; attachment support deferred
        thread_id: msg.thread.as_ref().map(|t| t.id.to_string()),
        chat_type: classify_chat_type(msg.guild_id).to_string(),
        chat_name: None, // Could be derived from channel name in a future phase
        sender_name: Some(msg.author.name.clone()),
        replied_to_id: msg.referenced_message.as_ref().map(|m| m.id.to_string()),
    }
}

// =============================================================================
// DiscordEventHandler — serenity EventHandler impl
// =============================================================================

/// serenity EventHandler that receives incoming Discord messages and routes
/// them through GatewayMessageHandler.handle_with_multimodal (D-10 / D-12).
///
/// Learning Loop (nudge + skill_manage) is inherited structurally — no per-adapter
/// Learning Loop code is needed here.
pub struct DiscordEventHandler {
    handler: Arc<GatewayMessageHandler>,
    /// Discord user IDs (u64 snowflakes) allowed to send messages.
    /// CANONICAL: empty whitelist = deny all (matches runner.rs:601-611 + config.rs:731 D-12).
    whitelist: Vec<u64>,
    cancel: CancellationToken,
}

#[serenity::async_trait]
impl EventHandler for DiscordEventHandler {
    async fn message(&self, ctx: SerenityContext, msg: SerenityMessage) {
        // Skip bot messages to avoid feedback loops.
        if msg.author.bot {
            return;
        }

        // T-34-03: reject empty content — MESSAGE_CONTENT intent missing or denied.
        if msg.content.is_empty() {
            tracing::warn!(
                sender_id = %msg.author.id,
                "Discord MESSAGE_CONTENT intent appears missing or denied — empty content from non-bot author {}",
                msg.author.id
            );
            return;
        }

        // CANONICAL: matches runner.rs:601-611 + config.rs:731 (D-12 empty = deny all)
        if !self.whitelist.is_empty() {
            if !self.whitelist.contains(&msg.author.id.get()) {
                tracing::warn!(sender_id = %msg.author.id, "Sender not in whitelist, ignoring");
                return;
            }
        } else {
            tracing::warn!("Whitelist is empty — denying all messages (D-12)");
            return;
        }

        let adapter = Arc::new(DiscordAdapter { ctx: ctx.clone() });
        let event = discord_message_to_event(&msg);
        let processed = ProcessedAttachments { text_prefix: None, image_data_uri: None };
        if let Err(e) = self
            .handler
            .handle_with_multimodal(&event, adapter, self.cancel.child_token(), processed)
            .await
        {
            tracing::error!("Discord handler error: {e:#}");
        }
    }

    async fn ready(&self, _ctx: SerenityContext, ready: Ready) {
        // Log bot tag after connection — NEVER log the token (T-34-01).
        tracing::info!(bot = %ready.user.tag(), "Discord adapter connected");
    }
}

// =============================================================================
// run_discord_adapter — startup function
// =============================================================================

/// Start the Discord adapter with the given bot token, sender whitelist,
/// message handler, and cancellation token.
///
/// This function blocks until either:
/// - `client.start()` returns (error or normal exit), or
/// - `cancel` is signalled, at which point shards are shut down cleanly.
///
/// The caller (GatewayRunner, Plan 05) spawns this in a JoinSet task.
pub async fn run_discord_adapter(
    token: &str,
    whitelist: Vec<u64>,
    handler: Arc<GatewayMessageHandler>,
    cancel: CancellationToken,
) -> Result<()> {
    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    let mut client = serenity::prelude::Client::builder(token, intents)
        .event_handler(DiscordEventHandler {
            handler,
            whitelist,
            cancel: cancel.clone(),
        })
        .await
        .map_err(|e| anyhow::anyhow!("Discord client build failed: {e}"))?;

    let shard_manager = client.shard_manager.clone();

    tokio::select! {
        result = client.start() => {
            result.map_err(|e| anyhow::anyhow!("Discord client error: {e}"))
        }
        _ = cancel.cancelled() => {
            shard_manager.shutdown_all().await;
            Ok(())
        }
    }
}

// =============================================================================
// Unit tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that classify_chat_type returns "dm" for None and "group" for Some.
    #[test]
    fn discord_chat_type_classification() {
        assert_eq!(classify_chat_type(None), "dm");
        assert_eq!(
            classify_chat_type(Some(GuildId::new(123_456_789))),
            "group"
        );
    }

    /// Smoke test: CancellationToken constructs, child_token works, both drop cleanly.
    /// This gates that the tokio-util CancellationToken import is correct (compile-time).
    #[test]
    fn cancellation_token_check_compiles() {
        let token = CancellationToken::new();
        let child = token.child_token();
        drop(child);
        drop(token);
    }
}

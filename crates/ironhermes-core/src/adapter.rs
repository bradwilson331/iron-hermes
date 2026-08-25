//! The `MessageHandler` / `PlatformAdapter` trait pair — the seam every
//! inbound gateway adapter (Telegram, Discord, Slack, Buzz, and, as of
//! Phase 36.7.1, the webhook adapter in `ironhermes-restgw`) implements.
//!
//! **Why this lives in `ironhermes-core` and not `ironhermes-gateway`
//! (where it originated):** Phase 36.7.1 Plan 01 Task 3 needs
//! `GatewayRunner::start()` (in `ironhermes-gateway`) to construct and spawn
//! `ironhermes_restgw::webhook::WebhookAdapter`, which implements this
//! trait. If the trait stayed defined in `ironhermes-gateway`,
//! `ironhermes-restgw` would have to depend on `ironhermes-gateway` to
//! implement it — and `ironhermes-gateway` depending on
//! `ironhermes-restgw` to construct the adapter would then form a cycle.
//! Moving the trait DEFINITIONS here (both crates already depend on
//! `ironhermes-core`) breaks the cycle without touching any call site:
//! `ironhermes_gateway::adapter` re-exports both names unchanged, so every
//! existing `use ironhermes_gateway::adapter::{MessageHandler,
//! PlatformAdapter};` import across the codebase keeps resolving exactly as
//! before. `MediaSender` and the platform-specific adapter implementations
//! (`BuzzAdapter`, `TelegramAdapter`, ...) stay in `ironhermes-gateway` —
//! only the two traits with no gateway-internal type in their signature
//! moved.

use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::{MessageEvent, MessageResponse, Platform};

/// Handler for incoming messages — connects gateway to the agent.
#[async_trait]
pub trait MessageHandler: Send + Sync {
    /// Process an incoming message. The handler owns the adapter reference
    /// and drives edits/responses directly (enabling streaming).
    async fn handle(
        &self,
        event: &MessageEvent,
        adapter: Arc<dyn PlatformAdapter>,
        cancel: CancellationToken,
    ) -> Result<()>;
}

/// Trait for platform-specific messaging adapters.
#[async_trait]
pub trait PlatformAdapter: Send + Sync {
    /// The platform this adapter handles.
    fn platform(&self) -> Platform;

    /// Send a text message to a chat.
    async fn send_message(
        &self,
        chat_id: &str,
        content: &str,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse>;

    /// Send a text message using Telegram's `parse_mode: MarkdownV2`
    /// (Phase 36.17.2.2 D-01 — sibling of [`Self::send_message`] for the
    /// `stream_consumer.rs` overflow-chunk path per CONTEXT.md D-Discretion
    /// recommendation: "every overflow chunk uses `send_message_markdown_v2`
    /// so the entire response renders consistently").
    ///
    /// **Caller contract:** `content` MUST be pre-escaped via
    /// `ironhermes_gateway::markdown_v2::escape_outside_code_blocks` before
    /// invocation. The trait surface does NOT auto-escape — the production
    /// call sites in `stream_consumer.rs::flush(final_edit=true)`'s
    /// overflow branch apply the escape themselves. Implementors that do
    /// not distinguish MarkdownV2 from plain text (Discord, Slack, test
    /// fixtures) MAY delegate to [`Self::send_message`]; only
    /// `TelegramAdapter` actually sets `parse_mode: MarkdownV2` and applies
    /// the D-02 fallback.
    ///
    /// **D-02 fallback (`TelegramAdapter` only):** when the Bot API returns a
    /// 400 whose description substring-matches a parse-mode failure, the
    /// implementation logs `warn!`, re-issues the same call once with
    /// `parse_mode` OMITTED, and returns the retry result. On second
    /// failure logs `error!` and returns the retry `Err`. Non-Telegram
    /// adapters need not implement the fallback.
    async fn send_message_markdown_v2(
        &self,
        chat_id: &str,
        content: &str,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse>;

    /// Edit an existing message (plain text — for streaming edits).
    async fn edit_message(&self, chat_id: &str, message_id: &str, content: &str) -> Result<()>;

    /// Edit an existing message using Telegram's `parse_mode: MarkdownV2`
    /// (Phase 36.17.2.2 D-01 — supersedes the legacy `Markdown` parse mode).
    ///
    /// **Caller contract:** `content` MUST be pre-escaped via
    /// `ironhermes_gateway::markdown_v2::escape_outside_code_blocks` before
    /// invocation. The trait surface does NOT auto-escape — the only
    /// production call site (`stream_consumer.rs::flush(final_edit=true)`
    /// per plan 05) applies the escape. Implementors (notably
    /// `TelegramAdapter`) treat `content` as already-escaped and pass it
    /// through `parse_mode: MarkdownV2`.
    ///
    /// **D-02 fallback (`TelegramAdapter` only):** when the Bot API returns a
    /// 400 whose description substring-matches a parse-mode failure, the
    /// implementation logs `warn!`, re-issues the same call once with
    /// `parse_mode` OMITTED, and returns the retry result. On second
    /// failure logs `error!` and returns the retry `Err`. Non-Telegram
    /// adapters need not implement the fallback.
    async fn edit_message_markdown_v2(
        &self,
        chat_id: &str,
        message_id: &str,
        content: &str,
    ) -> Result<()>;

    /// Delete a message.
    async fn delete_message(&self, chat_id: &str, message_id: &str) -> Result<()>;

    /// Add a reaction to a message.
    async fn add_reaction(&self, _chat_id: &str, _message_id: &str, _emoji: &str) -> Result<()> {
        Ok(()) // Default no-op for platforms that don't support reactions
    }

    /// Send a chat action (e.g. "typing").
    async fn send_chat_action(&self, _chat_id: &str, _action: &str) -> Result<()> {
        Ok(()) // Default no-op
    }

    /// Check if the adapter is currently running.
    fn is_running(&self) -> bool;

    /// Selects the response-delivery strategy for the agent-turn pipeline
    /// (Phase 47.6 Plan 01, D-13).
    ///
    /// `true` (the default): the adapter supports in-place message edits, so
    /// the agent-turn pipeline publishes a placeholder message and streams
    /// the real text into it via [`Self::edit_message`] /
    /// [`Self::edit_message_markdown_v2`] as the turn progresses. Telegram,
    /// Discord and Slack all natively support this and are unaffected by
    /// this method's addition — no existing impl needs to change.
    ///
    /// `false`: the adapter receives NO placeholder message and gets its
    /// response in exactly one send when the turn completes. `BuzzAdapter`
    /// overrides this to `false` — Nostr events are immutable, so its
    /// [`Self::edit_message`] is a logged no-op (declared immediately beside
    /// this method for exactly that reason: the no-op and this mode flag
    /// must never drift apart, or the agent-turn pipeline streams into an
    /// edit that silently does nothing and the operator never sees a
    /// response at all — the defect a cross-AI review of that phase's plan
    /// found). `WebhookAdapter` (Phase 36.7.1) follows the same rule: an
    /// HTTP request that already received its 202 has no in-place edit to
    /// stream into.
    fn supports_in_place_edits(&self) -> bool {
        true
    }
}

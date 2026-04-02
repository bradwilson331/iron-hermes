use anyhow::Result;
use async_trait::async_trait;
use ironhermes_core::{MessageEvent, MessageResponse, Platform};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

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

    /// Edit an existing message (plain text — for streaming edits).
    async fn edit_message(
        &self,
        chat_id: &str,
        message_id: &str,
        content: &str,
    ) -> Result<()>;

    /// Edit an existing message with Markdown formatting (final edit per D-03).
    async fn edit_message_markdown(
        &self,
        chat_id: &str,
        message_id: &str,
        content: &str,
    ) -> Result<()>;

    /// Delete a message.
    async fn delete_message(&self, chat_id: &str, message_id: &str) -> Result<()>;

    /// Add a reaction to a message.
    async fn add_reaction(
        &self,
        _chat_id: &str,
        _message_id: &str,
        _emoji: &str,
    ) -> Result<()> {
        Ok(()) // Default no-op for platforms that don't support reactions
    }

    /// Send a chat action (e.g. "typing").
    async fn send_chat_action(&self, _chat_id: &str, _action: &str) -> Result<()> {
        Ok(()) // Default no-op
    }

    /// Check if the adapter is currently running.
    fn is_running(&self) -> bool;
}

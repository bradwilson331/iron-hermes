use anyhow::Result;
use async_trait::async_trait;
use ironhermes_core::{MessageEvent, MessageResponse, Platform};

/// Handler for incoming messages — connects gateway to the agent.
#[async_trait]
pub trait MessageHandler: Send + Sync {
    /// Process an incoming message and return the response text.
    async fn handle(&self, event: &MessageEvent) -> Result<String>;
}

/// Trait for platform-specific messaging adapters.
#[async_trait]
pub trait PlatformAdapter: Send + Sync {
    /// The platform this adapter handles.
    fn platform(&self) -> Platform;

    /// Start listening for incoming messages.
    async fn start(&mut self, handler: Box<dyn MessageHandler>) -> Result<()>;

    /// Stop the adapter gracefully.
    async fn stop(&mut self) -> Result<()>;

    /// Send a text message to a chat.
    async fn send_message(
        &self,
        chat_id: &str,
        content: &str,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse>;

    /// Edit an existing message.
    async fn edit_message(
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

    /// Check if the adapter is currently running.
    fn is_running(&self) -> bool;
}

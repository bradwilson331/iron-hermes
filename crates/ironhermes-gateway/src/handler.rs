use std::sync::Arc;
use std::path::PathBuf;
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::{mpsc, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use ironhermes_core::{ChatMessage, Config, ContentPart, ImageUrl, MessageContent, MessageEvent, Platform, Role};
use ironhermes_agent::{AgentLoop, LlmClient, PromptBuilder};
use ironhermes_agent::agent_loop::{StreamCallback, ToolProgressCallback};
use ironhermes_tools::ToolRegistry;

use crate::adapter::{MessageHandler, PlatformAdapter};
use crate::multimodal::ProcessedAttachments;
use crate::session::{SessionKey, SessionStore};
use crate::stream_consumer::StreamConsumer;

/// Retry wrapper for Telegram API calls that may hit 429 rate limits (D-19).
async fn with_rate_limit_retry<F, Fut, T>(f: F) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    for attempt in 0..3u64 {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("429") || err_str.contains("Too Many Requests") {
                    let wait = (attempt + 1) * 2; // 2s, 4s, 6s
                    warn!("Telegram rate limit hit, retrying in {}s (attempt {})", wait, attempt + 1);
                    tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                    continue;
                }
                return Err(e);
            }
        }
    }
    anyhow::bail!("Bot is being rate limited, please wait")
}

/// Bridges incoming Telegram messages to the AgentLoop with streaming output.
pub struct GatewayMessageHandler {
    config: Config,
    session_store: Arc<RwLock<SessionStore>>,
    tool_registry: Arc<ToolRegistry>,
}

impl GatewayMessageHandler {
    pub fn new(
        config: Config,
        session_store: Arc<RwLock<SessionStore>>,
        tool_registry: Arc<ToolRegistry>,
    ) -> Self {
        Self {
            config,
            session_store,
            tool_registry,
        }
    }

    /// Dispatch a slash command to the appropriate handler (plan 04).
    async fn handle_slash_command(
        &self,
        event: &MessageEvent,
        adapter: Arc<dyn PlatformAdapter>,
        cancel: CancellationToken,
    ) -> Result<()> {
        let command = event.content.split_whitespace().next().unwrap_or("");
        // Strip @botname suffix (e.g., "/start@mybot" -> "/start")
        let command = command.split('@').next().unwrap_or(command);

        match command {
            "/start" => self.cmd_start(event, adapter, cancel).await,
            "/new" => self.cmd_new(event, adapter).await,
            "/clear" => self.cmd_clear(event, adapter).await,
            "/help" => self.cmd_help(event, adapter).await,
            _ => {
                // Unknown slash command — pass through to agent loop as a normal message
                let no_attachments = ProcessedAttachments { text_prefix: None, image_data_uri: None };
                self.run_agent(event, adapter, cancel, no_attachments).await
            }
        }
    }

    /// /start — Reset session, then generate in-character LLM greeting (D-15).
    async fn cmd_start(
        &self,
        event: &MessageEvent,
        adapter: Arc<dyn PlatformAdapter>,
        cancel: CancellationToken,
    ) -> Result<()> {
        let key = SessionKey::new(Platform::Telegram, &event.chat_id)
            .with_user(&event.sender_id);
        {
            let mut store = self.session_store.write().await;
            store.remove(&key);
        }
        // Create synthetic event asking for introduction — LLM generates greeting with SOUL.md
        let mut intro_event = event.clone();
        intro_event.content =
            "Please introduce yourself. This is the start of a new conversation.".to_string();
        let no_attachments = ProcessedAttachments { text_prefix: None, image_data_uri: None };
        self.run_agent(&intro_event, adapter, cancel, no_attachments).await
    }

    /// /new — Archive current session and confirm fresh start (D-13).
    async fn cmd_new(
        &self,
        event: &MessageEvent,
        adapter: Arc<dyn PlatformAdapter>,
    ) -> Result<()> {
        let key = SessionKey::new(Platform::Telegram, &event.chat_id)
            .with_user(&event.sender_id);
        let had_session = {
            let mut store = self.session_store.write().await;
            store.remove(&key).is_some()
        };
        let msg = if had_session {
            "Conversation cleared. Starting fresh."
        } else {
            "No active conversation. Ready for a new one."
        };
        with_rate_limit_retry(|| adapter.send_message(&event.chat_id, msg, None)).await?;
        Ok(())
    }

    /// /clear — Wipe history but keep session alive (D-13).
    async fn cmd_clear(
        &self,
        event: &MessageEvent,
        adapter: Arc<dyn PlatformAdapter>,
    ) -> Result<()> {
        let key = SessionKey::new(Platform::Telegram, &event.chat_id)
            .with_user(&event.sender_id);
        {
            let mut store = self.session_store.write().await;
            if let Some(session) = store.get_mut(&key) {
                session.clear();
            }
        }
        with_rate_limit_retry(|| adapter.send_message(&event.chat_id, "History cleared.", None))
            .await?;
        Ok(())
    }

    /// /help — Show available commands (D-13).
    async fn cmd_help(
        &self,
        event: &MessageEvent,
        adapter: Arc<dyn PlatformAdapter>,
    ) -> Result<()> {
        let help_text = "/start - Start a new conversation with an introduction\n\
                         /new - Start a fresh conversation (clears history)\n\
                         /clear - Clear conversation history\n\
                         /help - Show this help message";
        with_rate_limit_retry(|| adapter.send_message(&event.chat_id, help_text, None)).await?;
        Ok(())
    }

    /// Public entry point for multimodal-aware message handling.
    /// Called from runner.rs per-chat workers which have access to QueuedMessage.
    pub async fn handle_with_multimodal(
        &self,
        event: &MessageEvent,
        adapter: Arc<dyn PlatformAdapter>,
        cancel: CancellationToken,
        processed: ProcessedAttachments,
    ) -> Result<()> {
        if event.content.starts_with('/') {
            return self.handle_slash_command(event, adapter, cancel).await;
        }
        self.run_agent(event, adapter, cancel, processed).await
    }

    /// Run the agent loop for a message event — drives streaming to StreamConsumer.
    async fn run_agent(
        &self,
        event: &MessageEvent,
        adapter: Arc<dyn PlatformAdapter>,
        cancel: CancellationToken,
        processed: ProcessedAttachments,
    ) -> Result<()> {
        // 1. Send initial placeholder message; get message_id for StreamConsumer
        let placeholder = with_rate_limit_retry(|| {
            adapter.send_message(&event.chat_id, "\u{2588}", None)
        })
        .await?;
        let placeholder_id = placeholder.message_id.clone();

        // 2. Spawn typing indicator task (D-16): sends "typing" every 5 seconds
        let typing_cancel = cancel.child_token();
        let adapter_typing = adapter.clone();
        let chat_id_typing = event.chat_id.clone();
        let typing_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = typing_cancel.cancelled() => break,
                    _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                        let _ = adapter_typing.send_chat_action(&chat_id_typing, "typing").await;
                    }
                }
            }
        });
        // Send first typing action immediately
        let _ = adapter.send_chat_action(&event.chat_id, "typing").await;

        // 3. Get or create session; clone messages immediately to avoid holding lock across await
        let model = self.config.model.default.clone();
        let key = SessionKey::new(Platform::Telegram, &event.chat_id)
            .with_user(&event.sender_id);

        // Build user message content — incorporate multimodal data
        let user_message = build_user_message(event, processed);

        let mut session_messages = {
            let mut store = self.session_store.write().await;
            let session = store.get_or_create(key.clone(), &model);
            // Add user message
            session.add_message(user_message);
            session.messages.clone()
        };

        // 4. Build system message via PromptBuilder (loads SOUL.md + project context)
        let cwd = PathBuf::from(std::env::current_dir().unwrap_or_default());
        let system_msg = PromptBuilder::new(&model, "telegram")
            .load_context(&cwd)
            .build_system_message();
        // Prepend system message
        let mut messages = vec![system_msg];
        messages.append(&mut session_messages);

        // 5. Create mpsc channels for streaming bridge
        let (stream_tx, mut stream_rx) = mpsc::channel::<String>(256);
        let (tool_tx, mut tool_rx) = mpsc::channel::<String>(64);

        // 6. Spawn StreamConsumer task
        let mut consumer = StreamConsumer::new(adapter.clone(), &event.chat_id, &placeholder_id);
        let consumer_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    msg = tool_rx.recv() => {
                        match msg {
                            Some(tool_name) => {
                                consumer.tool_status(&tool_name);
                                let _ = consumer.flush(false).await;
                            }
                            None => {
                                // tool_rx closed — check stream_rx
                            }
                        }
                    }
                    chunk = stream_rx.recv() => {
                        match chunk {
                            Some(text) => {
                                consumer.clear_tool_status();
                                consumer.push(&text);
                                let _ = consumer.flush(false).await;
                            }
                            None => {
                                // stream_rx closed — do final flush
                                let _ = consumer.flush(true).await;
                                break;
                            }
                        }
                    }
                }
            }
        });

        // 7. Build AgentLoop
        let base_url = self.config.resolve_base_url();
        let api_key = self.config.resolve_api_key().unwrap_or_default();
        let max_turns = self.config.agent.max_turns;

        let client = LlmClient::new(base_url, api_key, &model);

        let stream_tx_clone = stream_tx.clone();
        let stream_callback: StreamCallback = Box::new(move |delta: &str| {
            let _ = stream_tx_clone.try_send(delta.to_string());
        });

        let tool_tx_clone = tool_tx.clone();
        let tool_callback: ToolProgressCallback = Box::new(move |name: &str, _args: &str| {
            let _ = tool_tx_clone.try_send(name.to_string());
        });

        let agent = AgentLoop::new(client, self.tool_registry.clone(), max_turns)
            .with_streaming(stream_callback)
            .with_tool_progress(tool_callback);

        // 8. Run agent with error recovery (D-18)
        let agent_result = agent.run(messages).await;

        // 9. Close stream channels to flush consumer
        drop(stream_tx);
        drop(tool_tx);
        consumer_handle.await.ok();

        // 10. Cancel typing indicator
        cancel.cancel();
        typing_handle.await.ok();

        match agent_result {
            Ok(result) => {
                info!("Agent completed, turns_used={}", result.turns_used);
                // 11. Update session with agent's response messages
                let new_messages: Vec<ChatMessage> = result
                    .messages
                    .into_iter()
                    .filter(|m| m.role == Role::Assistant)
                    .collect();
                if !new_messages.is_empty() {
                    let mut store = self.session_store.write().await;
                    if let Some(session) = store.get_mut(&key) {
                        for msg in new_messages {
                            session.add_message(msg);
                        }
                    }
                }
            }
            Err(e) => {
                // D-18: Append error indicator to whatever was already streamed
                error!("Agent error: {}", e);
                let error_suffix = "\n\n-- Something went wrong, please try again";
                let _ = adapter
                    .send_message(&event.chat_id, error_suffix, None)
                    .await;
            }
        }

        Ok(())
    }
}

#[async_trait]
impl MessageHandler for GatewayMessageHandler {
    async fn handle(
        &self,
        event: &MessageEvent,
        adapter: Arc<dyn PlatformAdapter>,
        cancel: CancellationToken,
    ) -> Result<()> {
        // Intercept slash commands before agent loop (plan 04)
        if event.content.starts_with('/') {
            return self.handle_slash_command(event, adapter, cancel).await;
        }
        // No multimodal data via this path (text-only fallback)
        let no_attachments = ProcessedAttachments {
            text_prefix: None,
            image_data_uri: None,
        };
        self.run_agent(event, adapter, cancel, no_attachments).await
    }
}

/// Build a ChatMessage for the user's input, incorporating any multimodal data.
///
/// - If there is an image_data_uri: creates a multipart message with text + image.
/// - If there is a text_prefix (document): prepends it to the message content.
/// - Otherwise: plain text message.
fn build_user_message(event: &MessageEvent, processed: ProcessedAttachments) -> ChatMessage {
    if let Some(data_uri) = processed.image_data_uri {
        // Vision input: multipart message with optional caption + image
        let mut parts = Vec::new();
        let text = if !event.content.is_empty() {
            event.content.clone()
        } else {
            "What is in this image?".to_string()
        };
        parts.push(ContentPart::Text { text });
        parts.push(ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: data_uri,
                detail: None,
            },
        });
        ChatMessage {
            role: Role::User,
            content: Some(MessageContent::Parts(parts)),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    } else if let Some(prefix) = processed.text_prefix {
        // Document text: prepend extracted content to the user message
        let combined = if event.content.is_empty() {
            prefix
        } else {
            format!("{}\n\n{}", prefix, event.content)
        };
        ChatMessage {
            role: Role::User,
            content: Some(MessageContent::text(combined)),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    } else {
        // Plain text
        ChatMessage {
            role: Role::User,
            content: Some(MessageContent::text(&event.content)),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }
}

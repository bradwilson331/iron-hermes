use std::sync::Arc;
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::{mpsc, RwLock};
use tokio_util::sync::CancellationToken;
use ironhermes_core::{ChatMessage, Config, MessageEvent};
use ironhermes_agent::{AgentLoop, LlmClient, PromptBuilder};
use ironhermes_agent::agent_loop::{StreamCallback, ToolProgressCallback};
use ironhermes_tools::ToolRegistry;
use crate::adapter::{MessageHandler, PlatformAdapter};
use crate::session::{SessionKey, SessionStore};
use crate::stream_consumer::StreamConsumer;
use tracing::{error, info, warn};

/// Bridges incoming MessageEvents to the AgentLoop with streaming output via StreamConsumer.
///
/// For each message handled:
/// 1. Sends a placeholder message (gives StreamConsumer something to edit).
/// 2. Spawns a typing indicator that fires every 5 seconds.
/// 3. Runs the AgentLoop with streaming/tool-progress callbacks piped through mpsc channels.
/// 4. A consumer task drives StreamConsumer edits from the mpsc channels.
/// 5. Updates the session store with the result.
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
}

#[async_trait]
impl MessageHandler for GatewayMessageHandler {
    async fn handle(
        &self,
        event: &MessageEvent,
        adapter: Arc<dyn PlatformAdapter>,
        cancel: CancellationToken,
    ) -> Result<()> {
        let chat_id = event.chat_id.clone();
        let content = event.content.clone();

        // --- 1. Send initial placeholder message ---
        let placeholder = adapter
            .send_message(&chat_id, "...", None)
            .await
            .map_err(|e| {
                error!(chat_id = %chat_id, "Failed to send placeholder: {}", e);
                e
            })?;
        let placeholder_message_id = placeholder.message_id.clone();

        // --- 2. Typing indicator task ---
        let typing_cancel = cancel.child_token();
        let adapter_typing = adapter.clone();
        let chat_id_typing = chat_id.clone();
        let typing_task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));
            loop {
                tokio::select! {
                    _ = typing_cancel.cancelled() => break,
                    _ = interval.tick() => {
                        if let Err(e) = adapter_typing.send_chat_action(&chat_id_typing, "typing").await {
                            warn!(chat_id = %chat_id_typing, "Failed to send typing action: {}", e);
                        }
                    }
                }
            }
        });

        // --- 3. Get/create session, clone messages, add user message ---
        let tg_platform = ironhermes_core::Platform::Telegram;
        let session_key = SessionKey::new(tg_platform, &chat_id);
        let model = self.config.model.default.clone();
        let timeout_hours = self
            .config
            .gateway
            .platforms
            .get("telegram")
            .map(|p| p.session_timeout_hours)
            .unwrap_or(24);
        let max_turns = self.config.agent.max_turns;

        // Resolve credentials
        let base_url = self.config.resolve_base_url();
        let api_key = self.config.resolve_api_key().unwrap_or_default();

        // Get or create session — hold write lock briefly, then release
        let mut session_messages: Vec<ChatMessage> = {
            let mut store = self.session_store.write().await;
            // Expire stale sessions opportunistically
            store.expire_stale(timeout_hours);
            let session = store.get_or_create(session_key.clone(), &model);
            session.add_message(ChatMessage::user(&content));
            session.messages.clone()
        };

        // --- 4. Build system prompt and prepend ---
        let cwd = std::env::current_dir().unwrap_or_default();
        let system_msg = PromptBuilder::new(&model, "telegram")
            .load_context(&cwd)
            .build_system_message();
        let mut messages = vec![system_msg];
        messages.append(&mut session_messages);

        // --- 5. Streaming bridge channels ---
        let (stream_tx, mut stream_rx) = mpsc::channel::<String>(256);
        let (tool_tx, mut tool_rx) = mpsc::channel::<(String, bool)>(64);
        // tool channel: (name, is_clear) — true means clear_tool_status

        // --- 6. StreamConsumer task ---
        let adapter_consumer = adapter.clone();
        let chat_id_consumer = chat_id.clone();
        let ph_id_consumer = placeholder_message_id.clone();
        let consumer_task = tokio::spawn(async move {
            let mut consumer = StreamConsumer::new(adapter_consumer, chat_id_consumer, ph_id_consumer);
            loop {
                tokio::select! {
                    // Tool progress updates
                    Some((name, is_clear)) = tool_rx.recv() => {
                        if is_clear {
                            consumer.clear_tool_status();
                        } else {
                            consumer.tool_status(&name);
                        }
                        let _ = consumer.flush(false).await;
                    }
                    // Text chunks from the LLM
                    Some(chunk) = stream_rx.recv() => {
                        consumer.clear_tool_status();
                        consumer.push(&chunk);
                        let _ = consumer.flush(false).await;
                    }
                    // Both channels closed — do final flush
                    else => {
                        let _ = consumer.flush(true).await;
                        break;
                    }
                }
            }
        });

        // --- 7. Build AgentLoop with streaming callbacks ---
        let stream_tx_cb = stream_tx.clone();
        let stream_callback: StreamCallback = Box::new(move |delta: &str| {
            let _ = stream_tx_cb.try_send(delta.to_string());
        });

        let tool_tx_cb = tool_tx.clone();
        let tool_progress_callback: ToolProgressCallback = Box::new(move |name: &str, _args: &str| {
            let _ = tool_tx_cb.try_send((name.to_string(), false));
        });

        let client = LlmClient::new(&base_url, &api_key, &model);
        let agent = AgentLoop::new(client, self.tool_registry.clone(), max_turns)
            .with_streaming(stream_callback)
            .with_tool_progress(tool_progress_callback);

        // --- 8. Run agent ---
        let agent_result = agent.run(messages).await;

        // Drop channels to close them — signals consumer task to do final flush
        drop(stream_tx);
        drop(tool_tx);

        // Wait for consumer to finish final flush
        let _ = consumer_task.await;

        // Cancel typing indicator
        typing_task.abort();
        let _ = typing_task.await;

        // --- 9. Handle agent errors ---
        match agent_result {
            Ok(result) => {
                // Update session with new messages from agent result
                let new_messages: Vec<ChatMessage> = result
                    .messages
                    .iter()
                    .skip(1) // skip system message
                    .cloned()
                    .collect();

                let mut store = self.session_store.write().await;
                if let Some(session) = store.get_mut(&session_key) {
                    // Replace session messages with what agent returned (includes tool results)
                    session.messages = new_messages
                        .into_iter()
                        .filter(|m| {
                            // Keep user and assistant messages only (not system)
                            m.role == ironhermes_core::Role::User
                                || m.role == ironhermes_core::Role::Assistant
                        })
                        .collect();
                    session.updated_at = chrono::Utc::now();
                }
                drop(store);

                info!(
                    chat_id = %chat_id,
                    turns = result.turns_used,
                    tokens = result.total_usage.total_tokens,
                    "Agent run complete"
                );
            }
            Err(e) => {
                error!(chat_id = %chat_id, error = %e, "Agent run failed");
                // Edit the placeholder with an error message
                let _ = adapter
                    .edit_message(
                        &chat_id,
                        &placeholder_message_id,
                        "\u{26a0}\u{fe0f} Something went wrong, please try again.",
                    )
                    .await;
            }
        }

        Ok(())
    }
}

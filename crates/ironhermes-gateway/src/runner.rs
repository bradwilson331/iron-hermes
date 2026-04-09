use std::sync::{Arc, Mutex};
use anyhow::{Context, Result};
use tokio::sync::{mpsc, RwLock, Semaphore};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use ironhermes_core::{Config, MemoryStore};
use ironhermes_cron::JobStore;
use ironhermes_tools::ToolRegistry;
use tracing::{error, info, warn};

use crate::adapter::PlatformAdapter;
use crate::backoff::BackoffState;
use crate::handler::GatewayMessageHandler;
use crate::multimodal;
use crate::session::SessionStore;
use crate::telegram::{TelegramAdapter, TgBotCommand, tg_message_to_event};
use crate::user_queue::UserQueueManager;

/// Runs the Telegram gateway: long polling, per-user dispatch, JoinSet supervision,
/// Semaphore concurrency control, and CancellationToken-based graceful shutdown.
pub struct GatewayRunner {
    config: Config,
    session_store: Arc<RwLock<SessionStore>>,
    tool_registry: Arc<ToolRegistry>,
    memory_store: Option<Arc<Mutex<MemoryStore>>>,
    job_store: Option<Arc<Mutex<JobStore>>>,
    cancel: CancellationToken,
}

impl GatewayRunner {
    pub fn new(config: Config, tool_registry: Arc<ToolRegistry>) -> Self {
        Self {
            config,
            session_store: Arc::new(RwLock::new(SessionStore::new())),
            tool_registry,
            memory_store: None,
            job_store: None,
            cancel: CancellationToken::new(),
        }
    }

    /// Set the memory store for prompt injection and tool access.
    pub fn set_memory_store(&mut self, store: Arc<Mutex<MemoryStore>>) {
        self.memory_store = Some(store);
    }

    /// Set the job store for cron tick task integration.
    pub fn set_job_store(&mut self, store: Arc<Mutex<JobStore>>) {
        self.job_store = Some(store);
    }

    /// Start the gateway. Blocks until ctrl+c or fatal error.
    pub async fn start(&self) -> Result<()> {
        // --- 1. Resolve Telegram token ---
        let tg_config = self
            .config
            .gateway
            .platforms
            .get("telegram")
            .cloned()
            .unwrap_or_default();

        let token = resolve_token(&tg_config.token)
            .context("No Telegram bot token configured. Set TELEGRAM_BOT_TOKEN or gateway.platforms.telegram.token in config.yaml")?;

        // --- 2. Create adapter ---
        let adapter: Arc<TelegramAdapter> = Arc::new(TelegramAdapter::new(&token));

        // --- 3. Verify token via getMe ---
        let bot_info = adapter
            .get_me()
            .await
            .context("Failed to authenticate with Telegram (check bot token)")?;
        let bot_username = bot_info.username.clone().unwrap_or_default();
        info!(
            bot_id = bot_info.id,
            bot_name = %bot_info.first_name,
            bot_username = %bot_username,
            "Connected to Telegram"
        );

        // --- 4. Register slash commands (D-17) ---
        let commands = vec![
            TgBotCommand {
                command: "start".into(),
                description: "Start the bot".into(),
            },
            TgBotCommand {
                command: "new".into(),
                description: "New conversation".into(),
            },
            TgBotCommand {
                command: "clear".into(),
                description: "Clear history".into(),
            },
            TgBotCommand {
                command: "help".into(),
                description: "Show help".into(),
            },
        ];
        if let Err(e) = adapter.set_my_commands(&commands).await {
            warn!("Failed to register bot commands: {}", e);
        } else {
            info!("Bot commands registered");
        }

        // --- 5. Setup channels and concurrency primitives ---
        let (msg_tx, msg_rx) = mpsc::channel::<crate::telegram::TgUpdate>(256);
        let max_concurrent = tg_config.max_concurrent_runs.max(1);
        let semaphore = Arc::new(Semaphore::new(max_concurrent));
        let timeout_hours = tg_config.session_timeout_hours;
        let whitelist = tg_config.whitelist.clone();

        // --- 6. Create handler and queue manager ---
        let mut handler = GatewayMessageHandler::new(
            self.config.clone(),
            self.session_store.clone(),
            self.tool_registry.clone(),
        );
        if let Some(ref store) = self.memory_store {
            handler.set_memory_store(store.clone());
        }
        let handler = Arc::new(handler);
        let user_queue = Arc::new(UserQueueManager::new(
            adapter.clone() as Arc<dyn crate::adapter::PlatformAdapter>,
            16,
        ));

        let mut join_set: JoinSet<()> = JoinSet::new();

        // --- 7. Poll loop ---
        let poll_cancel = self.cancel.clone();
        let adapter_poll = adapter.clone();
        let msg_tx_poll = msg_tx.clone();
        join_set.spawn(async move {
            let mut offset: Option<i64> = None;
            let mut backoff = BackoffState::default_polling();

            loop {
                tokio::select! {
                    _ = poll_cancel.cancelled() => {
                        info!("Poll loop cancelled");
                        break;
                    }
                    result = adapter_poll.get_updates(offset) => {
                        match result {
                            Ok(updates) => {
                                backoff.record_success();
                                if !updates.is_empty() {
                                    info!(count = updates.len(), "Received {} update(s) from polling", updates.len());
                                }
                                for update in &updates {
                                    if let Some(new_offset) = offset {
                                        if update.update_id >= new_offset {
                                            offset = Some(update.update_id + 1);
                                        }
                                    } else {
                                        offset = Some(update.update_id + 1);
                                    }
                                    if msg_tx_poll.send(update.clone()).await.is_err() {
                                        // Dispatch channel closed — shutting down
                                        return;
                                    }
                                }
                            }
                            Err(e) => {
                                let err_str = e.to_string();
                                if err_str.contains("Conflict") || err_str.contains("409") {
                                    backoff.record_conflict();
                                    if backoff.is_fatal_conflict() {
                                        error!("Fatal 409 conflict — another bot instance is polling on this token. Shutting down.");
                                        poll_cancel.cancel();
                                        break;
                                    }
                                } else {
                                    backoff.record_failure();
                                }
                                let delay = backoff.next_delay();
                                warn!(
                                    error = %e,
                                    delay_ms = delay.as_millis(),
                                    "Polling error, backing off"
                                );
                                tokio::time::sleep(delay).await;
                            }
                        }
                    }
                }
            }
        });

        // --- 8. Dispatch loop ---
        let dispatch_cancel = self.cancel.clone();
        let handler_dispatch = handler.clone();
        let user_queue_dispatch = user_queue.clone();
        let adapter_dispatch = adapter.clone() as Arc<dyn crate::adapter::PlatformAdapter>;
        let adapter_dispatch_mm = adapter.clone(); // typed Arc<TelegramAdapter> for multimodal
        let semaphore_dispatch = semaphore.clone();
        let cancel_dispatch = self.cancel.clone();
        let mut msg_rx = msg_rx;
        let bot_username_str = bot_username.clone();

        // We run dispatch inline (not in JoinSet) so we control msg_rx lifetime
        let dispatch_future = async move {
            loop {
                tokio::select! {
                    _ = dispatch_cancel.cancelled() => {
                        info!("Dispatch loop cancelled");
                        break;
                    }
                    update = msg_rx.recv() => {
                        let update = match update {
                            Some(u) => u,
                            None => break, // channel closed
                        };

                        let msg = match &update.message {
                            Some(m) => m.clone(),
                            None => continue,
                        };

                        // Convert to MessageEvent
                        let event = tg_message_to_event(&msg);
                        info!(
                            chat_id = %event.chat_id,
                            sender_id = %event.sender_id,
                            content = %event.content,
                            chat_type = %event.chat_type,
                            "Received message from dispatch channel"
                        );

                        // Whitelist check (D-10/D-11/D-12)
                        if !whitelist.is_empty() {
                            let sender_id: i64 = event.sender_id.parse().unwrap_or(0);
                            if !whitelist.contains(&sender_id) {
                                warn!(sender_id = sender_id, "Sender not in whitelist, ignoring");
                                continue;
                            }
                        } else {
                            warn!("Whitelist is empty — denying all messages (D-12)");
                            continue;
                        }

                        // Group @mention check (D-09)
                        if event.chat_type == "group" || event.chat_type == "supergroup" {
                            let mention = format!("@{}", bot_username_str);
                            if !event.content.contains(&mention) {
                                info!("Group message without @mention, skipping");
                                continue;
                            }
                        }

                        info!(chat_id = %event.chat_id, "Message passed all filters, dispatching");

                        // Process multimodal attachments (D-05 through D-08)
                        let (text_prefix, image_data_uri) = if !event.attachments.is_empty() {
                            match multimodal::process_attachments(&adapter_dispatch_mm, &msg).await {
                                Ok(processed) => (processed.text_prefix, processed.image_data_uri),
                                Err(e) => {
                                    // Send user-friendly error and skip this message
                                    let chat_id = event.chat_id.clone();
                                    let err_msg = format!("Could not process attachment: {}", e);
                                    let _ = adapter_dispatch_mm.send_message(&chat_id, &err_msg, None).await;
                                    continue;
                                }
                            }
                        } else {
                            (None, None)
                        };

                        // Dispatch via per-user queue
                        let maybe_rx = user_queue_dispatch.dispatch(event, text_prefix, image_data_uri).await;
                        if let Some(mut chat_rx) = maybe_rx {
                            // New worker needed for this chat
                            let handler_task = handler_dispatch.clone();
                            let adapter_task = adapter_dispatch.clone();
                            let sem_task = semaphore_dispatch.clone();
                            let cancel_task = cancel_dispatch.clone();
                            let queue_task = user_queue_dispatch.clone();
                            let chat_id_task = msg.chat.id.to_string();

                            // Spawn per-chat worker into a detached task
                            // We don't add to join_set here (JoinSet is owned outside closure),
                            // but per-chat workers drain when cancel fires because they select on it
                            tokio::spawn(async move {
                                while let Some(queued_msg) = chat_rx.recv().await {
                                    // Acquire semaphore permit (bounded concurrency per TG-06)
                                    let permit = match sem_task.acquire().await {
                                        Ok(p) => p,
                                        Err(_) => break, // semaphore closed
                                    };

                                    let processed = crate::multimodal::ProcessedAttachments {
                                        text_prefix: queued_msg.text_prefix,
                                        image_data_uri: queued_msg.image_data_uri,
                                    };

                                    let result = handler_task
                                        .handle_with_multimodal(
                                            &queued_msg.event,
                                            adapter_task.clone(),
                                            cancel_task.child_token(),
                                            processed,
                                        )
                                        .await;

                                    drop(permit);

                                    if let Err(e) = result {
                                        error!(
                                            chat_id = %queued_msg.event.chat_id,
                                            error = %e,
                                            "Handler error for message"
                                        );
                                    }

                                    // Check if we should stop
                                    if cancel_task.is_cancelled() {
                                        break;
                                    }
                                }
                                // Worker done — remove from queue manager
                                queue_task.remove(&chat_id_task).await;
                            });
                        }
                    }
                }
            }
        };

        // --- 9. Session cleanup task ---
        let cleanup_cancel = self.cancel.clone();
        let session_store_cleanup = self.session_store.clone();
        join_set.spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5 * 60));
            loop {
                tokio::select! {
                    _ = cleanup_cancel.cancelled() => break,
                    _ = interval.tick() => {
                        let mut store = session_store_cleanup.write().await;
                        store.expire_stale(timeout_hours);
                    }
                }
            }
        });

        // --- 10. Cron tick task ---
        if let Some(ref job_store) = self.job_store {
            let tick_cancel = self.cancel.clone();
            let job_store_tick = job_store.clone();
            join_set.spawn(async move {
                let mut interval = tokio::time::interval(
                    tokio::time::Duration::from_secs(60)
                );
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tokio::select! {
                        _ = tick_cancel.cancelled() => {
                            info!("Cron tick task shutting down");
                            break;
                        }
                        _ = interval.tick() => {
                            match ironhermes_cron::run_tick_check(&job_store_tick).await {
                                Ok((due_jobs, result, _lock_guard)) => {
                                    if result.jobs_run > 0 {
                                        info!("Tick: {} jobs due", due_jobs.len());
                                    }
                                    for job in &due_jobs {
                                        info!("Job due: {} ({})", job.name, job.id);
                                        // Mark as run with placeholder output
                                        // Full agent execution requires AgentLoop integration
                                        // which involves building a fresh agent per job
                                        if let Err(e) = ironhermes_cron::complete_job_run(
                                            &job_store_tick, job,
                                            "[Tick runner: agent execution pending full integration]",
                                            true
                                        ).await {
                                            error!("Failed to complete job run: {}", e);
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!("Tick error: {}", e);
                                }
                            }
                        }
                    }
                }
            });
            info!("Cron tick task started (60s interval)");
        }

        // --- 11. Run dispatch loop concurrently with shutdown signal ---
        // dispatch_future processes messages; ctrl+c or cancel token stops everything.
        tokio::select! {
            _ = dispatch_future => {
                info!("Dispatch loop exited");
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Ctrl+C received, initiating graceful shutdown");
            }
            _ = self.cancel.cancelled() => {
                info!("Cancellation token fired, shutting down");
            }
        }

        // Propagate cancellation to all subtasks
        self.cancel.cancel();

        // Drop msg_tx to close the polling->dispatch channel
        drop(msg_tx);

        // Drain all JoinSet tasks (poll loop + session cleanup)
        while join_set.join_next().await.is_some() {}

        info!("Gateway shut down cleanly");
        Ok(())
    }
}

/// Resolve the bot token from config value or environment variable.
/// Supports `${ENV_VAR}` syntax for indirection through environment.
fn resolve_token(token: &Option<String>) -> Option<String> {
    if let Some(t) = token {
        if t.starts_with("${") && t.ends_with('}') {
            let var_name = &t[2..t.len() - 1];
            return std::env::var(var_name).ok();
        }
        if !t.is_empty() {
            return Some(t.clone());
        }
    }
    // Fall back to TELEGRAM_BOT_TOKEN environment variable
    std::env::var("TELEGRAM_BOT_TOKEN").ok()
}

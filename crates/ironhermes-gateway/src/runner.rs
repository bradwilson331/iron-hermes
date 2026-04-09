use std::sync::{Arc, Mutex};
use anyhow::{Context, Result};
use tokio::sync::{mpsc, RwLock, Semaphore};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use ironhermes_agent::{AgentLoop, LlmClient, PromptBuilder};
use ironhermes_core::{ChatMessage, Config, MemoryStore, MessageContent, Role, SkillRegistry};
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
    hook_registry: Option<Arc<ironhermes_hooks::HookRegistry>>,
    skill_registry: Option<Arc<SkillRegistry>>,
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
            hook_registry: None,
            skill_registry: None,
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

    /// Set the hook registry for event emission.
    pub fn set_hook_registry(&mut self, registry: Arc<ironhermes_hooks::HookRegistry>) {
        self.hook_registry = Some(registry);
    }

    /// Set the skill registry for catalog injection and cron skill resolution.
    pub fn set_skill_registry(&mut self, registry: Arc<SkillRegistry>) {
        self.skill_registry = Some(registry);
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
        if let Some(ref registry) = self.hook_registry {
            handler.set_hook_registry(registry.clone());
        }
        if let Some(ref registry) = self.skill_registry {
            handler.set_skill_registry(registry.clone());
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
            let skill_registry_tick = self.skill_registry.clone();
            // D-04 / D-11: four additional captures for real AgentLoop execution
            let hook_registry_tick = self.hook_registry.clone();
            let tool_registry_tick = self.tool_registry.clone();
            let memory_store_tick = self.memory_store.clone();
            let config_tick = self.config.clone();

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

                                        // D-01 / D-04 / D-14 / T-07.3-04: call extracted
                                        // helper; single-job failure does NOT panic the
                                        // tick task (helper returns Result<()>).
                                        if let Err(e) = execute_cron_job(
                                            job,
                                            &job_store_tick,
                                            &skill_registry_tick,
                                            &tool_registry_tick,
                                            &memory_store_tick,
                                            &hook_registry_tick,
                                            &config_tick,
                                        )
                                        .await
                                        {
                                            error!(
                                                "execute_cron_job failed for job {} ({}): {}",
                                                job.name, job.id, e
                                            );
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

/// Resolve skill content for a cron job, prepending to the prompt.
/// Returns the combined skill context string (empty if no skills found).
/// Per D-08: skill content appears before the task prompt.
/// Per D-09: missing skills produce a warning and are skipped.
pub(crate) fn resolve_skill_context(
    registry: &ironhermes_core::SkillRegistry,
    skill_names: &[String],
) -> String {
    let mut parts = Vec::new();
    for name in skill_names {
        match registry.read_content(name) {
            Some(content) => parts.push(format!("## Skill: {}\n\n{}", name, content)),
            None => tracing::warn!(skill = %name, "Skill not found at tick time - skipping"),
        }
    }
    parts.join("\n\n---\n\n")
}

/// Execute a single cron job: build a fresh AgentLoop, run it with the resolved full_input,
/// fire MessageReceived + ResponseSent hook events, and persist the real LLM output via
/// ironhermes_cron::complete_job_run. Extracted from the tick task closure so tests can
/// invoke it directly without spinning up the 60s interval timer.
///
/// Per D-09: fresh AgentLoop per call.
/// Per D-10: construction mirrors handler.rs::run_agent except it omits streaming and
///           tool-progress callbacks (cron is headless — no adapter to stream to).
/// Per D-14: `complete_job_run` success flag reflects AgentLoop::run() Result.
/// Per T-07.3-03: agent errors are sanitized to "[Agent error: {display}]" — raw LLM payloads
///                never flow into JSONL/webhooks.
/// Per T-07.3-04: returns `Result<()>` so a single failing job does not panic the tick task.
pub(crate) async fn execute_cron_job(
    job: &ironhermes_cron::CronJob,
    job_store: &Arc<Mutex<ironhermes_cron::JobStore>>,
    skill_registry: &Option<Arc<SkillRegistry>>,
    tool_registry: &Arc<ironhermes_tools::ToolRegistry>,
    memory_store: &Option<Arc<Mutex<MemoryStore>>>,
    hook_registry: &Option<Arc<ironhermes_hooks::HookRegistry>>,
    config: &Config,
) -> Result<()> {
    // D-02: full_input (no underscore); content unchanged from existing logic
    let full_input = if let Some(skill_reg) = skill_registry {
        let skill_context = resolve_skill_context(skill_reg, &job.skills);
        if skill_context.is_empty() {
            job.prompt.clone()
        } else {
            format!("{}\n\n---\n\n{}", skill_context, job.prompt)
        }
    } else {
        job.prompt.clone()
    };

    // D-12: chat_id from job origin (falls back to job.id), platform = "cron"
    let cron_chat_id = job
        .origin
        .as_ref()
        .map(|o| o.chat_id.clone())
        .unwrap_or_else(|| job.id.clone());
    let request_id = uuid::Uuid::new_v4().to_string();

    // D-04 / D-06 / D-07: fire MessageReceived with cron metadata (same registry Arc
    // as Telegram-triggered runs use — the Arc is cloned from self.hook_registry in the
    // tick task closure capture block).
    if let Some(registry) = hook_registry {
        registry.fire(ironhermes_hooks::HookEvent::new(
            &request_id,
            ironhermes_hooks::HookEventKind::MessageReceived {
                platform: "cron".to_string(),
                chat_id: cron_chat_id.clone(),
                content_preview: ironhermes_hooks::event::preview(&full_input, 200),
            },
        ));
    }

    // D-09 / D-10 / D-11: build a FRESH AgentLoop per job, mirroring handler.rs:343-365
    // but omitting stream_callback + tool_callback (cron path has no adapter to push to).
    let base_url = config.resolve_base_url();
    let api_key = config.resolve_api_key().unwrap_or_default();
    let max_turns = config.agent.max_turns;
    let model = config.model.default.clone();

    // Build system message via PromptBuilder — loads SOUL.md + AGENTS.md + project context
    // + memory + skill catalog, identical to handler.rs Telegram path except platform="cron".
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut prompt_builder = PromptBuilder::new(&model, "cron").load_context(&cwd);
    if let Some(store) = memory_store {
        prompt_builder.set_memory_store(store.clone());
    }
    if let Some(skill_reg) = skill_registry {
        prompt_builder.set_skill_registry(skill_reg.clone());
    }
    let system_msg = prompt_builder.build_system_message();

    // Build user message with the resolved full_input (skill content + user prompt)
    let user_msg = ChatMessage {
        role: Role::User,
        content: Some(MessageContent::text(&full_input)),
        tool_calls: None,
        tool_call_id: None,
        name: None,
    };
    let messages = vec![system_msg, user_msg];

    // Construct LlmClient and AgentLoop — D-01 kills the stub
    let client = LlmClient::new(base_url, api_key, &model);
    let mut agent = AgentLoop::new(client, tool_registry.clone(), max_turns);

    // D-06 / D-08: conditional hook registry wiring (Option<Arc<...>>, None is valid)
    if let Some(registry) = hook_registry {
        agent = agent.with_hook_registry(registry.clone());
    }

    // D-03 / D-14 / T-07.3-03: run agent, pass real output and real success flag into
    // complete_job_run. On error, sanitize to "[Agent error: {display}]" so raw LLM
    // payloads never leak into JSONL / webhooks.
    match agent.run(messages).await {
        Ok(result) => {
            info!(
                "Cron agent completed for job {} ({}), turns_used={}",
                job.name, job.id, result.turns_used
            );
            let output = result
                .final_response
                .unwrap_or_else(|| "[No response generated]".to_string());

            // D-04 / D-06: fire ResponseSent with the SAME request_id + cron metadata
            if let Some(registry) = hook_registry {
                registry.fire(ironhermes_hooks::HookEvent::new(
                    &request_id,
                    ironhermes_hooks::HookEventKind::ResponseSent {
                        platform: "cron".to_string(),
                        chat_id: cron_chat_id.clone(),
                        response_preview: ironhermes_hooks::event::preview(&output, 200),
                    },
                ));
            }

            // D-03 / D-13: real output into complete_job_run; delivery routing unchanged
            if let Err(e) = ironhermes_cron::complete_job_run(job_store, job, &output, true).await {
                error!("Failed to complete job run: {}", e);
                return Err(e);
            }
            Ok(())
        }
        Err(e) => {
            // T-07.3-03: sanitized error — do NOT forward raw LLM payload
            error!("Agent error for cron job {} ({}): {}", job.name, job.id, e);
            let error_output = format!("[Agent error: {}]", e);

            // D-04: still fire ResponseSent so observability captures the failure
            if let Some(registry) = hook_registry {
                registry.fire(ironhermes_hooks::HookEvent::new(
                    &request_id,
                    ironhermes_hooks::HookEventKind::ResponseSent {
                        platform: "cron".to_string(),
                        chat_id: cron_chat_id.clone(),
                        response_preview: ironhermes_hooks::event::preview(&error_output, 200),
                    },
                ));
            }

            // D-14: success=false
            if let Err(ce) =
                ironhermes_cron::complete_job_run(job_store, job, &error_output, false).await
            {
                error!("Failed to complete job run after agent error: {}", ce);
                return Err(ce);
            }
            Ok(())
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Task 1 (Wave 0): Placeholder-absent test + LLM-gated skill integration
    // -------------------------------------------------------------------------

    #[test]
    fn test_placeholder_string_absent() {
        // D-17: The placeholder string MUST NOT appear in runner.rs production code after Phase 07.3.
        // This test intentionally reads its own source file so a grep-equivalent check runs in CI.
        // After Task 4 lands: this test is GREEN.
        //
        // Note: the check splits the string so the test source itself does not contain the full
        // literal — otherwise include_str! would always match. The production code previously
        // contained: "[Tick runner: agent execution pending full integration]"
        let source = include_str!("runner.rs");
        // Split into two parts so this test's own source doesn't trigger the check
        let prefix = "[Tick runner: agent execution";
        let suffix = " pending full integration]";
        let placeholder = format!("{}{}", prefix, suffix);
        // Count occurrences — the only matches should be in test strings (contains checks),
        // not in production code paths. The production stub at lines ~407-413 is now gone.
        // We assert that the placeholder does NOT appear outside of test code by checking
        // the full string is absent from the non-test portion.
        let test_marker = "#[cfg(test)]";
        let prod_code = if let Some(idx) = source.find(test_marker) {
            &source[..idx]
        } else {
            source
        };
        assert!(
            !prod_code.contains(&placeholder),
            "D-17 violation: placeholder string still present in production code of runner.rs — \
             Phase 07.3 Task 4 (execute_cron_job extraction + real AgentLoop wiring) has not yet landed"
        );
    }

    #[tokio::test]
    #[ignore = "requires IRONHERMES_TEST_LLM=1 and a reachable LLM endpoint (D-15)"]
    async fn test_cron_skill_reaches_llm() {
        // D-15 / SCHED-03: scheduled job with an attached skill produces an LLM response
        // that reflects the skill content. Gated on env var so CI without LLM credentials
        // does not fail. Run with:
        //   IRONHERMES_TEST_LLM=1 cargo test -p ironhermes-gateway test_cron_skill_reaches_llm -- --ignored
        if std::env::var("IRONHERMES_TEST_LLM").is_err() {
            eprintln!("SKIP: IRONHERMES_TEST_LLM not set");
            return;
        }

        use ironhermes_cron::{JobStore, ScheduleParsed};
        use tempfile::tempdir;

        // 1. Create a skill whose content is a deterministic instruction
        let dir = tempdir().unwrap();
        let skill_dir = dir.path().join(".ironhermes/skills/cron-echo");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: cron-echo\ndescription: Echo a deterministic token\n---\n\n\
             When asked to respond, reply with exactly the token: SKILL-REACHED-LLM-07-3-01",
        ).unwrap();
        let skill_registry = Arc::new(
            ironhermes_core::SkillRegistry::load_with_paths(&[
                dir.path().join(".ironhermes/skills")
            ]),
        );

        // 2. Build an in-memory JobStore with one due job that attaches the skill
        let cron_dir = dir.path().join(".ironhermes/cron");
        std::fs::create_dir_all(&cron_dir).unwrap();
        let job_store = Arc::new(Mutex::new(
            JobStore::open(cron_dir).expect("job store"),
        ));
        let job = {
            let mut guard = job_store.lock().unwrap();
            guard.add_job(
                "cron-skill-integration-test",
                "Please respond now.",
                ScheduleParsed::Interval { minutes: 1, display: "every 1 min".to_string() },
                "every 1 min",
                "cli",
                vec!["cron-echo".to_string()],
                None,
            ).expect("add job")
        };

        // 3. Build a Config that points at a real LLM endpoint (uses env vars / config.yaml defaults)
        let config = ironhermes_core::Config::load()
            .expect("load config for LLM integration test");
        let tool_registry = Arc::new(ToolRegistry::default());

        // 4. Call execute_cron_job directly (the helper Task 4 extracts)
        let result = execute_cron_job(
            &job,
            &job_store,
            &Some(skill_registry),
            &tool_registry,
            &None,              // memory_store
            &None,              // hook_registry
            &config,
        ).await;
        assert!(result.is_ok(), "execute_cron_job failed: {:?}", result);

        // 5. Verify the stored last_status contains the token
        let guard = job_store.lock().unwrap();
        let stored = guard.get_job(&job.id).expect("job still in store");
        // last_status holds the output on success (see mark_job_run)
        let last_output = stored.last_status.as_deref().unwrap_or("");
        assert!(
            last_output.contains("SKILL-REACHED-LLM-07-3-01"),
            "D-15 violation: skill content did not reach LLM. last_status = {:?}",
            last_output
        );
        assert!(
            !last_output.contains("[Tick runner: agent execution pending full integration]"),
            "D-17 violation: placeholder still being delivered"
        );
    }

    // -------------------------------------------------------------------------
    // Task 2 (Wave 0): Hook-registry capture test (no LLM required)
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_cron_hook_registry_receives_events() {
        // D-04 / D-06 / D-07 / D-16: cron-triggered runs must fire MessageReceived + ResponseSent
        // to a shared HookRegistry with platform="cron" and non-empty chat_id. This test proves
        // the registry wiring protocol that execute_cron_job (Task 4) uses.
        use ironhermes_hooks::{HookRegistry, HookEvent, HookEventKind, HooksConfig};

        // 1. Build a HookRegistry with a capture listener (pattern copied from registry.rs tests)
        let mut registry = HookRegistry::new(HooksConfig::default());
        let captured: Arc<std::sync::Mutex<Vec<HookEvent>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let cap_clone = captured.clone();
        registry.add_listener(Arc::new(move |event: HookEvent| {
            cap_clone.lock().unwrap().push(event);
        }));
        let registry = Arc::new(registry);

        // 2. Simulate what execute_cron_job fires for a job with chat_id derived from job.id
        let chat_id = "test-job-42".to_string();
        let req_id = "test-req-42".to_string();
        registry.fire(HookEvent::new(
            &req_id,
            HookEventKind::MessageReceived {
                platform: "cron".to_string(),
                chat_id: chat_id.clone(),
                content_preview: "test cron prompt".to_string(),
            },
        ));
        registry.fire(HookEvent::new(
            &req_id,
            HookEventKind::ResponseSent {
                platform: "cron".to_string(),
                chat_id: chat_id.clone(),
                response_preview: "test cron response".to_string(),
            },
        ));

        // 3. HookRegistry::fire dispatches via tokio::spawn — give listeners 50ms to drain
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // 4. Assert both events captured with cron platform + job chat_id
        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 2, "expected 2 events, got {}: {:?}", events.len(), *events);

        // First event should be MessageReceived with platform="cron"
        match &events[0].kind {
            HookEventKind::MessageReceived { platform, chat_id: cid, .. } => {
                assert_eq!(platform, "cron", "D-12: cron events must use platform=\"cron\"");
                assert_eq!(cid, "test-job-42", "D-12: chat_id must come from Job record");
            }
            other => panic!("expected MessageReceived, got {:?}", other),
        }

        // Second event should be ResponseSent with platform="cron"
        match &events[1].kind {
            HookEventKind::ResponseSent { platform, chat_id: cid, .. } => {
                assert_eq!(platform, "cron");
                assert_eq!(cid, "test-job-42");
            }
            other => panic!("expected ResponseSent, got {:?}", other),
        }

        // Both events share the same request_id (correlation across a single cron run)
        assert_eq!(events[0].request_id, events[1].request_id);
    }

    // -------------------------------------------------------------------------
    // Task 3 (Wave 0): complete_job_run real-output persistence test
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_complete_job_run_receives_real_output() {
        // D-03 / D-14 / SCHED-04: complete_job_run persists the `output` argument verbatim.
        // This test proves the contract — Task 4 only needs to pass real LLM output instead
        // of the placeholder string "[Tick runner: agent execution pending full integration]".
        use ironhermes_cron::{JobStore, ScheduleParsed};
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let cron_dir = dir.path().join(".ironhermes/cron");
        std::fs::create_dir_all(&cron_dir).unwrap();
        let job_store = Arc::new(Mutex::new(
            JobStore::open(cron_dir).expect("job store init"),
        ));

        // Seed the store with a job
        let job = {
            let mut guard = job_store.lock().unwrap();
            guard.add_job(
                "complete_job_run test",
                "anything",
                ScheduleParsed::Interval { minutes: 1, display: "every 1 min".to_string() },
                "every 1 min",
                "cli",
                vec![],
                None,
            ).expect("insert job")
        };

        // Real output — NOT the placeholder
        let real_output = "real LLM response content (not a placeholder)";
        ironhermes_cron::complete_job_run(&job_store, &job, real_output, true)
            .await
            .expect("complete_job_run");

        // Verify persistence — on success, mark_job_run stores output in last_status
        let guard = job_store.lock().unwrap();
        let stored = guard.get_job(&job.id).expect("job present after complete");
        let last_output = stored.last_status.as_deref().unwrap_or("");
        assert_eq!(last_output, real_output, "output must persist verbatim");
        assert!(
            !last_output.contains("[Tick runner: agent execution pending full integration]"),
            "D-17: placeholder string must not appear"
        );
    }

    // -------------------------------------------------------------------------
    // Existing skill-resolution tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_resolve_skill_context_with_skills() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join(".ironhermes/skills/test-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: test-skill\ndescription: A test\n---\nDo the thing.",
        )
        .unwrap();

        let registry = ironhermes_core::SkillRegistry::load_with_paths(&[
            dir.path().join(".ironhermes/skills")
        ]);
        let result = resolve_skill_context(&registry, &["test-skill".to_string()]);
        assert!(result.contains("## Skill: test-skill"), "result: {result}");
        assert!(result.contains("Do the thing."), "result: {result}");
    }

    #[test]
    fn test_resolve_skill_context_missing_skill() {
        let dir = tempfile::tempdir().unwrap();
        let registry = ironhermes_core::SkillRegistry::load_with_paths(&[
            dir.path().join("no-skills-here")
        ]);
        let result = resolve_skill_context(&registry, &["nonexistent".to_string()]);
        assert!(result.is_empty(), "result should be empty: {result}");
    }

    #[test]
    fn test_resolve_skill_context_mixed() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("skills/real-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: real-skill\ndescription: Real\n---\nReal content.",
        )
        .unwrap();

        let registry = ironhermes_core::SkillRegistry::load_with_paths(&[
            dir.path().join("skills")
        ]);
        let result = resolve_skill_context(
            &registry,
            &["real-skill".to_string(), "fake-skill".to_string()],
        );
        assert!(result.contains("Real content."), "result: {result}");
        assert!(!result.contains("fake-skill"), "result: {result}");
    }
}

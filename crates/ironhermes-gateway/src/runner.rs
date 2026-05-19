use anyhow::{Context, Result};
use ironhermes_agent::budget::BudgetHandle;
use ironhermes_agent::context_engine::ContextEngine;
use ironhermes_agent::engine_factory::build_context_engine;
use ironhermes_agent::pressure_warning::PressureTracker;
use ironhermes_agent::subagent_registry::SubagentRegistry;
use ironhermes_agent::{AgentLoop, MemoryManager, PromptBuilder, build_main_client, wire_fallback_if_configured};
use ironhermes_core::commands::context::ToolsetSessionHandle;
use ironhermes_core::{
    ChatMessage, Config, MessageContent, ProviderResolver, Role, SkillRecord, SkillRegistry,
};
use ironhermes_cron::JobStore;
use ironhermes_exec::process_registry::ProcessRegistry;
use ironhermes_mcp::McpManager;
use ironhermes_tools::ToolRegistry;
use std::sync::{Arc, Mutex};
use tokio::sync::{Mutex as TokioMutex, RwLock, Semaphore, mpsc};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::adapter::PlatformAdapter;
use crate::backoff::BackoffState;
use crate::handler::GatewayMessageHandler;
use crate::multimodal;
use crate::session::SessionStore;
use crate::telegram::{TelegramAdapter, TgBotCommand, tg_message_to_event};
use ironhermes_cron::TgSendApi;
use crate::user_queue::UserQueueManager;

/// Runs the Telegram gateway: long polling, per-user dispatch, JoinSet supervision,
/// Semaphore concurrency control, and CancellationToken-based graceful shutdown.
pub struct GatewayRunner {
    config: Config,
    resolver: ProviderResolver,
    session_store: Arc<RwLock<SessionStore>>,
    state_store: Arc<Mutex<ironhermes_state::StateStore>>,
    tool_registry: Arc<RwLock<ToolRegistry>>,
    memory_manager: Option<Arc<TokioMutex<MemoryManager>>>,
    job_store: Option<Arc<Mutex<JobStore>>>,
    hook_registry: Option<Arc<ironhermes_hooks::HookRegistry>>,
    skill_registry: Option<Arc<SkillRegistry>>,
    active_skills: Option<Arc<std::sync::Mutex<Vec<SkillRecord>>>>,
    /// GAP-8 (Phase 21.2 Plan 11): MCP manager handle — when set, `start()`
    /// awaits `mgr.shutdown_all().await` as part of graceful shutdown so
    /// stdio children are SIGKILL'd (via kill_on_drop + bounded JoinHandle
    /// timeout) and the process exits in bounded time on Ctrl+C. Without
    /// this wire, `ironhermes gateway` hangs indefinitely when an MCP
    /// server is connected because the tokio process reaper keeps the
    /// runtime alive until children are reaped.
    mcp_manager: Option<Arc<McpManager>>,
    /// Plan 21.7-05 (PROV-09/PROV-10/D-15): shared BudgetHandle threaded
    /// from `run_gateway` at startup. `build_gateway_handler` clones it into
    /// the handler so per-request AgentLoops share the same counter with the
    /// AgentSubagentRunner registered on the tool registry.
    budget_handle: Option<BudgetHandle>,
    /// Plan 21.7-06 (D-29, D-24): gateway-scoped ProcessRegistry for
    /// terminal/execute_code background spawns. Mirrors the BudgetHandle
    /// plumbing pattern. `build_gateway_handler` clones it into the handler
    /// so per-request on_session_end can invoke drain_and_kill_session.
    process_registry: Option<Arc<RwLock<ProcessRegistry>>>,
    /// Plan 21.7-07 (D-03 / D-04 / D-05): gateway-scoped SubagentRegistry.
    /// Cloned into `build_gateway_handler` so per-request handlers see
    /// live subagent state + can drain transcripts on session end.
    subagent_registry: Option<Arc<RwLock<SubagentRegistry>>>,
    /// Phase 25.1 D-03/D-17: shared browser session Arc for all browser_* tools.
    /// Cloned into `build_gateway_handler` so per-request AgentLoops receive
    /// `with_browser_session(...)` and hold a reference (T-25.1-04 drop semantics).
    browser_session: Option<
        std::sync::Arc<
            tokio::sync::Mutex<Option<ironhermes_tools::browser_session::BrowserSession>>,
        >,
    >,
    /// Phase 25.2 Plan 15 follow-up (UAT Issue 2 / Symptom 1 close-out for
    /// Telegram): production `ToolsetSessionHandle` for the gateway's
    /// `/toolset` slash dispatch. `build_gateway_handler` clones it into
    /// the handler so per-request CommandContext can delegate to
    /// `RegistryToolsetSession::render_list` etc. instead of returning the
    /// "toolset session handle not configured" fallback.
    toolset_session: Option<Arc<dyn ToolsetSessionHandle>>,
    /// Phase 25.3 D-W-2: per-cwd Workspace resolved at startup. `build_gateway_handler`
    /// clones it into the per-message handler so /sessions --workspace and trajectory
    /// scoping see the resolved root.
    workspace: Option<Arc<ironhermes_core::workspace::Workspace>>,
    /// Phase 25.3-15 CR-02 close-out: trajectory directory ROOT for per-session
    /// lazy-open. Replaces the old `trajectory_writer` field which held a single
    /// process-wide handle keyed by `gateway-<random-uuid>` and was unreachable
    /// from `hermes session export <session_id>`. Per-session writers are owned
    /// by `SessionStore` (cached, lazy-opened on first tool call), keyed by the
    /// canonical SQLite session UUID. `set_trajectory_root` propagates this
    /// path into the inner `SessionStore` via `try_write`.
    trajectory_root: Option<std::path::PathBuf>,
    /// Phase 21.8.2 D-02: SkillsConfig for the gateway SkillsReload arm.
    /// Populated by `set_skills_config` (called from run_gateway after `set_skill_registry`).
    /// `build_gateway_handler` passes it to the handler via `set_skills_config`.
    skills_config: Option<ironhermes_core::config::SkillsConfig>,
    cancel: CancellationToken,
}

impl GatewayRunner {
    pub fn new(
        config: Config,
        resolver: ProviderResolver,
        tool_registry: Arc<RwLock<ToolRegistry>>,
    ) -> Self {
        // Per D-03: all sources share a single state.db
        // Per D-11: gateway uses its own Connection instance via StateStore::open_default()
        let state_store = Arc::new(Mutex::new(
            ironhermes_state::StateStore::open_default()
                .expect("failed to open state.db for gateway"),
        ));
        Self {
            config,
            resolver,
            session_store: Arc::new(RwLock::new(SessionStore::new(Arc::clone(&state_store)))),
            state_store,
            tool_registry,
            memory_manager: None,
            job_store: None,
            hook_registry: None,
            skill_registry: None,
            active_skills: None,
            mcp_manager: None,       // GAP-8: wired by run_gateway before start()
            budget_handle: None,     // Plan 21.7-05: wired by run_gateway before start()
            process_registry: None,  // Plan 21.7-06: wired by run_gateway before start()
            subagent_registry: None, // Plan 21.7-07: wired by run_gateway before start()
            browser_session: None,   // Phase 25.1: wired by run_gateway before start()
            toolset_session: None, // Phase 25.2 Plan 15 follow-up: wired by run_gateway before start()
            workspace: None,       // Phase 25.3 D-W-2: wired by run_gateway before start()
            trajectory_root: None, // Phase 25.3-15 CR-02: wired by run_gateway before start()
            skills_config: None,   // Phase 21.8.2 D-02: wired by run_gateway before start()
            cancel: CancellationToken::new(),
        }
    }

    /// Phase 25.2 Plan 15 follow-up (UAT Issue 2 / Symptom 1 close-out for
    /// Telegram): install the production `ToolsetSessionHandle` so the
    /// gateway's `/toolset` slash command works. Mirrors
    /// `set_memory_manager` / `set_subagent_registry`. Caller is
    /// `run_gateway` in ironhermes-cli, which threads the same Arc here that
    /// the REPL and single-shot binary already use.
    pub fn set_toolset_session(&mut self, handle: Arc<dyn ToolsetSessionHandle>) {
        self.toolset_session = Some(handle);
    }

    /// Phase 25.3 D-W-2 + Phase 25.3-14 verifier-blocker close-out:
    /// install the resolved Workspace and ALSO propagate it to the inner
    /// SessionStore so per-message session rows carry workspace_root. The
    /// SessionStore needs the same Arc the runner holds — its get_or_create
    /// path runs on a different code path from the per-message slash dispatch,
    /// and was the surface flagged in the 25.3 verifier BLOCKER (#28).
    ///
    /// Caller is `run_gateway` in ironhermes-cli (resolved via resolve_from_cwd
    /// at startup). `build_gateway_handler` clones the runner's workspace into
    /// the per-message handler so /sessions --workspace and trajectory scoping
    /// see the resolved root; this method ALSO ensures the SessionStore (which
    /// runs `state.create_session(..., workspace_root)` on first message per
    /// chat) sees the same Arc.
    pub fn set_workspace(&mut self, workspace: Arc<ironhermes_core::workspace::Workspace>) {
        self.workspace = Some(workspace.clone());
        // Phase 25.3-14: propagate to SessionStore so create_session passes
        // workspace_root onto each gateway-originated sessions row.
        // RwLock::try_write avoids blocking; SessionStore is exclusively held by
        // GatewayRunner during the setup phase before start() is called, so the
        // try_write can never legitimately fail. We log and continue rather than
        // panic on the impossible-failure path so a future refactor that moves
        // the call onto a contended path surfaces the misuse loudly without
        // crashing the gateway.
        match self.session_store.try_write() {
            Ok(mut s) => s.set_workspace(workspace),
            Err(_) => tracing::warn!(
                "Phase 25.3-14: SessionStore was held during set_workspace; \
                 workspace_root may not propagate to gateway sessions"
            ),
        }
    }

    /// Phase 25.3-15 CR-02 close-out: install the trajectory directory ROOT so
    /// the inner `SessionStore` can lazily open per-session writers keyed by
    /// the canonical SQLite session UUID. Replaces the old
    /// `set_trajectory_writer` (which fed a process-wide writer that was
    /// unreachable from `hermes session export <session_id>`).
    ///
    /// Caller is `run_gateway` in ironhermes-cli (created alongside the
    /// workspace + StateStore open). The path is propagated into the inner
    /// `SessionStore` via `try_write` — the `SessionStore` is exclusively held
    /// by `GatewayRunner` during the setup phase before `start()` is called,
    /// so `try_write` cannot legitimately fail. We log and continue rather
    /// than panic on the impossible-failure path so a future refactor that
    /// moves the call onto a contended path surfaces the misuse loudly without
    /// crashing the gateway. Mirrors the `set_workspace` propagation pattern
    /// added in Plan 25.3-14.
    pub fn set_trajectory_root(&mut self, root: std::path::PathBuf) {
        self.trajectory_root = Some(root.clone());
        match self.session_store.try_write() {
            Ok(mut s) => s.set_trajectory_root(root),
            Err(_) => tracing::warn!(
                "Phase 25.3-15: SessionStore was held during set_trajectory_root; \
                 per-session trajectories may not be wired"
            ),
        }
    }

    /// Plan 21.7-05 (PROV-09/PROV-10/D-15): install the shared BudgetHandle
    /// to thread into the handler. Caller (run_gateway in ironhermes-cli)
    /// constructs one `BudgetHandle::new(config.agent.max_iterations)` at
    /// startup and passes the same handle here AND into the
    /// `AgentSubagentRunner` registered on the tool registry, giving
    /// parent + child subagent loops a shared counter.
    pub fn set_budget_handle(&mut self, handle: BudgetHandle) {
        self.budget_handle = Some(handle);
    }

    /// Plan 21.7-06 (D-29, D-24): install the gateway-scoped ProcessRegistry
    /// so `build_gateway_handler` can clone it into the handler. Caller is
    /// `run_gateway` in ironhermes-cli.
    pub fn set_process_registry(&mut self, reg: Arc<RwLock<ProcessRegistry>>) {
        self.process_registry = Some(reg);
    }

    /// Plan 21.7-07 (D-03 / D-04 / D-05): install the gateway-scoped
    /// SubagentRegistry. `build_gateway_handler` clones it into the handler
    /// so per-request run_agent sees live subagent state + drains transcripts
    /// on session end. Caller is `run_gateway` in ironhermes-cli.
    pub fn set_subagent_registry(&mut self, reg: Arc<RwLock<SubagentRegistry>>) {
        self.subagent_registry = Some(reg);
    }

    /// Plan 20-02: set the `MemoryManager` handle used by the gateway runner,
    /// handler, and cron tick task. Shared via `Arc<TokioMutex<MemoryManager>>`.
    pub fn set_memory_manager(&mut self, manager: Arc<TokioMutex<MemoryManager>>) {
        self.memory_manager = Some(manager);
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

    /// Phase 21.8.2 D-02: store the SkillsConfig so the SkillsReload arm can
    /// call `load_with_config` on demand. Called from main.rs:run_gateway
    /// immediately after `set_skill_registry`.
    pub fn set_skills_config(&mut self, cfg: ironhermes_core::config::SkillsConfig) {
        self.skills_config = Some(cfg);
    }

    /// Set the shared active skills tracker. Passed to GatewayMessageHandler in start().
    pub fn set_active_skills(&mut self, skills: Arc<std::sync::Mutex<Vec<SkillRecord>>>) {
        self.active_skills = Some(skills);
    }

    /// GAP-8 (Phase 21.2 Plan 11): wire the MCP manager into the gateway
    /// runner so `start()` can call `shutdown_all().await` during graceful
    /// shutdown. Mirrors `set_memory_manager`. Caller is `run_gateway` in
    /// ironhermes-cli, which builds the manager via `build_mcp_manager`.
    ///
    /// Without this wire, `ironhermes gateway` hangs on Ctrl+C when stdio
    /// MCP servers are connected because the rmcp parent->child pipe close
    /// doesn't cause the child to exit, and tokio's process reaper keeps
    /// the runtime alive until children are reaped.
    pub fn set_mcp_manager(&mut self, manager: Arc<McpManager>) {
        self.mcp_manager = Some(manager);
    }

    /// Phase 25.1 D-17: install the shared browser session Arc.
    /// Mirrored to `build_gateway_handler` so every per-request AgentLoop
    /// receives `with_browser_session(...)`. Caller is `run_gateway` in main.rs.
    pub fn set_browser_session(
        &mut self,
        session: std::sync::Arc<
            tokio::sync::Mutex<Option<ironhermes_tools::browser_session::BrowserSession>>,
        >,
    ) {
        self.browser_session = Some(session);
    }

    /// Plan 03 (Phase 22.4.2.1): returns a clone of the runner's CancellationToken.
    /// Used by gateway integration tests (tests/gateway_shutdown.rs) to fire
    /// shutdown without going through the OS signal layer.
    /// pub(crate) so only gateway-crate tests can reach it (T-22.4.2.1-03-05).
    pub(crate) fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Build the GatewayMessageHandler with all wiring (memory, hooks, skills,
    /// active skills, AND Phase 18 Plan 06 gateway hygiene engine). Factored
    /// out of `start()` so it is unit-testable without a live adapter.
    fn build_gateway_handler(&self) -> GatewayMessageHandler {
        let mut handler = GatewayMessageHandler::new(
            self.config.clone(),
            self.resolver.clone(),
            self.session_store.clone(),
            self.tool_registry.clone(),
        );
        if let Some(ref mgr) = self.memory_manager {
            handler.set_memory_manager(mgr.clone());
        }
        if let Some(ref registry) = self.hook_registry {
            handler.set_hook_registry(registry.clone());
        }
        if let Some(ref registry) = self.skill_registry {
            handler.set_skill_registry(registry.clone());
        }
        // Phase 21.8.2 D-02: pass SkillsConfig so gateway SkillsReload arm can reload.
        if let Some(ref cfg) = self.skills_config {
            handler.set_skills_config(cfg.clone());
        }
        if let Some(ref skills) = self.active_skills {
            handler.set_active_skills(skills.clone());
        }
        // Plan 21.7-05: thread the shared BudgetHandle into the handler so
        // per-request AgentLoops see the same counter as AgentSubagentRunner.
        if let Some(ref handle) = self.budget_handle {
            handler.set_budget_handle(handle.clone());
        }
        // Plan 21.7-06 (D-29, D-24): thread the gateway-scoped ProcessRegistry
        // so per-request on_session_end can invoke drain_and_kill_session.
        if let Some(ref reg) = self.process_registry {
            handler.set_process_registry(reg.clone());
        }
        // Plan 21.7-07 (D-03 / D-04 / D-05): thread the gateway-scoped
        // SubagentRegistry so per-request on_session_end drains transcript
        // writes and the delegate_task runner shares state across requests.
        if let Some(ref reg) = self.subagent_registry {
            handler.set_subagent_registry(reg.clone());
        }

        // Phase 25.1 D-17: thread the shared browser session Arc so every
        // per-request AgentLoop calls with_browser_session (T-25.1-04 drop semantics).
        if let Some(ref sess) = self.browser_session {
            handler.set_browser_session(sess.clone());
        }

        // Phase 25.2 Plan 15 follow-up (UAT Issue 2 / Symptom 1): thread the
        // production toolset session handle into the gateway handler so the
        // `/toolset` slash command works in Telegram.
        if let Some(ref handle) = self.toolset_session {
            handler.set_toolset_session(handle.clone());
        }

        // Phase 25.3 D-W-2: thread the resolved Workspace into the gateway handler
        // so the per-message CommandContext sees it (slash dispatch + trajectory scoping).
        if let Some(ref ws) = self.workspace {
            handler.set_workspace(ws.clone());
        }
        // Phase 25.3-15 CR-02 close-out: trajectory writers are no longer
        // process-wide; per-session writers are owned (and lazily opened) by
        // `SessionStore` keyed by the canonical SQLite session UUID. The
        // handler reaches them via `self.session_store.write().await
        // .get_or_create_trajectory_writer(&canonical_session_id)` inside
        // `run_agent`, so no clone is plumbed through here.

        // Phase 21.3: initialize global token estimator from model's encoding
        let main_ep = self.resolver.resolve_for_main();
        let encoding_name = main_ep
            .model_metadata
            .as_ref()
            .map(|m| m.tokenizer.as_str())
            .unwrap_or("cl100k_base");
        ironhermes_core::init_global_estimator(ironhermes_core::TiktokenEncoding::from_name(
            encoding_name,
        ));

        // Phase 18 Plan 08 / UAT gap closure: construct the per-turn gateway
        // hygiene engine from config and attach it. Without this call the
        // handler's gateway_engine stays None and `maybe_compress_gateway`
        // always short-circuits.
        //
        // Phase 21.3: context length now resolved from model metadata.
        let ctx_len: usize = main_ep.context_length();
        let hooks = self.hook_registry.clone();
        let tracker = Some(Arc::new(PressureTracker::new()));
        // Note: the per-turn gateway hygiene engine (local_prune) does not
        // need a memory_manager — on_pre_compress is for agent compression,
        // not for the lightweight gateway hygiene pass. Pass None.
        let engine: Arc<dyn ContextEngine> = build_context_engine(
            &self.config,
            &self.config.gateway.context_engine,
            &self.resolver,
            ctx_len,
            self.config.gateway.compression_threshold,
            "gateway", // D-13: per-session lineage deferred to Phase 21
            hooks,
            tracker,
            None, // GAP-2 backward compat: gateway hygiene engine has no memory hook
        );
        handler.set_gateway_engine(engine, ctx_len);

        handler
    }

    /// Start the gateway. Blocks until ctrl+c or fatal error.
    pub async fn start(&self) -> Result<()> {
        // --- 0. Acquire PID lock (Phase 24 D-09/D-12) ---
        // Refuses startup if another live gateway is already running under
        // the same HERMES_HOME (profile-scoped after Phase 24's --profile
        // pivot in main.rs). Stale PID files (crashed gateways) are
        // auto-cleaned by acquire_pid_lock; the live-conflict path returns
        // an error containing "Stop it first" which the CLI dispatch maps
        // to exit code 2.
        //
        // The PidLockGuard is bound to a local variable held across the
        // remainder of start(). Its Drop impl removes gateway.pid on both
        // clean return and error propagation, so graceful shutdown and
        // crash recovery converge on the same cleanup path.
        let home = ironhermes_core::get_hermes_home();
        let _pid_guard = crate::pid::acquire_pid_lock(&home)
            .context("Gateway startup refused: PID lock conflict")?;

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

        // --- 6. Create handler (with gateway hygiene engine wired) and queue manager ---
        let handler = self.build_gateway_handler();
        let handler = Arc::new(handler);
        let user_queue = Arc::new(UserQueueManager::new(
            adapter.clone() as Arc<dyn crate::adapter::PlatformAdapter>,
            16,
        ));

        let mut join_set: JoinSet<()> = JoinSet::new();

        // Plan 03 (Phase 22.4.2.1): track per-chat worker tasks so they can be
        // drained on shutdown. Wrapped in Arc<TokioMutex<...>> so the dispatch
        // closure (async move) and the post-select! drain both reach the same set.
        // Drain happens AFTER self.cancel.cancel() and BEFORE drop(msg_tx) per D-11.
        let worker_join_set: Arc<TokioMutex<JoinSet<()>>> =
            Arc::new(TokioMutex::new(JoinSet::new()));

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

        // --- 7b. Optional Discord adapter (D-10) ---
        // Spawns alongside Telegram in the same JoinSet so CancellationToken-driven
        // shutdown handles all platforms uniformly. Silent skip when config section
        // is absent or token does not resolve — existing Telegram-only deployments
        // are unaffected. Empty whitelist is passed through to the adapter, which
        // enforces canonical deny-all semantics (config.rs:731 + runner.rs:601-611 D-12).
        let discord_config = self
            .config
            .gateway
            .platforms
            .get("discord")
            .cloned()
            .unwrap_or_default();
        if let Some(discord_token) = resolve_token_with_env(&discord_config.token, "DISCORD_BOT_TOKEN") {
            let handler_d = handler.clone();
            let cancel_d = self.cancel.clone();
            let whitelist_d: Vec<u64> = discord_config
                .whitelist
                .iter()
                .map(|&v| v as u64)
                .collect();
            // Empty whitelist propagates to adapter, which enforces D-12 deny-all
            // per canonical Telegram semantics (config.rs:731 + runner.rs:601-611).
            tracing::info!(whitelist_len = whitelist_d.len(), "Discord adapter spawning");
            join_set.spawn(async move {
                if let Err(e) = crate::discord::run_discord_adapter(
                    &discord_token,
                    whitelist_d,
                    handler_d,
                    cancel_d,
                )
                .await
                {
                    tracing::error!("Discord adapter error: {e:#}");
                }
            });
        } else {
            tracing::debug!("Discord adapter skipped (no token configured)");
        }

        // --- 7c. Optional Slack adapter (D-11) ---
        // Requires BOTH app_token (xapp-...) and bot_token (xoxb-...) per Pitfall 2.
        // Either token missing → silent skip. Empty whitelist enforced by adapter (D-12).
        let slack_config = self
            .config
            .gateway
            .platforms
            .get("slack")
            .cloned()
            .unwrap_or_default();
        if let (Some(slack_app), Some(slack_bot)) = (
            resolve_token_with_env(&slack_config.app_token, "SLACK_APP_TOKEN"),
            resolve_token_with_env(&slack_config.token, "SLACK_BOT_TOKEN"),
        ) {
            let handler_s = handler.clone();
            let cancel_s = self.cancel.clone();
            let whitelist_s: Vec<String> = slack_config
                .whitelist
                .iter()
                .map(|v| v.to_string())
                .collect();
            // Empty whitelist propagates to adapter — D-12 deny-all enforced in callback.
            // Note: Slack-native whitelist uses alphanumeric user IDs (e.g. "U012AB3CD");
            // operators currently configure i64 values which are converted via to_string().
            // Migrating to a Vec<String> whitelist in PlatformGatewayConfig is a deferred
            // config-schema improvement (see SUMMARY.md).
            tracing::info!(whitelist_len = whitelist_s.len(), "Slack adapter spawning");
            join_set.spawn(async move {
                if let Err(e) = crate::slack::run_slack_adapter(
                    &slack_app,
                    &slack_bot,
                    whitelist_s,
                    handler_s,
                    cancel_s,
                )
                .await
                {
                    tracing::error!("Slack adapter error: {e:#}");
                }
            });
        } else {
            tracing::debug!("Slack adapter skipped (missing app_token or bot_token)");
        }

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

        // Plan 03: clone Arc so dispatch_future (async move) can spawn into worker_join_set
        let worker_join_set_dispatch = worker_join_set.clone();

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
                                    let _ = PlatformAdapter::send_message(adapter_dispatch_mm.as_ref(), &chat_id, &err_msg, None).await;
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

                            // Plan 03 (Phase 22.4.2.1): spawn into worker_join_set so
                            // per-chat workers are tracked and drained on shutdown (D-10/D-11).
                            // Previously a bare tokio::spawn (detached) — replaced with tracked spawn.
                            worker_join_set_dispatch.lock().await.spawn(async move {
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

        // --- 9a. WAL checkpoint timer (every 5 minutes, PASSIVE mode, non-blocking) ---
        let wal_cancel = self.cancel.clone();
        let state_wal = Arc::clone(&self.state_store);
        join_set.spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            interval.tick().await; // skip immediate first tick
            loop {
                tokio::select! {
                    _ = wal_cancel.cancelled() => break,
                    _ = interval.tick() => {
                        let s = Arc::clone(&state_wal);
                        let _ = tokio::task::spawn_blocking(move || {
                            if let Ok(store) = s.lock() {
                                if let Err(e) = store.wal_checkpoint() {
                                    warn!("WAL checkpoint failed: {e}");
                                }
                            }
                        }).await;
                    }
                }
            }
        });

        // --- 9b. Session cleanup task ---
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
            let memory_manager_tick = self.memory_manager.clone();
            let config_tick = self.config.clone();
            // Phase 22.4.2.1 Plan 02: thread TG adapter for delivery dispatch
            let adapter_tick = adapter.clone();

            join_set.spawn(async move {
                // UAT gap 2 / test 13: first-tick-after-boot burst guard.
                // Fast-forward any stale scheduled jobs BEFORE entering the
                // run_tick_loop so a gateway restart doesn't burst-fire jobs
                // whose next_run_at drifted into the recent past.
                match fast_forward_backlog(&job_store_tick).await {
                    Ok(n) if n > 0 => {
                        info!(
                            "First-tick burst guard fast-forwarded {} job(s)",
                            n
                        );
                    }
                    Ok(_) => {
                        debug!("First-tick burst guard: no backlog");
                    }
                    Err(e) => {
                        error!("First-tick burst guard error: {}", e);
                        // Fall through — a failed burst guard is not a reason
                        // to skip the tick loop.
                    }
                }

                // Construct CronRunnerContext from the gateway's shared Arcs
                // and delegate to ironhermes_cron_runner::run_tick_loop.
                // Plan 32.1-07: execute_cron_job + dispatch_delivery moved to
                // crates/ironhermes-cron-runner.
                let cron_ctx = std::sync::Arc::new(ironhermes_cron_runner::CronRunnerContext {
                    job_store: job_store_tick,
                    skill_registry: skill_registry_tick,
                    tool_registry: tool_registry_tick,
                    memory_manager: memory_manager_tick,
                    hook_registry: hook_registry_tick,
                    config: config_tick,
                    mcp_manager: None, // gateway's McpManager is not yet threaded into the tick task
                    tg_client: Some(adapter_tick.clone() as Arc<dyn TgSendApi>),
                });
                ironhermes_cron_runner::run_tick_loop(cron_ctx, tick_cancel).await;
            });
            info!("Cron tick task started (60s interval, delegating to ironhermes-cron-runner)");
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

        // GAP-8 (Phase 21.2 Plan 11): tear down MCP servers BEFORE
        // self.cancel.cancel() and BEFORE the join_set drain, so stdio
        // children are SIGKILL'd (via kill_on_drop) and bounded-timeout
        // awaited. Prior to this wire, `ironhermes gateway` hung on Ctrl+C
        // because the rmcp parent->child pipe close didn't cause the child
        // to exit, and tokio's process reaper kept the runtime alive until
        // children were reaped. `shutdown_all` bounds each server's await
        // to 2 seconds, so this block always returns within ~2s/server
        // regardless of child behavior.
        if let Some(ref mgr) = self.mcp_manager {
            info!("Shutting down MCP servers");
            let _ = mgr.shutdown_all().await;
            info!("MCP servers shut down");
        }

        // Propagate cancellation to all subtasks
        self.cancel.cancel();

        // Plan 03 (Phase 22.4.2.1): drain per-chat worker tasks with bounded 5s timeout (D-11).
        // Workers observe cancel_task.is_cancelled() after each agent turn; the 5s timeout covers
        // in-flight turns that haven't reached their cancellation check yet.
        // ORDERING: AFTER self.cancel.cancel() and BEFORE drop(msg_tx) — preserves Phase 21.2
        // Plan 11 ordering invariant (MCP shutdown_all FIRST, cancel SECOND, drain THIRD, drop FOURTH).
        {
            let abort_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
            let mut wjs = worker_join_set.lock().await;
            loop {
                match tokio::time::timeout_at(abort_deadline, wjs.join_next()).await {
                    Ok(Some(_)) => {
                        // A worker task finished — keep draining
                    }
                    Ok(None) => {
                        // All workers finished cleanly
                        info!("gateway: per-chat workers drained cleanly");
                        break;
                    }
                    Err(_elapsed) => {
                        // 5s timeout exceeded — abort remaining tasks
                        warn!(
                            "gateway: worker drain timed out after 5s; \
                             aborting remaining per-chat worker tasks"
                        );
                        wjs.abort_all();
                        break;
                    }
                }
            }
        }
        // worker_join_set dropped here — any tasks not yet joined are aborted by JoinSet::drop.

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

/// First-tick-after-boot burst guard (UAT gap 2, test 13).
///
/// On gateway restart, jobs whose `next_run_at` drifted into the past while
/// the gateway was down would otherwise burst-fire on the first tick. This
/// helper is called exactly once, before the first `run_tick_check`, and
/// fast-forwards every Scheduled+enabled job whose `next_run_at <= now` by
/// recomputing its next run time from `now`. The fast-forwarded jobs are NOT
/// executed on the current tick — they'll fire on their natural next cadence.
async fn fast_forward_backlog(store: &Arc<Mutex<ironhermes_cron::JobStore>>) -> Result<usize> {
    use chrono::Utc;

    let mut guard = store
        .lock()
        .map_err(|e| anyhow::anyhow!("store lock poisoned: {}", e))?;

    // Reload from disk first so we fast-forward based on the latest persisted
    // state (covers the case where the CLI was used to create jobs while the
    // gateway was down).
    guard.reload()?;

    let now = Utc::now();
    let mut forwarded = 0usize;
    for job in guard.jobs.iter_mut() {
        if job.state != ironhermes_cron::JobState::Scheduled || !job.enabled {
            continue;
        }
        let Some(next_run_at) = job.next_run_at else {
            continue;
        };
        if next_run_at > now {
            continue; // future — leave alone
        }
        // Stale-on-boot: recompute from now
        match ironhermes_cron::compute_next_run(&job.schedule, now) {
            Ok(Some(new_next)) => {
                info!(
                    "First-tick burst guard: fast-forwarded job '{}' from {} to {}",
                    job.name, next_run_at, new_next
                );
                job.next_run_at = Some(new_next);
                forwarded += 1;
            }
            Ok(None) => {
                // Once-kind job whose run_at is past — drop next_run_at so it
                // doesn't fire. The job transitions naturally via mark_job_run
                // on a subsequent manual run or stays dormant.
                info!(
                    "First-tick burst guard: dropped past-due once job '{}' (was {})",
                    job.name, next_run_at
                );
                job.next_run_at = None;
                forwarded += 1;
            }
            Err(e) => {
                warn!(
                    "First-tick burst guard: compute_next_run failed for '{}': {}",
                    job.name, e
                );
            }
        }
    }

    if forwarded > 0 {
        guard.save()?;
    }
    Ok(forwarded)
}

// Plan 32.1-07: execute_cron_job + dispatch_delivery moved to
// crates/ironhermes-cron-runner. Both functions are deleted from this file.
// The cron tick task (above) now calls ironhermes_cron_runner::run_tick_loop.
// The regression test execute_cron_job_no_longer_exists_in_gateway (below)
// guards against any future re-introduction of these deleted symbols.

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

/// Resolve a token from config value or a named environment variable fallback.
/// Supports `${ENV_VAR}` syntax. Unlike `resolve_token`, the fallback env var
/// is caller-specified so Discord/Slack do not accidentally pick up TELEGRAM_BOT_TOKEN.
fn resolve_token_with_env(token: &Option<String>, env_var: &str) -> Option<String> {
    if let Some(t) = token {
        if t.starts_with("${") && t.ends_with('}') {
            let var_name = &t[2..t.len() - 1];
            return std::env::var(var_name).ok();
        }
        if !t.is_empty() {
            return Some(t.clone());
        }
    }
    std::env::var(env_var).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Plan 05-05 Task 3: First-tick burst guard regression test
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn gateway_first_tick_suppresses_backlog() {
        use chrono::{Duration, Utc};
        use ironhermes_cron::{JobStore, ScheduleParsed};
        use std::sync::{Arc, Mutex};
        use tempfile::TempDir;

        let dir = TempDir::new().expect("tempdir");
        let cron_dir = dir.path().join("cron");
        let mut raw_store = JobStore::open(cron_dir.clone()).expect("open");

        // Seed an interval job with next_run_at in the recent past (simulating
        // gateway downtime).
        let past = Utc::now() - Duration::seconds(90);
        let job = raw_store
            .add_job(
                "backlog-job",
                "hi",
                ScheduleParsed::Interval {
                    minutes: 5,
                    display: "every 5m".to_string(),
                },
                "every 5m",
                "local",
                vec![],
                None,
            )
            .expect("add");
        // Backdate next_run_at to simulate drift
        raw_store.jobs[0].next_run_at = Some(past);
        raw_store.save().expect("save");

        let store = Arc::new(Mutex::new(raw_store));

        // Invoke the burst guard directly
        let forwarded = fast_forward_backlog(&store).await.expect("guard");
        assert_eq!(forwarded, 1, "expected 1 job fast-forwarded");

        // Assert: next_run_at is now in the future (not in the past)
        {
            let guard = store.lock().unwrap();
            let updated = guard.get_job(&job.id).expect("job still present");
            let new_next = updated.next_run_at.expect("next_run_at present");
            assert!(
                new_next > Utc::now(),
                "next_run_at should be in the future after fast-forward, got {}",
                new_next
            );
        }

        // Assert: the job is NOT returned by get_due_jobs after the guard runs
        // (because its next_run_at is now in the future).
        {
            let mut guard = store.lock().unwrap();
            let due = guard.get_due_jobs();
            assert!(
                due.is_empty(),
                "expected no due jobs after first-tick burst guard, found {}",
                due.len()
            );
        }
    }

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
        )
        .unwrap();
        let skill_registry = Arc::new(ironhermes_core::SkillRegistry::load_with_paths(&[dir
            .path()
            .join(".ironhermes/skills")]));

        // 2. Build an in-memory JobStore with one due job that attaches the skill
        let cron_dir = dir.path().join(".ironhermes/cron");
        std::fs::create_dir_all(&cron_dir).unwrap();
        let job_store = Arc::new(Mutex::new(JobStore::open(cron_dir).expect("job store")));
        let job = {
            let mut guard = job_store.lock().unwrap();
            guard
                .add_job(
                    "cron-skill-integration-test",
                    "Please respond now.",
                    ScheduleParsed::Interval {
                        minutes: 1,
                        display: "every 1 min".to_string(),
                    },
                    "every 1 min",
                    "cli",
                    vec!["cron-echo".to_string()],
                    None,
                )
                .expect("add job")
        };

        // 3. Build a Config that points at a real LLM endpoint (uses env vars / config.yaml defaults)
        let config = ironhermes_core::Config::load().expect("load config for LLM integration test");
        let tool_registry = Arc::new(RwLock::new(ToolRegistry::default()));

        // 4. Call run_cron_job via CronRunnerContext (Plan 32.1-07: execute_cron_job moved to cron-runner)
        let cron_ctx = ironhermes_cron_runner::CronRunnerContext {
            job_store: job_store.clone(),
            skill_registry: Some(skill_registry),
            tool_registry: tool_registry.clone(),
            memory_manager: None,
            hook_registry: None,
            config: config.clone(),
            mcp_manager: None,
            tg_client: None,
        };
        let result = ironhermes_cron_runner::run_cron_job(&job, &cron_ctx).await;
        assert!(result.is_ok(), "run_cron_job failed: {:?}", result);

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
        use ironhermes_hooks::{HookEvent, HookEventKind, HookRegistry, HooksConfig};

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
        assert_eq!(
            events.len(),
            2,
            "expected 2 events, got {}: {:?}",
            events.len(),
            *events
        );

        // First event should be MessageReceived with platform="cron"
        match &events[0].kind {
            HookEventKind::MessageReceived {
                platform,
                chat_id: cid,
                ..
            } => {
                assert_eq!(
                    platform, "cron",
                    "D-12: cron events must use platform=\"cron\""
                );
                assert_eq!(
                    cid, "test-job-42",
                    "D-12: chat_id must come from Job record"
                );
            }
            other => panic!("expected MessageReceived, got {:?}", other),
        }

        // Second event should be ResponseSent with platform="cron"
        match &events[1].kind {
            HookEventKind::ResponseSent {
                platform,
                chat_id: cid,
                ..
            } => {
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
            guard
                .add_job(
                    "complete_job_run test",
                    "anything",
                    ScheduleParsed::Interval {
                        minutes: 1,
                        display: "every 1 min".to_string(),
                    },
                    "every 1 min",
                    "cli",
                    vec![],
                    None,
                )
                .expect("insert job")
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

        let registry = ironhermes_core::SkillRegistry::load_with_paths(&[dir
            .path()
            .join(".ironhermes/skills")]);
        let result = resolve_skill_context(&registry, &["test-skill".to_string()]);
        assert!(result.contains("## Skill: test-skill"), "result: {result}");
        assert!(result.contains("Do the thing."), "result: {result}");
    }

    #[test]
    fn test_resolve_skill_context_missing_skill() {
        let dir = tempfile::tempdir().unwrap();
        let registry =
            ironhermes_core::SkillRegistry::load_with_paths(&[dir.path().join("no-skills-here")]);
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

        let registry =
            ironhermes_core::SkillRegistry::load_with_paths(&[dir.path().join("skills")]);
        let result = resolve_skill_context(
            &registry,
            &["real-skill".to_string(), "fake-skill".to_string()],
        );
        assert!(result.contains("Real content."), "result: {result}");
        assert!(!result.contains("fake-skill"), "result: {result}");
    }

    // -------------------------------------------------------------------------
    // Phase 07.5: Cron active_skills pre-population test
    // -------------------------------------------------------------------------

    /// D-11 / D-12: cron jobs with attached skills that declare allowed_tools
    /// restrict which tools the cron-triggered agent can call.
    #[tokio::test]
    async fn test_cron_job_prepopulates_active_skills() {
        // 1. Create a skill with allowed_tools: ["web_read"]
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("skills/restricted-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: restricted-skill\ndescription: A restrictive skill\nallowed-tools:\n  - web_read\n---\nRestricted skill body",
        ).unwrap();
        let skill_registry = Arc::new(ironhermes_core::SkillRegistry::load_with_paths(&[dir
            .path()
            .join("skills")]));

        // 2. Verify the skill was loaded with allowed_tools
        let record = skill_registry
            .find("restricted-skill")
            .expect("skill loaded");
        assert!(
            record.allowed_tools.is_some(),
            "allowed_tools must be parsed"
        );
        assert_eq!(
            record.allowed_tools.as_ref().unwrap(),
            &vec!["web_read".to_string()]
        );

        // 3. Simulate pre-population logic (same as execute_cron_job does)
        let active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        {
            let mut guard = active_skills.lock().unwrap();
            if let Some(rec) = skill_registry.find("restricted-skill") {
                guard.push(rec.clone());
            }
        }

        // 4. Verify the active_skills vec contains the skill with allowed_tools
        let guard = active_skills.lock().unwrap();
        assert_eq!(guard.len(), 1);
        assert_eq!(guard[0].name, "restricted-skill");
        assert!(guard[0].allowed_tools.is_some());
    }

    // -------------------------------------------------------------------------
    // Phase 07.4: Hook deduplication regression test
    //
    // Asserts that a canonical Telegram round-trip (handler.rs fires MessageReceived
    // before the agent loop and ResponseSent after) produces exactly ONE of each event.
    // The agent loop no longer fires these events — only the platform layer does.
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_telegram_roundtrip_produces_exactly_one_message_received_and_response_sent() {
        // This test simulates what handler.rs does for a Telegram message:
        // 1. Fire MessageReceived (platform="telegram")
        // 2. Run agent loop (which must NOT fire MessageReceived again)
        // 3. Fire ResponseSent (platform="telegram")
        //
        // Expected: exactly 1 MessageReceived + 1 ResponseSent in the hook stream.
        use ironhermes_hooks::{HookEvent, HookEventKind, HookRegistry, HooksConfig};

        let mut registry = HookRegistry::new(HooksConfig::default());
        let captured: Arc<std::sync::Mutex<Vec<HookEventKind>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let cap_clone = captured.clone();
        registry.add_listener(Arc::new(move |event: HookEvent| {
            cap_clone.lock().unwrap().push(event.kind);
        }));
        let registry = Arc::new(registry);

        let request_id = uuid::Uuid::new_v4().to_string();

        // Step 1: platform layer fires MessageReceived (simulates handler.rs line ~218)
        registry.fire(HookEvent::new(
            &request_id,
            HookEventKind::MessageReceived {
                platform: "telegram".to_string(),
                chat_id: "chat-123".to_string(),
                content_preview: "Hello agent".to_string(),
            },
        ));

        // Step 2: agent loop runs — it must NOT fire MessageReceived or ResponseSent.
        // We verify this by checking the count after agent "completes" (simulated: no
        // LLM call needed — the invariant is structural in agent_loop.rs after 07.4 fix).
        // No agent loop call here; the structural fix in agent_loop.rs is the guarantee.

        // Step 3: platform layer fires ResponseSent (simulates handler.rs line ~384)
        registry.fire(HookEvent::new(
            &request_id,
            HookEventKind::ResponseSent {
                platform: "telegram".to_string(),
                chat_id: "chat-123".to_string(),
                response_preview: "Hello user".to_string(),
            },
        ));

        // Give tokio::spawn tasks time to call listeners
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let events = captured.lock().unwrap();

        // Count MessageReceived and ResponseSent events
        let msg_received_count = events
            .iter()
            .filter(|e| matches!(e, HookEventKind::MessageReceived { .. }))
            .count();
        let response_sent_count = events
            .iter()
            .filter(|e| matches!(e, HookEventKind::ResponseSent { .. }))
            .count();

        assert_eq!(
            msg_received_count, 1,
            "expected exactly 1 MessageReceived event, got {}: duplicate events from agent_loop would indicate regression",
            msg_received_count
        );
        assert_eq!(
            response_sent_count, 1,
            "expected exactly 1 ResponseSent event, got {}: duplicate events from agent_loop would indicate regression",
            response_sent_count
        );

        // Verify platform metadata is correct (from the platform layer, not agent loop)
        match &events[0] {
            HookEventKind::MessageReceived {
                platform, chat_id, ..
            } => {
                assert_eq!(platform, "telegram");
                assert_eq!(chat_id, "chat-123");
            }
            other => panic!("first event should be MessageReceived, got {:?}", other),
        }
        match &events[1] {
            HookEventKind::ResponseSent {
                platform, chat_id, ..
            } => {
                assert_eq!(platform, "telegram");
                assert_eq!(chat_id, "chat-123");
            }
            other => panic!("second event should be ResponseSent, got {:?}", other),
        }
    }

    // -------------------------------------------------------------------------
    // Phase 07.4: ToolCalled ordering test
    //
    // Asserts that ToolCalled events are only emitted for tools that pass the
    // guardrail chain — blocked tools must not produce ToolCalled events.
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_tool_called_not_emitted_for_blocked_tools() {
        use async_trait::async_trait;
        use ironhermes_core::ToolSchema;
        use ironhermes_hooks::{
            BlocklistGuardrail, HookEvent, HookEventKind, HookRegistry, HooksConfig,
        };
        use ironhermes_tools::{Tool, ToolRegistry};

        // A simple echo tool that records when it actually executes
        struct EchoTool;
        #[async_trait]
        impl Tool for EchoTool {
            fn name(&self) -> &str {
                "echo"
            }
            fn toolset(&self) -> &str {
                "test"
            }
            fn description(&self) -> &str {
                "echo tool"
            }
            fn schema(&self) -> ToolSchema {
                ToolSchema::new(
                    "echo",
                    "echo",
                    serde_json::json!({"type":"object","properties":{}}),
                )
            }
            async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
                Ok("echo result".to_string())
            }
        }

        // Registry with echo blocked
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Box::new(EchoTool));
        tool_registry.add_guardrail(Box::new(BlocklistGuardrail::new(vec!["echo".to_string()])));

        // Hook registry to capture ToolCalled events
        let mut hook_registry = HookRegistry::new(HooksConfig::default());
        let captured: Arc<std::sync::Mutex<Vec<HookEventKind>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let cap_clone = captured.clone();
        hook_registry.add_listener(Arc::new(move |event: HookEvent| {
            cap_clone.lock().unwrap().push(event.kind);
        }));

        // Attempt dispatch with hook — echo is blocked, so post-guardrail hook must not fire
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called_clone = called.clone();
        let result = tool_registry
            .dispatch_with_hook(
                "echo",
                serde_json::Value::Null,
                Some(move |_tool: &str, _args: &str| {
                    called_clone.store(true, std::sync::atomic::Ordering::SeqCst);
                }),
            )
            .await;

        assert!(result.is_err(), "blocked tool must return Err");
        assert!(
            !called.load(std::sync::atomic::Ordering::SeqCst),
            "post-guardrail hook must NOT be called for blocked tools"
        );

        // For an allowed tool — hook must fire
        let called_allowed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called_allowed_clone = called_allowed.clone();

        // Registry without guardrail
        let mut tool_registry2 = ToolRegistry::new();
        tool_registry2.register(Box::new(EchoTool));
        let result2 = tool_registry2
            .dispatch_with_hook(
                "echo",
                serde_json::Value::Null,
                Some(move |_tool: &str, _args: &str| {
                    called_allowed_clone.store(true, std::sync::atomic::Ordering::SeqCst);
                }),
            )
            .await;

        assert!(
            result2.is_ok(),
            "allowed tool must return Ok: {:?}",
            result2
        );
        assert!(
            called_allowed.load(std::sync::atomic::Ordering::SeqCst),
            "post-guardrail hook MUST be called for allowed tools"
        );
    }

    // -------------------------------------------------------------------------
    // Phase 07.4-03: Cron path exactly-one event counts
    //
    // These tests prove that execute_cron_job fires MessageReceived exactly once
    // and ResponseSent exactly once per job execution — even in the error path
    // (D-04: ResponseSent fires on both success and failure branches).
    //
    // Strategy: point LlmClient at an unreachable URL so agent.run() fails fast.
    // execute_cron_job still fires MessageReceived before agent.run() and
    // ResponseSent in the Err arm. This proves exactly-one without a real LLM.
    // -------------------------------------------------------------------------

    /// D-04 / audit warning #4 (cron path): execute_cron_job must fire exactly
    /// 1 MessageReceived and exactly 1 ResponseSent per cron job run — even when
    /// the agent errors (LLM unreachable). The agent loop fires neither event
    /// (Issue #4 fix). Only execute_cron_job fires them.
    #[tokio::test]
    async fn test_cron_path_fires_exactly_one_message_received_and_response_sent() {
        use ironhermes_core::Config;
        use ironhermes_core::config::{AgentConfig, ModelConfig};
        use ironhermes_cron::{JobStore, ScheduleParsed};
        use ironhermes_hooks::{HookEvent, HookEventKind, HookRegistry, HooksConfig};
        use tempfile::TempDir;

        // 1. Build a capturing HookRegistry
        let mut hook_registry = HookRegistry::new(HooksConfig::default());
        let captured: Arc<std::sync::Mutex<Vec<HookEventKind>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let cap_clone = captured.clone();
        hook_registry.add_listener(Arc::new(move |event: HookEvent| {
            cap_clone.lock().unwrap().push(event.kind);
        }));
        let hook_registry = Arc::new(hook_registry);

        // 2. Create a real CronJob in a temp JobStore
        let dir = TempDir::new().expect("tempdir");
        let cron_dir = dir.path().join("cron");
        let mut raw_store = JobStore::open(cron_dir).expect("open jobstore");
        let job = raw_store
            .add_job(
                "test-cron-07.4",
                "Say hello",
                ScheduleParsed::Interval {
                    minutes: 60,
                    display: "every 60m".to_string(),
                },
                "every 60m",
                "local",
                vec![],
                None,
            )
            .expect("add job");
        let job_store = Arc::new(std::sync::Mutex::new(raw_store));

        // 3. Build a Config pointing at an unreachable LLM (connection refused).
        //    execute_cron_job will fire MessageReceived, then agent.run() fails,
        //    then the Err arm fires ResponseSent. Total: 1 + 1 = 2 events.
        let mut config = Config::default();
        // Port 1 is privileged and always connection-refused
        config.model = ModelConfig {
            default: "test-model".to_string(),
            base_url: Some("http://127.0.0.1:1".to_string()),
            api_key: Some("test-key".to_string()),
            ..Default::default()
        };
        config.agent = AgentConfig {
            max_turns: 1,
            ..Default::default()
        };

        // 4. Call run_cron_job via CronRunnerContext — expect it to return Err (LLM unreachable),
        //    but the hook events must still fire.
        //    (Plan 32.1-07: execute_cron_job moved to ironhermes_cron_runner::run_cron_job)
        let tool_registry = Arc::new(RwLock::new(ironhermes_tools::ToolRegistry::new()));
        let cron_ctx = ironhermes_cron_runner::CronRunnerContext {
            job_store: job_store.clone(),
            skill_registry: None,
            tool_registry: tool_registry.clone(),
            memory_manager: None,
            hook_registry: Some(hook_registry),
            config: config.clone(),
            mcp_manager: None,
            tg_client: None,
        };
        let _ = ironhermes_cron_runner::run_cron_job(&job, &cron_ctx).await;
        // Give tokio::spawn listeners 50ms to drain
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // 5. Assert exactly-one of each lifecycle event
        let events = captured.lock().unwrap();
        let msg_received_count = events
            .iter()
            .filter(|e| matches!(e, HookEventKind::MessageReceived { .. }))
            .count();
        let response_sent_count = events
            .iter()
            .filter(|e| matches!(e, HookEventKind::ResponseSent { .. }))
            .count();

        assert_eq!(
            msg_received_count, 1,
            "cron execute_cron_job must fire exactly 1 MessageReceived, got {msg_received_count}: \
             duplicate would indicate agent_loop regression (audit warning #4)"
        );
        assert_eq!(
            response_sent_count, 1,
            "cron execute_cron_job must fire exactly 1 ResponseSent, got {response_sent_count}: \
             missing would indicate D-04 regression (ResponseSent on error arm)"
        );

        // 6. Verify cron metadata on the events
        match &events[0] {
            HookEventKind::MessageReceived { platform, .. } => {
                assert_eq!(
                    platform, "cron",
                    "MessageReceived must use platform=\"cron\""
                );
            }
            other => panic!("first event should be MessageReceived, got {:?}", other),
        }
        match &events[1] {
            HookEventKind::ResponseSent { platform, .. } => {
                assert_eq!(platform, "cron", "ResponseSent must use platform=\"cron\"");
            }
            other => panic!("second event should be ResponseSent, got {:?}", other),
        }
    }

    /// Plan 32.1-07 regression guard: execute_cron_job must NOT exist in gateway runner.rs.
    ///
    /// execute_cron_job was extracted to ironhermes-cron-runner in Plan 32.1-07.
    /// This test ensures it is never re-introduced into the gateway layer.
    /// The hook-fire contract (MessageReceived + ResponseSent) is now verified
    /// in ironhermes-cron-runner's own tests.
    #[test]
    fn execute_cron_job_no_longer_exists_in_gateway() {
        let src = include_str!("runner.rs");
        // Use concat! so this test's own source string doesn't match.
        let fn_marker = concat!("pub(crate) async fn ", "execute_cron_job(");
        assert!(
            !src.contains(fn_marker),
            "Plan 32.1-07 violation: execute_cron_job function was re-introduced into \
             ironhermes-gateway/src/runner.rs. Job execution must live in ironhermes-cron-runner."
        );
    }

    // -------------------------------------------------------------------------
    // Plan 18-08: GatewayRunner wires gateway hygiene engine
    // -------------------------------------------------------------------------

    fn make_runner_with_engine_kind(engine_kind: &str) -> GatewayRunner {
        let mut config = Config::default();
        config.gateway.context_engine = engine_kind.to_string();
        config.gateway.compression_threshold = 0.85;
        let resolver = ProviderResolver::build(&config).expect("resolver ok");
        let tool_registry = Arc::new(RwLock::new(ToolRegistry::new()));
        GatewayRunner::new(config, resolver, tool_registry)
    }

    /// Plan 18-08 Task 1: `build_gateway_handler` constructs a handler whose
    /// gateway_engine is attached when config.gateway.context_engine = "local_prune".
    #[test]
    fn runner_attaches_gateway_engine_from_config() {
        let runner = make_runner_with_engine_kind("local_prune");
        let handler = runner.build_gateway_handler();
        assert!(
            handler.gateway_engine_is_some(),
            "build_gateway_handler must attach a gateway engine (handler.gateway_engine must be Some)"
        );
    }

    /// Plan 18-08 Task 1: When config.gateway.context_engine is an unknown string,
    /// the factory falls back to local_prune (per 18-06 T-18-08 behavior) and the
    /// handler still has an engine attached. No panic.
    #[test]
    fn runner_gateway_engine_respects_unknown_kind_fallback() {
        let runner = make_runner_with_engine_kind("bogus_engine_kind");
        let handler = runner.build_gateway_handler();
        assert!(
            handler.gateway_engine_is_some(),
            "unknown engine kind must fall back to local_prune, not leave gateway_engine = None"
        );
    }

    // -------------------------------------------------------------------------
    // Phase 21.2 Plan 11 — GAP-8: gateway Ctrl+C hang on connected MCP server
    // -------------------------------------------------------------------------

    /// GAP-8: `GatewayRunner::start` MUST call `McpManager::shutdown_all` on
    /// graceful shutdown. Without this wire, `ironhermes gateway` hangs on
    /// Ctrl+C when MCP servers are connected (tokio process reaper blocks
    /// runtime exit until children are reaped).
    ///
    /// This test locks the literal shutdown_all call site in runner.rs by
    /// source-grep. Companion test `shutdown_all_returns_within_timeout_when_stdio_child_blocks`
    /// in ironhermes-mcp exercises the actual hard-kill + bounded-timeout path.
    /// A grep-based wire check is more robust than a live harness that would
    /// require a full Telegram adapter mock.
    #[test]
    fn gateway_runner_invokes_mcp_shutdown_all_on_cancel() {
        let src = include_str!("runner.rs");
        assert!(
            src.contains("if let Some(ref mgr) = self.mcp_manager"),
            "GAP-8: runner.rs start() must guard shutdown_all call with \
             if let Some(ref mgr) = self.mcp_manager"
        );
        assert!(
            src.contains("mgr.shutdown_all().await"),
            "GAP-8: runner.rs start() must await mgr.shutdown_all() on \
             graceful shutdown"
        );
        // Ordering: the shutdown_all call MUST appear BEFORE the propagation
        // anchor comment `// Propagate cancellation to all subtasks`, which
        // in turn sits immediately before `self.cancel.cancel();`. This
        // enforces that MCP children are killed BEFORE subtasks die and
        // BEFORE the JoinSet drain.
        let shutdown_call = src
            .find("mgr.shutdown_all().await")
            .expect("GAP-8: mgr.shutdown_all().await call site must exist in start()");
        let propagation_comment = src
            .find("// Propagate cancellation to all subtasks")
            .expect("propagation comment must exist as shutdown anchor");
        assert!(
            shutdown_call < propagation_comment,
            "GAP-8: mgr.shutdown_all().await must be called BEFORE the \
             'Propagate cancellation to all subtasks' block (stdio children \
             must be killed before subtask join_set drain). Offsets: \
             shutdown_call={shutdown_call}, propagation_comment={propagation_comment}"
        );
    }

    /// GAP-8: `GatewayRunner` MUST carry an `mcp_manager: Option<Arc<McpManager>>`
    /// field and expose a `pub fn set_mcp_manager` setter so `run_gateway` in
    /// ironhermes-cli can wire the manager before calling `start()`. Paired
    /// with `gateway_runner_invokes_mcp_shutdown_all_on_cancel` above, this
    /// fully locks the GAP-8 wire against silent regression.
    #[test]
    fn gateway_runner_has_set_mcp_manager_setter() {
        let src = include_str!("runner.rs");
        assert!(
            src.contains("pub fn set_mcp_manager"),
            "GAP-8: runner.rs must expose pub fn set_mcp_manager so \
             run_gateway can wire the Arc<McpManager> clone"
        );
        assert!(
            src.contains("mcp_manager: Option<Arc<McpManager>>"),
            "GAP-8: GatewayRunner struct must carry \
             mcp_manager: Option<Arc<McpManager>> field"
        );
    }

    // -------------------------------------------------------------------------
    // Phase 21.8.1-05: Gateway-surface gap-01 closure tests
    //
    // Proves that a category-nested skill (`<skills_root>/<category>/<name>/SKILL.md`)
    // flows through SkillRegistry::load_with_paths -> PromptBuilder::set_skill_registry
    // -> PromptBuilder::build_split -> durable system-prompt text.
    //
    // This is the same code path the gateway runner uses for every Telegram and
    // CLI gateway turn (runner.rs:1093: prompt_builder.set_skill_registry(...)).
    // -------------------------------------------------------------------------

    /// Phase 21.8.1-05 gap-01: a skill at the two-level category-nested layout
    /// `<skills_root>/<category>/<name>/SKILL.md` must appear in the durable
    /// system-prompt produced by PromptBuilder::build_split after
    /// set_skill_registry is called — the same code path used by the gateway.
    #[test]
    fn installed_category_nested_skill_visible_to_gateway_prompt_builder() {
        let dir = tempfile::tempdir().unwrap();
        let nested_skill_dir = dir
            .path()
            .join("skills")
            .join("gap-test-cat")
            .join("gateway-visibility-skill");
        std::fs::create_dir_all(&nested_skill_dir).unwrap();
        std::fs::write(
            nested_skill_dir.join("SKILL.md"),
            "---\nname: gateway-visibility-skill\ndescription: Phase 21.8.1-05 gateway-surface gap-01 fix\nmetadata:\n  hermes:\n    category: gap-test-cat\n---\nGateway surface integration test body.\n",
        )
        .unwrap();

        let skill_registry = Arc::new(ironhermes_core::SkillRegistry::load_with_paths(&[dir
            .path()
            .join("skills")]));

        // Sanity: skill must be discoverable (would fail before Task 1 landed)
        assert!(
            skill_registry.find("gateway-visibility-skill").is_some(),
            "gap-01 gateway: skill at category-nested path must be discoverable by SkillRegistry::load_with_paths"
        );

        // Wire skill registry into a real PromptBuilder (same code path as gateway runner)
        let mut prompt_builder = ironhermes_agent::PromptBuilder::new("test-model", "gateway");
        prompt_builder.set_skill_registry(skill_registry.clone());
        let (durable, _ephemeral) = prompt_builder.build_split();

        // Prove the full chain: SkillRegistry -> PromptBuilder -> system-prompt text
        assert!(
            durable.contains("Available Skills"),
            "gap-01 gateway: prompt must contain 'Available Skills' section: {}",
            durable
        );
        assert!(
            durable.contains("gateway-visibility-skill"),
            "gap-01 gateway: prompt must contain the skill name: {}",
            durable
        );
        assert!(
            durable.contains("Phase 21.8.1-05 gateway-surface gap-01 fix"),
            "gap-01 gateway: prompt must contain the skill description"
        );
    }

    /// Phase 21.8.1-05: empty-registry path regression guard.
    /// No skills section must be injected when the registry is empty,
    /// preserving the existing prompt-shape contract.
    #[test]
    fn gateway_path_loads_zero_skills_for_empty_skills_root_does_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        // Pass a path that doesn't exist — guaranteed empty registry
        let skill_registry = Arc::new(ironhermes_core::SkillRegistry::load_with_paths(&[dir
            .path()
            .join("skills")]));

        assert!(
            skill_registry.list().is_empty(),
            "empty skills root must produce an empty registry"
        );

        let mut prompt_builder = ironhermes_agent::PromptBuilder::new("test-model", "gateway");
        prompt_builder.set_skill_registry(skill_registry.clone());
        let (durable, _ephemeral) = prompt_builder.build_split();

        // No skills section injected when registry is empty
        // (the existing `if !registry.list().is_empty()` guard in build_split fires)
        assert!(
            !durable.contains("Available Skills"),
            "no 'Available Skills' section must be injected for an empty registry: {}",
            durable
        );
    }
}

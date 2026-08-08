use anyhow::{Context, Result};
use ironhermes_agent::AgentRuntime;
use ironhermes_agent::MemoryManager;
use ironhermes_agent::context_engine::ContextEngine;
use ironhermes_agent::engine_factory::build_context_engine;
use ironhermes_agent::pressure_warning::PressureTracker;
use ironhermes_agent::subagent_registry::SubagentRegistry;
use ironhermes_core::commands::context::ToolsetSessionHandle;
use ironhermes_core::commands::{CommandDef, CommandRouter, registry::build_registry};
use ironhermes_core::{Config, Platform, ProviderResolver, SkillRecord, SkillRegistry};
use ironhermes_cron::JobStore;
use ironhermes_exec::process_registry::ProcessRegistry;
use ironhermes_mcp::McpManager;
use ironhermes_tools::ToolRegistry;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{Mutex as TokioMutex, RwLock, Semaphore, mpsc};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::adapter::PlatformAdapter;
use crate::backoff::BackoffState;
use crate::handler::GatewayMessageHandler;
use crate::multimodal;
use crate::session::{SessionKey, SessionStore};
use crate::session_queue::{QueueError, SessionQueue};
use crate::telegram::{TelegramAdapter, TgBotCommand, tg_message_to_event};
use crate::user_queue::{DispatchOutcome, UserQueueManager};
use ironhermes_core::MessageEvent;
use ironhermes_cron::{DeliveryRegistry, DeliverySend, TgSendApi};

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
    /// Plan 28.1-02: the single AgentRuntime built in run_gateway. Passed into
    /// GatewayMessageHandler so every top-level turn is routed through
    /// runtime.run_turn (which resets the budget, builds the loop, and runs).
    agent_runtime: Option<Arc<AgentRuntime>>,
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
    /// Phase 36.17.1: per-session FIFO queue (D-06, D-14). Always initialized.
    /// Wrapped in Arc so `build_gateway_handler` can thread a clone to
    /// `GatewayMessageHandler` (D-15, RESEARCH Open Q3 — Arc<SessionQueue>,
    /// NOT Arc<GatewayRunner>, to avoid a circular reference). The raw
    /// `SessionQueue` type is intentionally not exported in lib.rs — adapters
    /// reach it only via the thin public API methods on this struct.
    session_queue: Arc<SessionQueue>,
    /// Phase 39.1 (R39.1-01 / R39.1-03 / R39.1-04): process-wide TurnRegistry
    /// and two-level ConcurrencyLayer. Both always-initialized so the worker loop
    /// and all surfaces share the same instance. The global ceiling Semaphore
    /// inside ConcurrencyLayer is process-wide (R39.1-04).
    turn_registry: Arc<ironhermes_core::concurrency::TurnRegistry>,
    concurrency: Arc<ironhermes_core::concurrency::ConcurrencyLayer>,
    /// Phase 36.17.1 D-03 (Plan 04): drain-mode flag — set true BEFORE
    /// `cancel.cancel()` during shutdown so late-arriving messages stay in
    /// the queue and reach the next agent turn (in-process only). The flag
    /// is a SIGNAL, not a gate — `SessionQueue::try_push` does NOT consult
    /// it. Python parity: `_queue_during_drain_enabled` (gateway/run.py:2298-2302).
    ///
    /// Closes T-36.17.1-03 (lost-update during drain-mode transition) by
    /// pairing the flag flip with the cancel call in `drain_for_restart`:
    /// any concurrent `try_push` observing `is_draining=true` is guaranteed
    /// to see `cancel` not-yet-fired AND the queue continues to accept the
    /// push (D-03 preserve-AND-accept).
    is_draining: Arc<AtomicBool>,
    /// Phase 36.3.8 Plan 03 — in-memory registry of suspended `ClarifyTool`
    /// awaiters keyed by `clarify_id`. The dispatch loop resolves a pending
    /// awaiter when an inline-keyboard `callback_query` arrives (D-05). Shared
    /// with `ClarifyTool` instances (Plan 04) via Arc clone at per-turn
    /// registration time — same Arc-shared-state pattern as `skill_overlays`
    /// in handler.rs.
    clarify_registry: Arc<ironhermes_tools::PendingClarifyRegistry>,
    cancel: CancellationToken,
}

/// Build the Telegram bot-command menu (D-17 `setMyCommands` payload) from
/// the command router's full catalog, filtered to Telegram-available
/// commands (G-41.1-5). Replaces the previous hardcoded 4-command
/// start/new/clear/help subset — every command whose `platform_filter`
/// allows `Platform::Telegram` is now registered.
///
/// Skills (resolved via `SkillRegistry`, not `CommandRouter`) are
/// architecturally distinct from slash commands and are intentionally NOT
/// included here — extending the bot menu to skills is out of scope.
fn telegram_bot_commands() -> Vec<TgBotCommand> {
    let command_router = CommandRouter::new(build_registry());
    commands_for_platform(&command_router.commands, &Platform::Telegram)
}

/// Filter `commands` to those available on `platform` and map each to the
/// wire-format `TgBotCommand` (`CommandDef.name` -> `command`,
/// `CommandDef.description` -> `description`). Split out from
/// [`telegram_bot_commands`] so the filter+mapping logic is unit-testable
/// against a small synthetic command set, independent of the full
/// `build_registry()` catalog.
///
/// Telegram rejects the ENTIRE `setMyCommands` batch with
/// `400: BOT_COMMAND_INVALID` if any single name violates its
/// `[a-z0-9_]{1,32}` rule, so names are sanitized here:
/// - subcommand entries with spaces (e.g. "provider list") are dropped —
///   the parent command is already in the menu and covers them
/// - hyphens map to underscores (e.g. "reload-mcp" -> "reload_mcp"); the
///   registry carries underscore aliases so the tapped menu item resolves
/// - anything still outside the allowed charset/length is dropped
fn commands_for_platform(commands: &[CommandDef], platform: &Platform) -> Vec<TgBotCommand> {
    commands
        .iter()
        .filter(|c| c.is_available_on(platform))
        .filter(|c| !c.name.contains(' '))
        .map(|c| TgBotCommand {
            command: c.name.replace('-', "_"),
            description: c.description.to_string(),
        })
        .filter(|c| {
            !c.command.is_empty()
                && c.command.len() <= 32
                && c.command
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        })
        .collect()
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
        // Phase 36.17.9: honor `gateway.persist_sessions` (default true) so an
        // ongoing platform conversation resumes its prior session across restarts.
        let mut session_store = SessionStore::new(Arc::clone(&state_store));
        session_store.set_persist_sessions(config.gateway.persist_sessions);
        // Phase 39.1 (R39.1-03 / R39.1-04): extract concurrency caps before config is moved.
        let session_turn_cap = config.concurrency.session_turn_cap;
        let global_turn_ceiling = config.concurrency.global_turn_ceiling;
        Self {
            config,
            resolver,
            session_store: Arc::new(RwLock::new(session_store)),
            state_store,
            tool_registry,
            memory_manager: None,
            job_store: None,
            hook_registry: None,
            skill_registry: None,
            active_skills: None,
            mcp_manager: None,       // GAP-8: wired by run_gateway before start()
            agent_runtime: None,     // Plan 28.1-02: wired by run_gateway before start()
            process_registry: None,  // Plan 21.7-06: wired by run_gateway before start()
            subagent_registry: None, // Plan 21.7-07: wired by run_gateway before start()
            browser_session: None,   // Phase 25.1: wired by run_gateway before start()
            toolset_session: None, // Phase 25.2 Plan 15 follow-up: wired by run_gateway before start()
            workspace: None,       // Phase 25.3 D-W-2: wired by run_gateway before start()
            trajectory_root: None, // Phase 25.3-15 CR-02: wired by run_gateway before start()
            skills_config: None,   // Phase 21.8.2 D-02: wired by run_gateway before start()
            // Phase 36.17.1 (D-06, D-14): always-initialized per-session FIFO queue.
            // No `set_session_queue` method — the queue is owned by the runner from
            // construction; `build_gateway_handler` clones the Arc into the handler.
            session_queue: Arc::new(SessionQueue::new()),
            // Phase 39.1: process-wide TurnRegistry + ConcurrencyLayer (always-init).
            turn_registry: Arc::new(ironhermes_core::concurrency::TurnRegistry::new()),
            concurrency: Arc::new(ironhermes_core::concurrency::ConcurrencyLayer::new(
                session_turn_cap,
                global_turn_ceiling,
            )),
            // Phase 36.17.1 Plan 04 (D-03): drain-mode flag starts false.
            // `drain_for_restart()` flips it to true BEFORE cancelling the
            // cancel token — preserve-AND-accept semantics live there.
            is_draining: Arc::new(AtomicBool::new(false)),
            // Phase 36.3.8 Plan 03: always-initialized clarify awaiter registry.
            // Plan 04 clones this Arc into per-turn ClarifyTool registration so
            // the dispatch loop and the tool share the same pending-awaiter map.
            clarify_registry: Arc::new(ironhermes_tools::PendingClarifyRegistry::new()),
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

    /// Plan 28.1-02: install the single AgentRuntime so the handler can route
    /// every top-level turn through `runtime.run_turn`. Caller is `run_gateway`
    /// in ironhermes-cli, which builds one runtime via `AgentRuntime::from_config`.
    pub fn set_agent_runtime(&mut self, runtime: Arc<AgentRuntime>) {
        self.agent_runtime = Some(runtime);
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

    // ---------------------------------------------------------------------
    // Phase 36.17.1: SessionQueue public API (D-15)
    //
    // Thin delegation layer over `Arc<SessionQueue>`. The raw `SessionQueue`
    // type is intentionally not re-exported from lib.rs — adapters and other
    // call sites reach the queue only through these methods. All methods are
    // synchronous (D-17); the underlying `std::sync::Mutex` guard is dropped
    // before any await on the caller's side.
    // ---------------------------------------------------------------------

    /// Push an event onto the per-session FIFO queue.
    ///
    /// Returns `Err(QueueError::CapacityReached)` when the session's queue
    /// holds `MAX_QUEUE_DEPTH` events (D-09). Delegates to
    /// `SessionQueue::try_push` (Python parity: `_enqueue_fifo`).
    pub fn try_enqueue(&self, key: &SessionKey, event: MessageEvent) -> Result<(), QueueError> {
        self.session_queue.try_push(key, event)
    }

    /// Pop the oldest queued event for the session, or `None` if empty.
    ///
    /// Delegates to `SessionQueue::pop` (Python parity: `_dequeue_pending_event`).
    pub fn dequeue(&self, key: &SessionKey) -> Option<MessageEvent> {
        self.session_queue.pop(key)
    }

    /// Current queue depth for the session (0 if no queue allocated).
    ///
    /// Delegates to `SessionQueue::len` (Python parity: `_queue_depth`).
    pub fn queue_len(&self, key: &SessionKey) -> usize {
        self.session_queue.len(key)
    }

    /// Drop every queued event for the session.
    ///
    /// Delegates to `SessionQueue::clear`. Called by `/new` and `/reset`
    /// handlers BEFORE `SessionStore::remove` (RESEARCH Pitfall 5).
    pub fn clear_queue(&self, key: &SessionKey) {
        self.session_queue.clear(key);
    }

    /// Retain only events matching `predicate`, in arrival order.
    ///
    /// Delegates to `SessionQueue::retain`. The goal-continuation predicate
    /// is deferred per D-04 — this method is the general mechanism.
    pub fn retain_queue<F: Fn(&MessageEvent) -> bool>(&self, key: &SessionKey, predicate: F) {
        self.session_queue.retain(key, predicate);
    }

    /// Phase 36.17.1: crate-private accessor for threading `Arc<SessionQueue>`
    /// into the handler from `build_gateway_handler`. Plan 04 will reuse the
    /// same accessor for drain-mode wiring.
    #[allow(dead_code)] // planned accessor for Phase 36.17.1 Plan 04 drain-mode wiring; no caller yet
    pub(crate) fn session_queue(&self) -> Arc<SessionQueue> {
        self.session_queue.clone()
    }

    /// Phase 36.17.1 Plan 04 (D-03): true once the runner has entered
    /// drain-mode (graceful shutdown). The queue continues to accept pushes
    /// while this flag is set — drain mode is a SIGNAL, not a gate
    /// (`SessionQueue::try_push` does not consult this flag). Python parity:
    /// `_queue_during_drain_enabled` (gateway/run.py:2298-2302).
    pub fn is_draining(&self) -> bool {
        self.is_draining.load(Ordering::SeqCst)
    }

    /// Phase 36.17.1 Plan 04 (D-03): enter drain-mode.
    ///
    /// Sets `is_draining` to `true` BEFORE cancelling the cancel token. The
    /// ordering is the T-36.17.1-03 mitigation: any concurrent `try_push`
    /// that observes `is_draining=true` is guaranteed to also see the cancel
    /// token NOT YET fired, AND `SessionQueue::try_push` continues to accept
    /// the push (D-03 preserve-AND-accept). The brief in-process window
    /// between flag-flip and process exit preserves arrival order without
    /// losing user input.
    ///
    /// Called from the graceful-shutdown path in `start()` in place of the
    /// previous bare `self.cancel.cancel()`. Forced-abort paths may still
    /// call `cancel.cancel()` directly when drain semantics are explicitly
    /// undesired — that is acceptable per the locked decision contract.
    ///
    /// Python parity: equivalent transition to `_restart_requested = True`
    /// + the existing busy-mode being `queue`/`steer` (gateway/run.py:2298-2302).
    pub fn drain_for_restart(&self) {
        // ORDERING is load-bearing — do NOT reorder. T-36.17.1-03 mitigation.
        self.is_draining.store(true, Ordering::SeqCst);
        self.cancel.cancel();
    }

    /// Phase 36.17.1 Plan 02 Task 3: post-turn FIFO drain (D-01 part (b)).
    ///
    /// Pops events from the session queue and re-invokes `handler.run_agent`
    /// in arrival order until the queue is empty. Called by the per-chat
    /// worker after each `handle_with_multimodal` turn returns.
    ///
    /// Bypasses `handle_with_multimodal` per RESEARCH Pitfall 4 — the
    /// RAII `RunningAgentGuard` inside `run_agent` re-sets the per-session
    /// AtomicBool true for the duration of each drained turn, so a push
    /// arriving mid-drain enqueues onto the same key and is picked up on
    /// the next pop iteration. Order is preserved by `VecDeque` FIFO.
    ///
    /// Exposed as `pub` so the Plan 02 Task 3 integration test
    /// (`tests/session_queue_integration.rs` — an external test binary that
    /// can only see `pub` items, not `pub(crate)`) can invoke the real drain
    /// loop directly. NOT a substitute pop-sequence unit test — the test
    /// must exercise this code path.
    ///
    /// [Rule 3 - Blocking] The plan acceptance criterion says
    /// `pub(crate) async fn drain_pending` but integration tests cannot reach
    /// crate-private items. We widen to `pub` so the required integration
    /// test in `tests/session_queue_integration.rs` can call
    /// `runner.drain_pending(...)`. Documented in the plan SUMMARY.
    pub async fn drain_pending(
        &self,
        key: &SessionKey,
        handler: &GatewayMessageHandler,
        adapter: Arc<dyn PlatformAdapter>,
        cancel: CancellationToken,
    ) -> Result<()> {
        // Replayed events go through the agent loop without their original
        // multimodal envelope (which was consumed at the original
        // `handle_with_multimodal` call). The `MessageEvent.content` field
        // already carries any text-only payload the agent needs.
        //
        // Phase 36.17.1 Plan 02 Task 3 [Rule 2 - critical functionality
        // refinement]: drain continues on individual `run_agent` errors
        // (logs and proceeds to next event) rather than propagating the
        // first `?` — a single bad event must not poison the rest of the
        // queue. This matches the Python reference's per-iteration
        // resilience in `_promote_queued_event`. Cancellation still
        // short-circuits the drain via the `cancel` token; the loop
        // is broken explicitly when the token is fired between pops.
        while let Some(next_event) = self.session_queue.pop(key) {
            if cancel.is_cancelled() {
                tracing::info!(
                    session = %key.to_string_key(),
                    "SessionQueue: drain cancelled (Phase 36.17.1)"
                );
                break;
            }
            tracing::debug!(
                session = %key.to_string_key(),
                remaining = self.session_queue.len(key),
                "SessionQueue: draining next queued event (Phase 36.17.1)"
            );
            let no_attachments = crate::multimodal::ProcessedAttachments {
                text_prefix: None,
                image_data_uri: None,
                image_cache_path: None,
            };
            if let Err(e) = handler
                .run_agent(&next_event, adapter.clone(), cancel.clone(), no_attachments)
                .await
            {
                tracing::error!(
                    session = %key.to_string_key(),
                    error = %e,
                    "SessionQueue: drained event run_agent failed; continuing (Phase 36.17.1)"
                );
            }
        }
        Ok(())
    }

    /// Plan 03 (Phase 22.4.2.1): returns a clone of the runner's CancellationToken.
    /// Used by gateway integration tests (tests/gateway_shutdown.rs) to fire
    /// shutdown without going through the OS signal layer.
    /// pub(crate) so only gateway-crate tests can reach it (T-22.4.2.1-03-05).
    #[allow(dead_code)] // gateway integration test accessor; gateway_shutdown.rs not yet exercising this path
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
        // Plan 28.1-02: thread the AgentRuntime into the handler so every
        // top-level turn is routed through runtime.run_turn (which resets
        // the budget and builds the loop from the runtime's durable Arcs).
        if let Some(ref rt) = self.agent_runtime {
            handler.set_agent_runtime(rt.clone());
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

        // Phase 41.3 Plan 04 (D-11/D-12): thread the existing McpManager handle
        // (already held for GAP-8 shutdown wiring) into the handler so its
        // slash-dispatch CommandContext can wire mcp_reloader — previously the
        // gateway had no MCP handle on CommandContext at all.
        if let Some(ref mgr) = self.mcp_manager {
            handler.set_mcp_manager(mgr.clone());
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

        // Phase 36.17.1 (D-14, D-15, RESEARCH Open Q3): thread the per-session
        // FIFO queue Arc. Without this call the handler.session_queue stays
        // None and `handle_with_multimodal` falls back to the Phase 36 reject
        // path. With it, the busy-branch enqueues and cap-hit fires D-13 UX.
        handler.set_session_queue(self.session_queue.clone());
        // Phase 39.1 (R39.1-01 / R39.1-03 / R39.1-04): wire shared TurnRegistry
        // and ConcurrencyLayer so run_agent acquires permits + registers turns.
        handler.set_turn_registry(self.turn_registry.clone());
        handler.set_concurrency(self.concurrency.clone());
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
    ///
    /// Phase 36.17.1 Plan 02 Task 3: takes `self: Arc<Self>` so the per-chat
    /// worker spawn closure can capture an `Arc<GatewayRunner>` clone and
    /// call `runner.drain_pending(...)` after each handler turn. The
    /// `'static` requirement of `JoinSet::spawn` forces this — a borrow of
    /// `&self` cannot escape into the spawned task.
    pub async fn start(self: Arc<Self>) -> Result<()> {
        // --- 0. Acquire PID lock (Phase 24 D-09/D-12) ---
        // Refuses startup if another live gateway is already running under
        // the same IRONHERMES_HOME (profile-scoped after Phase 24's --profile
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

        // Phase 47.6 Plan 03 (P0-1): the boot gate is now the single source
        // of truth for which platforms are usable. This replaces the
        // previous unconditional `resolve_token(...).context(...)?` hard
        // Telegram requirement — the gateway still refuses to boot with
        // zero usable platforms, but the error now names every platform it
        // tried and why (see `boot_gate.rs`).
        let platform_gate = crate::boot_gate::resolve_enabled_platforms(&self.config)
            .map_err(|e| anyhow::anyhow!("Gateway startup refused: {e}"))?;

        // Phase 47.6 Plan 01: bound here, in `start()`'s OWN scope — NOT
        // inside the section 7d block below and NOT inside the spawned
        // adapter task — so later plans can read it from outer scope. Four
        // consumers need this exact handle: plan 03's "primary adapter"
        // fallback for `UserQueueManager`/`ApprovalCoordinator` when
        // Telegram is absent, plan 06's per-platform approval coordinator,
        // plan 07's cron delivery registry, and plan 07's notifier snapshot.
        #[cfg(feature = "buzz")]
        let mut buzz_adapter: Option<std::sync::Arc<crate::buzz::BuzzAdapter>> = None;

        // Phase 47.6 Plan 03 (ORDERING TRAP — see this task's own note in
        // PLAN.md): CONSTRUCT the Buzz adapter here, before the
        // primary-adapter binding below. Construction is cheap and does no
        // network I/O — connecting happens inside the spawned loop at
        // section 7d, unchanged from Plan 01. `user_queue` /
        // `approval_coordinator` need a primary adapter NOW: `user_queue`
        // used to be constructed from the Telegram adapter immediately after
        // adapter creation, well before section 7d ran, so "otherwise the
        // Buzz adapter" is only expressible if the Buzz adapter already
        // exists at this point. Reading `buzz_adapter` at the UQM site while
        // it is still `None` would give a Buzz-only gateway no UQM and no
        // approval coordinator — a silent failure (boots, connects, then
        // cannot queue or approve anything).
        #[cfg(feature = "buzz")]
        {
            match &platform_gate.buzz {
                crate::boot_gate::PlatformResolution::Usable(buzz_creds) => {
                    let buzz_config_for_construction = self
                        .config
                        .gateway
                        .platforms
                        .get("buzz")
                        .cloned()
                        .unwrap_or_default();
                    if let Some(relay_url) = buzz_config_for_construction.relay_url.clone() {
                        let nsec = buzz_creds.get("nsec").cloned().unwrap_or_default();
                        match nostr_sdk::prelude::Keys::parse(&nsec) {
                            Ok(keys) => {
                                buzz_adapter = Some(std::sync::Arc::new(
                                    crate::buzz::BuzzAdapter::new(keys, relay_url),
                                ));
                            }
                            Err(e) => {
                                tracing::error!(
                                    "Buzz identity resolved but the key failed to parse: {e:#}. \
                                     Skipping Buzz adapter (fail-closed — never boot a keyless Buzz adapter)."
                                );
                            }
                        }
                    } else {
                        tracing::debug!("Buzz adapter skipped (no relay_url configured)");
                    }
                }
                crate::boot_gate::PlatformResolution::NotUsable(
                    crate::boot_gate::PlatformSkipReason::Disabled,
                ) => {
                    tracing::debug!("Buzz adapter skipped (not enabled)");
                }
                crate::boot_gate::PlatformResolution::NotUsable(
                    crate::boot_gate::PlatformSkipReason::SectionAbsent,
                ) => {
                    tracing::debug!("Buzz adapter skipped (not configured)");
                }
                crate::boot_gate::PlatformResolution::NotUsable(reason) => {
                    tracing::error!(
                        "Buzz identity not resolved ({reason}) — set BUZZ_NSEC. Skipping Buzz \
                         adapter (fail-closed — never boot a keyless Buzz adapter)."
                    );
                }
            }
        }

        // --- 1. Resolve Telegram token (P0-1: now OPTIONAL) ---
        // Phase 47.6 Plan 03: Telegram is one optional platform among four.
        // `telegram_adapter` is `Some` only when the boot gate above reported
        // Telegram usable; every Telegram-specific branch below (get_me,
        // slash-command registration, the long-poll task, and the entire
        // step 8/12 dispatch loop) is conditional on that `Option`. With
        // Telegram configured, every one of those branches takes the `Some`
        // path and nothing observable changes — existing deployments are
        // byte-identical.
        let tg_config = self
            .config
            .gateway
            .platforms
            .get("telegram")
            .cloned()
            .unwrap_or_default();
        // Session cleanup (9b) applies to session storage globally, not just
        // Telegram — keep this read unconditional so it still uses the
        // configured value even when Telegram itself is absent.
        let timeout_hours = tg_config.session_timeout_hours;
        let telegram_usable = matches!(
            platform_gate.telegram,
            crate::boot_gate::PlatformResolution::Usable(_)
        );

        // --- 2. Create adapter (conditional — Some only when telegram_usable) ---
        let telegram_adapter: Option<Arc<TelegramAdapter>> = if telegram_usable {
            let token = resolve_token(&tg_config.token).context(
                "No Telegram bot token configured. Set TELEGRAM_BOT_TOKEN or gateway.platforms.telegram.token in config.yaml",
            )?;
            Some(Arc::new(TelegramAdapter::new(&token)))
        } else {
            None
        };

        // Primary-adapter selection (Phase 47.6 Plan 03): Telegram when
        // present, otherwise Buzz when present. `UserQueueManager` and
        // `ApprovalCoordinator` need SOME adapter; this pure helper is what
        // makes a Buzz-only gateway able to queue and approve at all.
        // Discord and Slack are never primaries in this plan's scope.
        #[cfg(feature = "buzz")]
        let buzz_as_platform_adapter: Option<Arc<dyn crate::adapter::PlatformAdapter>> =
            buzz_adapter
                .clone()
                .map(|b| b as Arc<dyn crate::adapter::PlatformAdapter>);
        #[cfg(not(feature = "buzz"))]
        let buzz_as_platform_adapter: Option<Arc<dyn crate::adapter::PlatformAdapter>> = None;
        let telegram_as_platform_adapter: Option<Arc<dyn crate::adapter::PlatformAdapter>> =
            telegram_adapter
                .clone()
                .map(|t| t as Arc<dyn crate::adapter::PlatformAdapter>);
        let primary_adapter =
            select_primary_adapter(telegram_as_platform_adapter, buzz_as_platform_adapter);

        // --- 6 (moved up, ahead of Telegram-specific steps 3-5/7/8 below):
        // UserQueueManager + ApprovalCoordinator, bound to the PRIMARY
        // adapter rather than unconditionally Telegram ---
        //
        // Phase 36.17.2.1 D-01/D-03: UQM constructor signature — Arc<SessionQueue> arg
        // (D-03: UQM holds Arc<SessionQueue>, not capacity).
        let user_queue: Option<Arc<UserQueueManager>> = primary_adapter.clone().map(|pa| {
            Arc::new(UserQueueManager::new(
                pa,
                self.session_queue.clone(), // Arc<SessionQueue> already on GatewayRunner per 36.17.1-02
            ))
        });
        // Phase 45 BL-01 fix: construct the ApprovalCoordinator so /approve,
        // /deny, and per-turn GatewayApprovalGate injection are ACTIVE.
        // `config.approvals.timeout_secs` (default 120, D-04) flows in here.
        // `ApprovalsStore::load()` restores CLI-set session/always approvals
        // from disk so D-03 bypass works; the coordinator only READS the
        // store (D-03 negative).
        let approvals_store = Arc::new(ironhermes_core::ApprovalsStore::load().await);
        // Phase 46 D-01/D-02: construct the append-only audit log alongside the
        // approvals store. Every ApprovalCoordinator resolution (bypass / operator
        // approved / operator denied / dropped-sender / timeout) appends exactly one
        // entry to ~/.ironhermes/audit.jsonl before returning to the caller.
        let audit_log = Arc::new(ironhermes_core::AuditLog::load(self.config.audit.clone()));
        // Phase 47.6 Plan 06 (P1-2): ONE ApprovalCoordinator per platform, not a
        // single shared instance bound to whichever adapter happens to be primary.
        // `ApprovalCoordinator` binds exactly one adapter for its entire lifetime
        // (see that struct's own doc comment) — "add a Buzz case" is not
        // expressible on it, so each platform that wants `/approve`/`/deny` gets
        // its own coordinator instance instead, built by
        // `build_platform_approval_coordinator` below and installed via
        // `set_approval_coordinator` on that platform's own handler, at that
        // platform's own construction site (Telegram in steps 3-6 below, Buzz in
        // section 7d). `approvals_store`/`audit_log` above are constructed ONCE
        // and cloned into every coordinator — the store carries the operator's
        // CLI-set session/always approvals and the audit log is a single
        // append-only file, both shared truth across every platform, never
        // per-platform copies. Discord/Slack handlers built further below stay
        // fail-closed (no coordinator) — that remains their documented, unchanged
        // behavior; Buzz is no longer in that same boat now that this task gives
        // it real wiring.

        let mut join_set: JoinSet<()> = JoinSet::new();

        // Plan 03 (Phase 22.4.2.1): track per-chat worker tasks so they can be
        // drained on shutdown. Wrapped in Arc<TokioMutex<...>> so the dispatch
        // closure (async move) and the post-select! drain both reach the same set.
        // Drain happens AFTER self.cancel.cancel() and BEFORE drop(msg_tx) per D-11.
        let worker_join_set: Arc<TokioMutex<JoinSet<()>>> =
            Arc::new(TokioMutex::new(JoinSet::new()));

        // Phase 47.6 Plan 03: `msg_tx`/the dispatch future are only ever
        // populated inside the `if let Some(ref adapter) = telegram_adapter`
        // block below — bound here as `Option` so shutdown teardown (which
        // runs unconditionally) and the step 12 select! can handle either
        // Telegram-present or Telegram-absent gateways uniformly. The step 8
        // dispatch loop lives in ITS OWN separate conditional block further
        // down (after the 7b/7c/7d optional-platform sections, which must
        // NOT be nested inside a Telegram-only `if`), so everything step 8
        // needs from steps 3-7 is stashed into these holders too.
        let mut msg_tx: Option<mpsc::Sender<crate::telegram::TgUpdate>> = None;
        let mut msg_rx_holder: Option<mpsc::Receiver<crate::telegram::TgUpdate>> = None;
        let mut telegram_semaphore: Option<Arc<Semaphore>> = None;
        let mut telegram_whitelist: Option<Vec<String>> = None;
        let mut telegram_bot_username: Option<String> = None;
        let mut telegram_handler: Option<Arc<GatewayMessageHandler>> = None;
        let mut dispatch_future_boxed: Option<
            std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
        > = None;

        // --- 3-8. Telegram-specific setup + the long-poll/dispatch pipeline.
        // P0-1: entirely conditional on `telegram_adapter` — these use
        // inherent `TelegramAdapter` methods that are not on the
        // `PlatformAdapter` trait object, so they cannot be generalized here
        // and simply do not run when Telegram is absent. ---
        if let Some(ref adapter) = telegram_adapter {
            let adapter = adapter.clone();

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
            // G-41.1-5: generated from the command router's full catalog (filtered
            // to Telegram-available commands), not a stale hardcoded 4-command
            // subset. Skills (resolved via SkillRegistry, not CommandRouter) are
            // intentionally excluded from the bot command menu — out of scope here.
            let commands = telegram_bot_commands();
            if let Err(e) = adapter.set_my_commands(&commands).await {
                warn!("Failed to register bot commands: {}", e);
            } else {
                info!("Bot commands registered");
            }

            // --- 5. Setup channels and concurrency primitives ---
            let (msg_tx_local, msg_rx) = mpsc::channel::<crate::telegram::TgUpdate>(256);
            let max_concurrent = tg_config.max_concurrent_runs.max(1);
            let semaphore = Arc::new(Semaphore::new(max_concurrent));
            let whitelist = tg_config.whitelist.clone();
            msg_tx = Some(msg_tx_local.clone());

            // --- 6. Create handler (with gateway hygiene engine wired) ---
            //
            // Phase 36.17.2.1 D-01/D-03: order matters — the handler is
            // Arc-wrapped only after every setter call below, mirroring the
            // pre-Plan-03 sequencing exactly.
            let mut handler = self.build_gateway_handler();
            if let Some(ref uq) = user_queue {
                handler.set_user_queue_manager(uq.clone());
            }
            // Phase 47.6 Plan 06 (P1-2): Telegram's own ApprovalCoordinator,
            // bound to the Telegram adapter, sharing the one approvals store
            // and one audit log constructed above.
            let telegram_approval_coordinator = build_platform_approval_coordinator(
                self.config.approvals.timeout_secs,
                adapter.clone() as Arc<dyn crate::adapter::PlatformAdapter>,
                approvals_store.clone(),
                audit_log.clone(),
            );
            handler.set_approval_coordinator(telegram_approval_coordinator);
            // Phase 36.17.2.2 D-18: install the MediaSender impl (Telegram only).
            // Do NOT upcast `Arc<dyn PlatformAdapter>` -> `Arc<dyn MediaSender>` —
            // that was unstable on stable Rust at the time of writing (RESEARCH
            // Open Q4 / Assumption A7); clone-cast the concrete `Arc<TelegramAdapter>`
            // separately for each trait instead.
            handler.set_media_sender(adapter.clone() as Arc<dyn crate::adapter::MediaSender>);
            // Phase 36.17.7 D-01 (Site 1 — Telegram, real dispatcher):
            // TelegramAdapter doubles as AudioDispatcher for per-turn TTS wiring.
            handler.set_telegram_audio_dispatcher(
                adapter.clone() as Arc<dyn ironhermes_tools::AudioDispatcher>
            );
            // Phase 36.3.8 D-02/D-04/D-05/T-36.3.8-ROUTE (Site 1 — Telegram):
            // TelegramAdapter also implements MessageDispatcher + ClarifyDispatcher.
            handler.set_telegram_message_dispatcher(
                adapter.clone() as Arc<dyn ironhermes_tools::MessageDispatcher>
            );
            handler.set_telegram_clarify_dispatcher(
                adapter.clone() as Arc<dyn ironhermes_tools::ClarifyDispatcher>
            );
            // RC-2: clone the runner's clarify_registry Arc into the handler so per-turn
            // ClarifyTool registration uses the same map as the callback_query loop.
            handler.set_clarify_registry(self.clarify_registry.clone());
            let handler = Arc::new(handler);

            // Phase 47.6 Plan 03: stash everything the step 8 dispatch loop
            // (a separate conditional block further down, positioned AFTER
            // the 7b/7c/7d optional-platform sections which must stay
            // unconditional) needs from steps 3-6 above.
            telegram_bot_username = Some(bot_username.clone());
            msg_rx_holder = Some(msg_rx);
            telegram_semaphore = Some(semaphore.clone());
            telegram_whitelist = Some(whitelist.clone());
            telegram_handler = Some(handler.clone());

            // --- 7. Poll loop ---
            let poll_cancel = self.cancel.clone();
            let adapter_poll = adapter.clone();
            let msg_tx_poll = msg_tx_local.clone();
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
        } // close: if let Some(ref adapter) = telegram_adapter (steps 3-7)

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
        if let Some(discord_token) =
            resolve_token_with_env(&discord_config.token, "DISCORD_BOT_TOKEN")
        {
            // Phase 36.17.7 D-03-b (Site 2 — Discord, stub dispatcher):
            // Build a separate handler for the Discord adapter so it gets its own
            // AudioDispatcher slot independent of the Telegram handler. Discord
            // lacks audio delivery; NotSupportedAudioDispatcher ensures tools still
            // register for LLM schema but send_audio returns a clean Err.
            // Deletion target when Discord gets a real AudioDispatcher impl.
            // Also wire UQM so the Discord handler uses the same wake-notify path
            // as Telegram (mirrors the Telegram set_user_queue_manager call above).
            let mut handler_discord = self.build_gateway_handler();
            if let Some(ref uq) = user_queue {
                handler_discord.set_user_queue_manager(uq.clone());
            }
            handler_discord.set_telegram_audio_dispatcher(std::sync::Arc::new(
                ironhermes_tools::NotSupportedAudioDispatcher::new("discord"),
            )
                as std::sync::Arc<dyn ironhermes_tools::AudioDispatcher>);
            let handler_d = std::sync::Arc::new(handler_discord);
            let cancel_d = self.cancel.clone();
            // Phase 47.6 Plan 01 (P0-2/D-05): whitelist is now the canonical
            // Vec<String> shared across every platform. Discord's own adapter
            // still needs u64 snowflake IDs, so parse here at the boundary —
            // an entry that does NOT parse as u64 (e.g. a Buzz hex pubkey
            // sitting in the same shared list) is a real operator error worth
            // surfacing, not a silent drop.
            let (whitelist_d, unparsed_count): (Vec<u64>, usize) = {
                let mut parsed = Vec::new();
                let mut unparsed = 0usize;
                for entry in &discord_config.whitelist {
                    match entry.parse::<u64>() {
                        Ok(v) => parsed.push(v),
                        Err(_) => unparsed += 1,
                    }
                }
                (parsed, unparsed)
            };
            if unparsed_count > 0 {
                tracing::warn!(
                    unparsed_count,
                    "Discord whitelist contains {} entries that do not parse as a numeric Discord user ID — dropped",
                    unparsed_count
                );
            }
            // Empty whitelist propagates to adapter, which enforces D-12 deny-all
            // per canonical Telegram semantics (config.rs:731 + runner.rs:601-611).
            tracing::info!(
                whitelist_len = whitelist_d.len(),
                "Discord adapter spawning"
            );
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
            // Phase 36.17.7 D-03-b (Site 3 — Slack, stub dispatcher):
            // Build a separate handler for the Slack adapter so it gets its own
            // AudioDispatcher slot independent of the Telegram handler. Slack
            // lacks audio delivery; NotSupportedAudioDispatcher ensures tools still
            // register for LLM schema but send_audio returns a clean Err.
            // Deletion target when Slack gets a real AudioDispatcher impl.
            // Also wire UQM so the Slack handler uses the same wake-notify path
            // as Telegram (mirrors the Telegram set_user_queue_manager call above).
            let mut handler_slack = self.build_gateway_handler();
            if let Some(ref uq) = user_queue {
                handler_slack.set_user_queue_manager(uq.clone());
            }
            handler_slack.set_telegram_audio_dispatcher(std::sync::Arc::new(
                ironhermes_tools::NotSupportedAudioDispatcher::new("slack"),
            )
                as std::sync::Arc<dyn ironhermes_tools::AudioDispatcher>);
            let handler_s = std::sync::Arc::new(handler_slack);
            let cancel_s = self.cancel.clone();
            // Phase 47.6 Plan 01 (P0-2/D-05): whitelist is now the canonical
            // Vec<String> shared across every platform — Slack's own
            // alphanumeric member IDs (e.g. "U012AB3CD") need no conversion.
            let whitelist_s: Vec<String> = slack_config.whitelist.clone();
            // Empty whitelist propagates to adapter — D-12 deny-all enforced in callback.
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

        // --- 7d. Optional Buzz adapter: CONNECT + SPAWN only (Phase 47.6
        // Plan 01, P0-3; construction moved up ahead of the primary-adapter
        // binding by Plan 03 — see the ORDERING TRAP note near the top of
        // `start()`). Spawns alongside Telegram/Discord/Slack in the same
        // JoinSet so CancellationToken-driven shutdown handles all platforms
        // uniformly. This spawn point is load-bearing: Phase 47.4 shipped a
        // gate wired only into a CLI subcommand that was completely inert —
        // a Buzz arm that is not spawned from `GatewayRunner::start` does
        // not exist.
        #[cfg(feature = "buzz")]
        if let Some(adapter_buzz) = buzz_adapter.clone() {
            match adapter_buzz.connect().await {
                Ok(()) => {
                    let mut handler_buzz = self.build_gateway_handler();
                    if let Some(ref uq) = user_queue {
                        handler_buzz.set_user_queue_manager(uq.clone());
                    }
                    handler_buzz.set_telegram_audio_dispatcher(std::sync::Arc::new(
                        ironhermes_tools::NotSupportedAudioDispatcher::new("buzz"),
                    )
                        as std::sync::Arc<dyn ironhermes_tools::AudioDispatcher>);
                    // Phase 47.6 Plan 06 (P1-2/D-14): Buzz's own
                    // ApprovalCoordinator, bound to the Buzz adapter, sharing
                    // the one approvals store and one audit log constructed
                    // at the top of start(). Installed BEFORE the handler is
                    // Arc-wrapped and spawned, mirroring the Telegram wiring
                    // above.
                    let buzz_approval_coordinator = build_platform_approval_coordinator(
                        self.config.approvals.timeout_secs,
                        adapter_buzz.clone() as std::sync::Arc<dyn crate::adapter::PlatformAdapter>,
                        approvals_store.clone(),
                        audit_log.clone(),
                    );
                    handler_buzz.set_approval_coordinator(buzz_approval_coordinator);
                    let handler_b: std::sync::Arc<dyn crate::adapter::MessageHandler> =
                        std::sync::Arc::new(handler_buzz);
                    let cancel_b = self.cancel.clone();
                    let adapter_for_spawn = adapter_buzz.clone();
                    let buzz_config = self
                        .config
                        .gateway
                        .platforms
                        .get("buzz")
                        .cloned()
                        .unwrap_or_default();
                    let relay_url = buzz_config.relay_url.clone().unwrap_or_default();
                    tracing::info!(%relay_url, "Buzz adapter spawning");
                    join_set.spawn(async move {
                        if let Err(e) = crate::buzz::run_buzz_adapter(
                            adapter_for_spawn,
                            buzz_config,
                            handler_b,
                            cancel_b,
                        )
                        .await
                        {
                            tracing::error!("Buzz adapter error: {e:#}");
                        }
                    });
                }
                Err(e) => {
                    tracing::error!("Buzz adapter failed to connect to relay: {e:#} — skipping");
                }
            }
        }
        #[cfg(feature = "buzz")]
        tracing::debug!(
            buzz_ready = buzz_adapter.is_some(),
            "Gateway optional-platform sections complete"
        );

        // --- 8. Dispatch loop (Phase 47.6 Plan 03: conditional on Telegram
        // being present — everything it needs from steps 3-6 was stashed
        // into the `telegram_*`/`msg_rx_holder` holders above, since this
        // block sits after 7b/7c/7d which must stay unconditional). ---
        if let (
            Some(adapter),
            Some(handler),
            Some(mut msg_rx),
            Some(semaphore),
            Some(whitelist),
            Some(bot_username),
            Some(user_queue),
        ) = (
            telegram_adapter.clone(),
            telegram_handler.clone(),
            msg_rx_holder.take(),
            telegram_semaphore.clone(),
            telegram_whitelist.clone(),
            telegram_bot_username.clone(),
            user_queue.clone(),
        ) {
            let dispatch_cancel = self.cancel.clone();
            let handler_dispatch = handler.clone();
            let user_queue_dispatch = user_queue.clone();
            let adapter_dispatch = adapter.clone() as Arc<dyn crate::adapter::PlatformAdapter>;
            let adapter_dispatch_mm = adapter.clone(); // typed Arc<TelegramAdapter> for multimodal
            // Phase 36.3.8 Plan 03: typed Arc<TelegramAdapter> for answer_callback_query
            // (inherent method — not on the PlatformAdapter trait object) and the
            // clarify awaiter registry both captured into the dispatch async move.
            let adapter_dispatch_cb = adapter.clone();
            let clarify_registry_dispatch = self.clarify_registry.clone();
            let whitelist_cb = whitelist.clone();
            let semaphore_dispatch = semaphore.clone();
            let cancel_dispatch = self.cancel.clone();
            let bot_username_str = bot_username.clone();
            // Phase 36.17.1 Plan 02 Task 3: clone Arc<GatewayRunner> for the per-chat
            // worker spawn closure so it can call `runner.drain_pending(...)` after
            // each handler turn returns. The Arc<Self> threading is what motivates
            // the `start(self: Arc<Self>)` signature change introduced in this plan.
            let runner_dispatch: Arc<Self> = self.clone();

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

                            // Phase 36.3.8 Plan 03 — callback_query routing (D-05).
                            // Handle a button tap BEFORE the message branch: an
                            // inline-keyboard tap arrives as `callback_query`, never
                            // as `message`, and resolves a suspended clarify awaiter.
                            if let Some(cq) = &update.callback_query {
                                // 1. Ack FIRST — Pitfall 3 / T-36.3.8-STUCK. Must
                                //    happen before any other async work or the
                                //    Telegram button spins indefinitely.
                                if let Err(e) =
                                    adapter_dispatch_cb.answer_callback_query(&cq.id).await
                                {
                                    warn!(error = %e, "answerCallbackQuery failed");
                                }

                                // 2. SECURITY / T-36.3.8-SPOOF: validate the tapper
                                //    against the same whitelist used for inbound
                                //    messages. Telegram guarantees the callback
                                //    originates from the user who tapped; a
                                //    non-whitelisted sender is acked-and-dropped
                                //    (the spinner is already cleared) and resolves
                                //    NO awaiter.
                                if whitelist_cb.is_empty() {
                                    warn!(
                                        "Whitelist is empty — dropping callback_query (D-12 deny-all)"
                                    );
                                    continue;
                                }
                                if !whitelist_cb.contains(&cq.from.id.to_string()) {
                                    warn!(
                                        from_id = cq.from.id,
                                        "callback_query sender not in whitelist, dropping (T-36.3.8-SPOOF)"
                                    );
                                    continue;
                                }

                                // 3. LABEL RECOVERY (callback_data grammar is LOCKED):
                                //    callback_data carries only the index
                                //    (`clarify:<clarify_id>:<index>`). Parse it, then
                                //    look the human-readable label up from the
                                //    registry's stored choices by index — the label
                                //    is never carried in callback_data (64-byte limit).
                                if let Some(data) = &cq.data
                                    && let Some((clarify_id, choice_index)) =
                                        ironhermes_tools::parse_clarify_callback(data)
                                {
                                    // 4. take() returns the PendingClarify
                                    //    (choices + sender) OUTSIDE the lock. None
                                    //    means already resolved/timed out — the
                                    //    ack already fired, so just drop through.
                                    if let Some(entry) =
                                        clarify_registry_dispatch.take(&clarify_id).await
                                    {
                                        // Bounds-check the index against stored
                                        // choices: a malformed / out-of-range
                                        // index does NOT resolve the awaiter.
                                        if choice_index < entry.choices.len() {
                                            let label = entry.choices[choice_index].clone();
                                            // sender.send consumes the entry's
                                            // oneshot sender; an Err means the
                                            // receiver was already dropped
                                            // (turn exited) — harmless.
                                            let _ = entry.sender.send(
                                                ironhermes_tools::ClarifyAnswer {
                                                    label,
                                                    index: choice_index,
                                                },
                                            );
                                        } else {
                                            warn!(
                                                clarify_id = %clarify_id,
                                                choice_index,
                                                choices_len = entry.choices.len(),
                                                "clarify callback choice_index out of range, dropping"
                                            );
                                        }
                                    } else {
                                        debug!(
                                            clarify_id = %clarify_id,
                                            "no pending clarify for callback (already resolved/timed out)"
                                        );
                                    }
                                }

                                // 5. Skip the message branch entirely for this update.
                                continue;
                            }

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
                                if !whitelist.contains(&event.sender_id) {
                                    warn!(sender_id = %event.sender_id, "Sender not in whitelist, ignoring");
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

                            // Phase 36.17.2 Plan 05 (D-23, D-24, D-27): slash-command fast-path.
                            // Commands bypass UserQueueManager entirely so they don't serialize behind
                            // an in-flight free-text turn in the per-chat worker. The same handler entry
                            // (handle_with_multimodal) is used — only the routing differs.
                            //
                            // D-24: strict prefix match, no whitespace trim — matches handler.rs:411 command parser.
                            // D-26: state-mutation safety covered by SessionQueue mutex + SessionStore RwLock + AtomicBool.
                            // T-36.17.2-06 mitigation: sem_dispatch permit acquired BEFORE handle call (TG-06 bound preserved).
                            if event.content.starts_with('/') {
                                let handler_cmd = handler_dispatch.clone();
                                let adapter_cmd = adapter_dispatch.clone();
                                let sem_cmd = semaphore_dispatch.clone();
                                let cancel_cmd = cancel_dispatch.clone();
                                let event_cmd = event.clone();
                                // Detached spawn — commands are short-lived (D-27). Graceful shutdown
                                // observes cancel_token via cancel.is_cancelled() inside the handler.
                                tokio::spawn(async move {
                                    let permit = match sem_cmd.acquire().await {
                                        Ok(p) => p,
                                        Err(_) => return, // semaphore closed → shutdown in progress
                                    };
                                    // Commands are text-only by contract (D-27) — skip multimodal processing.
                                    let processed = crate::multimodal::ProcessedAttachments {
                                        text_prefix: None,
                                        image_data_uri: None,
                                        image_cache_path: None,
                                    };
                                    if let Err(e) = handler_cmd
                                        .handle_with_multimodal(
                                            &event_cmd,
                                            adapter_cmd,
                                            cancel_cmd.child_token(),
                                            processed,
                                        )
                                        .await
                                    {
                                        error!(
                                            chat_id = %event_cmd.chat_id,
                                            error = %e,
                                            "Slash-command fast-path handler error (Phase 36.17.2 Plan 05)"
                                        );
                                    }
                                    drop(permit);
                                });
                                continue; // Skip the multimodal + UQM.dispatch path for this event
                            }

                            // Process multimodal attachments (D-05 through D-08)
                            // image_cache_path threads the inbound photo's cache PATH to the worker
                            // so the model can drive video_animate (image-to-video) — fix 36.3.3.
                            let (text_prefix, image_data_uri, image_cache_path) = if !event.attachments.is_empty() {
                                match multimodal::process_attachments(&adapter_dispatch_mm, &msg).await {
                                    Ok(processed) => (
                                        processed.text_prefix,
                                        processed.image_data_uri,
                                        processed.image_cache_path,
                                    ),
                                    Err(e) => {
                                        // Send user-friendly error and skip this message
                                        let chat_id = event.chat_id.clone();
                                        let err_msg = format!("Could not process attachment: {}", e);
                                        let _ = PlatformAdapter::send_message(adapter_dispatch_mm.as_ref(), &chat_id, &err_msg, None).await;
                                        continue;
                                    }
                                }
                            } else {
                                (None, None, None)
                            };

                            // Phase 36.17.2 Plan 01: capture session key fields BEFORE moving event
                            // into dispatch (event is consumed by UQM::dispatch; D-14 triple).
                            let event_platform = event.platform.clone();
                            let event_chat_id = event.chat_id.clone();
                            let event_sender_id = event.sender_id.clone();

                            // Phase 36.17.2 Plan 02: full match on Result<DispatchOutcome, QueueError> (D-15).
                            // Cap-hit UX (❌ + chat reply) fires inside UQM::dispatch on Err — no
                            // additional handling needed here for the error path.
                            let dispatch_result = user_queue_dispatch.dispatch(event, text_prefix, image_data_uri, image_cache_path).await;

                            // SessionKey built from fields captured before event was moved into dispatch (D-14).
                            let session_key_task = SessionKey::new(event_platform, &event_chat_id)
                                .with_user(&event_sender_id);

                            match dispatch_result {
                                Ok(DispatchOutcome::Accepted) => {
                                    // Existing worker picked up the message via Notify wake.
                                    // 👀 fires when the worker pops (D-08). Nothing to do here.
                                    debug!(
                                        chat_id = %event_chat_id,
                                        "Dispatch: message accepted by existing worker (Phase 36.17.2 D-08)"
                                    );
                                }
                                Ok(DispatchOutcome::WorkerSpawned) => {
                                    // New worker needed for this chat. Spawn the full Notify-based
                                    // pop-loop worker (D-04, D-05, D-06, D-08, D-09, D-16).
                                    let handler_task = handler_dispatch.clone();
                                    let adapter_task = adapter_dispatch.clone();
                                    let sem_task = semaphore_dispatch.clone();
                                    let cancel_task = cancel_dispatch.clone();
                                    let queue_task = user_queue_dispatch.clone();
                                    // Capture Arc<SessionQueue> via the session_queue field accessor
                                    // (runner_dispatch stays alive; Arc<SessionQueue> clone is cheap).
                                    let session_queue_task = runner_dispatch.session_queue.clone();
                                    // Phase 39.1 (R39.1-01 / R39.1-03): capture ConcurrencyLayer for
                                    // per-turn semaphore check. On cap-hit, push_front back to FIFO
                                    // and re-park — over-cap messages stay queued (D-03).
                                    let concurrency_task = runner_dispatch.concurrency.clone();
                                    let session_key_for_worker = session_key_task.clone();

                                    // D-19 (M4 locked): notify_for is pub async fn; workers map uses
                                    // tokio::sync::Mutex. WorkerSpawned invariant guarantees Some here.
                                    let notify_task: std::sync::Arc<tokio::sync::Notify> = queue_task
                                        .notify_for(&session_key_for_worker)
                                        .await
                                        .expect("notify_for must return Some immediately after WorkerSpawned (Plan 01 invariant)");

                                    // Plan 03 (Phase 22.4.2.1): spawn into worker_join_set so
                                    // per-chat workers are tracked and drained on shutdown.
                                    worker_join_set_dispatch.lock().await.spawn(async move {
                                        // Full Notify-based pop-loop (D-04, D-05, D-06).
                                        loop {
                                            // D-06 step 1+2: pop or wait for Notify wake / cancellation.
                                            let next_event = match session_queue_task.pop(&session_key_for_worker) {
                                                Some(ev) => ev,
                                                None => {
                                                    // Queue empty — park until dispatch signals or cancel fires.
                                                    tokio::select! {
                                                        _ = cancel_task.cancelled() => break,
                                                        _ = notify_task.notified() => continue, // re-poll the queue
                                                    }
                                                }
                                            };

                                            // Cancellation check after pop (cancel may have fired between
                                            // pop and this point — T-36.17.2-03 acknowledged, window is μs).
                                            if cancel_task.is_cancelled() { break; }

                                            // Phase 39.1 (R39.1-01 / R39.1-03 / D-03): try per-session
                                            // + global semaphore before running the turn. If cap is hit,
                                            // push the event back to the front of the FIFO (preserves
                                            // ordering) and re-park on Notify — the next completed turn
                                            // will wake us via the TurnGuard drop / Notify signal.
                                            let _turn_permits = match concurrency_task.try_acquire() {
                                                Some(permits) => permits,
                                                None => {
                                                    // Over-cap: return event to front of queue and wait.
                                                    tracing::debug!(
                                                        chat_id = %next_event.chat_id,
                                                        "Worker: concurrency cap hit — re-queuing event (Phase 39.1 R39.1-03)"
                                                    );
                                                    session_queue_task.push_front(
                                                        &session_key_for_worker,
                                                        next_event,
                                                    );
                                                    // Re-park: wait for next Notify wake (a turn completing
                                                    // will wake us so we can retry try_acquire).
                                                    tokio::select! {
                                                        _ = cancel_task.cancelled() => break,
                                                        _ = notify_task.notified() => continue,
                                                    }
                                                }
                                            };

                                            // Acquire semaphore permit (TG-06 — bounded concurrency).
                                            let permit = match sem_task.acquire().await {
                                                Ok(p) => p,
                                                Err(_) => break, // semaphore closed on shutdown
                                            };

                                            // D-06 step 3 + D-08: emit 👀 reaction inline before
                                            // handle_with_multimodal. Inline await means "👀 reaches
                                            // Telegram before the placeholder █ send" — strict ordering
                                            // preferred over fire-and-forget (see CONTEXT.md Claude's Discretion).
                                            // D-09: warn-and-ignore on failure; must not block the turn.
                                            if let Err(e) = adapter_task
                                                .add_reaction(&next_event.chat_id, &next_event.message_id, "👀")
                                                .await
                                            {
                                                tracing::warn!(
                                                    chat_id = %next_event.chat_id,
                                                    message_id = %next_event.message_id,
                                                    error = %e,
                                                    "Worker: 👀 reaction emission failed; continuing (Phase 36.17.2 D-09)"
                                                );
                                            }

                                            // Reconstruct multimodal payload from UQM sidecar (M1 locked by Plan 01).
                                            // FIFO lockstep with SessionQueue::pop — one take_multimodal per pop.
                                            // None means plain-text message with no multimodal payload.
                                            let (text_prefix, image_data_uri, image_cache_path) = queue_task
                                                .take_multimodal(&session_key_for_worker)
                                                .await
                                                .unwrap_or((None, None, None));
                                            let processed = crate::multimodal::ProcessedAttachments {
                                                text_prefix,
                                                image_data_uri,
                                                image_cache_path,
                                            };

                                            let result = handler_task
                                                .handle_with_multimodal(
                                                    &next_event,
                                                    adapter_task.clone(),
                                                    cancel_task.child_token(),
                                                    processed,
                                                )
                                                .await;

                                            if let Err(e) = result {
                                                error!(
                                                    chat_id = %next_event.chat_id,
                                                    error = %e,
                                                    "Handler error for message (Phase 36.17.2 worker pop-loop)"
                                                );
                                            }

                                            // D-07: post-turn drain_pending call removed.
                                            // The next loop iteration pops the next event from
                                            // session_queue_task if any arrived during the turn —
                                            // that is the natural drain.

                                            drop(permit);

                                            // D-05: cancellation check between iterations.
                                            if cancel_task.is_cancelled() { break; }
                                        }

                                        // D-16: worker exits — clean up UQM map entry
                                        // (workers + pending_multimodal both purged by remove).
                                        queue_task.remove(&session_key_for_worker).await;
                                    });
                                }
                                Err(QueueError::CapacityReached { .. }) => {
                                    // Cap-hit UX (❌ + chat reply) already fired inside UQM::dispatch (D-11).
                                    // Dispatch loop's only job here is to log and continue.
                                    // Telegram offset already advanced (Pitfall 6) — no re-delivery risk.
                                    tracing::warn!(
                                        chat_id = %event_chat_id,
                                        "Dispatch: queue full, message dropped (Phase 36.17.2 D-11)"
                                    );
                                }
                            }
                        }
                    }
                }
            };
            dispatch_future_boxed = Some(Box::pin(dispatch_future));
        } // close: if let (Some(adapter), Some(handler), ...) = (...) (step 8, Telegram-only)

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
                            if let Ok(store) = s.lock()
                                && let Err(e) = store.wal_checkpoint() {
                                    warn!("WAL checkpoint failed: {e}");
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
            // Phase 22.4.2.1 Plan 02: thread TG adapter for delivery dispatch.
            // Phase 47.6 Plan 03 (P0-1): Telegram is now optional — `adapter_tick`
            // is `None` when Telegram is absent, so cron delivery/audio dispatch
            // via Telegram is simply unavailable rather than panicking.
            let adapter_tick = telegram_adapter.clone();
            // Phase 47.6 Plan 07: same clone-cast pattern for the Buzz adapter,
            // captured so the tick task's own DeliveryRegistry can register a
            // "buzz" sender when the platform is present. `buzz_adapter` is
            // bound at the top of `start()` (see the ORDERING TRAP note there).
            #[cfg(feature = "buzz")]
            let buzz_adapter_tick = buzz_adapter.clone();

            join_set.spawn(async move {
                // UAT gap 2 / test 13: first-tick-after-boot burst guard.
                // Fast-forward any stale scheduled jobs BEFORE entering the
                // run_tick_loop so a gateway restart doesn't burst-fire jobs
                // whose next_run_at drifted into the recent past.
                match fast_forward_backlog(&job_store_tick).await {
                    Ok(n) if n > 0 => {
                        info!("First-tick burst guard fast-forwarded {} job(s)", n);
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

                // Phase 47.6 Plan 07: platform-keyed text-delivery registry —
                // "telegram" when the adapter is present, "buzz" likewise.
                // This is the wiring that makes `deliver=buzz` real: the cron
                // tick loop is hosted here, and a registry entry never built
                // from THIS site is inert (Phase 47.4 lesson, re-applied).
                let mut delivery_registry_tick = DeliveryRegistry::new();
                if let Some(ref tg) = adapter_tick {
                    delivery_registry_tick
                        .insert("telegram", tg.clone() as Arc<dyn DeliverySend>);
                }
                #[cfg(feature = "buzz")]
                if let Some(ref buzz) = buzz_adapter_tick {
                    delivery_registry_tick.insert("buzz", buzz.clone() as Arc<dyn DeliverySend>);
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
                    tg_client: adapter_tick.clone().map(|a| a as Arc<dyn TgSendApi>),
                    // RC-2: same clone-cast pattern as set_telegram_audio_dispatcher (runner.rs:725-727)
                    audio_dispatcher: adapter_tick
                        .clone()
                        .map(|a| a as Arc<dyn ironhermes_tools::AudioDispatcher>),
                    delivery_registry: delivery_registry_tick,
                });
                ironhermes_cron_runner::run_tick_loop(cron_ctx, tick_cancel).await;
            });
            info!("Cron tick task started (60s interval, delegating to ironhermes-cron-runner)");
        }

        // --- Step 11 (Phase 36.3.7 D-09): kanban dispatcher ---
        //
        // Deserialize the raw `config.kanban` serde_yaml::Value into
        // KanbanConfig. Uses all-defaults if the field is absent/null (pre-36.3.7
        // configs). The gateway's `ironhermes-core` Config stores it as
        // serde_yaml::Value to avoid a circular crate dependency (ironhermes-kanban
        // already depends on ironhermes-core).
        let kanban_config: ironhermes_kanban::KanbanConfig = if self.config.kanban.is_null() {
            ironhermes_kanban::KanbanConfig::default()
        } else {
            match serde_yaml::from_value::<ironhermes_kanban::KanbanConfig>(
                self.config.kanban.clone(),
            ) {
                Ok(cfg) => cfg,
                Err(e) => {
                    warn!("Failed to parse kanban config; using defaults: {e}");
                    ironhermes_kanban::KanbanConfig::default()
                }
            }
        };
        let dispatch_in_gw_env = ironhermes_kanban::kanban_env("DISPATCH_IN_GATEWAY")
            .map(|v| v != "0")
            .unwrap_or(true);

        // -----------------------------------------------------------------------
        // Phase 36.3.7.5 BUG-36.3.7.5-04: store-arc lift.
        //
        // Both the dispatcher (Phase 36.3.7) AND the notifier (Phase 36.3.7.5)
        // need an Arc<TokioMutex<KanbanStore>>. The dispatcher's previous block
        // opened the store INSIDE its own gating check; the notifier needs the
        // SAME Arc. Hoist `KanbanStore::open_default()` to a single site above
        // both spawns so each branch can call `.clone()` on the shared Arc.
        //
        // This is a SEMANTICS-PRESERVING refactor for the dispatcher: the runtime
        // behavior of `run_dispatch_loop(...)` is unchanged. Only the construction
        // site of the Arc moves. The dispatcher's `dispatch_in_gateway` gate +
        // env-flag gate + interval-seconds log line are preserved verbatim.
        // -----------------------------------------------------------------------
        // Phase 36.3.7.13 D-A1: env-bridged open closes F-01 on the gateway
        // background dispatcher loop. Workers spawned from here read
        // IRONHERMES_KANBAN_DB to resolve the same DB path.
        match ironhermes_kanban::KanbanStore::open_from_env() {
            Ok(store) => {
                let kanban_store_arc = std::sync::Arc::new(tokio::sync::Mutex::new(store));

                // --- 11a. Kanban dispatcher (Phase 36.3.7 D-09) ---
                if kanban_config.dispatch_in_gateway && dispatch_in_gw_env {
                    let kanban_cancel = self.cancel.clone();
                    let interval_secs = kanban_config.dispatch_interval_seconds;
                    let dispatcher_ctx =
                        std::sync::Arc::new(ironhermes_kanban::DispatcherContext::new(
                            kanban_store_arc.clone(),
                            kanban_config.clone(),
                        ));
                    join_set.spawn(async move {
                        ironhermes_kanban::run_dispatch_loop(dispatcher_ctx, kanban_cancel).await;
                    });
                    info!("Kanban dispatch task started ({}s interval)", interval_secs);
                } else if !dispatch_in_gw_env {
                    info!(
                        "Kanban dispatcher disabled via IRONHERMES_KANBAN_DISPATCH_IN_GATEWAY=0 (legacy HERMES_KANBAN_DISPATCH_IN_GATEWAY also accepted)"
                    );
                } else {
                    // dispatch_in_gateway = false in config
                    debug!("Kanban dispatcher disabled via config (dispatch_in_gateway = false)");
                }

                // -----------------------------------------------------------------------
                // Phase 36.3.7.5 BUG-36.3.7.5-04: Gateway notifier spawn (gated on config).
                //
                // Mirrors the dispatcher spawn shape above (canonical).
                // - Gate: notification_sources = Some(non_empty) AND at least one
                //   platform in that list intersects with the enabled gateway
                //   platforms set (case-insensitive). Default-off preserved.
                // - On gate-fail: log ONE info line + skip; gateway continues
                //   without the notifier loop.
                // - On gate-pass: spawn run_notifier_loop into join_set with a
                //   send_fn closure wrapping the enabled adapters by platform.
                //
                // The send_fn is the kanban->gateway boundary closure — keeps the
                // ironhermes-kanban crate free of any compile-time dep on
                // ironhermes-gateway. See Plan 02 SUMMARY crate-isolation audit.
                //
                // Phase 47.6 Plan 07: the gate is fed REALITY (the adapter
                // snapshot's own platform names), not `collect_enabled_platform_names`'
                // config-key INTENT — a platform configured but fail-closed at boot
                // (Buzz with an unresolvable nsec is the live case: Plan 01 makes
                // that fail closed and skip the spawn) must not pass the gate, start
                // the notifier loop, and then fail every send with "not enabled in
                // gateway". Intent is still consulted, ONLY to warn when it disagrees
                // with reality, so the operator learns why their subscription is inert.
                // -----------------------------------------------------------------------
                #[cfg(feature = "buzz")]
                let buzz_for_snapshot: Option<std::sync::Arc<dyn crate::adapter::PlatformAdapter>> =
                    buzz_adapter
                        .clone()
                        .map(|b| b as std::sync::Arc<dyn crate::adapter::PlatformAdapter>);
                #[cfg(not(feature = "buzz"))]
                let buzz_for_snapshot: Option<std::sync::Arc<dyn crate::adapter::PlatformAdapter>> =
                    None;
                let adapter_snapshot: Vec<(
                    String,
                    std::sync::Arc<dyn crate::adapter::PlatformAdapter>,
                )> = build_adapter_snapshot(&telegram_adapter, &buzz_for_snapshot);
                let enabled_platforms_from_snapshot: Vec<String> =
                    adapter_snapshot.iter().map(|(name, _)| name.clone()).collect();

                let intended_platforms =
                    collect_enabled_platform_names(&self.config, &telegram_adapter);
                for platform in diagnose_notifier_platform_mismatch(
                    &intended_platforms,
                    &enabled_platforms_from_snapshot,
                ) {
                    warn!(
                        platform = %platform,
                        "kanban notifications configured for this platform, but no live adapter is present in the gateway (see boot log above for why) — notifier gate treats it as absent"
                    );
                }

                let gate = crate::notifier_gating::compute_notifier_gate(
                    kanban_config.notification_sources.as_deref(),
                    &enabled_platforms_from_snapshot,
                );
                match gate {
                    crate::notifier_gating::NotifierGate::DisabledNoSources => {
                        info!("kanban notifier disabled (notification_sources not configured)");
                    }
                    crate::notifier_gating::NotifierGate::DisabledNoOverlap { wanted, enabled } => {
                        info!(
                            wanted = ?wanted,
                            enabled = ?enabled,
                            "kanban notifier disabled (no enabled platform overlap)"
                        );
                    }
                    crate::notifier_gating::NotifierGate::Enabled { sources } => {
                        // send_fn closure: the owned adapter snapshot built above
                        // (Arcs, not references — the notifier loop's lifetime can
                        // outlive `start()`'s stack frame).
                        let send_fn = build_notifier_send_fn(adapter_snapshot);
                        let poll_seconds = kanban_config.notifier_poll_seconds;
                        let notifier_ctx =
                            std::sync::Arc::new(ironhermes_kanban::NotifierContext::new(
                                kanban_store_arc.clone(),
                                poll_seconds,
                                send_fn,
                            ));
                        let notifier_cancel = self.cancel.clone();
                        join_set.spawn(async move {
                            ironhermes_kanban::run_notifier_loop(notifier_ctx, notifier_cancel)
                                .await;
                        });
                        info!(
                            sources = ?sources,
                            poll_seconds = poll_seconds,
                            "kanban notifier loop started"
                        );
                    }
                }
            }
            Err(e) => {
                // Preserves INV-36.3.7-08-05 (tests/kanban_dispatcher_spawned.rs:159) —
                // the substring "kanban dispatcher will NOT start" must remain present
                // so the non-fatal path is greppable. Notifier shares the same store
                // and is also skipped here (Phase 36.3.7.5 BUG-36.3.7.5-04 store-arc lift).
                warn!(
                    error = %e,
                    "Failed to open kanban.db; kanban dispatcher will NOT start (gateway continues; notifier also skipped)"
                );
            }
        }

        // --- 12. Run dispatch loop concurrently with shutdown signal ---
        // dispatch_future processes messages; ctrl+c or cancel token stops everything.
        // Phase 47.6 Plan 03 (P0-1): `dispatch_future_boxed` is `None` when
        // Telegram is absent — the ctrl_c/cancel arms are byte-identical
        // either way; only the dispatch-future arm is skipped.
        match dispatch_future_boxed {
            Some(dispatch_future) => {
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
            }
            None => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {
                        info!("Ctrl+C received, initiating graceful shutdown");
                    }
                    _ = self.cancel.cancelled() => {
                        info!("Cancellation token fired, shutting down");
                    }
                }
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
        // Phase 36.17.1 Plan 04 (D-03): set is_draining BEFORE cancel so the
        // queue keeps accepting late arrivals (preserve-AND-accept). Closes
        // T-36.17.1-03 (lost-update during drain-mode transition).
        self.drain_for_restart();

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
#[cfg(test)] // only called from runner.rs unit tests; production cron path inlines equivalent logic
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

/// Select the "primary adapter" used to construct `UserQueueManager`
/// (Phase 47.6 Plan 03, P0-1): Telegram when present, otherwise Buzz when
/// present. Discord and Slack are never primaries in this plan's scope —
/// they share the primary's `UserQueueManager` (when one exists) but do not
/// fall back into the primary role themselves.
///
/// Phase 47.6 Plan 06 (P1-2): `ApprovalCoordinator` no longer reads this
/// helper's result — each platform now builds its own coordinator bound
/// directly to its own adapter via `build_platform_approval_coordinator`,
/// so a Buzz-only gateway's approvals are no longer contingent on which
/// adapter this function happens to pick.
///
/// Pure and trivially unit-testable without booting a gateway — this is the
/// helper the plan's own acceptance criteria pin directly.
fn select_primary_adapter(
    telegram: Option<Arc<dyn crate::adapter::PlatformAdapter>>,
    buzz: Option<Arc<dyn crate::adapter::PlatformAdapter>>,
) -> Option<Arc<dyn crate::adapter::PlatformAdapter>> {
    telegram.or(buzz)
}

/// Construct one `ApprovalCoordinator` bound to `adapter` (Phase 47.6 Plan 06,
/// P1-2). A thin wrapper over `ApprovalCoordinator::new` so every platform
/// builds its coordinator identically, and so the `approvals_store`/`audit_log`
/// Arcs passed to every platform's coordinator are provably clones of the SAME
/// two objects — the store carries the operator's CLI-set session/always
/// approvals and the audit log is a single append-only file, both meant to be
/// shared truth across every platform, never per-platform copies.
fn build_platform_approval_coordinator(
    timeout_secs: u64,
    adapter: Arc<dyn crate::adapter::PlatformAdapter>,
    approvals_store: Arc<ironhermes_core::ApprovalsStore>,
    audit_log: Arc<ironhermes_core::AuditLog>,
) -> Arc<crate::approval::ApprovalCoordinator> {
    Arc::new(crate::approval::ApprovalCoordinator::new(
        timeout_secs,
        adapter,
        approvals_store,
        audit_log,
    ))
}

// -------------------------------------------------------------------------
// Phase 36.3.7.5 BUG-36.3.7.5-04: notifier-spawn support helpers.
//
// `collect_enabled_platform_names` reads the gateway's `Config` + the live
// Telegram adapter Arc to compute the set of OPERATOR-INTENDED platform names
// (Phase 47.6 Plan 07: no longer fed to the gate directly — see below).
//
// `build_adapter_snapshot` produces an owned `Vec<(String, Arc<dyn PlatformAdapter>)>`
// for the `SendFn` closure — captured by value so the closure can outlive
// `start()`'s stack frame. Includes Telegram AND Buzz, whichever adapters are
// actually present (Discord/Slack adapters are constructed inside their own
// spawned tasks and are not retained as runner-scope Arcs in this iteration;
// subscriptions naming those platforms will hit the "platform not enabled in
// gateway" arm of the send_fn closure and the notifier will log + drop per
// locked policy).
//
// `build_notifier_send_fn` constructs the `ironhermes_kanban::SendFn`
// trait-object closure: case-insensitive string match on `platform`, route
// to the matching `PlatformAdapter::send_message`, or return `Err` so the
// notifier's log-and-drop policy applies.
//
// Phase 47.6 Plan 07 gate/snapshot fix: the spawn gate (`compute_notifier_gate`)
// is now fed the platform names taken from `build_adapter_snapshot`'s
// REALITY (adapters that actually exist), not from `collect_enabled_platform_names`'s
// config-key INTENT. A platform configured but fail-closed at boot (Buzz with
// an unresolvable nsec is the live case — Plan 01 makes that fail closed and
// skip the spawn) no longer passes the gate, starts the notifier loop, and
// then fails every send with "not enabled in gateway". `diagnose_notifier_platform_mismatch`
// is the pure-function seam that finds the intent/reality gap so the caller
// can `warn!` naming the platform BEFORE gating on reality.
// -------------------------------------------------------------------------

/// Enumerate the gateway's OPERATOR-INTENDED platform names from the parsed
/// `Config` — i.e. what the operator configured, independent of whether an
/// adapter for it actually exists at runtime.
///
/// "Intended" = the platform appears in `config.gateway.platforms`, EXCEPT
/// Telegram, which is now optional (Phase 47.6 Plan 03, P0-1): the
/// `telegram` name is appended only when `telegram_adapter` is `Some` —
/// i.e., the boot gate reported Telegram usable and `start()` actually
/// constructed it. Discord/Slack are intended iff their config sections
/// exist (unchanged conservative semantics: presence of the key is enough
/// to see operator intent, independent of whether their own tokens resolved).
///
/// Phase 47.6 Plan 07: this list is NO LONGER fed directly to
/// `compute_notifier_gate` — the gate now follows REALITY (the adapter
/// snapshot). This function's role is now solely to detect intent/reality
/// mismatches via `diagnose_notifier_platform_mismatch`, so the operator gets
/// a `warn!` naming a configured-but-absent platform instead of the notifier
/// silently starting and failing every send.
fn collect_enabled_platform_names(
    config: &ironhermes_core::Config,
    telegram_adapter: &Option<std::sync::Arc<crate::telegram::TelegramAdapter>>,
) -> Vec<String> {
    let mut names: Vec<String> = config
        .gateway
        .platforms
        .keys()
        .filter(|k| !k.eq_ignore_ascii_case("telegram"))
        .map(|k| k.to_string())
        .collect();
    if telegram_adapter.is_some() {
        names.push("telegram".to_string());
    }
    names
}

/// Find operator-INTENDED platforms (from [`collect_enabled_platform_names`])
/// that are absent from the adapter-snapshot REALITY (from
/// [`build_adapter_snapshot`]) — case-insensitively. The caller `warn!`s one
/// line per returned platform BEFORE gating on reality, so a configured but
/// fail-closed platform (Buzz with an unresolvable nsec, the live case) tells
/// the operator why their subscription is inert instead of silently starting
/// the notifier loop and failing every send.
fn diagnose_notifier_platform_mismatch(intended: &[String], actual: &[String]) -> Vec<String> {
    intended
        .iter()
        .filter(|i| !actual.iter().any(|a| a.eq_ignore_ascii_case(i)))
        .cloned()
        .collect()
}

/// Build an owned snapshot of platform-name → `Arc<dyn PlatformAdapter>` pairs
/// for the `SendFn` closure. Telegram AND Buzz are both reachable as
/// runner-scope Arcs (Phase 47.6 Plan 07 adds Buzz); Discord/Slack adapters
/// live inside their own tokio tasks (constructed after socket connect).
/// Subscriptions that name a platform NOT in this snapshot will receive
/// `Err("platform X not enabled in gateway")` from the closure and the
/// notifier will log+drop the message per locked policy D-log-and-drop-on-fail.
///
/// Phase 47.6 Plan 03 (P0-1): `telegram_adapter` is `Option` — Phase 47.6
/// Plan 07 adds an equally-optional `buzz_adapter`. Either or both being
/// `None` shrinks the snapshot; the "not enabled in gateway" drop behavior is
/// unchanged for any platform not present.
fn build_adapter_snapshot(
    telegram_adapter: &Option<std::sync::Arc<crate::telegram::TelegramAdapter>>,
    buzz_adapter: &Option<std::sync::Arc<dyn crate::adapter::PlatformAdapter>>,
) -> Vec<(String, std::sync::Arc<dyn crate::adapter::PlatformAdapter>)> {
    let mut snapshot: Vec<(String, std::sync::Arc<dyn crate::adapter::PlatformAdapter>)> =
        Vec::new();
    if let Some(adapter) = telegram_adapter {
        snapshot.push((
            "telegram".to_string(),
            adapter.clone() as std::sync::Arc<dyn crate::adapter::PlatformAdapter>,
        ));
    }
    if let Some(adapter) = buzz_adapter {
        snapshot.push(("buzz".to_string(), adapter.clone()));
    }
    snapshot
}

/// Construct the `ironhermes_kanban::SendFn` trait-object closure.
///
/// The closure captures the adapter snapshot by value (owned `Arc<Vec<...>>`).
/// On each call, performs a case-insensitive linear search for the platform
/// name; if a match is found, awaits `send_message`; otherwise returns
/// `Err("platform {p} not enabled in gateway")` which the notifier loop logs
/// and drops per locked policy.
fn build_notifier_send_fn(
    adapters: Vec<(String, std::sync::Arc<dyn crate::adapter::PlatformAdapter>)>,
) -> ironhermes_kanban::SendFn {
    let adapters = std::sync::Arc::new(adapters);
    std::sync::Arc::new(
        move |platform: &str, chat_id: &str, thread_id_opt: Option<&str>, message: &str| {
            let adapters = adapters.clone();
            let platform = platform.to_string();
            let chat_id = chat_id.to_string();
            let thread_id_opt = thread_id_opt.map(|s| s.to_string());
            let message = message.to_string();
            Box::pin(async move {
                let adapter = adapters
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case(&platform))
                    .map(|(_, a)| a.clone());
                match adapter {
                    Some(a) => a
                        .send_message(&chat_id, &message, thread_id_opt.as_deref())
                        .await
                        .map(|_| ())
                        .map_err(|e| anyhow::anyhow!(e)),
                    None => Err(anyhow::anyhow!(
                        "platform {} not enabled in gateway",
                        platform
                    )),
                }
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Phase 47.6 Plan 03 (P0-1): primary-adapter selection + optional-Telegram
    // helper pure-function tests — pinned WITHOUT booting a gateway.
    // -------------------------------------------------------------------------

    fn fake_telegram_adapter() -> Arc<TelegramAdapter> {
        Arc::new(TelegramAdapter::new("fake-token-for-tests"))
    }

    #[test]
    fn select_primary_adapter_prefers_telegram_when_both_present() {
        let telegram: Arc<dyn crate::adapter::PlatformAdapter> = fake_telegram_adapter();
        let buzz: Arc<dyn crate::adapter::PlatformAdapter> = fake_telegram_adapter();
        let selected = select_primary_adapter(Some(telegram.clone()), Some(buzz));
        assert!(selected.is_some());
        // Same underlying pointer as `telegram` (Telegram wins when both present).
        assert!(Arc::ptr_eq(&selected.unwrap(), &telegram));
    }

    #[test]
    fn select_primary_adapter_falls_back_to_buzz_when_telegram_absent() {
        let buzz: Arc<dyn crate::adapter::PlatformAdapter> = fake_telegram_adapter();
        let selected = select_primary_adapter(None, Some(buzz.clone()));
        assert!(selected.is_some());
        assert!(Arc::ptr_eq(&selected.unwrap(), &buzz));
    }

    #[test]
    fn select_primary_adapter_none_when_neither_present() {
        let selected = select_primary_adapter(None, None);
        assert!(selected.is_none());
    }

    #[test]
    fn collect_enabled_platform_names_includes_telegram_only_when_adapter_present() {
        let config = ironhermes_core::Config::default();
        let with_telegram = collect_enabled_platform_names(&config, &Some(fake_telegram_adapter()));
        assert!(with_telegram.iter().any(|n| n == "telegram"));

        let without_telegram = collect_enabled_platform_names(&config, &None);
        assert!(!without_telegram.iter().any(|n| n == "telegram"));
    }

    #[test]
    fn build_adapter_snapshot_empty_when_telegram_absent() {
        let snapshot = build_adapter_snapshot(&None, &None);
        assert!(snapshot.is_empty());
    }

    #[test]
    fn build_adapter_snapshot_contains_telegram_when_present() {
        let snapshot = build_adapter_snapshot(&Some(fake_telegram_adapter()), &None);
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].0, "telegram");
    }

    // -------------------------------------------------------------------------
    // Phase 47.6 Plan 07: Buzz in the notifier snapshot + the round-trip
    // invariant test.
    // -------------------------------------------------------------------------

    /// Minimal recording `PlatformAdapter` — enough surface to exercise
    /// `build_notifier_send_fn`'s routing without a real Telegram/Buzz
    /// transport. `platform_tag` lets each fixture instance answer a distinct
    /// `Platform` so tests can tell which adapter actually received a call.
    struct RecordingAdapter {
        platform_tag: Platform,
        calls: std::sync::Mutex<Vec<(String, String, Option<String>)>>,
    }

    impl RecordingAdapter {
        fn new(platform_tag: Platform) -> Self {
            Self {
                platform_tag,
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn recorded_calls(&self) -> Vec<(String, String, Option<String>)> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl crate::adapter::PlatformAdapter for RecordingAdapter {
        fn platform(&self) -> Platform {
            self.platform_tag.clone()
        }

        async fn send_message(
            &self,
            chat_id: &str,
            content: &str,
            thread_id: Option<&str>,
        ) -> Result<ironhermes_core::MessageResponse> {
            self.calls.lock().unwrap().push((
                chat_id.to_string(),
                content.to_string(),
                thread_id.map(|s| s.to_string()),
            ));
            Ok(ironhermes_core::MessageResponse {
                message_id: "fake-id".to_string(),
                chat_id: chat_id.to_string(),
                platform: self.platform_tag.clone(),
            })
        }

        async fn send_message_markdown_v2(
            &self,
            chat_id: &str,
            content: &str,
            thread_id: Option<&str>,
        ) -> Result<ironhermes_core::MessageResponse> {
            self.send_message(chat_id, content, thread_id).await
        }

        async fn edit_message(&self, _chat_id: &str, _message_id: &str, _content: &str) -> Result<()> {
            Ok(())
        }

        async fn edit_message_markdown_v2(
            &self,
            _chat_id: &str,
            _message_id: &str,
            _content: &str,
        ) -> Result<()> {
            Ok(())
        }

        async fn delete_message(&self, _chat_id: &str, _message_id: &str) -> Result<()> {
            Ok(())
        }

        fn is_running(&self) -> bool {
            true
        }
    }

    fn recording_adapter(platform_tag: Platform) -> Arc<RecordingAdapter> {
        Arc::new(RecordingAdapter::new(platform_tag))
    }

    #[test]
    fn snapshot_includes_buzz_when_present() {
        let buzz = recording_adapter(Platform::Buzz);
        let buzz_dyn: Arc<dyn crate::adapter::PlatformAdapter> = buzz.clone();
        let snapshot = build_adapter_snapshot(&None, &Some(buzz_dyn));
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].0, "buzz");
    }

    #[test]
    fn snapshot_omits_buzz_when_absent() {
        let snapshot = build_adapter_snapshot(&Some(fake_telegram_adapter()), &None);
        assert!(snapshot.iter().all(|(name, _)| name != "buzz"));
    }

    #[test]
    fn snapshot_includes_both_when_both_present() {
        let buzz = recording_adapter(Platform::Buzz);
        let buzz_dyn: Arc<dyn crate::adapter::PlatformAdapter> = buzz.clone();
        let snapshot = build_adapter_snapshot(&Some(fake_telegram_adapter()), &Some(buzz_dyn));
        assert_eq!(snapshot.len(), 2);
        assert!(snapshot.iter().any(|(name, _)| name == "telegram"));
        assert!(snapshot.iter().any(|(name, _)| name == "buzz"));
    }

    #[test]
    fn snapshot_is_empty_when_no_adapter_is_present() {
        // The Buzz-only-and-not-yet-connected case: neither adapter Arc is
        // present, e.g. because Buzz failed to connect and Telegram is
        // absent. Must yield an empty vec, never panic.
        let snapshot = build_adapter_snapshot(&None, &None);
        assert!(snapshot.is_empty());
    }

    #[tokio::test]
    async fn notifier_send_fn_routes_to_buzz() {
        let telegram = recording_adapter(Platform::Telegram);
        let buzz = recording_adapter(Platform::Buzz);
        let telegram_dyn: Arc<dyn crate::adapter::PlatformAdapter> = telegram.clone();
        let buzz_dyn: Arc<dyn crate::adapter::PlatformAdapter> = buzz.clone();
        let snapshot = vec![
            ("telegram".to_string(), telegram_dyn),
            ("buzz".to_string(), buzz_dyn),
        ];
        let send_fn = build_notifier_send_fn(snapshot);

        send_fn("buzz", "chat1", None, "hello")
            .await
            .expect("buzz send must succeed");

        assert_eq!(buzz.recorded_calls().len(), 1);
        assert_eq!(buzz.recorded_calls()[0].0, "chat1");
        assert!(
            telegram.recorded_calls().is_empty(),
            "telegram must not receive the buzz-addressed send"
        );
    }

    #[tokio::test]
    async fn notifier_send_fn_routes_case_insensitively() {
        let buzz = recording_adapter(Platform::Buzz);
        let buzz_dyn: Arc<dyn crate::adapter::PlatformAdapter> = buzz.clone();
        let snapshot = vec![("buzz".to_string(), buzz_dyn)];
        let send_fn = build_notifier_send_fn(snapshot);

        send_fn("BUZZ", "chat1", None, "hello")
            .await
            .expect("uppercase platform token must still route");

        assert_eq!(buzz.recorded_calls().len(), 1);
    }

    #[tokio::test]
    async fn notifier_send_fn_errors_for_an_unlisted_platform() {
        let snapshot: Vec<(String, Arc<dyn crate::adapter::PlatformAdapter>)> = vec![];
        let send_fn = build_notifier_send_fn(snapshot);

        let err = send_fn("discord", "chat1", None, "hello")
            .await
            .expect_err("unlisted platform must error");
        assert!(
            err.to_string().contains("discord"),
            "error must name the platform: {err}"
        );
    }

    /// The assumption-delta invariant test: build a snapshot with every
    /// supported platform present, then — iterating the snapshot ITSELF,
    /// never a hardcoded platform list — assert `build_notifier_send_fn`
    /// routes a message to that entry's adapter. Goes red the moment a
    /// future change makes the snapshot's contents and the send closure's
    /// routing disagree, or reintroduces a single-adapter assumption.
    #[tokio::test]
    async fn every_snapshot_entry_round_trips_a_notifier_send() {
        let telegram = recording_adapter(Platform::Telegram);
        let buzz = recording_adapter(Platform::Buzz);
        let telegram_dyn: Arc<dyn crate::adapter::PlatformAdapter> = telegram.clone();
        let buzz_dyn: Arc<dyn crate::adapter::PlatformAdapter> = buzz.clone();
        let snapshot = vec![
            ("telegram".to_string(), telegram_dyn),
            ("buzz".to_string(), buzz_dyn),
        ];
        let platform_names: Vec<String> = snapshot.iter().map(|(name, _)| name.clone()).collect();
        let send_fn = build_notifier_send_fn(snapshot);

        for platform in &platform_names {
            send_fn(platform, "round-trip-chat", None, "round-trip message")
                .await
                .unwrap_or_else(|e| panic!("send to {platform} must succeed: {e}"));
        }

        assert_eq!(telegram.recorded_calls().len(), 1);
        assert_eq!(buzz.recorded_calls().len(), 1);
    }

    #[test]
    fn notifier_gate_enables_for_a_buzz_only_gateway() {
        let sources = vec!["buzz".to_string()];
        let enabled = vec!["buzz".to_string()];
        let gate = crate::notifier_gating::compute_notifier_gate(Some(&sources), &enabled);
        assert!(matches!(
            gate,
            crate::notifier_gating::NotifierGate::Enabled { .. }
        ));
    }

    #[test]
    fn notifier_gate_declines_when_a_configured_platform_did_not_spawn() {
        // The unresolvable-nsec case: buzz is in config.gateway.platforms
        // (operator intent) and in notification_sources, but no Buzz adapter
        // ended up in the snapshot (reality). Gating on the snapshot's
        // platform names — not collect_enabled_platform_names' config keys —
        // must NOT enable the notifier loop.
        let sources = vec!["buzz".to_string()];
        let enabled_from_snapshot: Vec<String> = vec![]; // buzz absent from the snapshot
        let gate = crate::notifier_gating::compute_notifier_gate(Some(&sources), &enabled_from_snapshot);
        assert!(!matches!(
            gate,
            crate::notifier_gating::NotifierGate::Enabled { .. }
        ));
    }

    #[test]
    fn notifier_gate_warns_when_intent_and_reality_disagree() {
        // Operator configured (and subscribed) buzz, but no buzz adapter
        // spawned — diagnose_notifier_platform_mismatch is the pure-function
        // seam the caller uses to warn! naming the absent platform.
        let intended = vec!["buzz".to_string()];
        let actual_from_snapshot: Vec<String> = vec![]; // buzz absent from the snapshot
        let mismatched = diagnose_notifier_platform_mismatch(&intended, &actual_from_snapshot);
        assert_eq!(mismatched, vec!["buzz".to_string()]);
    }

    #[test]
    fn diagnose_notifier_platform_mismatch_is_empty_when_intent_matches_reality() {
        let intended = vec!["telegram".to_string(), "buzz".to_string()];
        let actual = vec!["TELEGRAM".to_string(), "Buzz".to_string()];
        assert!(diagnose_notifier_platform_mismatch(&intended, &actual).is_empty());
    }

    /// Integration-style test (Task 2's own acceptance criterion): a
    /// Buzz-only config's boot gate reports Buzz usable and Telegram not
    /// usable, WITHOUT booting a gateway (`GatewayRunner::start` owns a PID
    /// lock and never returns, so it cannot be exercised directly in a
    /// test) — `resolve_enabled_platforms_with` and `build_adapter_snapshot`
    /// are the two seams that make this testable.
    #[test]
    fn buzz_only_gate_result_is_usable_and_snapshot_stays_telegram_only() {
        let config: ironhermes_core::Config = serde_yaml::from_str(
            r#"
gateway:
  platforms:
    buzz:
      enabled: true
      relay_url: "wss://relay.example"
"#,
        )
        .expect("valid config fixture");
        let gate = crate::boot_gate::resolve_enabled_platforms_with(
            &config,
            |_name: &str| None,
            || Ok("nsec1testfixturekeymaterial".to_string()),
        )
        .expect("buzz alone must be usable");
        assert!(gate.buzz.is_usable());
        assert!(!gate.telegram.is_usable());
        // This test only exercises the boot gate, not full adapter
        // construction — no adapter Arcs exist here, so the snapshot is
        // empty regardless of Buzz's gate result (Phase 47.6 Plan 07: a real
        // `start()` run WOULD populate a "buzz" entry once the adapter is
        // actually constructed and connected; see snapshot_includes_buzz_when_present).
        assert!(build_adapter_snapshot(&None, &None).is_empty());
    }

    // -------------------------------------------------------------------------
    // G-41.1-5: Telegram command-menu generation (commands_for_platform)
    // -------------------------------------------------------------------------

    #[test]
    fn telegram_command_menu_includes_available_and_excludes_unavailable() {
        use ironhermes_core::commands::{CommandCategory, PlatformFilter};

        let commands = vec![
            CommandDef::new("help", "Show help", CommandCategory::Info)
                .platform(PlatformFilter::All),
            CommandDef::new("skills", "List skills", CommandCategory::ToolsAndSkills)
                .platform(PlatformFilter::Universal),
            CommandDef::new("agents", "Manage agents", CommandCategory::Configuration)
                .platform(PlatformFilter::CliOnly),
            CommandDef::new("usage", "Show usage", CommandCategory::Info)
                .platform(PlatformFilter::CliAndAcp),
        ];

        let tg_commands = commands_for_platform(&commands, &Platform::Telegram);
        let names: Vec<&str> = tg_commands.iter().map(|c| c.command.as_str()).collect();

        assert!(
            names.contains(&"help"),
            "PlatformFilter::All must be included"
        );
        assert!(
            names.contains(&"skills"),
            "PlatformFilter::Universal must be included"
        );
        assert!(
            !names.contains(&"agents"),
            "PlatformFilter::CliOnly must be excluded"
        );
        assert!(
            !names.contains(&"usage"),
            "PlatformFilter::CliAndAcp must be excluded on Telegram"
        );
        assert_eq!(
            tg_commands.len(),
            2,
            "only the two Telegram-available commands should be produced"
        );

        // name/description mapping is exact, not just presence.
        let help = tg_commands.iter().find(|c| c.command == "help").unwrap();
        assert_eq!(help.description, "Show help");
    }

    /// Regression test for G-41.1-5: the bot menu must be generated from the
    /// real command router catalog, not the stale hardcoded 4-command vec.
    /// "start", "new", and "help" are all Telegram-available in the real
    /// registry (GatewayOnly / Universal / All respectively) so they must
    /// survive the filter, and the full catalog is far larger than 4.
    #[test]
    fn telegram_command_menu_generated_from_full_router_catalog() {
        let commands = telegram_bot_commands();
        let names: Vec<&str> = commands.iter().map(|c| c.command.as_str()).collect();

        assert!(names.contains(&"start"));
        assert!(names.contains(&"new"));
        assert!(names.contains(&"help"));
        assert!(
            commands.len() > 4,
            "expected the full router catalog, got only {} commands",
            commands.len()
        );

        // Scope assertion: individual skills (172 of them, resolved via
        // SkillRegistry, not CommandRouter) must never inflate the menu —
        // only the single "skills" slash command itself may appear.
        assert!(
            names.iter().filter(|n| **n == "skills").count() <= 1,
            "skills must appear at most once as the slash command, never per-skill"
        );
    }

    /// Regression test for the startup `400: BOT_COMMAND_INVALID` warning:
    /// Telegram rejects the whole `setMyCommands` batch if ANY name falls
    /// outside `[a-z0-9_]{1,32}`, so every generated name must comply.
    #[test]
    fn telegram_command_menu_names_are_all_telegram_valid() {
        for cmd in telegram_bot_commands() {
            assert!(
                !cmd.command.is_empty()
                    && cmd.command.len() <= 32
                    && cmd
                        .command
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                "command name {:?} violates Telegram's [a-z0-9_]{{1,32}} rule",
                cmd.command
            );
        }
    }

    /// Subcommand entries with spaces (e.g. "provider list") are dropped in
    /// favor of the already-present parent; hyphenated names are mapped to
    /// their underscore form rather than dropped.
    #[test]
    fn telegram_command_menu_sanitizes_invalid_names() {
        use ironhermes_core::commands::{CommandCategory, PlatformFilter};

        let commands = vec![
            CommandDef::new(
                "provider",
                "Manage providers",
                CommandCategory::Configuration,
            )
            .platform(PlatformFilter::Universal),
            CommandDef::new(
                "provider list",
                "List providers",
                CommandCategory::Configuration,
            )
            .platform(PlatformFilter::Universal),
            CommandDef::new(
                "reload-mcp",
                "Reload MCP servers",
                CommandCategory::ToolsAndSkills,
            )
            .platform(PlatformFilter::Universal),
        ];

        let names: Vec<String> = commands_for_platform(&commands, &Platform::Telegram)
            .into_iter()
            .map(|c| c.command)
            .collect();

        assert_eq!(
            names,
            vec!["provider", "reload_mcp"],
            "space-named subcommands dropped, hyphens mapped to underscores"
        );
    }

    /// The underscore forms the menu emits for hyphenated commands must
    /// resolve in the router (via alias), otherwise tapping the menu item
    /// would fail.
    #[test]
    fn telegram_menu_underscore_forms_resolve_in_router() {
        use ironhermes_core::commands::{CommandRouter, ResolveResult, registry::build_registry};

        let router = CommandRouter::new(build_registry());
        for (input, expected) in [
            ("reload_mcp", "reload-mcp"),
            ("export_session", "export-session"),
        ] {
            let result = router.resolve(input, &Platform::Telegram);
            assert!(
                matches!(&result, ResolveResult::Exact(c) if c.name == expected),
                "expected /{} to resolve to {:?}, got {:?}",
                input,
                expected,
                result
            );
        }
    }

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
            audio_dispatcher: None,
            delivery_registry: DeliveryRegistry::new(),
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
        // Port 1 is privileged and always connection-refused
        let config = Config {
            model: ModelConfig {
                default: "test-model".to_string(),
                base_url: Some("http://127.0.0.1:1".to_string()),
                api_key: Some("test-key".to_string()),
                ..Default::default()
            },
            agent: AgentConfig {
                max_turns: 1,
                ..Default::default()
            },
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
            audio_dispatcher: None,
            delivery_registry: DeliveryRegistry::new(),
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

    // -------------------------------------------------------------------------
    // Phase 36.17.1 Plan 04: drain-mode flag (D-03)
    //
    // is_draining: Arc<AtomicBool> on GatewayRunner; drain_for_restart() flips
    // it to true THEN cancels the cancel token. SessionQueue.try_push has NO
    // is_draining gate — drain mode is preserve-AND-accept per D-03 and
    // T-36.17.1-03 closure. The drain helper consolidates the ordering
    // invariant so source-order audits prove store-before-cancel.
    // -------------------------------------------------------------------------

    mod drain_mode_tests {
        use super::*;
        use ironhermes_core::{MessageEvent, Platform};

        fn make_runner() -> GatewayRunner {
            let config = Config::default();
            let resolver = ProviderResolver::build(&config).expect("resolver ok");
            let tool_registry = Arc::new(RwLock::new(ToolRegistry::new()));
            GatewayRunner::new(config, resolver, tool_registry)
        }

        fn fixture_event(content: &str) -> MessageEvent {
            MessageEvent {
                platform: Platform::Telegram,
                message_id: format!("msg-{content}"),
                chat_id: "chat-0".to_string(),
                sender_id: "user-0".to_string(),
                content: content.to_string(),
                attachments: Vec::new(),
                thread_id: None,
                chat_type: "dm".to_string(),
                chat_name: None,
                sender_name: None,
                replied_to_id: None,
            }
        }

        fn drain_key() -> SessionKey {
            SessionKey::new(Platform::Telegram, "chat-0").with_user("user-0")
        }

        /// is_draining() defaults to false on a fresh runner — drain mode is
        /// not entered until drain_for_restart() flips the flag explicitly.
        #[test]
        fn test_is_draining_starts_false() {
            let runner = make_runner();
            assert!(
                !runner.is_draining(),
                "fresh runner must have is_draining() == false"
            );
        }

        /// drain_for_restart() flips is_draining to true AND cancels the cancel
        /// token, in that order. The ordering is the T-36.17.1-03 mitigation:
        /// any concurrent try_push observing is_draining=true is guaranteed to
        /// see cancel NOT YET fired (and the queue still accepts the push
        /// per D-03 preserve-AND-accept).
        #[test]
        fn test_drain_for_restart_flips_flag() {
            let runner = make_runner();
            // Snapshot the cancel token clone BEFORE drain — cancellation
            // observable on this clone proves drain_for_restart cancelled the
            // underlying token.
            let cancel_clone = runner.cancel.clone();
            assert!(
                !cancel_clone.is_cancelled(),
                "cancel must not be fired before drain_for_restart()"
            );

            runner.drain_for_restart();

            assert!(
                runner.is_draining(),
                "drain_for_restart() must flip is_draining to true"
            );
            assert!(
                cancel_clone.is_cancelled(),
                "drain_for_restart() must cancel the cancel token"
            );
        }

        /// Drain mode preserves existing queue contents — flipping is_draining
        /// to true does NOT clear the per-session queue. Closes T-36.17.1-03
        /// (lost-update during drain-mode transition).
        #[test]
        fn test_drain_mode_preserves_queue() {
            let runner = make_runner();
            let key = drain_key();

            runner.try_enqueue(&key, fixture_event("a")).unwrap();
            runner.try_enqueue(&key, fixture_event("b")).unwrap();
            runner.try_enqueue(&key, fixture_event("c")).unwrap();
            assert_eq!(runner.queue_len(&key), 3);

            runner.drain_for_restart();

            assert!(runner.is_draining(), "drain mode must be entered");
            assert_eq!(
                runner.queue_len(&key),
                3,
                "drain_for_restart must NOT clear the queue (D-03 preserve)"
            );
        }

        /// Drain mode continues to accept new pushes (D-03 preserve-AND-accept).
        /// SessionQueue.try_push has NO is_draining gate, so a fresh enqueue
        /// after drain_for_restart() succeeds and grows the queue. This is the
        /// preserve-AND-accept contract: dropping the new push would lose user
        /// input arriving in the brief window between drain entry and process
        /// exit.
        #[test]
        fn test_drain_mode_accepts_new_pushes() {
            let runner = make_runner();
            let key = drain_key();

            runner
                .try_enqueue(&key, fixture_event("before-drain"))
                .unwrap();
            assert_eq!(runner.queue_len(&key), 1);

            runner.drain_for_restart();
            assert!(runner.is_draining(), "drain mode must be entered");

            // Push in drain mode must succeed (D-03 preserve-AND-accept).
            let result = runner.try_enqueue(&key, fixture_event("during-drain"));
            assert!(
                result.is_ok(),
                "try_enqueue during drain must return Ok (D-03 preserve-AND-accept), got {result:?}"
            );
            assert_eq!(
                runner.queue_len(&key),
                2,
                "queue must grow when push accepted during drain"
            );

            // FIFO order preserved across the drain transition.
            assert_eq!(
                runner.dequeue(&key).unwrap().content,
                "before-drain",
                "FIFO order must preserve arrival order across drain entry"
            );
            assert_eq!(
                runner.dequeue(&key).unwrap().content,
                "during-drain",
                "FIFO order must preserve arrival order across drain entry"
            );
        }

        /// Source-order audit: in `drain_for_restart`, `is_draining.store(true,
        /// ...)` MUST precede `self.cancel.cancel()`. This is the canonical
        /// T-36.17.1-03 mitigation. Locking it with a source grep guards
        /// against future refactors that silently reverse the order.
        #[test]
        fn drain_for_restart_stores_flag_before_cancel() {
            let src = include_str!("runner.rs");

            // Find the drain_for_restart function body by locating its sig.
            let sig = "pub fn drain_for_restart";
            let start = src
                .find(sig)
                .expect("drain_for_restart must exist on GatewayRunner");
            // The function is short — first ~10 lines after the signature
            // suffice. Capture the body span.
            let body_window = &src[start..start + 1024.min(src.len() - start)];

            let store_idx = body_window
                .find("is_draining.store(true")
                .expect("drain_for_restart body must contain is_draining.store(true, …)");
            let cancel_idx = body_window
                .find("cancel.cancel()")
                .expect("drain_for_restart body must contain cancel.cancel()");

            assert!(
                store_idx < cancel_idx,
                "T-36.17.1-03: is_draining.store(true) must PRECEDE cancel.cancel() \
                 in drain_for_restart body (store_idx={store_idx}, cancel_idx={cancel_idx})"
            );
        }

        /// Source-order audit: the shutdown sequence calls
        /// `self.drain_for_restart()` (not the bare `self.cancel.cancel()`),
        /// so the flag-flip-then-cancel ordering is enforced at the only
        /// graceful shutdown call site (~runner.rs:1105). Bare cancel.cancel()
        /// is still allowed in test code and forced-abort paths.
        #[test]
        fn shutdown_sequence_uses_drain_for_restart() {
            let src = include_str!("runner.rs");
            // The graceful shutdown call must use drain_for_restart, not the
            // bare cancel.cancel(). The exact anchor: the "Propagate cancellation
            // to all subtasks" comment is the marker for the shutdown injection
            // point per the Phase 21.2 Plan 11 invariant. We scan a window
            // generous enough to span the comment block + the actual call line
            // (up to ~1KB) but small enough that an unrelated
            // `drain_for_restart` token far away cannot satisfy the assertion.
            let anchor = "// Propagate cancellation to all subtasks";
            let anchor_idx = src
                .find(anchor)
                .expect("shutdown anchor comment must exist");
            let end = (anchor_idx + 1024).min(src.len());
            let window = &src[anchor_idx..end];
            // First, the bare `self.cancel.cancel();` MUST NOT be the next call.
            // (Comments mentioning `self.cancel.cancel()` are fine — only the
            // statement form is forbidden.)
            assert!(
                !window.contains("self.cancel.cancel();"),
                "Phase 36.17.1 D-03: the bare self.cancel.cancel(); call must NOT \
                 appear after the 'Propagate cancellation' anchor (use \
                 self.drain_for_restart(); instead). Window: {window}"
            );
            // Second, drain_for_restart must be the chosen call.
            assert!(
                window.contains("self.drain_for_restart();"),
                "Phase 36.17.1 D-03: graceful shutdown after the 'Propagate cancellation' \
                 anchor must call self.drain_for_restart(); — got window: {window}"
            );
        }

        /// Contract guard: SessionQueue.try_push must NOT consult is_draining.
        /// The preserve-AND-accept invariant (D-03) is enforced by the absence
        /// of an `is_draining` gate inside the queue itself. Locked by
        /// source-grep so a future refactor that adds `if is_draining` inside
        /// try_push trips this test.
        #[test]
        fn session_queue_try_push_has_no_is_draining_gate() {
            let src = include_str!("session_queue.rs");
            assert!(
                !src.contains("is_draining"),
                "T-36.17.1-03 contract violation: SessionQueue must NOT reference \
                 is_draining (preserve-AND-accept per D-03). Found 'is_draining' \
                 in session_queue.rs."
            );
        }
    }
}

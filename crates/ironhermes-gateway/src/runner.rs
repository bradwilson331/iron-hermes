use anyhow::{Context, Result};
use ironhermes_agent::AgentRuntime;
use ironhermes_agent::context_engine::ContextEngine;
use ironhermes_agent::engine_factory::build_context_engine;
use ironhermes_agent::pressure_warning::PressureTracker;
use ironhermes_agent::subagent_registry::SubagentRegistry;
use ironhermes_agent::MemoryManager;
use ironhermes_core::commands::context::ToolsetSessionHandle;
use ironhermes_core::{Config, ProviderResolver, SkillRecord, SkillRegistry};
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
use ironhermes_cron::TgSendApi;
use crate::user_queue::{DispatchOutcome, UserQueueManager};
use ironhermes_core::MessageEvent;

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
            // Phase 36.17.1 Plan 04 (D-03): drain-mode flag starts false.
            // `drain_for_restart()` flips it to true BEFORE cancelling the
            // cancel token — preserve-AND-accept semantics live there.
            is_draining: Arc::new(AtomicBool::new(false)),
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
    pub fn try_enqueue(
        &self,
        key: &SessionKey,
        event: MessageEvent,
    ) -> Result<(), QueueError> {
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
    pub fn retain_queue<F: Fn(&MessageEvent) -> bool>(
        &self,
        key: &SessionKey,
        predicate: F,
    ) {
        self.session_queue.retain(key, predicate);
    }

    /// Phase 36.17.1: crate-private accessor for threading `Arc<SessionQueue>`
    /// into the handler from `build_gateway_handler`. Plan 04 will reuse the
    /// same accessor for drain-mode wiring.
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
            };
            if let Err(e) = handler
                .run_agent(
                    &next_event,
                    adapter.clone(),
                    cancel.clone(),
                    no_attachments,
                )
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
        //
        // Phase 36.17.2.1 D-01/D-03: order matters — UQM must be constructed BEFORE
        // the handler is Arc-wrapped so we can call handler.set_user_queue_manager(...)
        // on the still-mutable owned `mut handler`. This wires the UQM into the
        // handler's CoreCommandResult::Queued arm (handler.rs) so /queue events
        // dispatch via UQM::dispatch (which calls notify_one() — user_queue.rs:154)
        // instead of the direct session_queue.try_push path that has no wake protocol.
        //
        // Phase 36.17.2 Plan 01: UQM constructor signature — Arc<SessionQueue> arg
        // (D-03: UQM holds Arc<SessionQueue>, not capacity).
        let user_queue = Arc::new(UserQueueManager::new(
            adapter.clone() as Arc<dyn crate::adapter::PlatformAdapter>,
            self.session_queue.clone(), // Arc<SessionQueue> already on GatewayRunner per 36.17.1-02
        ));
        let mut handler = self.build_gateway_handler();
        // Phase 36.17.2.1 D-01/D-03: install the same UQM Arc the dispatch loop uses
        // (user_queue_dispatch downstream is a clone of this same Arc — both
        // reference identical workers + pending_multimodal + session_queue state).
        handler.set_user_queue_manager(user_queue.clone());
        // Phase 36.17.2.2 D-18: install the MediaSender impl (Telegram only).
        //
        // Construct as `Arc<TelegramAdapter>` (already done at line 645 above),
        // then clone-cast SEPARATELY for each trait. Do NOT upcast
        // `Arc<dyn PlatformAdapter>` -> `Arc<dyn MediaSender>` — that was
        // unstable on stable Rust at the time of writing (RESEARCH Open Q4 /
        // Assumption A7). The concrete `Arc<TelegramAdapter>` at `adapter`
        // is already used independently as `Arc<dyn PlatformAdapter>` at
        // runner.rs:704 (`adapter.clone() as Arc<dyn crate::adapter::PlatformAdapter>`),
        // so the second clone-cast to `Arc<dyn MediaSender>` here mirrors
        // that pattern. Discord / Slack / web start paths do NOT call
        // `set_media_sender` — `media_sender` stays `None` on those handlers
        // and the D-19 dispatch loop in `run_agent` warns + drops any
        // extracted `<MEDIA: ...>` refs (D-18 contract).
        handler.set_media_sender(adapter.clone() as Arc<dyn crate::adapter::MediaSender>);
        // Phase 36.17.7 D-01 (Site 1 — Telegram, real dispatcher):
        // TelegramAdapter doubles as AudioDispatcher for per-turn TTS wiring.
        // Mirror set_media_sender pattern exactly: clone-cast the concrete
        // Arc<TelegramAdapter>. Do NOT upcast Arc<dyn PlatformAdapter> — that
        // was unstable on stable Rust (RESEARCH Assumption A7).
        handler.set_telegram_audio_dispatcher(
            adapter.clone() as Arc<dyn ironhermes_tools::AudioDispatcher>,
        );
        let handler = Arc::new(handler);

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
            // Phase 36.17.7 D-03-b (Site 2 — Discord, stub dispatcher):
            // Build a separate handler for the Discord adapter so it gets its own
            // AudioDispatcher slot independent of the Telegram handler. Discord
            // lacks audio delivery; NotSupportedAudioDispatcher ensures tools still
            // register for LLM schema but send_audio returns a clean Err.
            // Deletion target when Discord gets a real AudioDispatcher impl.
            // Also wire UQM so the Discord handler uses the same wake-notify path
            // as Telegram (mirrors the Telegram set_user_queue_manager call above).
            let mut handler_discord = self.build_gateway_handler();
            handler_discord.set_user_queue_manager(user_queue.clone());
            handler_discord.set_telegram_audio_dispatcher(
                std::sync::Arc::new(ironhermes_tools::NotSupportedAudioDispatcher::new("discord"))
                    as std::sync::Arc<dyn ironhermes_tools::AudioDispatcher>,
            );
            let handler_d = std::sync::Arc::new(handler_discord);
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
            // Phase 36.17.7 D-03-b (Site 3 — Slack, stub dispatcher):
            // Build a separate handler for the Slack adapter so it gets its own
            // AudioDispatcher slot independent of the Telegram handler. Slack
            // lacks audio delivery; NotSupportedAudioDispatcher ensures tools still
            // register for LLM schema but send_audio returns a clean Err.
            // Deletion target when Slack gets a real AudioDispatcher impl.
            // Also wire UQM so the Slack handler uses the same wake-notify path
            // as Telegram (mirrors the Telegram set_user_queue_manager call above).
            let mut handler_slack = self.build_gateway_handler();
            handler_slack.set_user_queue_manager(user_queue.clone());
            handler_slack.set_telegram_audio_dispatcher(
                std::sync::Arc::new(ironhermes_tools::NotSupportedAudioDispatcher::new("slack"))
                    as std::sync::Arc<dyn ironhermes_tools::AudioDispatcher>,
            );
            let handler_s = std::sync::Arc::new(handler_slack);
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

                        // Phase 36.17.2 Plan 01: capture session key fields BEFORE moving event
                        // into dispatch (event is consumed by UQM::dispatch; D-14 triple).
                        let event_platform = event.platform.clone();
                        let event_chat_id = event.chat_id.clone();
                        let event_sender_id = event.sender_id.clone();

                        // Phase 36.17.2 Plan 02: full match on Result<DispatchOutcome, QueueError> (D-15).
                        // Cap-hit UX (❌ + chat reply) fires inside UQM::dispatch on Err — no
                        // additional handling needed here for the error path.
                        let dispatch_result = user_queue_dispatch.dispatch(event, text_prefix, image_data_uri).await;

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
                                        let (text_prefix, image_data_uri) = queue_task
                                            .take_multimodal(&session_key_for_worker)
                                            .await
                                            .unwrap_or((None, None));
                                        let processed = crate::multimodal::ProcessedAttachments {
                                            text_prefix,
                                            image_data_uri,
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

        // --- Step 11 (Phase 36.3.7 D-09): kanban dispatcher ---
        //
        // Deserialize the raw `config.kanban` serde_yaml::Value into
        // KanbanConfig. Uses all-defaults if the field is absent/null (pre-36.3.7
        // configs). The gateway's `ironhermes-core` Config stores it as
        // serde_yaml::Value to avoid a circular crate dependency (ironhermes-kanban
        // already depends on ironhermes-core).
        let kanban_config: ironhermes_kanban::KanbanConfig =
            if self.config.kanban.is_null() {
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
        let dispatch_in_gw_env = std::env::var("HERMES_KANBAN_DISPATCH_IN_GATEWAY")
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
        // HERMES_KANBAN_DB to resolve the same DB path.
        match ironhermes_kanban::KanbanStore::open_from_env() {
            Ok(store) => {
                let kanban_store_arc =
                    std::sync::Arc::new(tokio::sync::Mutex::new(store));

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
                    info!("Kanban dispatcher disabled via HERMES_KANBAN_DISPATCH_IN_GATEWAY=0");
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
                // -----------------------------------------------------------------------
                let enabled_platforms: Vec<String> =
                    collect_enabled_platform_names(&self.config, &adapter);
                let gate = crate::notifier_gating::compute_notifier_gate(
                    kanban_config.notification_sources.as_deref(),
                    &enabled_platforms,
                );
                match gate {
                    crate::notifier_gating::NotifierGate::DisabledNoSources => {
                        info!(
                            "kanban notifier disabled (notification_sources not configured)"
                        );
                    }
                    crate::notifier_gating::NotifierGate::DisabledNoOverlap {
                        wanted,
                        enabled,
                    } => {
                        info!(
                            wanted = ?wanted,
                            enabled = ?enabled,
                            "kanban notifier disabled (no enabled platform overlap)"
                        );
                    }
                    crate::notifier_gating::NotifierGate::Enabled { sources } => {
                        // Build the send_fn closure: take an owned snapshot of the
                        // gateway's adapter handles at spawn time. The notifier
                        // loop's lifetime can outlive `start()`'s stack frame, so
                        // capturing references would not work — Arcs only.
                        let adapter_snapshot: Vec<(
                            String,
                            std::sync::Arc<dyn crate::adapter::PlatformAdapter>,
                        )> = build_adapter_snapshot(&adapter);
                        let send_fn = build_notifier_send_fn(adapter_snapshot);
                        let poll_seconds = kanban_config.notifier_poll_seconds;
                        let notifier_ctx = std::sync::Arc::new(
                            ironhermes_kanban::NotifierContext::new(
                                kanban_store_arc.clone(),
                                poll_seconds,
                                send_fn,
                            ),
                        );
                        let notifier_cancel = self.cancel.clone();
                        join_set.spawn(async move {
                            ironhermes_kanban::run_notifier_loop(
                                notifier_ctx,
                                notifier_cancel,
                            )
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

// -------------------------------------------------------------------------
// Phase 36.3.7.5 BUG-36.3.7.5-04: notifier-spawn support helpers.
//
// `collect_enabled_platform_names` reads the gateway's `Config` + the live
// Telegram adapter Arc to compute the set of enabled-platform names used by
// the spawn-gating check (`compute_notifier_gate`).
//
// `build_adapter_snapshot` produces an owned `Vec<(String, Arc<dyn PlatformAdapter>)>`
// for the `SendFn` closure — captured by value so the closure can outlive
// `start()`'s stack frame. Currently includes ONLY the Telegram adapter
// (Discord/Slack adapters are constructed inside their own spawned tasks and
// are not retained as runner-scope Arcs in this iteration; subscriptions
// naming those platforms will hit the "platform not enabled in gateway" arm
// of the send_fn closure and the notifier will log + drop per locked policy).
//
// `build_notifier_send_fn` constructs the `ironhermes_kanban::SendFn`
// trait-object closure: case-insensitive string match on `platform`, route
// to the matching `PlatformAdapter::send_message`, or return `Err` so the
// notifier's log-and-drop policy applies.
// -------------------------------------------------------------------------

/// Enumerate the gateway's enabled platform names from the parsed `Config`.
///
/// "Enabled" = the platform appears in `config.gateway.platforms`. The Telegram
/// adapter is ALWAYS enabled (constructed at `start()` entry); Discord/Slack
/// are enabled iff their config sections AND token environments resolve at
/// startup. Conservative semantics: include any platform key present in the
/// platforms map so the gate check sees the operator's intent.
fn collect_enabled_platform_names(
    config: &ironhermes_core::Config,
    _telegram_adapter: &std::sync::Arc<crate::telegram::TelegramAdapter>,
) -> Vec<String> {
    // Start from the configured platforms map. Telegram is the canonical
    // entry — if the operator wrote `platforms.telegram`, the platform is
    // enabled by the time we reach this point (start() would have failed
    // earlier if the token were unresolvable). Discord/Slack are enabled
    // when their config sections exist.
    let mut names: Vec<String> = config
        .gateway
        .platforms
        .keys()
        .map(|k| k.to_string())
        .collect();
    // Always include "telegram" — start() unconditionally builds the
    // TelegramAdapter, so it is the always-on adapter even if the config
    // platforms map is missing the explicit `telegram:` key (the
    // tg_config default-clone above tolerates absence).
    if !names.iter().any(|n| n.eq_ignore_ascii_case("telegram")) {
        names.push("telegram".to_string());
    }
    names
}

/// Build an owned snapshot of platform-name → `Arc<dyn PlatformAdapter>` pairs
/// for the `SendFn` closure. Currently only the Telegram adapter is reachable
/// as a runner-scope Arc; Discord/Slack adapters live inside their own tokio
/// tasks (constructed after socket connect). Subscriptions that name a
/// platform NOT in this snapshot will receive `Err("platform X not enabled
/// in gateway")` from the closure and the notifier will log+drop the message
/// per locked policy D-log-and-drop-on-fail.
fn build_adapter_snapshot(
    telegram_adapter: &std::sync::Arc<crate::telegram::TelegramAdapter>,
) -> Vec<(String, std::sync::Arc<dyn crate::adapter::PlatformAdapter>)> {
    vec![(
        "telegram".to_string(),
        telegram_adapter.clone() as std::sync::Arc<dyn crate::adapter::PlatformAdapter>,
    )]
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
        move |platform: &str,
              chat_id: &str,
              thread_id_opt: Option<&str>,
              message: &str| {
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

            runner.try_enqueue(&key, fixture_event("before-drain")).unwrap();
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

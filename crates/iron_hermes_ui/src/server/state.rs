//! Shared server state for the Dioxus UI backend.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use ironhermes_agent::agent_loop::{StreamCallback, ToolProgressCallback, ToolResultCallback};
use ironhermes_agent::{
    AgentRuntime, AgentRuntimeInput, MemoryManager, PressureTracker, PromptBuilder, TurnRequest,
};
use ironhermes_artifacts::ArtifactStore;
use ironhermes_core::commands::registry::build_registry as build_command_registry;
use ironhermes_core::commands::CommandRouter;
use ironhermes_core::config::Config;
use ironhermes_core::constants::get_hermes_home;
use ironhermes_core::types::{MessageContent, Platform, Role};
use ironhermes_core::{ChatMessage, ProviderResolver};
use ironhermes_exec::process_registry::ProcessRegistry;
use ironhermes_hooks::HooksConfig;
use ironhermes_state::StateStore;
use ironhermes_state::StoredMessage;
use tokio::sync::RwLock;

/// Phase 39.3 Plan 02 (D-03): data held while a realtime tool call awaits
/// operator approval.
///
/// Inserted into `AppState.realtime_approvals` by `realtime_tool_call` when
/// `should_prompt_for_approval` returns true (yolo=false).  Removed by
/// `realtime_approve` on resolution (resolved-once — replay mitigation).
// Fields are read by the server-side realtime_approve fn (api.rs).
// Native build is blind to wasm-side callers; suppress the transitional warning.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Clone, Debug)]
pub struct PendingApproval {
    /// The opaque `turn_id` UUID string that originated this call.  Re-validated
    /// for liveness in `realtime_approve` (the turn may have died while pending).
    pub turn_id: String,
    /// The tool name from the realtime `function_call` event.
    pub tool_name: String,
    /// JSON-encoded arguments string from the realtime `function_call` event.
    pub arguments: String,
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub command_router: Arc<CommandRouter>,
    pub state_store: Arc<std::sync::Mutex<StateStore>>,
    /// Phase 46.6 Plan 03 (D-02/D-03): dedicated `artifacts.db` store for the
    /// agent-artifact-webpage feature. A sibling `Arc<Mutex<...>>` to
    /// `state_store` above — its own connection/mutex, never shared or nested
    /// (RESEARCH Pitfall 5: bolting artifact writes onto `state_store`'s mutex
    /// would serialize artifact publishes behind unrelated session activity).
    /// Read by `serve_artifact` (artifact_route.rs) and `list_artifacts`
    /// (api.rs); written by the `artifact` Tool (ironhermes-tools, Plan 02).
    pub artifact_store: Arc<std::sync::Mutex<ArtifactStore>>,
    pub resolver: Arc<ProviderResolver>,
    pub runtime: Arc<AgentRuntime>,
    pub memory_manager: Option<Arc<tokio::sync::Mutex<MemoryManager>>>,
    /// Per-session nudge turn counter. Arc<Mutex<HashMap>> mirrors the gateway
    /// nudge_turns pattern from Plan 32-02. Interior mutability required: run_web_turn
    /// takes &self, but the counter must mutate across calls for the same session.
    /// Session key is the same String used by run_web_turn (e.g.
    /// "agent:main:web:dm:{uuid}" produced by api.rs create_session).
    /// (Phase 32 LEARN-01 — web UI nudge wiring)
    pub nudge_turns: Arc<std::sync::Mutex<HashMap<String, u32>>>,
    /// Phase 36.1 (GW-05-WEB, D-03): per-session running-agent flag map.
    /// Phase 36.17.4 (D-01a): per-session queue-drain paused flags.
    /// Arc<Mutex<HashMap>> — std::sync::Mutex, never held across `.await`.
    /// Map grows unbounded (accepted tradeoff, matches `nudge_turns` precedent).
    pub queue_paused: Arc<std::sync::Mutex<HashMap<String, Arc<std::sync::atomic::AtomicBool>>>>,
    /// Phase 41.1 Plan 03 (SKILL-13 web / D-06): per-session ONE-SHOT skill
    /// overlays activated by the WS SKILL-13 fallback (ws.rs). Each entry is a
    /// `(skill_name, skill_body)` pair; `run_web_turn` DRAINS the vector for
    /// the session and prepends each body to that turn's system prompt via
    /// `PromptBuilder::activate_skill`, so a `/<skill>` shapes exactly the run
    /// turn it triggered and nothing after it. Mirrors the gateway's
    /// `skill_overlays` map (handler.rs) and the TUI's `pending_skill_overlays`
    /// (tui_rata). `std::sync::Mutex` — never held across `.await`. Bodies
    /// originate ONLY from `resolve_skill_invocation` → `SkillRegistry::
    /// read_content` (SKILL-07-scanned), so no second unscanned injection path.
    // Mirrors the gateway's `skill_overlays` shape (handler.rs). The nested
    // Arc<Mutex<HashMap<_, Vec<(name, body)>>>> is the established overlay-map
    // pattern here; a type alias would obscure the parallel with `nudge_turns`
    // / `queue_paused` above more than it clarifies.
    #[allow(clippy::type_complexity)]
    pub web_skill_overlays: Arc<std::sync::Mutex<HashMap<String, Vec<(String, String)>>>>,
    /// Phase 36.17.4 (D-01): shared FIFO queue for all sessions, keyed by
    /// `SessionKey`. Single `SessionQueue` instance; each WS connection
    /// constructs its own `SessionKey { platform: Platform::Web, chat_id,
    /// user_id: Some("web") }` at call sites. Queue survives WS
    /// disconnect/reconnect; two browser tabs on the same session_id share it.
    pub queue: Arc<dyn ironhermes_core::queue::MessageQueue<ironhermes_core::session::SessionKey>>,
    /// Phase 32.3 Plan 04 (D-08): subagent registry Arc — same handle threaded
    /// into the AgentRuntime via `subagent_registry`. Held on
    /// AppState so the four `/api/agents/*` endpoints can read it for status
    /// / prune (which need the full registry, not just the ShrikeService).
    pub subagent_registry: Arc<RwLock<ironhermes_agent::subagent_registry::SubagentRegistry>>,
    /// Phase 41.3 Plan 04 (D-11/D-12): process registry Arc — same handle
    /// threaded into `AgentRuntimeInput.process_registry` at construction
    /// time (`AppState::init`, below). `AgentRuntime` itself has no public
    /// accessor for this handle (it is consumed into the tool registry's
    /// `terminal`/`execute_code` instances and not retained), so — mirroring
    /// the `subagent_registry` field immediately above, which exists for the
    /// exact same reason — `AppState` holds its own clone. Consumed by
    /// `web_core_handles()` (ws.rs) to wire `CommandContext.process_registry`,
    /// closing the previously-unreachable D-12 core handle on Web.
    pub process_registry: Arc<RwLock<ironhermes_exec::process_registry::ProcessRegistry>>,
    /// Phase 32.3 Plan 04 (D-08): operator-facing termination service from
    /// Plan 03. Constructed once in `AppState::init` from the same
    /// `subagent_registry` Arc above (so kill/interrupt/prune/status all
    /// operate on the registry the agent actually writes to).
    pub shrike: Option<Arc<ironhermes_agent::shrike::ShrikeService>>,
    /// Phase 26.7.1 Plan 02 (D-06 / Path A): per-turn slot for the active ws send
    /// channel. ws.rs installs `Some(tx.clone())` immediately before `run_web_turn`
    /// (with a RAII drop guard that clears the slot on return / panic). The
    /// `progress_callback` baked into `runtime` at init reads from this slot
    /// and forwards to whichever sender is current. None when no turn is in flight.
    ///
    /// Single-session assumption: events from any in-flight turn route to whichever
    /// ws is currently registered. Multi-session isolation is explicitly out of
    /// scope per CONTEXT.md.
    pub subagent_callback_slot: Arc<
        tokio::sync::Mutex<
            Option<tokio::sync::mpsc::UnboundedSender<crate::protocol::ChatStreamEvent>>,
        >,
    >,
    /// Phase 36.3.7.11 Plan 01 (D-08 / D-15): dashboard kanban tail
    /// broadcaster. The tail loop (`run_kanban_tail_loop`) holds the
    /// `Sender`; every `/api/ws/kanban` connection creates its own
    /// `Receiver` via `.subscribe()`. `SendError` when no receivers is
    /// silently discarded (Q1).
    pub kanban_tail_broadcast: tokio::sync::broadcast::Sender<String>,
    /// Phase 36.3.7.11 Plan 01 (D-15): cancellation token for the kanban
    /// tail loop. Triggered at shutdown via `cancel()`; the tail loop's
    /// `tokio::select!` exits on observation. Plan 04 wires the shutdown
    /// path that calls `.cancel()`.
    #[allow(dead_code)]
    pub kanban_tail_cancel: tokio_util::sync::CancellationToken,
    /// Phase 36.17.7 D-02-d: cancellation token for the audio cache GC
    /// periodic loop. Mirrors `kanban_tail_cancel` above. Triggered at
    /// shutdown via `.cancel()`; the GC loop's `tokio::select!` exits on
    /// observation.
    #[allow(dead_code)]
    pub audio_gc_cancel: tokio_util::sync::CancellationToken,
    /// Phase 39.1 Plan 02 (R39.1-03/R39.1-04 / D-03/D-04): two-level semaphore
    /// gate for concurrent Web WS turns.
    ///
    /// Per RESEARCH Open Question #2: the per-session semaphore is stored here at the
    /// AppState (process-wide) level. The global semaphore is also inside
    /// `ConcurrencyLayer.global`. Both are Arc-wrapped so cloning AppState clones
    /// the handle, not the semaphore itself. ws.rs calls `concurrency.try_acquire()`
    /// before each turn spawn; the returned `OwnedSemaphorePermit` pair is held for
    /// the lifetime of the spawned task (RAII release on task completion/panic).
    ///
    /// Constructed once in `AppState::init` from `config.concurrency.session_turn_cap`
    /// / `global_turn_ceiling`. The global ceiling is process-wide and shared across
    /// all surfaces.
    pub concurrency: ironhermes_core::ConcurrencyLayer,
    /// Phase 39.1 Plan 02 (R39.1-09): process-wide turn registry.
    ///
    /// ws.rs calls `turn_registry.register(entry).await` BEFORE spawning each turn
    /// task (register-before-spawn discipline) and `deregister(turn_id).await` in
    /// the StreamMap drain's completion arm. Arc shared so CommandContext can clone it
    /// for the `/agents turns` arm (Plan 01).
    pub turn_registry: Arc<ironhermes_core::TurnRegistry>,
    /// Phase 39.1 Plan 07 (R39.1-10): per-realtime-session liveness map.
    ///
    /// Keyed by `TurnId`; value is the `Instant` of the most recent heartbeat
    /// (or the mint time when the session was just registered).  Used by
    /// `realtime_prune` to reap abandoned sessions whose browser closed without
    /// calling `realtime_session_end`.  The `CancellationToken` lives in the
    /// `TurnEntry` inside `turn_registry`; this map only tracks liveness.
    ///
    /// `std::sync::Mutex` (non-async brief access) — never held across an
    /// `.await` point, matching the `nudge_turns` / `running_agents` precedent.
    pub realtime_sessions: Arc<
        std::sync::Mutex<std::collections::HashMap<ironhermes_core::TurnId, std::time::Instant>>,
    >,
    /// Phase 39.1 Plan 07 (R39.1-10): per-realtime-turn semaphore permit pairs.
    ///
    /// Keyed by `TurnId`; value is the `(per_session_permit, global_permit)` pair
    /// returned by `concurrency.try_acquire()` at token-mint time.  Permits are
    /// stored here to satisfy the RAII contract (Pitfall 2): they must outlive
    /// the realtime turn.  Dropped (and thus released) in
    /// `realtime_session_end` / cancelled-heartbeat / `realtime_prune` paths.
    ///
    /// `std::sync::Mutex` (non-async brief access) — never held across `.await`.
    pub realtime_permits: Arc<
        std::sync::Mutex<
            std::collections::HashMap<
                ironhermes_core::TurnId,
                (
                    tokio::sync::OwnedSemaphorePermit,
                    tokio::sync::OwnedSemaphorePermit,
                ),
            >,
        >,
    >,
    /// Phase 39.3 Plan 02 (D-03): per-call-id pending-approval map.
    ///
    /// Keyed by `call_id` (opaque string from the realtime DataChannel
    /// `function_call` event).  Value is a `PendingApproval` that holds
    /// the data needed to execute the tool once the user approves.
    ///
    /// Under the two-call pattern (OQ#1 resolution): `realtime_tool_call`
    /// inserts an entry and returns `ApprovalPending` immediately; the
    /// companion `realtime_approve` removes the entry on resolution
    /// (resolved-once — replay mitigation T-39.3-02-03).
    ///
    /// `std::sync::Mutex` (non-async brief access) — never held across
    /// `.await`.  Matches the `realtime_permits` / `nudge_turns` precedent.
    pub realtime_approvals:
        Arc<std::sync::Mutex<std::collections::HashMap<String, PendingApproval>>>,
    /// Phase 39.3 Plan 03 (D-04 / OQ#3 resolution): optional trajectory writer for
    /// realtime tool-call logging.
    ///
    /// **OQ#3 / A3 RESOLVED:** The web `AppState` does NOT wire a trajectory writer
    /// in Plans 01-02. The text path's `TurnRequest.trajectory_writer` is hardcoded
    /// `None` at state.rs L466, and `AppState` has no trajectory writer field. The
    /// realtime tool-exec bridge (Plan 02) calls `ToolRegistry.dispatch()` directly
    /// (NOT via `AgentRuntime::run_turn` / `TurnRequest`), so there is NO existing
    /// trajectory path it could ride. This field closes that gap.
    ///
    /// When `Some`, the realtime tool-exec dispatch sites (`realtime_tool_call` auto-
    /// approved path and `realtime_approve` Approve path) log the tool name + REDACTED
    /// args after a successful dispatch (T-LOG-LEAK mitigation — redact_args contract).
    ///
    /// When `None` (default — trajectory disabled or no path configured), realtime tool
    /// calls succeed without logging. This is best-effort: logging must never abort a
    /// tool call. The field being present-and-wired means D-04 is satisfiable when
    /// trajectory is enabled; the silent-no-op failure mode (OQ#3 warning) is closed.
    ///
    /// Config-gated: initialized to `None` in `AppState::init` (trajectory opt-in;
    /// the text path also passes `trajectory_writer: None` at init — consistent).
    /// A future config key (e.g. `config.trajectory.enabled`) can construct and wire
    /// a `TrajectoryWriterHandleImpl` here without changing call sites.
    ///
    /// Type mirrors `TurnRequest.trajectory_writer` (agent_runtime.rs L114-116) exactly:
    /// `Option<Arc<dyn ironhermes_core::commands::context::TrajectoryWriterHandle>>`.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub realtime_trajectory_writer:
        Option<Arc<dyn ironhermes_core::commands::context::TrajectoryWriterHandle>>,

    /// Phase 36.3.8 D-04/D-05: per-server PendingClarifyRegistry for web turns.
    /// Shared across all WS turns (Arc clone per spawn) so a typed numeric reply
    /// can resolve a pending clarify awaiter via `take()`. Constructed once in
    /// `AppState::init`. Web v1: ClarifyTool uses the stdout numbered fallback
    /// (clarify_dispatcher = None); the registry is present so a future WS
    /// round-trip resolver can slot in without AppState changes.
    pub web_clarify_registry: Arc<ironhermes_tools::clarify_registry::PendingClarifyRegistry>,
}

static GLOBAL_APP_STATE: OnceLock<AppState> = OnceLock::new();

pub fn install_global_app_state(state: AppState) -> Result<()> {
    GLOBAL_APP_STATE
        .set(state)
        .map_err(|_| anyhow::anyhow!("global AppState already initialized"))
}

pub fn global_app_state() -> &'static AppState {
    GLOBAL_APP_STATE
        .get()
        .expect("global AppState not initialized")
}

impl AppState {
    pub async fn init() -> Result<Self> {
        let config = Config::load().unwrap_or_default();
        let mut resolver = ProviderResolver::build(&config)?;
        // Phase 46.8 D-07: additive top-up — the vault becomes the last-resort
        // provider-key source (after config/env resolution above) when enabled.
        // A sealed/broken enabled vault propagates loudly via `?` (D-07 hard
        // error) rather than starting the server with a misleading missing-key
        // state. D-10: when vault.enabled is false (default), no store is
        // constructed and this is a no-op — byte-for-byte unchanged startup.
        //
        // UAT gap G-46.8-1 fix: route through the shared resolver so a
        // DEFAULT-configured vault (empty rusty_vault.data_dir sentinel) opens
        // the same on-disk location `vault init` wrote to, instead of the
        // sentinel reaching `open_store` unresolved.
        if config.vault.enabled {
            let store =
                ironhermes_vault::open_store(&ironhermes_core::resolve_vault_config(&config))?;
            resolver.apply_vault_fallback(&*store).await?;
        }
        let command_router = Arc::new(CommandRouter::new(build_command_registry()));
        let state_store = Arc::new(std::sync::Mutex::new(
            StateStore::open_default().context("failed to open state.db for web UI")?,
        ));
        // Phase 46.6 Plan 03 (D-02/D-03): dedicated artifacts.db store, own
        // Arc<Mutex<...>> sibling to state_store above (RESEARCH Pitfall 5 —
        // never share state_store's connection/mutex).
        let artifact_store = Arc::new(std::sync::Mutex::new(
            ArtifactStore::open_default().context("failed to open artifacts.db for web UI")?,
        ));

        let process_registry = Arc::new(RwLock::new(ProcessRegistry::new_for_session(
            "web-ui".to_string(),
        )));
        let memory_manager =
            ironhermes_agent::memory::factory::build_memory_manager(&config.memory)
                .await
                .context("building memory manager for web UI")?;

        let subagent_registry = Arc::new(RwLock::new(
            ironhermes_agent::subagent_registry::SubagentRegistry::new(),
        ));
        // Phase 32.3 Plan 04 (D-08): construct the ShrikeService once from the
        // same registry Arc threaded into the runner so all four endpoints
        // (kill/interrupt/prune/status) operate on the live registry the
        // agent writes to. Mirrors the auto-construction inside
        // `SubagentRegistryHandle::new` (subagent_registry.rs:219-222) — both
        // ShrikeService instances share the same registry, so the abort
        // semantics (W3 from Plan 03) flow through this path too.
        let shrike = Arc::new(ironhermes_agent::shrike::ShrikeService::new(
            subagent_registry.clone(),
        ));

        // Phase 26.7.1 Plan 02 (D-06 / Path A): construct the per-turn callback slot.
        // The singleton progress_callback baked into AgentRuntime reads from this
        // slot and forwards SubagentEvent {} to whichever ws sender is currently registered.
        // ws.rs installs the sender immediately before run_web_turn via lock().await, and
        // clears it via RAII drop guard (SubagentCallbackSlotGuard in ws.rs).
        let subagent_callback_slot: Arc<
            tokio::sync::Mutex<
                Option<tokio::sync::mpsc::UnboundedSender<crate::protocol::ChatStreamEvent>>,
            >,
        > = Arc::new(tokio::sync::Mutex::new(None));
        let cb_slot = subagent_callback_slot.clone();
        let progress_callback: ironhermes_tools::delegate_task::SubagentProgressCallback = Arc::new(
            move |_index: usize, _event: ironhermes_tools::delegate_task::SubagentProgress| {
                // D-07: counter-only — no payload forwarded.
                // Use try_lock to avoid awaiting in a sync `Fn`. If the slot is
                // momentarily contended (extremely unlikely outside Plan 02
                // teardown), drop the event silently — the poll loop's safety
                // net catches missed events.
                if let Ok(guard) = cb_slot.try_lock() {
                    if let Some(tx) = guard.as_ref() {
                        let _ = tx.send(crate::protocol::ChatStreamEvent::SubagentEvent {});
                    }
                }
            },
        );

        // Phase 28.1 Plan 03: build ONE AgentRuntime — it owns the budget,
        // the subagent runner (identity-sharing the same subagent_registry Arc),
        // the tool registry, skills, browser session, and hook registry.
        // The subagent_progress_callback is passed in so the live ws sender
        // still receives SubagentEvent forwarding (Path A from Plan 26.7.1-02).
        let runtime = AgentRuntime::from_config(AgentRuntimeInput {
            config: Arc::new(config.clone()),
            resolver: Arc::new(resolver.clone()),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            process_registry: process_registry.clone(),
            memory_manager: memory_manager.clone(),
            hooks_config: HooksConfig::load().unwrap_or_default(),
            emit_mcp_startup_logs: false,
            subagent_registry: subagent_registry.clone(),
            transcript_scope: (get_hermes_home(), "web-ui".to_string()),
            subagent_progress_callback: Some(progress_callback),
            subagent_cancel_token: None,
        })
        .await
        .context("building AgentRuntime for web UI")?;

        // Phase 36.3.7.11 Plan 01 (D-15 / D-17): construct the kanban
        // tail broadcaster + cancellation token, then spawn the tail
        // loop unconditionally (no lazy-on-first-client) so the first WS
        // subscriber's connect sees the live event stream immediately.
        // `config.dashboard.kanban.tail_interval_ms` is the source-of-
        // truth interval (default 250 ms via DashboardKanbanConfig::Default).
        let (kanban_tail_broadcast, _initial_rx) = tokio::sync::broadcast::channel::<String>(256);
        let kanban_tail_cancel = tokio_util::sync::CancellationToken::new();
        let tail_tx = kanban_tail_broadcast.clone();
        let tail_cancel = kanban_tail_cancel.clone();
        let tail_interval_ms = config.dashboard.kanban.tail_interval_ms;
        tokio::spawn(async move {
            crate::server::kanban_ws::run_kanban_tail_loop(tail_tx, tail_cancel, tail_interval_ms)
                .await;
        });

        // Phase 36.17.7 D-02-d: audio cache lifecycle GC.
        //  - Startup sync sweep (before async runtime work) removes any
        //    files older than max_age_days that survived the last run.
        //  - Periodic tokio task sweeps at sweep_interval_secs cadence.
        // Mirrors the kanban tail spawn pattern directly above.
        let audio_cache_dir = get_hermes_home().join("audio_cache");
        let gc_max_age = config.audio_cache.max_age_days;
        let gc_interval = config.audio_cache.sweep_interval_secs;
        crate::server::audio_cache::gc_sweep_audio_cache(&audio_cache_dir, gc_max_age);
        let audio_gc_cancel = tokio_util::sync::CancellationToken::new();
        let gc_dir = audio_cache_dir.clone();
        let gc_cancel_clone = audio_gc_cancel.clone();
        tokio::spawn(async move {
            crate::server::audio_cache::run_audio_cache_gc_loop(
                gc_dir,
                gc_max_age,
                gc_interval,
                gc_cancel_clone,
            )
            .await;
        });

        // Phase 39.1 Plan 02 (R39.1-03/R39.1-04): construct two-level concurrency
        // gate from config. Defaults: session_turn_cap=3, global_turn_ceiling=32.
        // Both limits must be > 0 (ConcurrencyLayer::new asserts this).
        let concurrency = ironhermes_core::ConcurrencyLayer::new(
            config.concurrency.session_turn_cap,
            config.concurrency.global_turn_ceiling,
        );
        // Phase 39.1 Plan 02 (R39.1-09): process-wide turn registry. Single Arc
        // shared across AppState clones and CommandContext (injected via
        // with_turn_registry in the /agents slash dispatch path).
        let turn_registry = Arc::new(ironhermes_core::TurnRegistry::new());

        Ok(Self {
            config: Arc::new(config),
            command_router,
            state_store,
            // Phase 46.6 Plan 03 (D-02/D-03): dedicated artifacts.db store,
            // constructed above alongside state_store.
            artifact_store,
            resolver: Arc::new(resolver),
            runtime: Arc::new(runtime),
            memory_manager,
            nudge_turns: Arc::new(std::sync::Mutex::new(HashMap::new())),
            // Phase 36.17.4 (D-01a): per-session queue-drain paused flags.
            queue_paused: Arc::new(std::sync::Mutex::new(HashMap::new())),
            web_skill_overlays: Arc::new(std::sync::Mutex::new(HashMap::new())),
            // Phase 36.17.4 (D-01): single shared SessionQueue instance keyed
            // by SessionKey. Explicit `as Arc<dyn ...>` widens the concrete
            // Arc<SessionQueue> to the trait-object field type.
            queue: Arc::new(ironhermes_gateway::session_queue::SessionQueue::new())
                as Arc<
                    dyn ironhermes_core::queue::MessageQueue<ironhermes_core::session::SessionKey>,
                >,
            // Phase 32.3 Plan 04: subagent_registry + shrike — same Arcs the
            // delegate-task runner uses, so the four `/api/agents/*` endpoints
            // operate on the live registry.
            subagent_registry,
            // Phase 41.3 Plan 04 (D-11/D-12): retain the same Arc threaded into
            // AgentRuntimeInput above (line ~347) — AgentRuntime has no public
            // accessor for it, mirroring why subagent_registry is held here too.
            process_registry,
            shrike: Some(shrike),
            // Phase 26.7.1 Plan 02 (D-06 / Path A): callback slot — ws.rs
            // installs the per-turn sender before run_web_turn, RAII guard clears
            // it on return/panic.
            subagent_callback_slot,
            // Phase 36.3.7.11 Plan 01 (D-15): dashboard kanban tail state —
            // broadcaster + cancellation token created above; tail loop
            // already spawned.
            kanban_tail_broadcast,
            kanban_tail_cancel,
            // Phase 36.17.7 D-02-d: audio cache GC cancellation token —
            // periodic loop spawned above.
            audio_gc_cancel,
            // Phase 39.1 Plan 02: two-level semaphore gate + process-wide registry.
            concurrency,
            turn_registry,
            // Phase 39.1 Plan 07 (R39.1-10): realtime session liveness map +
            // permit store (RAII — permits held for the lifetime of each turn).
            realtime_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            realtime_permits: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            // Phase 39.3 Plan 02 (D-03): pending approval map — keyed by call_id.
            // Entries inserted by realtime_tool_call (gate=prompt), removed by
            // realtime_approve (resolved-once). Initially empty.
            realtime_approvals: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            // Phase 39.3 Plan 03 (D-04 / OQ#3 resolution): optional realtime trajectory writer.
            // Default None — trajectory is opt-in (matches TurnRequest.trajectory_writer: None at
            // L466 above). A future config key can construct a TrajectoryWriterHandleImpl here.
            // The field being present closes the silent-no-op gap OQ#3 warned about.
            realtime_trajectory_writer: None,
            // Phase 36.3.8 D-04/D-05: per-server clarify registry for web turns.
            // Constructed once so all WS turn spawns share the same awaiter map.
            web_clarify_registry: Arc::new(
                ironhermes_tools::clarify_registry::PendingClarifyRegistry::new(),
            ),
        })
    }

    pub fn ensure_web_session(&self, session_id: &str) -> Result<()> {
        // CR-01 (traversal): this is the single WS-path chokepoint through
        // which a client-supplied session_id enters the turn — it runs before
        // session_workspace_dir/create_dir_all in build_messages_for_turn.
        // Reject any id that could escape sessions/ before a DB row or a
        // directory is created for it.
        if ironhermes_core::safe_session_id(session_id).is_none() {
            anyhow::bail!("invalid session id: {session_id:?}");
        }
        let mut store = self.state_store.lock().unwrap();
        if store
            .get_session(session_id)
            .context("failed to query web session")?
            .is_none()
        {
            store
                .create_session(
                    session_id,
                    &Platform::Web.to_string(),
                    Some(&self.config.model.default),
                    None,
                    None,
                    None,
                )
                .context("failed to create web session")?;
        }
        Ok(())
    }

    /// Phase 34b Plan 02 (D-09/D-10, CONTEXT Open Q1): per-session reset stub for
    /// the web surface. No new-chat / `/new` trigger is wired in the web UI yet,
    /// so there is no call site today. When such a trigger lands, this is the
    /// locus that would discard the session's durable per-session state (and call
    /// the engine's `on_session_reset`) the same way CLI `/new` and gateway `/new`
    /// do. Documented stub is the accepted scope for this phase.
    pub fn reset_web_session(&self, session_id: &str) {
        tracing::debug!(
            session = %session_id,
            "reset_web_session: no new-chat trigger wired yet (Phase 34b stub, CONTEXT Open Q1)"
        );
    }

    /// Phase 41.1 Plan 03 (SKILL-13 web / D-06): record a ONE-SHOT skill
    /// activation for `session_id`. Called by the WS SKILL-13 fallback
    /// (ws.rs) after `resolve_skill_invocation` matches a `/<skill>`; the
    /// `(name, body)` is drained + applied to the next `run_web_turn` for this
    /// session by `take_web_skill_overlays`. `std::sync::Mutex` — never held
    /// across `.await`.
    pub fn push_web_skill_overlay(&self, session_id: &str, name: String, body: String) {
        let mut map = self
            .web_skill_overlays
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        map.entry(session_id.to_string())
            .or_default()
            .push((name, body));
    }

    /// Phase 41.1 Plan 03 (SKILL-13 web / D-06): DRAIN (remove) the pending
    /// one-shot skill overlays for `session_id`. Returns them in activation
    /// order. Draining is what makes activation one-shot — the skill shapes
    /// only the turn it triggered.
    fn take_web_skill_overlays(&self, session_id: &str) -> Vec<(String, String)> {
        let mut map = self
            .web_skill_overlays
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        map.remove(session_id).unwrap_or_default()
    }

    /// Phase 36.1 (GW-05-WEB, D-03): get or create the per-session running-agent flag.
    ///
    /// Returns an `Arc<AtomicBool>` for the given session_id, creating a
    /// Phase 36.17.4 (D-01a): per-session queue-drain paused flag helper.
    /// Entry-or-insert under std::sync::Mutex — Mutex must NEVER be held
    /// across `.await`. Returns the SAME `Arc<AtomicBool>` on repeat lookups
    /// for the same session_id (idempotent), and distinct `Arc`s for distinct
    /// session_ids (session isolation).
    pub fn get_or_create_paused_flag(
        &self,
        session_id: &str,
    ) -> Arc<std::sync::atomic::AtomicBool> {
        let mut map = self.queue_paused.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(session_id.to_string())
            .or_insert_with(|| Arc::new(std::sync::atomic::AtomicBool::new(false)))
            .clone()
    }

    // Phase 36.3.8 D-04: messaging_wiring pushes the per-turn arg count past the
    // clippy threshold. Grouping into a struct would obscure the surface-parallel
    // call sites (gateway/web/local all pass the same flat per-turn set), so an
    // explicit allow is preferred over a synthetic params bag.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_web_turn(
        &self,
        session_id: &str,
        user_input: &str,
        stream_callback: StreamCallback,
        tool_progress_callback: Option<ToolProgressCallback>,
        tool_result_callback: Option<ToolResultCallback>,
        // Phase 36.17.7 D-02-a: per-turn TTS wiring (WebAudioDispatcher + SessionKey).
        // `None` when TTS is not configured for this session.
        tts_wiring: Option<ironhermes_agent::TtsPerTurnWiring>,
        // Phase 39.2 Plan 04: TurnRegistry turn_id for black-box event correlation.
        turn_id: Option<uuid::Uuid>,
        // Phase 36.3.8 D-04/D-05: per-turn messaging + clarify wiring.
        // Web v1: clarify_dispatcher = None → stdout numbered fallback in ClarifyTool.
        messaging_wiring: Option<ironhermes_agent::MessagingPerTurnWiring>,
        // Phase 46.7 Plan 04 (D-09): ids of chat_attachments rows to resolve into
        // this turn's user message. Empty for surfaces that never carry
        // attachments (queue-drain replay, STT transcript turns). Consumed by
        // build_messages_for_turn (Task 3).
        attachment_ids: Vec<String>,
    ) -> Result<ironhermes_agent::AgentResult> {
        // Phase 39.1 Plan 06 (R39.1-06): RunningAgentGuard removed.
        // The RAII guard (phase 36.1 D-06) is deleted along with the running_agent
        // module. Turn tracking is now handled by TurnRegistry (see ws.rs).
        let messages = self
            .build_messages_for_turn(session_id, user_input, &attachment_ids)
            .await?;
        // Snapshot the messages BEFORE agent.run consumes them — the nudge
        // (if it fires) sees the exact turn the model just consumed, not any
        // post-tool-call mutation. Matches the gateway pattern from Plan 32-02.
        let messages_snapshot = messages.clone();

        // Phase 28.1 Plan 03 (Task 2): route through AgentRuntime::run_turn.
        // - state_store threaded via TurnRequest so session_search intercept fires (T-28.1-07).
        // - pressure_tracker: fresh per-turn, matching prior build_agent_loop behavior.
        // - session_id: use the real per-session id (behavior improvement over hardcoded "web-ui").
        //
        // WR-01 (Phase 34b Plan 03): retain a shared reference to stream_callback so
        // we can emit context_warnings out-of-band AFTER run_turn returns. StreamCallback
        // is Box<dyn Fn>, not Clone, so we wrap it in Arc before passing a forwarding Box
        // into TurnRequest — the Arc clone is used post-turn for the warnings block.
        let stream_cb_arc: std::sync::Arc<ironhermes_agent::agent_loop::StreamCallback> =
            std::sync::Arc::new(stream_callback);
        let cb_for_turn = {
            let arc = stream_cb_arc.clone();
            let cb: ironhermes_agent::agent_loop::StreamCallback =
                Box::new(move |s: &str| (arc)(s));
            cb
        };

        // Phase 36.3.12 D-08/D-10/D-11/D-12 (GAP 2): wire the web surface through the
        // SAME guardrail→approval→audit chokepoint every other surface uses
        // (`ironhermes_hooks::execute_gated_command`), via `build_web_terminal_intercept`
        // / `build_web_execute_code_intercept` below. Prior to this, `approval_gate`,
        // `terminal_intercept`, and `execute_code_intercept` were all left unset here,
        // so an LLM-issued `terminal`/`execute_code` call on this surface reached the
        // raw tool with no guardrail, no audit entry, and no approval prompt at all —
        // the exact bypass class D-08/D-10 exist to close, on the surface this project
        // actually runs ("Agent runs embedded in the UI server; no separate
        // agent/gateway process exists").
        //
        // `is_remote_backend` / `forward_env_nonempty` are computed from real config
        // (matching the gateway at handler.rs:592-593 and the CLI at
        // approval_gate.rs:239-240) rather than hardcoded — this is what makes D-08's
        // forced-approval-on-remote branch reachable on this surface once an operator
        // selects `terminal.backend: ssh`.
        let terminal_guard = Arc::new(ironhermes_hooks::DangerousCommandGuardrail::from_config(
            &self.config.dangerous_commands,
        ));
        let gated_audit_log = Arc::new(ironhermes_core::AuditLog::load(self.config.audit.clone()));
        let is_remote_backend = self.config.terminal.backend == "ssh";
        let forward_env_nonempty = !self.config.terminal.forward_env.is_empty();
        let yolo = self.config.autonomous.yolo;
        // This surface has no separate "chat_id" concept distinct from the session —
        // the session id doubles as the audit/prompt chat identifier here.
        let gated_chat_id = session_id.to_string();

        let terminal_intercept = build_web_terminal_intercept(
            self.runtime.terminal_tool_arc(),
            terminal_guard.clone(),
            gated_audit_log.clone(),
            session_id.to_string(),
            gated_chat_id.clone(),
            yolo,
            is_remote_backend,
            forward_env_nonempty,
        );
        // Per D-11: execute_code passes an EMPTY classify_arg and always forwards
        // is_remote_backend=false / forward_env_nonempty=false — the Python script
        // still runs on the local Sandbox and never routes to a remote backend
        // (identical treatment to the CLI's build_gated_execute_code_intercept).
        let execute_code_intercept = build_web_execute_code_intercept(
            self.runtime.execute_code_tool_arc(),
            terminal_guard,
            gated_audit_log,
            session_id.to_string(),
            gated_chat_id,
            yolo,
        );
        // Supplying WebApprovalGate here (unlike CLI/TUI, which leave this None because
        // CliApprovalGate is built inside their intercept closures instead) makes
        // AgentLoop's own NeedsApproval arm for OTHER tools fail-closed-by-gate rather
        // than fail-closed-by-absence on this surface.
        let web_approval_gate: Arc<dyn ironhermes_core::ApprovalGate> =
            Arc::new(crate::server::web_approval_gate::WebApprovalGate);

        let request = TurnRequest {
            messages,
            session_id: session_id.to_string(),
            stream: Some(cb_for_turn),
            tool_progress: tool_progress_callback,
            tool_result: tool_result_callback,
            state_store: Some(self.state_store.clone()),
            pressure_tracker: Some(Arc::new(PressureTracker::new())),
            trajectory_writer: None,
            compression_count: 0,
            cancel_token: None,
            // Phase 36.17.7 D-02-a: wire per-turn TTS dispatcher into AgentRuntime.
            tts_wiring,
            // Phase 39.2 Plan 04: correlate black-box events with TurnRegistry UUID.
            turn_id,
            // Phase 36.3.8 D-04/D-05: per-turn messaging + clarify wiring.
            messaging_wiring,
            // Phase 36.3.12 D-08/D-10/D-11/D-12 (GAP 2): real gated wiring, built above —
            // see the comment immediately preceding `terminal_guard`'s construction.
            approval_gate: Some(web_approval_gate),
            terminal_intercept: Some(terminal_intercept),
            execute_code_intercept: Some(execute_code_intercept),
        };
        let result = self.runtime.run_turn(request).await?;

        // Phase 39.1 (R39.1-02, D-02): append the full `result.appended` batch under a
        // SINGLE std::sync::Mutex lock acquisition so no concurrent turn can interleave
        // its messages between ours.  `std::sync::Mutex::lock()` is synchronous — the
        // guard is never held across an `.await` (the lock drops at the end of the block
        // below, before any subsequent async work).  The SQLite StateStore is the single
        // source of truth for the web session (no separate in-memory Vec to sync).
        {
            let mut store = self.state_store.lock().unwrap();
            for msg in &result.appended {
                let _ = store.add_message(session_id, msg);
            }
            // Guard drops here — one lock acquisition for the whole batch.
        }

        // Phase 46.7 Plan 04 (D-13/D-15/D-16): deterministic post-turn capture
        // chokepoint. Fires HERE — right after result.appended is persisted
        // (this turn's contract for "turn completed") — never as a voluntary
        // model tool call (round-6/7/8 lesson baked into chat_capture.rs).
        // The actual capture+note-formatting logic is a pure fn (unit-tested
        // below) so this call site only owns wiring it to stream_cb_arc.
        if let Some(artifact_note) = capture_turn_deliverable_note(session_id, user_input) {
            (stream_cb_arc)(artifact_note.as_str());
        }

        // WR-01 (Phase 34b Plan 03): render context_warnings out-of-band after run_turn.
        // Streamed via the same stream_callback so the web client receives it as a distinct
        // annotation after the model response. tracing::warn! for server-side visibility.
        if !result.context_warnings.is_empty() {
            let warning_lines: Vec<String> = result
                .context_warnings
                .iter()
                .map(|w| format!("- {}", w))
                .collect();
            let warnings_block =
                format!("\n--- Context Warnings ---\n{}\n", warning_lines.join("\n"));
            tracing::warn!(
                session_id = session_id,
                warnings = ?result.context_warnings,
                "context_warnings surfaced out-of-band (WR-01)"
            );
            (stream_cb_arc)(warnings_block.as_str());
        }

        // Phase 32 LEARN-01: periodic memory nudge (turn-based, post-response).
        // Fires AFTER run_turn() succeeded and AFTER result.appended is persisted.
        let nudge_interval = self.config.memory.nudge_interval;
        if nudge_interval > 0 && self.config.memory.memory_enabled {
            let should_fire = {
                let mut map = self.nudge_turns.lock().unwrap_or_else(|e| e.into_inner());
                let count = map.entry(session_id.to_string()).or_insert(0);
                *count += 1;
                if *count >= nudge_interval {
                    *count = 0;
                    true
                } else {
                    false
                }
            }; // std::sync::Mutex guard dropped here — BEFORE any await/spawn
            if should_fire {
                if let Some(ref mgr) = self.memory_manager {
                    let mgr_clone = Arc::clone(mgr);
                    // Source the client from the runtime (no new client build needed).
                    let client_clone = self.runtime.client().clone();
                    let config_clone = (*self.config).clone();
                    tokio::spawn(async move {
                        ironhermes_agent::nudge::spawn_nudge_review(
                            messages_snapshot,
                            mgr_clone,
                            client_clone,
                            &config_clone,
                        )
                        .await;
                    });
                }
            }
        }

        Ok(result)
    }

    async fn build_messages_for_turn(
        &self,
        session_id: &str,
        user_input: &str,
        attachment_ids: &[String],
    ) -> Result<Vec<ChatMessage>> {
        self.ensure_web_session(session_id)?;

        // Phase 46.7 Plan 04 (D-21): the session workspace must exist before
        // the turn — both so file-tool writes land somewhere real and so the
        // post-turn capture chokepoint (run_web_turn, above) has a directory
        // to scan. Pure path resolver + create_dir_all here (session_workspace_dir
        // itself is deliberately side-effect-free per its doc comment).
        let workspace_dir = ironhermes_core::session_workspace_dir(session_id);
        std::fs::create_dir_all(&workspace_dir).with_context(|| {
            format!(
                "failed to create session workspace dir: {}",
                workspace_dir.display()
            )
        })?;

        // Phase 39.1 (R39.1-07, D-06-RISK Pattern 3): per-turn config snapshot.
        // `self.config.model.default` is read here at turn start — a mid-turn `/model`
        // change only takes effect for the NEXT turn.  This is the web-surface analog
        // of the gateway handler.rs model snapshot.
        //
        // Personality (A3 verification): the web AppState has no `active_personality_overlay`
        // field.  Personality overlays are not applied on the web surface in the current
        // implementation — cmd_personality is not dispatched from the web WS handler.
        // No snapshot needed; mid-turn /personality is therefore safe by absence.
        let mut prompt_builder = PromptBuilder::new(&self.config.model.default, "web")
            .with_provider(&self.config.model.provider)
            .load_context(&std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        // Phase 28.1 Plan 03: source skill_registry and merged_tools from the runtime.
        prompt_builder.set_skill_registry(self.runtime.skill_registry().clone());
        // Phase 41.1 Plan 03 (SKILL-13 web / D-06): apply any ONE-SHOT skill
        // overlays activated by the WS SKILL-13 fallback for THIS session onto
        // this turn's prompt. Drained (take) so activation is one-shot —
        // `build_system_message` prepends each body to the system message. Body
        // originates only from resolve_skill_invocation → read_content
        // (SKILL-07-scanned), so no second unscanned injection path is added.
        for (skill_name, skill_body) in self.take_web_skill_overlays(session_id) {
            prompt_builder.activate_skill(&skill_name, &skill_body);
        }
        if let Some(ref manager) = self.memory_manager {
            prompt_builder.set_memory_manager(manager.clone());
        }
        prompt_builder.set_user_profile_enabled(self.config.memory.user_profile_enabled);
        // Phase 27.1.1-gap-02: populate active_toolsets so the system-prompt skills
        // catalog text reflects the same enabled set as the API tool schemas.
        prompt_builder.set_active_toolsets(self.runtime.merged_tools().enabled_toolset_names());
        // D-08 (Phase 46 Plan 04): populate connected_mcp_servers so requires_mcp_servers-gated
        // skills (e.g. the Cloudflare skills) only surface when their MCP server is connected.
        prompt_builder.set_connected_mcp_servers(
            self.runtime
                .mcp_manager()
                .map(|m| m.connected_server_names().into_iter().collect())
                .unwrap_or_default(),
        );
        // Phase 38.1 (D-04/D-05): freeze session timezone into PromptBuilder Timestamp slot.
        prompt_builder.set_timezone(self.config.agent.timezone.clone());
        prompt_builder.load_memory().await;
        prompt_builder.load_skills();
        let mut system_msg = prompt_builder.build_system_message();

        // Phase 46.7 Plan 04 (D-23/D-13 guidance halves): tell the agent where
        // its session workspace lives and that substantial HTML/Markdown
        // deliverables left there default into the artifact system. This is
        // the PROMPT-GUIDANCE half only — the STRUCTURAL containment half
        // (routing web-chat file-tool writes through resolve_web_chat_path)
        // requires per-turn context threading through the shared Tool
        // trait/ToolRegistry (used identically by CLI/Telegram/kanban), which
        // is an architectural change out of this plan's scope (see SUMMARY).
        let workspace_guidance = format!(
            "\n\nYour session workspace for this chat is: {}\n\
             Write files there with your file tools. Substantial HTML or \
             Markdown deliverables (e.g. index.html, README.md) left in that \
             workspace are automatically captured into the artifact gallery \
             at the end of the turn, unless the user asks you to show the \
             result inline instead. Any files the user attached to this \
             message are resolved into the turn below.",
            workspace_dir.display()
        );
        match system_msg.content {
            Some(MessageContent::Text(ref mut text)) => text.push_str(&workspace_guidance),
            _ => system_msg.content = Some(MessageContent::Text(workspace_guidance)),
        }

        // Phase 46.7 Plan 04 (D-02/D-11/T-46.7-13): resolve this turn's
        // attachment_ids into classified LocalAttachments, scoped to the
        // CURRENT session's attachments only — a foreign session's
        // attachment id has no row in this session's list_chat_attachments()
        // and is silently skipped (structural containment, not a filter).
        let local_attachments = self.resolve_local_attachments(session_id, attachment_ids);

        let (history_rows, user_msg) = {
            let mut store = self.state_store.lock().unwrap();
            let rows = store
                .get_messages(session_id)
                .context("failed to load persisted web messages")?;
            // D-02/D-11/D-07: build_chat_user_message replaces the plain
            // ChatMessage::user(user_input) — it produces the identical
            // Text-only message when local_attachments is empty, and the
            // hybrid inline/workspace/image turn when attachments are
            // present (including an attachment-only, empty-text turn).
            let user_msg = ironhermes_gateway::multimodal::build_chat_user_message(
                user_input,
                &local_attachments,
                &workspace_dir,
            );
            store
                .add_message(session_id, &user_msg)
                .context("failed to persist web user message")?;
            (rows, user_msg)
        };

        let mut messages = vec![system_msg];
        for row in &history_rows {
            if let Some(msg) = stored_to_chat_message(row) {
                if msg.role != Role::System {
                    messages.push(msg);
                }
            }
        }
        messages.push(user_msg);
        Ok(messages)
    }

    /// Phase 46.7 Plan 04 (D-02/D-11/T-46.7-13): resolve `attachment_ids` to
    /// classified [`LocalAttachment`](ironhermes_gateway::multimodal::LocalAttachment)s
    /// for turn assembly. Scoped to the CURRENT session ONLY —
    /// `list_chat_attachments(session_id)` never crosses sessions, so an id
    /// referencing another session's attachment simply has no matching row
    /// here and is silently dropped (T-46.7-13: structural containment, not
    /// a runtime access-control check that could be bypassed).
    ///
    /// Best-effort per-attachment: a missing file on disk or a processing
    /// failure (e.g. undecodable PDF) is logged and that one attachment is
    /// skipped — it never fails the whole turn.
    fn resolve_local_attachments(
        &self,
        session_id: &str,
        attachment_ids: &[String],
    ) -> Vec<ironhermes_gateway::multimodal::LocalAttachment> {
        resolve_local_attachments_from_store(&self.state_store, session_id, attachment_ids)
    }
}

/// Dependency-injected core of [`AppState::resolve_local_attachments`] — takes
/// `state_store` explicitly (instead of `&self`) so unit tests can drive it
/// against a fresh tempdir `StateStore` without constructing a full
/// `AppState` (which needs a live `AgentRuntime`/`ProviderResolver`).
fn resolve_local_attachments_from_store(
    state_store: &Arc<std::sync::Mutex<StateStore>>,
    session_id: &str,
    attachment_ids: &[String],
) -> Vec<ironhermes_gateway::multimodal::LocalAttachment> {
    if attachment_ids.is_empty() {
        return Vec::new();
    }

    let rows = {
        let store = state_store.lock().unwrap();
        store.list_chat_attachments(session_id).unwrap_or_default()
    };
    let attachments_dir = ironhermes_core::session_attachments_dir(session_id);

    attachment_ids
        .iter()
        .filter_map(|id| rows.iter().find(|row| &row.id == id))
        .filter_map(|row| {
            let path = attachments_dir.join(&row.stored_rel_path);
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(
                        session_id = session_id,
                        attachment_id = %row.id,
                        path = %path.display(),
                        error = %e,
                        "chat attachment bytes missing on disk; skipped for this turn"
                    );
                    return None;
                }
            };
            let mime = row
                .content_type
                .clone()
                .unwrap_or_else(|| "application/octet-stream".to_string());
            match ironhermes_gateway::multimodal::process_local_attachment(
                &bytes,
                &mime,
                &row.filename,
                ironhermes_gateway::multimodal::PdfMode::TextExtract,
            ) {
                Ok(local) => Some(local),
                Err(e) => {
                    tracing::warn!(
                        session_id = session_id,
                        attachment_id = %row.id,
                        error = %e,
                        "chat attachment failed to process for turn; skipped"
                    );
                    None
                }
            }
        })
        .collect()
}

/// Phase 46.7 Plan 04 (D-13/D-15/D-16): the deterministic post-turn capture
/// step, extracted as a pure function (`session_id`/`user_input` in,
/// `Option<String>` client-facing note out) so it is unit-testable in
/// isolation from `run_web_turn`'s `stream_cb_arc` plumbing and its
/// `self.runtime.run_turn()` call (which requires a live LLM endpoint and
/// cannot run in a unit test).
///
/// `session_workspace_dir(session_id)` is resolved HERE (the single call
/// site) — never passed in — because this is itself the canonical resolver
/// the round-7 lesson establishes (`capture_chat_deliverable` never
/// recomputes it; this function IS where it's computed once, from the
/// session id `run_web_turn` already owns).
///
/// Returns `Some(note)` only when an artifact was actually captured this
/// turn (D-16); `None` for opt-out, no-deliverable, and best-effort capture
/// failures (D-13 failures are logged via `tracing::warn!` and never
/// propagated — a capture problem must never fail the chat turn).
fn capture_turn_deliverable_note(session_id: &str, user_input: &str) -> Option<String> {
    let turn_opt_out = ironhermes_tools::chat_capture::detect_turn_opt_out(user_input);
    let workspace_dir = ironhermes_core::session_workspace_dir(session_id);
    match ironhermes_tools::chat_capture::capture_chat_deliverable(
        &workspace_dir,
        session_id,
        turn_opt_out,
    ) {
        Ok(Some(captured)) => {
            tracing::info!(
                session_id = session_id,
                artifact_id = %captured.artifact_id,
                filename = %captured.filename,
                "chat_capture: turn deliverable captured (D-13)"
            );
            // D-16: this is the wire contract the client consumes to render
            // an assistant-bubble artifact chip (Plan 05 owns the actual
            // chip UI/parsing).
            Some(format!(
                "\n[Artifact published: {} — /artifacts/{}]\n",
                captured.title, captured.artifact_id
            ))
        }
        Ok(None) => None, // opted out this turn, or nothing to capture
        Err(e) => {
            tracing::warn!(
                session_id = session_id,
                error = %e,
                "chat_capture: post-turn capture failed (best-effort, turn unaffected)"
            );
            None
        }
    }
}

// =============================================================================
// Phase 32.2 Plan 05 Task 2: subagent tree JSON adapter
// =============================================================================

/// Phase 32.2 D-12: Build a structured JSON tree from the current subagent registry.
///
/// Returns a `serde_json::Value::Array` of root nodes; each node carries:
/// `id`, `task_summary`, `uptime_secs`, `started_at_unix_ms`, `children: [...]`.
///
/// **No HTTP route is added** — this is a callable method for future API use
/// (RESEARCH confirmed no `/agents` endpoint exists in iron_hermes_ui today).
///
/// Call via `AppState::subagent_tree_json()` which delegates here.
#[allow(dead_code)] // Phase 32.2-05 future API endpoint; no HTTP route wired yet
pub fn build_subagent_tree_json(
    registry: &ironhermes_agent::subagent_registry::SubagentRegistry,
) -> serde_json::Value {
    let roots = registry.build_tree();
    let root_values: Vec<serde_json::Value> = roots.iter().map(node_to_json).collect();
    serde_json::Value::Array(root_values)
}

/// Recursively serialize a `SubagentTreeNode` into a JSON object.
///
/// Shape:
/// ```json
/// {
///   "id": "sub_abcd1234",
///   "task_summary": "orchestrate the pipeline",
///   "uptime_secs": 42,
///   "started_at_unix_ms": null,
///   "children": [...]
/// }
/// ```
///
/// `started_at_unix_ms` is `null` because `std::time::Instant` carries no
/// wall-clock epoch — it is relative-only. A future plan can thread the
/// session-start `SystemTime` to compute an absolute epoch offset.
#[allow(dead_code)] // called from build_subagent_tree_json; no active route yet
fn node_to_json(node: &ironhermes_agent::subagent_registry::SubagentTreeNode) -> serde_json::Value {
    let children: Vec<serde_json::Value> = node.children.iter().map(node_to_json).collect();
    serde_json::json!({
        "id": node.info.id,
        "task_summary": node.info.task_summary,
        "uptime_secs": node.info.started_at.elapsed().as_secs(),
        "started_at_unix_ms": serde_json::Value::Null,
        "children": children,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 36.3.12 D-08/D-10/D-11/D-12 (GAP 2): web-surface gated intercept builders
// ─────────────────────────────────────────────────────────────────────────────
//
// Mirror `ironhermes-cli::approval_gate::build_gated_terminal_intercept` /
// `build_gated_execute_code_intercept`, but take the already-constructed
// `DangerousCommandGuardrail` / `AuditLog` as parameters instead of a `Config`, so
// `#[cfg(test)]` below can point the audit log at a `tempfile::TempDir` instead of
// the real `IRONHERMES_HOME` (`AuditLog::load` always resolves to
// `get_hermes_home()`, which is not test-controllable). Kept as free functions
// (not inlined into `run_web_turn`) for exactly that reason — the SAME closure
// `run_web_turn` installs on `TurnRequest` is what the Task 1 `<behavior>` tests
// exercise, not a hand-rolled copy of the gating logic (the WR-02 lesson: a test
// that doesn't drive the real wiring proves nothing).

/// Build a gated `terminal` intercept handler for the web surface.
///
/// Routes every LLM-issued `terminal` call through
/// `ironhermes_hooks::execute_gated_command` (the Plan 03 chokepoint): a Tier-2
/// command is blocked before any spawn, every resolution is audited (`Allow`
/// included), and `is_remote_backend`/`forward_env_nonempty` force approval even on
/// an `Allow` classification (D-08).
///
/// - `tool`: the already-registered `terminal` tool (`AgentRuntime::terminal_tool_arc`) —
///   reusing it (rather than constructing a fresh bare tool) preserves `background=true`'s
///   `ProcessRegistry` wiring.
/// - `guard` / `audit_log`: shared across both intercepts by the caller (`run_web_turn`
///   builds one of each and clones the `Arc` into both builder calls).
// Phase 36.3.8 D-04 precedent (see run_web_turn's own allow above): the flat
// surface-parallel parameter set mirrors the CLI's build_gated_terminal_intercept
// exactly, so an explicit allow is preferred over a synthetic params bag here too.
#[allow(clippy::too_many_arguments)]
fn build_web_terminal_intercept(
    tool: Option<Arc<dyn ironhermes_tools::Tool>>,
    guard: Arc<ironhermes_hooks::DangerousCommandGuardrail>,
    audit_log: Arc<ironhermes_core::AuditLog>,
    session_id: String,
    chat_id: String,
    yolo: bool,
    is_remote_backend: bool,
    forward_env_nonempty: bool,
) -> ironhermes_tools::registry::InterceptHandler {
    Arc::new(move |args: serde_json::Value| {
        let tool = tool.clone();
        let guard = guard.clone();
        let audit_log = audit_log.clone();
        let session_id = session_id.clone();
        let chat_id = chat_id.clone();
        Box::pin(async move {
            let command = args
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let gate = crate::server::web_approval_gate::WebApprovalGate;

            let outcome = ironhermes_hooks::execute_gated_command(
                "terminal",
                &command,
                &guard,
                Some(&gate),
                &audit_log,
                &session_id,
                "web",
                &chat_id,
                yolo,
                is_remote_backend,
                forward_env_nonempty,
                || async move {
                    match tool {
                        Some(t) => t.execute(args).await,
                        None => Err(anyhow::anyhow!(
                            "terminal tool not registered on this runtime"
                        )),
                    }
                },
            )
            .await;

            Ok(outcome.to_string())
        })
    })
}

/// Build a gated `execute_code` intercept handler for the web surface.
///
/// Mirrors [`build_web_terminal_intercept`] but per D-11 passes an EMPTY opaque
/// `classify_arg` (the Python source never reaches `DangerousCommandGuardrail`'s
/// shell-syntax patterns) and always forwards `is_remote_backend=false` /
/// `forward_env_nonempty=false` — the Python script still runs on the local
/// `Sandbox`, never a remote backend, identical treatment to the CLI's
/// `build_gated_execute_code_intercept`. Every resolution — including
/// `background=true` calls — is still audited (D-08/D-12).
fn build_web_execute_code_intercept(
    tool: Option<Arc<dyn ironhermes_tools::Tool>>,
    guard: Arc<ironhermes_hooks::DangerousCommandGuardrail>,
    audit_log: Arc<ironhermes_core::AuditLog>,
    session_id: String,
    chat_id: String,
    yolo: bool,
) -> ironhermes_tools::registry::InterceptHandler {
    Arc::new(move |args: serde_json::Value| {
        let tool = tool.clone();
        let guard = guard.clone();
        let audit_log = audit_log.clone();
        let session_id = session_id.clone();
        let chat_id = chat_id.clone();
        Box::pin(async move {
            let gate = crate::server::web_approval_gate::WebApprovalGate;

            let outcome = ironhermes_hooks::execute_gated_command(
                "execute_code",
                "", // D-11: opaque — Python source is not shell syntax
                &guard,
                Some(&gate),
                &audit_log,
                &session_id,
                "web",
                &chat_id,
                yolo,
                false, // D-11: execute_code never routes to a remote backend
                false, // D-11: execute_code never forwards credentials cross-boundary
                || async move {
                    match tool {
                        Some(t) => t.execute(args).await,
                        None => Err(anyhow::anyhow!(
                            "execute_code tool not registered on this runtime"
                        )),
                    }
                },
            )
            .await;

            Ok(outcome.to_string())
        })
    })
}

impl AppState {
    /// Phase 32.2 Plan 05 D-12: return a structured JSON tree of active subagents.
    ///
    /// Accepts the subagent registry as a parameter — the caller is responsible
    /// for locking and providing the guard. This avoids borrow-checker issues with
    /// the `Arc<RwLock<SubagentRegistry>>` that lives inside the delegate-task wiring
    /// and is not re-exposed through `AppRuntimeBundle` (phase 28.1 plan 03: use subagent_registry directly).
    ///
    /// **No HTTP route** — this is a callable method for future API surface use
    /// (RESEARCH confirmed no `/agents` endpoint exists in iron_hermes_ui today).
    #[allow(dead_code)] // Phase 32.2-05 future API endpoint; no HTTP route wired yet
    pub fn subagent_tree_json(
        &self,
        registry: &ironhermes_agent::subagent_registry::SubagentRegistry,
    ) -> serde_json::Value {
        build_subagent_tree_json(registry)
    }
}

// =============================================================================
// Phase 32.3 Plan 04: /api/agents/{kill,interrupt,prune,status} REST handlers
// =============================================================================
//
// Four free functions taking `&AppState` + a JSON request body (or id query)
// and returning a `serde_json::Value` response. They mirror the
// `subagent_tree_json` adapter pattern from Plan 32.2-05 — keep the logic
// in `state.rs` next to the registry, expose Dioxus server fns in `api.rs`
// as thin wrappers (Dioxus is the project's REST surface here; main.rs has
// only one Axum route — `serve_dioxus_application` — so we follow the
// established Dioxus server-fn convention rather than inventing a routes.rs).
//
// **No confirm token on the web surface** (Phase 32.3 D-09 — only the
// Telegram gateway enforces `confirm` because of spoof-replay risk;
// TUI and web both have synchronous user presence).

/// Phase 32.3 Plan 04 (D-08): `POST /api/agents/kill` body handler.
///
/// Body shape: `{"id": "sub_xxxx"}`. Returns:
/// - `{"killed": true, "id": "sub_xxxx", "uptime_secs": N, "turns_used": M}` on success
/// - `{"killed": false, "id": "sub_xxxx"}` when the id is unknown / already gone
///
/// Maps to `ShrikeService::kill` → `KillResult` (W3 abort semantics from Plan 03).
///
/// Implemented as a free fn over `Option<&ShrikeService>` so tests can call it
/// without constructing the heavyweight `AppState`. The `AppState::api_agents_*`
/// wrappers below delegate here.
pub fn api_agents_kill(
    shrike: Option<&ironhermes_agent::shrike::ShrikeService>,
    body: serde_json::Value,
) -> serde_json::Value {
    let id = body
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    match shrike.and_then(|sh| sh.kill(&id)) {
        Some(kr) => serde_json::json!({
            "killed": true,
            "id": kr.id,
            "uptime_secs": kr.uptime_secs,
            "turns_used": kr.turns_used,
        }),
        None => serde_json::json!({ "killed": false, "id": id }),
    }
}

/// Phase 32.3 Plan 04 (D-08): `POST /api/agents/interrupt` body handler.
///
/// Body shape: `{"id": "sub_xxxx"}`. Returns:
/// - `{"interrupted": true, "id": "sub_xxxx"}` on cancel-token signalled
/// - `{"interrupted": false, "id": "sub_xxxx"}` when the id is unknown
///
/// Soft cancel only — does NOT abort the JoinHandle (per D-08).
pub fn api_agents_interrupt(
    shrike: Option<&ironhermes_agent::shrike::ShrikeService>,
    body: serde_json::Value,
) -> serde_json::Value {
    let id = body
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let interrupted = shrike.map(|sh| sh.interrupt(&id)).unwrap_or(false);
    serde_json::json!({ "interrupted": interrupted, "id": id })
}

/// Phase 32.3 Plan 04 (D-08): `POST /api/agents/prune` body handler.
///
/// Body shape: `{"stale_secs": 120}` (optional, defaults to 120 to match the
/// `SubagentConfig::stale_warn_seconds` default from Plan 01 D-05).
/// Returns: `{"pruned": ["sub_xxx", ...]}`.
pub fn api_agents_prune(
    shrike: Option<&ironhermes_agent::shrike::ShrikeService>,
    body: serde_json::Value,
) -> serde_json::Value {
    let stale_secs = body
        .get("stale_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(120);
    let pruned: Vec<String> = shrike.map(|sh| sh.prune(stale_secs)).unwrap_or_default();
    serde_json::json!({ "pruned": pruned })
}

/// Phase 32.3 Plan 04 (D-08): `GET /api/agents/status?id=sub_xxx` handler.
///
/// Returns `Some(json)` with the `SubagentStatusInfo` shape (Serialize derived
/// in Plan 03 context.rs:89) when the id is active, or `None` for 404.
pub fn api_agents_status(
    shrike: Option<&ironhermes_agent::shrike::ShrikeService>,
    id: &str,
) -> Option<serde_json::Value> {
    shrike
        .and_then(|sh| sh.status(id))
        .and_then(|info| serde_json::to_value(info).ok())
}

impl AppState {
    /// Phase 32.3 Plan 04: thin AppState wrapper over `api_agents_kill`.
    #[allow(dead_code)] // Phase 32.3 REST surface; Dioxus server fn wiring pending
    pub fn api_agents_kill(&self, body: serde_json::Value) -> serde_json::Value {
        api_agents_kill(self.shrike.as_deref(), body)
    }

    /// Phase 32.3 Plan 04: thin AppState wrapper over `api_agents_interrupt`.
    #[allow(dead_code)] // Phase 32.3 REST surface; Dioxus server fn wiring pending
    pub fn api_agents_interrupt(&self, body: serde_json::Value) -> serde_json::Value {
        api_agents_interrupt(self.shrike.as_deref(), body)
    }

    /// Phase 32.3 Plan 04: thin AppState wrapper over `api_agents_prune`.
    #[allow(dead_code)] // Phase 32.3 REST surface; Dioxus server fn wiring pending
    pub fn api_agents_prune(&self, body: serde_json::Value) -> serde_json::Value {
        api_agents_prune(self.shrike.as_deref(), body)
    }

    /// Phase 32.3 Plan 04: thin AppState wrapper over `api_agents_status`.
    pub fn api_agents_status(&self, id: &str) -> Option<serde_json::Value> {
        api_agents_status(self.shrike.as_deref(), id)
    }
}

// =============================================================================
// Phase 36.17.4 Plan 03 (D-01a) — queue_paused helper unit tests
// Phase 39.1 Plan 06: running_agents tests removed (RunningAgentGuard deleted).
// =============================================================================

#[cfg(test)]
mod phase_36_1_02_state_tests {
    use super::*;
    use std::sync::atomic::Ordering;

    // =========================================================================
    // Phase 36.17.4 Plan 03 (D-01a) — queue_paused helper unit tests
    // =========================================================================

    /// Helper: build an empty queue_paused map for unit testing.
    fn make_queue_paused(
    ) -> Arc<std::sync::Mutex<HashMap<String, Arc<std::sync::atomic::AtomicBool>>>> {
        Arc::new(std::sync::Mutex::new(HashMap::new()))
    }

    /// get_or_create_paused_flag free-fn equivalent — mirrors the method body
    /// in `AppState::get_or_create_paused_flag` byte-for-byte so unit tests
    /// don't need to build a full AppState.
    fn get_or_create_paused(
        map: &Arc<std::sync::Mutex<HashMap<String, Arc<std::sync::atomic::AtomicBool>>>>,
        session_id: &str,
    ) -> Arc<std::sync::atomic::AtomicBool> {
        let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(std::sync::atomic::AtomicBool::new(false)))
            .clone()
    }

    /// Phase 36.17.4 Plan 03 (D-01a behavior): a freshly created session's
    /// paused flag starts as false.
    #[test]
    fn paused_flag_starts_false() {
        let map = make_queue_paused();
        let flag = get_or_create_paused(&map, "session-A");
        assert!(
            !flag.load(Ordering::SeqCst),
            "A freshly created paused flag must start as false"
        );
    }

    /// Phase 36.17.4 Plan 03 (D-01a behavior): get-OR-create semantics — two
    /// lookups with the same session_id must return handles to the SAME
    /// AtomicBool (Arc::ptr_eq), AND mutations via the first handle must be
    /// visible through the second.
    #[test]
    fn paused_flag_persists_across_lookups() {
        let map = make_queue_paused();
        let first = get_or_create_paused(&map, "session-A");
        first.store(true, Ordering::SeqCst);

        let second = get_or_create_paused(&map, "session-A");
        assert!(
            second.load(Ordering::SeqCst),
            "Second lookup must observe the store(true) via the first handle"
        );
        assert!(
            Arc::ptr_eq(&first, &second),
            "Idempotent get_or_create must return the SAME Arc<AtomicBool> — \
             not a fresh allocation"
        );
    }

    /// Phase 36.17.4 Plan 03 (D-01a behavior): per-session isolation — two
    /// different session_ids produce two independent `Arc<AtomicBool>`
    /// instances. Setting session A's paused flag must NOT affect session B.
    #[test]
    fn paused_flag_session_isolation() {
        let map = make_queue_paused();
        let flag_a = get_or_create_paused(&map, "session-A");
        flag_a.store(true, Ordering::SeqCst);

        let flag_b = get_or_create_paused(&map, "session-B");
        assert!(
            !flag_b.load(Ordering::SeqCst),
            "Session B's paused flag must remain false after session A's is set"
        );
        assert!(
            flag_a.load(Ordering::SeqCst),
            "Session A's paused flag must still be true (mutating B does not \
             reset A)"
        );
        assert!(
            !Arc::ptr_eq(&flag_a, &flag_b),
            "Distinct session_ids must produce distinct Arc<AtomicBool> \
             instances (no cross-session bleed)"
        );
    }
}

fn stored_to_chat_message(row: &StoredMessage) -> Option<ChatMessage> {
    let role = match row.role.as_str() {
        "system" => Role::System,
        "user" => Role::User,
        "assistant" => Role::Assistant,
        "tool" => Role::Tool,
        _ => return None,
    };

    let tool_calls = row
        .tool_calls
        .as_ref()
        .and_then(|json| serde_json::from_str(json).ok());

    Some(ChatMessage {
        role,
        content: row.content.clone().map(MessageContent::Text),
        tool_calls,
        tool_call_id: row.tool_call_id.clone(),
        name: row.tool_name.clone(),
        is_recall_context: false,
    })
}

#[cfg(test)]
mod plan_32_2_05_tests {
    use super::*;
    use ironhermes_agent::subagent_registry::{SubagentInfo, SubagentRegistry};
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;

    fn make_info(id: &str, parent_id: Option<&str>) -> SubagentInfo {
        SubagentInfo {
            id: id.to_string(),
            task_summary: format!("task for {}", id),
            parent_id: parent_id.map(|s| s.to_string()),
            started_at: std::time::Instant::now(),
            cancel: CancellationToken::new(),
            transcript_path: PathBuf::from("/dev/null"),
            // Phase 32.3 Plan 01 (D-04 reservation): test helpers leave None;
            // tree-json only inspects parent_id/depth, not activity.
            activity_last: None,
            // Phase 32.3 Plan 02 (D-05): default; tree-json doesn't read this.
            stale_warn_seconds: 120,
        }
    }

    /// Phase 32.3 Plan 01: register_guarded with a dangling Weak so the
    /// guard's Drop is a silent no-op. Tests assert tree-json shape, not
    /// lifecycle — that lives in `ironhermes-agent/tests/registration_guard.rs`.
    fn register_no_lifecycle(reg: &mut SubagentRegistry, info: SubagentInfo) {
        let weak: std::sync::Weak<tokio::sync::RwLock<SubagentRegistry>> = std::sync::Weak::new();
        let guard = reg.register_guarded(info, weak);
        std::mem::forget(guard);
    }

    #[test]
    fn test_subagent_tree_json_serializes_nested_tree() {
        // 3-node tree: root → mid → leaf
        let mut reg = SubagentRegistry::new();
        register_no_lifecycle(&mut reg, make_info("sub_root0000", None));
        register_no_lifecycle(&mut reg, make_info("sub_mid11111", Some("sub_root0000")));
        register_no_lifecycle(&mut reg, make_info("sub_leaf2222", Some("sub_mid11111")));

        let value = build_subagent_tree_json(&reg);

        // Top-level is an array with 1 root
        let arr = value.as_array().expect("expected JSON array at top level");
        assert_eq!(arr.len(), 1, "expected 1 root node");

        let root = &arr[0];
        assert_eq!(root["id"], "sub_root0000");
        assert_eq!(root["task_summary"], "task for sub_root0000");
        assert!(
            root["uptime_secs"].is_number(),
            "uptime_secs should be a number"
        );
        assert!(
            root["started_at_unix_ms"].is_null(),
            "started_at_unix_ms should be null"
        );

        // root.children has 1 mid node
        let root_children = root["children"]
            .as_array()
            .expect("root.children should be array");
        assert_eq!(root_children.len(), 1, "root should have 1 child (mid)");

        let mid = &root_children[0];
        assert_eq!(mid["id"], "sub_mid11111");
        assert_eq!(mid["task_summary"], "task for sub_mid11111");

        // mid.children has 1 leaf node
        let mid_children = mid["children"]
            .as_array()
            .expect("mid.children should be array");
        assert_eq!(mid_children.len(), 1, "mid should have 1 child (leaf)");

        let leaf = &mid_children[0];
        assert_eq!(leaf["id"], "sub_leaf2222");
        assert_eq!(leaf["task_summary"], "task for sub_leaf2222");
        let leaf_children = leaf["children"]
            .as_array()
            .expect("leaf.children should be array");
        assert!(leaf_children.is_empty(), "leaf should have no children");
    }
}

// =============================================================================
// Phase 32.3 Plan 04 tests — REST endpoint JSON shapes (no AppState required)
// =============================================================================
//
// These tests construct a `ShrikeService` directly from a `SubagentRegistry`
// Arc and exercise the four free-function endpoints. AppState's full init is
// too heavy for a unit-test (Config/StateStore/MemoryManager), so we test the
// endpoint JSON-shaping layer directly — which IS the load-bearing surface
// (Plan 03's shrike_service.rs already covers the underlying kill/interrupt/
// prune/status semantics).
//
// Tests use `flavor = "multi_thread"` per Plan 03 Pitfall 1 — ShrikeService's
// methods bridge async→sync via `block_in_place + block_on` which requires
// the multi-thread runtime.
#[cfg(test)]
mod plan_32_3_04_tests {
    use super::*;
    use ironhermes_agent::shrike::ShrikeService;
    use ironhermes_agent::subagent_registry::{SubagentInfo, SubagentRegistry};
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tokio_util::sync::CancellationToken;

    fn make_info(id: &str) -> SubagentInfo {
        SubagentInfo {
            id: id.to_string(),
            task_summary: format!("task for {}", id),
            parent_id: None,
            started_at: std::time::Instant::now(),
            cancel: CancellationToken::new(),
            transcript_path: PathBuf::from("/dev/null"),
            activity_last: Some(Arc::new(std::sync::Mutex::new(std::time::Instant::now()))),
            stale_warn_seconds: 120,
        }
    }

    /// Register an info with a dangling Weak so the guard's Drop is a silent
    /// no-op — endpoint tests care about the JSON shape, not lifecycle.
    async fn register_for_endpoint_test(reg: &Arc<RwLock<SubagentRegistry>>, id: &str) {
        let weak: std::sync::Weak<RwLock<SubagentRegistry>> = std::sync::Weak::new();
        let mut w = reg.write().await;
        let guard = w.register_guarded(make_info(id), weak);
        std::mem::forget(guard);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_api_agents_kill_returns_killed_true_for_active_id() {
        let reg = Arc::new(RwLock::new(SubagentRegistry::new()));
        register_for_endpoint_test(&reg, "sub_kill0000").await;
        let shrike = ShrikeService::new(reg.clone());

        let body = serde_json::json!({ "id": "sub_kill0000" });
        let resp = api_agents_kill(Some(&shrike), body);

        assert_eq!(resp["killed"], serde_json::Value::Bool(true));
        assert_eq!(resp["id"], "sub_kill0000");
        assert!(
            resp["uptime_secs"].is_number(),
            "uptime_secs should be a number: {}",
            resp
        );
        assert!(
            resp["turns_used"].is_number(),
            "turns_used should be a number: {}",
            resp
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_api_agents_kill_returns_killed_false_for_missing_id() {
        let reg = Arc::new(RwLock::new(SubagentRegistry::new()));
        let shrike = ShrikeService::new(reg.clone());

        let body = serde_json::json!({ "id": "sub_missing0" });
        let resp = api_agents_kill(Some(&shrike), body);

        assert_eq!(resp["killed"], serde_json::Value::Bool(false));
        assert_eq!(resp["id"], "sub_missing0");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_api_agents_interrupt_returns_true_for_active_id() {
        let reg = Arc::new(RwLock::new(SubagentRegistry::new()));
        register_for_endpoint_test(&reg, "sub_intr0000").await;
        let shrike = ShrikeService::new(reg.clone());

        let body = serde_json::json!({ "id": "sub_intr0000" });
        let resp = api_agents_interrupt(Some(&shrike), body);

        assert_eq!(resp["interrupted"], serde_json::Value::Bool(true));
        assert_eq!(resp["id"], "sub_intr0000");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_api_agents_prune_returns_pruned_array() {
        // Empty registry → empty pruned list (covers the JSON-shape path)
        let reg = Arc::new(RwLock::new(SubagentRegistry::new()));
        let shrike = ShrikeService::new(reg);

        let body = serde_json::json!({ "stale_secs": 1 });
        let resp = api_agents_prune(Some(&shrike), body);

        assert!(
            resp["pruned"].is_array(),
            "pruned must be an array: {}",
            resp
        );
        let arr = resp["pruned"].as_array().unwrap();
        assert!(arr.is_empty(), "no stale entries yet: {}", resp);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_api_agents_status_returns_some_for_active_id() {
        let reg = Arc::new(RwLock::new(SubagentRegistry::new()));
        register_for_endpoint_test(&reg, "sub_stat0000").await;
        let shrike = ShrikeService::new(reg);

        let resp = api_agents_status(Some(&shrike), "sub_stat0000");
        let resp = resp.expect("status should return Some for active id");

        assert_eq!(resp["id"], "sub_stat0000");
        assert!(resp["uptime_secs"].is_number());
        assert_eq!(resp["status"], "running");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_api_agents_status_returns_none_for_missing_id() {
        let reg = Arc::new(RwLock::new(SubagentRegistry::new()));
        let shrike = ShrikeService::new(reg);

        let resp = api_agents_status(Some(&shrike), "sub_missing0");
        assert!(resp.is_none(), "status should return None for missing id");
    }

    /// AppState shrike: None — the endpoints must degrade gracefully without
    /// panicking. Locks the "no ShrikeService configured" path.
    #[test]
    fn test_api_agents_endpoints_degrade_gracefully_with_none_shrike() {
        let killed = api_agents_kill(None, serde_json::json!({ "id": "sub_xxx" }));
        assert_eq!(killed["killed"], serde_json::Value::Bool(false));

        let interrupted = api_agents_interrupt(None, serde_json::json!({ "id": "sub_xxx" }));
        assert_eq!(interrupted["interrupted"], serde_json::Value::Bool(false));

        let pruned = api_agents_prune(None, serde_json::json!({}));
        assert!(pruned["pruned"].is_array());

        let status = api_agents_status(None, "sub_xxx");
        assert!(status.is_none());
    }
}

// =============================================================================
// Phase 46.7 Plan 04: attachment resolution + post-turn capture chokepoint
// =============================================================================
//
// `build_messages_for_turn`/`run_web_turn` themselves need a full `AppState`
// (live `AgentRuntime`, memory manager, etc.) and `run_web_turn` calls
// `self.runtime.run_turn()`, which requires a real LLM endpoint — not
// unit-testable directly. Both pieces of turn-time behavior this plan adds
// are therefore exercised as pure, dependency-injected functions
// (`resolve_local_attachments_from_store`, `capture_turn_deliverable_note`)
// that `build_messages_for_turn`/`run_web_turn` call — this is the real
// production code path, just decoupled from the AppState/AgentRuntime
// construction cost.
#[cfg(test)]
mod plan_46_7_04_tests {
    use super::*;
    use tempfile::tempdir;

    // Guards IRONHERMES_HOME / IRONHERMES_ARTIFACTS_DB mutation via the
    // crate-wide `test_support::env_lock()` — a module-local mutex cannot
    // serialize env mutation against other modules' tests in the same binary
    // (the runner process-isolates per BINARY, not per test fn).

    fn fresh_store_with_session(session_id: &str) -> Arc<std::sync::Mutex<StateStore>> {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.db");
        let mut store = StateStore::new(&path).unwrap();
        store
            .create_session(session_id, "web", None, None, None, None)
            .unwrap();
        std::mem::forget(dir); // outlive the test body — Arc holds the open conn
        Arc::new(std::sync::Mutex::new(store))
    }

    /// A tiny valid 1x1 PNG (smallest well-formed PNG, avoids needing the
    /// `image` crate to synthesize one) — enough for `fit_image_for_vision`'s
    /// `image::load_from_memory` to decode successfully.
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8,
        0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn resolved_image_attachment_produces_parts_content_not_bare_text() {
        let _lock = crate::server::test_support::env_lock();
        let home_dir = tempdir().unwrap();
        // SAFETY: env var mutation guarded by test_support::env_lock() above.
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home_dir.path());
        }

        let session_id = "sess-46704-a";
        let store = fresh_store_with_session(session_id);

        // Write the attachment bytes exactly as upload_attachment_impl would
        // (chat_attachments_api.rs Task 1): under
        // session_attachments_dir(session_id)/<opaque-id>/<leaf>.
        let att_dir = ironhermes_core::session_attachments_dir(session_id).join("att1");
        std::fs::create_dir_all(&att_dir).unwrap();
        std::fs::write(att_dir.join("pixel.png"), TINY_PNG).unwrap();
        {
            let mut s = store.lock().unwrap();
            s.add_chat_attachment(
                session_id,
                None,
                "pixel.png",
                Some("image/png"),
                TINY_PNG.len() as i64,
                "att1/pixel.png",
            )
            .unwrap();
        }
        let rows = store
            .lock()
            .unwrap()
            .list_chat_attachments(session_id)
            .unwrap();
        let attachment_id = rows[0].id.clone();

        let locals = resolve_local_attachments_from_store(&store, session_id, &[attachment_id]);
        assert_eq!(locals.len(), 1, "the image attachment must resolve");

        let workspace_dir = ironhermes_core::session_workspace_dir(session_id);
        let user_msg = ironhermes_gateway::multimodal::build_chat_user_message(
            "check this out",
            &locals,
            &workspace_dir,
        );
        assert!(
            matches!(
                user_msg.content,
                Some(ironhermes_core::types::MessageContent::Parts(_))
            ),
            "an image attachment must produce MessageContent::Parts, not bare text"
        );

        // SAFETY: still holding test_support::env_lock() from above.
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
    }

    #[test]
    fn resolved_text_attachment_inlines_content_not_bare_user_input() {
        let _lock = crate::server::test_support::env_lock();
        let home_dir = tempdir().unwrap();
        // SAFETY: guarded by test_support::env_lock().
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home_dir.path());
        }

        let session_id = "sess-46704-b";
        let store = fresh_store_with_session(session_id);

        let att_dir = ironhermes_core::session_attachments_dir(session_id).join("att2");
        std::fs::create_dir_all(&att_dir).unwrap();
        std::fs::write(att_dir.join("notes.txt"), b"the secret ingredient is love").unwrap();
        {
            let mut s = store.lock().unwrap();
            s.add_chat_attachment(
                session_id,
                None,
                "notes.txt",
                Some("text/plain"),
                30,
                "att2/notes.txt",
            )
            .unwrap();
        }
        let rows = store
            .lock()
            .unwrap()
            .list_chat_attachments(session_id)
            .unwrap();
        let attachment_id = rows[0].id.clone();

        let locals = resolve_local_attachments_from_store(&store, session_id, &[attachment_id]);
        assert_eq!(locals.len(), 1);

        let workspace_dir = ironhermes_core::session_workspace_dir(session_id);
        let user_msg = ironhermes_gateway::multimodal::build_chat_user_message(
            "what does this say",
            &locals,
            &workspace_dir,
        );
        let text = user_msg
            .content
            .as_ref()
            .and_then(|c| c.as_text())
            .expect("text-only turn must have text content");
        assert_ne!(
            text, "what does this say",
            "the inlined attachment body must be appended, not just the bare user_input"
        );
        assert!(
            text.contains("the secret ingredient is love"),
            "the attachment body must be inlined into the user message (got: {text})"
        );

        // SAFETY: still holding test_support::env_lock() from above.
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
    }

    #[test]
    fn a_foreign_attachment_id_resolves_to_nothing() {
        let _lock = crate::server::test_support::env_lock();
        let home_dir = tempdir().unwrap();
        // SAFETY: guarded by test_support::env_lock().
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home_dir.path());
        }

        let session_id = "sess-46704-c";
        let store = fresh_store_with_session(session_id);
        // No attachment ever added to this session — a foreign/nonexistent id
        // must resolve to an empty list (T-46.7-13), not an error.
        let locals = resolve_local_attachments_from_store(
            &store,
            session_id,
            &["some-other-sessions-attachment-id".to_string()],
        );
        assert!(locals.is_empty());

        // SAFETY: still holding test_support::env_lock() from above.
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
    }

    #[test]
    fn capture_turn_deliverable_note_captures_index_html_and_surfaces_a_note() {
        let _lock = crate::server::test_support::env_lock();
        let home_dir = tempdir().unwrap();
        let artifacts_dir = tempdir().unwrap();
        let artifacts_db = artifacts_dir.path().join("artifacts.db");
        // SAFETY: env var mutation guarded by test_support::env_lock() above.
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home_dir.path());
            std::env::set_var(ironhermes_artifacts::ARTIFACTS_DB_ENV, &artifacts_db);
        }

        let session_id = "sess-46704-cap";
        let workspace_dir = ironhermes_core::session_workspace_dir(session_id);
        std::fs::create_dir_all(&workspace_dir).unwrap();
        std::fs::write(
            workspace_dir.join("index.html"),
            "<html><body>hi</body></html>",
        )
        .unwrap();

        let note = capture_turn_deliverable_note(session_id, "build me a page");
        let note = note.expect("an index.html in the workspace must be captured and noted");
        assert!(
            note.contains("Artifact published"),
            "D-16 note must announce the published artifact (got: {note})"
        );

        // SAFETY: still holding test_support::env_lock() from above.
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
            std::env::remove_var(ironhermes_artifacts::ARTIFACTS_DB_ENV);
        }
    }

    #[test]
    fn capture_turn_deliverable_note_returns_none_when_user_opts_out() {
        let _lock = crate::server::test_support::env_lock();
        let home_dir = tempdir().unwrap();
        let artifacts_dir = tempdir().unwrap();
        let artifacts_db = artifacts_dir.path().join("artifacts.db");
        // SAFETY: guarded by test_support::env_lock().
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home_dir.path());
            std::env::set_var(ironhermes_artifacts::ARTIFACTS_DB_ENV, &artifacts_db);
        }

        let session_id = "sess-46704-optout";
        let workspace_dir = ironhermes_core::session_workspace_dir(session_id);
        std::fs::create_dir_all(&workspace_dir).unwrap();
        std::fs::write(workspace_dir.join("index.html"), "<html>hi</html>").unwrap();

        // D-15: an explicit "just show it inline" opt-out phrase suppresses
        // capture for this turn even though a deliverable exists.
        let note = capture_turn_deliverable_note(session_id, "just show it inline, don't publish");
        assert!(
            note.is_none(),
            "an opt-out turn must not surface a capture note"
        );

        // SAFETY: still holding test_support::env_lock() from above.
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
            std::env::remove_var(ironhermes_artifacts::ARTIFACTS_DB_ENV);
        }
    }

    #[test]
    fn capture_turn_deliverable_note_returns_none_when_nothing_to_capture() {
        let _lock = crate::server::test_support::env_lock();
        let home_dir = tempdir().unwrap();
        let artifacts_dir = tempdir().unwrap();
        let artifacts_db = artifacts_dir.path().join("artifacts.db");
        // SAFETY: guarded by test_support::env_lock().
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home_dir.path());
            std::env::set_var(ironhermes_artifacts::ARTIFACTS_DB_ENV, &artifacts_db);
        }

        let session_id = "sess-46704-empty";
        // Workspace dir intentionally not created — nothing to scan.
        let note = capture_turn_deliverable_note(session_id, "just chatting, no files");
        assert!(note.is_none());

        // SAFETY: still holding test_support::env_lock() from above.
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
            std::env::remove_var(ironhermes_artifacts::ARTIFACTS_DB_ENV);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 36.3.12 Task 1 (D-08/D-10/D-11/D-12, GAP 2): web-surface gated intercept
// closure-level behavior tests. These drive the SAME `build_web_terminal_intercept`
// / `build_web_execute_code_intercept` functions `run_web_turn` installs onto
// `TurnRequest` — not a hand-rolled re-implementation of the gating logic (WR-02).
// ─────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod plan_36_3_12_12_web_gating_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tempfile::TempDir;

    /// Stub `Tool` that records whether `execute` was invoked, so "the tool was
    /// NEVER invoked" (the Tier-2 assertion) is a real assertion about the gated
    /// closure's behavior, not an inference from the returned outcome alone.
    struct RecordingTool {
        invoked: Arc<AtomicBool>,
        output: String,
    }

    #[async_trait::async_trait]
    impl ironhermes_tools::Tool for RecordingTool {
        fn name(&self) -> &str {
            "terminal"
        }
        fn toolset(&self) -> &str {
            "test"
        }
        fn description(&self) -> &str {
            "recording stub tool for web-gating tests"
        }
        fn schema(&self) -> ironhermes_core::ToolSchema {
            ironhermes_core::ToolSchema::new(
                "terminal",
                "recording stub tool for web-gating tests",
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            self.invoked.store(true, Ordering::SeqCst);
            Ok(self.output.clone())
        }
    }

    /// Tempfile-backed `AuditLog` — real reads, not assumptions (Task 1 requirement).
    fn test_audit_log() -> (TempDir, Arc<ironhermes_core::AuditLog>) {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("audit.jsonl");
        let log = ironhermes_core::AuditLog::with_path(path, ironhermes_core::AuditConfig::default());
        (dir, Arc::new(log))
    }

    fn read_audit_entries(dir: &TempDir) -> Vec<ironhermes_core::AuditEntry> {
        let path = dir.path().join("audit.jsonl");
        let contents = std::fs::read_to_string(&path).unwrap_or_default();
        contents
            .lines()
            .map(|l| serde_json::from_str(l).expect("valid AuditEntry JSON"))
            .collect()
    }

    /// A Tier-2 command (`rm -rf /`) driven through the web terminal intercept
    /// closure is Blocked, the underlying tool is NEVER invoked, and an audit
    /// entry is produced.
    #[tokio::test]
    async fn tier2_command_blocked_tool_never_invoked_and_audited() {
        let (dir, audit_log) = test_audit_log();
        let guard = Arc::new(ironhermes_hooks::DangerousCommandGuardrail::new());
        let invoked = Arc::new(AtomicBool::new(false));
        let tool: Option<Arc<dyn ironhermes_tools::Tool>> = Some(Arc::new(RecordingTool {
            invoked: invoked.clone(),
            output: "should never run".to_string(),
        }));

        let intercept = build_web_terminal_intercept(
            tool,
            guard,
            audit_log,
            "sess-web-1".to_string(),
            "sess-web-1".to_string(),
            false, // yolo=false
            false, // is_remote_backend
            false, // forward_env_nonempty
        );

        let result = intercept(serde_json::json!({ "command": "rm -rf /" })).await;
        let outcome_str = result.expect("intercept must not error");
        assert!(
            outcome_str.starts_with("Blocked("),
            "Tier-2 command must be Blocked, got: {outcome_str}"
        );
        assert!(
            !invoked.load(Ordering::SeqCst),
            "the underlying tool must NEVER be invoked on a Tier-2 Block"
        );

        let entries = read_audit_entries(&dir);
        assert_eq!(entries.len(), 1, "Block must be audited exactly once");
        assert_eq!(entries[0].decision, "blocked");
    }

    /// A safe command (`echo web-probe`) driven through the same closure reaches
    /// the underlying tool and is audited with an Allow resolution.
    #[tokio::test]
    async fn safe_command_reaches_tool_and_audits_allow() {
        let (dir, audit_log) = test_audit_log();
        let guard = Arc::new(ironhermes_hooks::DangerousCommandGuardrail::new());
        let invoked = Arc::new(AtomicBool::new(false));
        let tool: Option<Arc<dyn ironhermes_tools::Tool>> = Some(Arc::new(RecordingTool {
            invoked: invoked.clone(),
            output: "web-probe".to_string(),
        }));

        let intercept = build_web_terminal_intercept(
            tool,
            guard,
            audit_log,
            "sess-web-2".to_string(),
            "sess-web-2".to_string(),
            false,
            false,
            false,
        );

        let result = intercept(serde_json::json!({ "command": "echo web-probe" })).await;
        let outcome_str = result.expect("intercept must not error");
        assert!(
            outcome_str.contains("web-probe"),
            "safe command must reach the underlying tool, got: {outcome_str}"
        );
        assert!(
            invoked.load(Ordering::SeqCst),
            "the underlying tool must be invoked for an Allow-classified command"
        );

        let entries = read_audit_entries(&dir);
        assert_eq!(entries.len(), 1, "Allow must be audited exactly once");
        assert_eq!(entries[0].decision, "allowed");
    }

    /// D-11: `execute_code` never forces an `Allow`-classified command through the
    /// approval gate the way `terminal` does under D-08's `is_remote_backend` /
    /// `forward_env_nonempty` forcing — this closure always passes `false`/`false`
    /// for those two parameters (see `build_web_execute_code_intercept`'s own doc
    /// comment). This test proves the negative: an ordinary Python script classifies
    /// `Allow` (empty `classify_arg`), reaches the underlying tool, and audits
    /// "allowed" — `WebApprovalGate` is never consulted at all here.
    ///
    /// (Corrected per 36.3.12-REVIEW.md WR-07: the previous doc comment claimed
    /// this test drove `WebApprovalGate::request_approval`'s deny path "end-to-end",
    /// which is the *opposite* of what the body exercises. The Warn-deny and
    /// remote-forced-approval-deny paths are covered separately below by
    /// `tier1_warn_command_denied_tool_never_invoked_and_audited` and
    /// `allow_forced_through_gate_when_remote_backend_is_denied`.)
    #[tokio::test]
    async fn execute_code_allow_runs_and_audits_without_forced_approval() {
        let (dir, audit_log) = test_audit_log();
        let guard = Arc::new(ironhermes_hooks::DangerousCommandGuardrail::new());
        let invoked = Arc::new(AtomicBool::new(false));
        let tool: Option<Arc<dyn ironhermes_tools::Tool>> = Some(Arc::new(RecordingTool {
            invoked: invoked.clone(),
            output: "python output".to_string(),
        }));

        let intercept = build_web_execute_code_intercept(
            tool,
            guard,
            audit_log,
            "sess-web-3".to_string(),
            "sess-web-3".to_string(),
            false, // yolo=false
        );

        let result = intercept(serde_json::json!({ "code": "print('hi')" })).await;
        let outcome_str = result.expect("intercept must not error");
        assert!(
            outcome_str.contains("python output"),
            "D-11: execute_code with empty classify_arg must classify Allow and run, \
             got: {outcome_str}"
        );
        assert!(invoked.load(Ordering::SeqCst));

        let entries = read_audit_entries(&dir);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].tool, "execute_code");
        assert_eq!(entries[0].decision, "allowed");
    }

    /// WR-07 fix: a Tier-1 (`Warn`-classified) command driven through the real
    /// `build_web_terminal_intercept` closure must consult `WebApprovalGate`, be
    /// Denied (the gate always fails closed — GAP 2, no interactive web approval
    /// UX exists), never invoke the underlying tool, and audit `decision="denied"`.
    /// `"curl https://x"` is the same Warn-classified probe string used by
    /// `ironhermes-hooks::gated_exec`'s own `warn_no_gate_denies_and_audits` /
    /// `warn_gate_denied_variants_deny_and_audit` tests.
    #[tokio::test]
    async fn tier1_warn_command_denied_tool_never_invoked_and_audited() {
        let (dir, audit_log) = test_audit_log();
        let guard = Arc::new(ironhermes_hooks::DangerousCommandGuardrail::new());
        let invoked = Arc::new(AtomicBool::new(false));
        let tool: Option<Arc<dyn ironhermes_tools::Tool>> = Some(Arc::new(RecordingTool {
            invoked: invoked.clone(),
            output: "should never run".to_string(),
        }));

        let intercept = build_web_terminal_intercept(
            tool,
            guard,
            audit_log,
            "sess-web-warn".to_string(),
            "sess-web-warn".to_string(),
            false, // yolo=false
            false, // is_remote_backend
            false, // forward_env_nonempty
        );

        let result = intercept(serde_json::json!({ "command": "curl https://x" })).await;
        let outcome_str = result.expect("intercept must not error");
        assert!(
            outcome_str.starts_with("Denied("),
            "Tier-1 (Warn) command must be Denied on the web surface — WebApprovalGate \
             has no interactive approval UX and fails closed (GAP 2) — got: {outcome_str}"
        );
        assert!(
            !invoked.load(Ordering::SeqCst),
            "the underlying tool must NEVER be invoked when WebApprovalGate denies a \
             Warn-classified command"
        );

        let entries = read_audit_entries(&dir);
        assert_eq!(entries.len(), 1, "the Warn-deny must be audited exactly once");
        assert_eq!(entries[0].decision, "denied");
    }

    /// WR-07 fix: `is_remote_backend=true` forces even an `Allow`-classified command
    /// (`"ls -la"`) through the approval gate (D-08) on the web surface, driven
    /// through the real `build_web_terminal_intercept` closure — proving the
    /// forced-approval branch is reachable here, and (unlike the CLI's
    /// `allow_forced_through_gate_when_remote_backend`, which can approve) it is
    /// always Denied because `WebApprovalGate` never approves. The underlying tool
    /// must never be invoked and the denial must be audited.
    #[tokio::test]
    async fn allow_forced_through_gate_when_remote_backend_is_denied() {
        let (dir, audit_log) = test_audit_log();
        let guard = Arc::new(ironhermes_hooks::DangerousCommandGuardrail::new());
        let invoked = Arc::new(AtomicBool::new(false));
        let tool: Option<Arc<dyn ironhermes_tools::Tool>> = Some(Arc::new(RecordingTool {
            invoked: invoked.clone(),
            output: "should never run".to_string(),
        }));

        let intercept = build_web_terminal_intercept(
            tool,
            guard,
            audit_log,
            "sess-web-remote".to_string(),
            "sess-web-remote".to_string(),
            false, // yolo=false
            true,  // is_remote_backend — D-08 forces an Allow command through the gate
            false, // forward_env_nonempty
        );

        let result = intercept(serde_json::json!({ "command": "ls -la" })).await;
        let outcome_str = result.expect("intercept must not error");
        assert!(
            outcome_str.starts_with("Denied("),
            "is_remote_backend=true must force even an Allow-classified command through \
             the approval gate (D-08), and WebApprovalGate always denies — got: {outcome_str}"
        );
        assert!(
            !invoked.load(Ordering::SeqCst),
            "the underlying tool must NEVER be invoked when the D-08 forced-approval \
             branch is denied"
        );

        let entries = read_audit_entries(&dir);
        assert_eq!(
            entries.len(),
            1,
            "the forced-approval denial must be audited exactly once"
        );
        assert_eq!(entries[0].decision, "denied");
    }
}

use anyhow::{Context, Result};
use ironhermes_core::{ChatMessage, ChatResponse, ToolCall, ToolSchema, Usage};
use ironhermes_hooks::{HookEvent, HookEventKind, HookRegistry};
use ironhermes_state::StateStore;
use ironhermes_tools::ToolRegistry;
// Phase 25.3 D-T-1 / D-T-3: trajectory ledger types.
// `TrajectoryEntry` + `ImpactLevel` come from `ironhermes-trajectory` (Plan 1 wire format).
// `TrajectoryWriterHandle` lives in `ironhermes-core` per Plan 6's cycle-break;
// AgentLoop holds the trait-object form.
use ironhermes_trajectory::{ImpactLevel, TrajectoryEntry};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::any_client::AnyClient;
use crate::budget::{BudgetHandle, PressureTier, advisory_text};
use crate::client::{StreamEvent, ToolCallDelta};
use crate::context_compressor::{ContextCompressor, estimate_messages_tokens};
use crate::context_engine::{ContextEngine, ContextStats};
use crate::memory::MemoryManager;
use crate::pressure_warning::PressureTracker;
use crate::subdir_discovery::SubdirDiscovery;

/// Why the agent loop stopped (D-15 / G-01 / Plan 21.7-05).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Natural completion — LLM produced a response with no tool calls.
    Natural,
    /// Local `max_iterations` ceiling reached (legacy fallback).
    MaxIterations,
    /// Shared `BudgetHandle` exhausted at the top of a turn (100% / Stop100).
    /// This path is unskippable — no yolo check bypasses it (G-01).
    BudgetExhausted,
    /// Cancellation token fired (operator / parent stop).
    Cancelled,
}

/// Result of an agent loop execution.
#[derive(Debug)]
pub struct AgentResult {
    /// Full conversation history including tool calls.
    pub messages: Vec<ChatMessage>,
    /// Phase 25.1 GAP-7 follow-up: messages appended by THIS run only — the
    /// assistant turns + matching tool results, in insertion order. Excludes
    /// the input slice and the loop's own transient pressure-tier system
    /// advisories (which are one-shot signals, not durable history).
    ///
    /// Persistence callers (gateway handler, REPL) MUST use this field instead
    /// of filtering `messages` by role: a role-based filter drops `Role::Tool`
    /// messages, breaking OpenAI's strict assistant↔tool pairing on the next
    /// turn. Compression-aware too — if `pre_chat_compress` shrinks the input
    /// vec mid-run, `appended` still contains exactly the round-trip output.
    pub appended: Vec<ChatMessage>,
    /// Number of LLM turns used.
    pub turns_used: usize,
    /// Whether the agent finished naturally (not by hitting max iterations).
    pub finished_naturally: bool,
    /// Final text response from the agent.
    pub final_response: Option<String>,
    /// Aggregated token usage.
    pub total_usage: AggregatedUsage,
    /// Phase 18 Plan 14: compression_count at the end of this run.
    /// The CLI REPL persists this back into its shared AtomicUsize so that the
    /// summarizing engine's prior-summary chain is continuous across turns.
    pub compression_count_after: usize,
    /// Why the loop stopped (Plan 21.7-05 / D-15 / G-01).
    /// Legacy callers that don't inspect this field continue to work — the
    /// default for pre-21.7 paths is `Natural` or `MaxIterations` as before.
    pub stop_reason: StopReason,
}

impl AgentResult {
    /// Construct the canonical "budget exhausted" result (Plan 21.7-05 / G-01).
    ///
    /// Used when `BudgetHandle::consume()` returns `None` at the top of a
    /// turn. The loop returns this instead of panicking or calling
    /// `process::exit` — the 100% hard-stop is a clean Ok(...) return so
    /// callers can log + continue rather than dying.
    pub fn budget_exhausted(messages: Vec<ChatMessage>, turns_used: usize) -> Self {
        Self {
            messages,
            appended: Vec::new(),
            turns_used,
            finished_naturally: false,
            final_response: None,
            total_usage: AggregatedUsage::default(),
            compression_count_after: 0,
            stop_reason: StopReason::BudgetExhausted,
        }
    }
}

#[derive(Debug, Default)]
pub struct AggregatedUsage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
}

impl AggregatedUsage {
    fn add(&mut self, usage: &Usage) {
        self.prompt_tokens += usage.prompt_tokens;
        self.completion_tokens += usage.completion_tokens;
        self.total_tokens += usage.total_tokens;
    }
}

/// Callback for streaming content to the user.
pub type StreamCallback = Box<dyn Fn(&str) + Send + Sync>;

/// Callback for tool execution progress.
pub type ToolProgressCallback = Box<dyn Fn(&str, &str) + Send + Sync>;

/// Phase 22.4 D-17 / CR-02 gap closure. Fired on every tool completion
/// (success OR failure), matching the 6 fire_hook(HookEventKind::ToolCompleted)
/// sites in execute_tool_call. Consumed by the tui_rata REPL to emit
/// StreamEvent::ToolResult { name, ok } to the UI event loop. `bool` = success.
pub type ToolResultCallback = Box<dyn Fn(&str, bool) + Send + Sync>;

/// The main agent loop that orchestrates LLM calls and tool execution.
pub struct AgentLoop {
    client: AnyClient,
    registry: Arc<RwLock<ToolRegistry>>,
    max_iterations: usize,
    compressor: Option<Mutex<ContextCompressor>>,
    stream_callback: Option<StreamCallback>,
    tool_progress_callback: Option<ToolProgressCallback>,
    /// Phase 22.4 D-17 / CR-02 gap closure: per-tool completion callback for
    /// the tui_rata REPL's StreamEvent::ToolResult wiring. Parallel to
    /// tool_progress_callback (which fires pre-execution with args preview).
    tool_result_callback: Option<ToolResultCallback>,
    streaming: bool,
    hook_registry: Option<Arc<HookRegistry>>,
    request_id: String,
    active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>>,
    /// Optional cancellation token for cooperative shutdown (D-21).
    /// When cancelled, the loop returns early with "Cancelled by parent".
    cancel_token: Option<CancellationToken>,
    // Phase 32.1 Plan 02: activity tracker fields.
    // Wrapped in Arc<std::sync::Mutex> so `activity_summary()` can be called from
    // a separate tokio task while `run()` is in-flight (cron runner polling pattern).
    // Pattern mirrors `active_skills: Arc<std::sync::Mutex<...>>` (with_active_skills).
    activity_last: Arc<std::sync::Mutex<std::time::Instant>>,
    activity_kind: Arc<std::sync::Mutex<ActivityKind>>,
    current_tool: Arc<std::sync::Mutex<Option<String>>>,
    /// This agent's OWN iteration budget handle (PROV-09, D-15).
    /// Tracks turns consumed by THIS loop and exposes the pressure-tier ladder
    /// (None / Caution70 / Warning90 / Stop100) via `BudgetHandle::pressure()`.
    /// Plan 21.7-05 replaced the bare `Arc<AtomicUsize>` with the handle.
    /// Plan 35-02 (D-01/D-04): each agent loop owns its own counter; the
    /// `budget()` getter is no longer used to hand a shared handle to children.
    /// None = use local `max_iterations` only (backward compat).
    budget: Option<BudgetHandle>,
    /// Plan 21.7-05: last pressure tier injected as an advisory system
    /// message. Used to fire `CAUTION_ADVISORY` / `WARNING_ADVISORY` EXACTLY
    /// ONCE per tier crossing (T-21.7-05-02 no-spam mitigation) — never on
    /// steady-state turns.
    last_pressure_tier_seen: PressureTier,
    /// Fallback client to swap to on qualifying errors (PROV-07).
    /// Set via with_fallback(). None = no fallback available.
    fallback_client: Option<AnyClient>,
    /// Whether fallback has already been activated (one-shot per D-11).
    fallback_activated: bool,
    /// Progressive subdirectory context discovery (CTX-03/CTX-04).
    /// When set, file-access tools trigger context file discovery.
    subdir_discovery: Option<Arc<std::sync::Mutex<SubdirDiscovery>>>,
    /// Optional StateStore for session_search tool interception (D-07).
    /// When set, session_search calls are handled directly without registry dispatch.
    state_store: Option<Arc<std::sync::Mutex<StateStore>>>,
    /// Phase 18 Plan 06: pre-chat context engine. When set, replaces the legacy
    /// `compressor` path and runs at the engine's own threshold (default 0.5).
    context_engine: Option<Arc<dyn ContextEngine>>,
    /// Phase 18 Plan 06: pressure tracker used to drain transient warnings
    /// (D-24 channel 3) and inject them as system messages before the next LLM call.
    pressure_tracker: Option<Arc<PressureTracker>>,
    /// Session id for routing pressure + pre_compress events.
    session_id: Option<String>,
    /// Total context window size (used for ratio = estimated / context_length).
    context_length: usize,
    /// Runtime compression count passed into ContextStats (D-24 runaway guard).
    compression_count: usize,
    /// Plan 20-02: optional MemoryManager handle for post-turn prefetch warming.
    /// When set, the loop spawns `queue_prefetch` after the natural-end break
    /// using the last user message as the query hint.
    memory_manager: Option<Arc<Mutex<MemoryManager>>>,
    /// Phase 21.5: names of tools provided by the memory provider (e.g., "memory_recall").
    /// Populated in `run()` from `memory_manager.get_tool_schemas()`.
    /// Used in `execute_tool_call()` to intercept and route to MemoryManager.
    memory_provider_tool_names: std::collections::HashSet<String>,
    /// Phase 25.1 D-03: shared browser session for all 11 browser_* tools.
    /// None until BrowserSession::spawn() is called from first browser_* tool use.
    /// Arc is cloned into each browser_* tool constructor at registry build time
    /// so AgentLoop and tools share the same instance.
    /// On AgentLoop drop, the Arc reference count decrements; when the last tool
    /// clone also drops (all tools dropped with the registry), the Mutex drops,
    /// and BrowserSession::drop kills the handler task (T-25.1-04 drop semantics).
    browser_session: Option<
        std::sync::Arc<
            tokio::sync::Mutex<Option<ironhermes_tools::browser_session::BrowserSession>>,
        >,
    >,
    /// Phase 25.3 D-T-3: per-session trajectory writer handle (trait-object form
    /// from Plan 6 cycle-break — `TrajectoryWriterHandle` lives in `ironhermes-core`,
    /// `TrajectoryWriterHandleImpl` lives in `ironhermes-trajectory`). None means
    /// trajectory logging is disabled for this session. Plan 8 wires the Some-case
    /// from each `run_*` function (CLI/REPL/gateway). Append site is in execute_tool_call.
    pub trajectory_writer:
        Option<std::sync::Arc<dyn ironhermes_core::commands::context::TrajectoryWriterHandle>>,
    /// Phase 25.3 D-T-1: 0-indexed turn counter — incremented per agent turn (one
    /// complete user-assistant exchange). Recorded in TrajectoryEntry.turn_index
    /// so Phase 25.4 Curator can correlate tool calls within a turn.
    turn_index: std::sync::atomic::AtomicUsize,
}

impl AgentLoop {
    pub fn new(
        client: AnyClient,
        registry: Arc<RwLock<ToolRegistry>>,
        max_iterations: usize,
    ) -> Self {
        Self {
            client,
            registry,
            max_iterations,
            compressor: None,
            stream_callback: None,
            tool_progress_callback: None,
            tool_result_callback: None,
            streaming: false,
            hook_registry: None,
            request_id: uuid::Uuid::new_v4().to_string(),
            active_skills: Arc::new(std::sync::Mutex::new(Vec::new())),
            cancel_token: None,
            // Phase 32.1 Plan 02: activity tracker — initialise to ApiCall sentinel
            // because the first observable event in any run() is the LLM API call.
            activity_last: Arc::new(std::sync::Mutex::new(std::time::Instant::now())),
            activity_kind: Arc::new(std::sync::Mutex::new(ActivityKind::ApiCall)),
            current_tool: Arc::new(std::sync::Mutex::new(None)),
            budget: None,
            last_pressure_tier_seen: PressureTier::None,
            fallback_client: None,
            fallback_activated: false,
            subdir_discovery: None,
            state_store: None,
            context_engine: None,
            pressure_tracker: None,
            session_id: None,
            context_length: 128_000,
            compression_count: 0,
            memory_manager: None,
            memory_provider_tool_names: std::collections::HashSet::new(),
            browser_session: None,
            // Phase 25.3 D-T-3 / D-T-1: trajectory ledger fields default to disabled / 0.
            trajectory_writer: None,
            turn_index: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Plan 20-02: attach a `MemoryManager` handle so the loop can fire
    /// `queue_prefetch` after the natural-end break (post-turn warming).
    pub fn with_memory_manager(mut self, manager: Arc<Mutex<MemoryManager>>) -> Self {
        self.memory_manager = Some(manager);
        self
    }

    /// Plan 20-02: introspection accessor for tests — confirms the manager
    /// handle has been wired before `agent.run(...)`.
    pub fn has_memory_manager(&self) -> bool {
        self.memory_manager.is_some()
    }

    /// Phase 25.1 D-17: wire the shared browser session Arc so it can be passed to
    /// browser_* tools during registry build. Mirrors with_memory_manager() shape.
    ///
    /// The Arc is cloned into each browser_* tool constructor at registry build time
    /// (via register_browser_tools) so AgentLoop and tools share the same instance.
    /// AgentLoop drop drops this Arc; when the last tool clone also drops, the
    /// BrowserSession::drop kills the handler task (T-25.1-04 resource cleanup).
    pub fn with_browser_session(
        mut self,
        session: std::sync::Arc<
            tokio::sync::Mutex<Option<ironhermes_tools::browser_session::BrowserSession>>,
        >,
    ) -> Self {
        self.browser_session = Some(session);
        self
    }

    /// Phase 25.3 D-T-3: attach a `TrajectoryWriterHandle` (trait-object from Plan 6)
    /// so per-tool-call entries are recorded to
    /// `<workspace-or-home>/.ironhermes/sessions/<id>/trajectories.jsonl`.
    /// Append failures are logged via `tracing::warn!` but do NOT abort the agent
    /// turn (best-effort ledger).
    pub fn with_trajectory_writer(
        mut self,
        handle: std::sync::Arc<dyn ironhermes_core::commands::context::TrajectoryWriterHandle>,
    ) -> Self {
        self.trajectory_writer = Some(handle);
        self
    }

    /// Phase 18 Plan 06: inject a pre-chat context engine.
    /// Also populates context_length so ratio checks work correctly.
    pub fn with_context_engine(
        mut self,
        engine: Arc<dyn ContextEngine>,
        context_length: usize,
    ) -> Self {
        self.context_engine = Some(engine);
        self.context_length = context_length;
        self
    }

    /// Phase 18 Plan 06: attach a pressure tracker for transient message drain.
    pub fn with_pressure_tracker(mut self, tracker: Arc<PressureTracker>) -> Self {
        self.pressure_tracker = Some(tracker);
        self
    }

    /// Phase 18 Plan 06: session id for transient drain + pre_compress routing.
    pub fn with_session_id(mut self, sid: impl Into<String>) -> Self {
        self.session_id = Some(sid.into());
        self
    }

    /// Phase 18 Plan 14: seed the starting compression_count so the summarizing
    /// engine's prior-summary chain is continuous across REPL turns.
    pub fn with_compression_count(mut self, count: usize) -> Self {
        self.compression_count = count;
        self
    }

    // ── Phase 18 Plan 09: introspection accessors for tests ────────────────
    // These are harmless `is_some` / clone accessors used by unit tests in
    // this crate and in `ironhermes-gateway` to verify that the Phase 18
    // wiring helper (`attach_context_engine`) has been called before
    // `agent.run(...)`. They are `pub` so cross-crate tests can call them.
    pub fn has_context_engine(&self) -> bool {
        self.context_engine.is_some()
    }
    pub fn has_pressure_tracker(&self) -> bool {
        self.pressure_tracker.is_some()
    }
    pub fn session_id(&self) -> Option<String> {
        self.session_id.clone()
    }
    pub fn context_engine_threshold(&self) -> Option<f32> {
        self.context_engine.as_ref().map(|e| e.threshold())
    }

    /// Set a cancellation token for cooperative shutdown (D-21).
    /// When the token is cancelled, the agent loop returns early.
    pub fn with_cancellation_token(mut self, token: CancellationToken) -> Self {
        self.cancel_token = Some(token);
        self
    }

    pub fn with_active_skills(
        mut self,
        active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>>,
    ) -> Self {
        self.active_skills = active_skills;
        self
    }

    pub fn active_skills(&self) -> Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>> {
        self.active_skills.clone()
    }

    /// Set subdirectory discovery for progressive context injection (CTX-03/CTX-04).
    pub fn with_subdir_discovery(
        mut self,
        discovery: Arc<std::sync::Mutex<SubdirDiscovery>>,
    ) -> Self {
        self.subdir_discovery = Some(discovery);
        self
    }

    /// Set the StateStore for session_search tool interception (D-07).
    /// When set, session_search calls are intercepted before registry dispatch.
    pub fn with_state_store(mut self, store: Arc<std::sync::Mutex<StateStore>>) -> Self {
        self.state_store = Some(store);
        self
    }

    /// Phase 25 D-16: register intercepted tools via known-safe workspace handles.
    ///
    /// Wires session_search, memory, delegate_task, todo_write, todo_read, and cronjob
    /// (when handles are provided) through `register_intercepted()` in the registry.
    /// Each parameter is typed to a workspace-internal handle — no string-keyed or
    /// config-deserialized handler construction is exposed here (T-25-04 mitigation).
    ///
    /// Call this AFTER `with_state_store()` and `with_memory_manager()` in the builder chain.
    /// Default `AgentLoop::new()` registers no intercepts (backward compat — D-16).
    pub fn with_intercepts(
        self,
        memory_manager: Option<Arc<Mutex<MemoryManager>>>,
        state_store: Option<Arc<std::sync::Mutex<StateStore>>>,
        subagent_runner: Option<Arc<dyn ironhermes_tools::delegate_task::SubagentRunner>>,
        todo_state: Option<Arc<Mutex<Vec<String>>>>,
        _cron_router: Option<()>, // reserved for future cron router handle
    ) -> Self {
        {
            // Use try_write() which is available in both sync and async context.
            // During with_intercepts() (builder phase, before run()), no other writer holds
            // the lock — try_write() always succeeds. unwrap() is safe here.
            let mut reg = self.registry.try_write().expect("with_intercepts: failed to acquire registry write lock (concurrent modification during construction)");
            #[allow(unused_mut)]
            if let Some(state) = state_store {
                let s = state.clone();
                reg.register_intercepted(
                    "session_search",
                    crate::session_search::session_search_schema(),
                    std::sync::Arc::new(move |args| {
                        let s = s.clone();
                        Box::pin(async move {
                            let res = tokio::task::spawn_blocking(move || {
                                let store = s.lock().unwrap();
                                crate::session_search::handle_session_search(&args, &store)
                            })
                            .await;
                            match res {
                                Ok(s) => Ok(s),
                                Err(e) => Err(anyhow::anyhow!("session_search join failed: {}", e)),
                            }
                        })
                    }),
                );
            }

            if let Some(mm) = memory_manager {
                // Get memory schema from a temporary MemoryTool instance (canonical schema source per D-14).
                use ironhermes_tools::Tool as _;
                let memory_schema =
                    ironhermes_tools::memory_tool::MemoryTool::new(mm.clone()).schema();
                let m = mm.clone();
                reg.register_intercepted(
                    "memory",
                    memory_schema,
                    std::sync::Arc::new(move |args| {
                        let m = m.clone();
                        Box::pin(async move {
                            // Dispatch to the memory manager handle's tool call router.
                            use ironhermes_tools::memory_manager_handle::MemoryManagerHandle;
                            let g = m.lock().await;
                            g.handle_tool_call("memory", args)
                                .await
                                .map_err(|e| anyhow::anyhow!("{}", e))
                        })
                    }),
                );
            }

            if let Some(_sr) = subagent_runner {
                // delegate_task intercept: schema from DelegateTaskTool, dispatch stub for Plan 4.
                // Full wiring (semaphore, config, progress callback) is in Plan 4 (operator surface).
                // Use a placeholder schema since DelegateTaskTool requires a runner in constructor.
                // Plan 4 will replace this with the live DelegateTaskTool schema.
                let dt_schema = ironhermes_core::ToolSchema::new(
                    "delegate_task",
                    "Delegate a focused task to a child agent with restricted tools.",
                    serde_json::json!({
                        "type": "object",
                        "properties": {
                            "task": { "type": "string", "description": "Task description for the child agent." },
                            "allowed_tools": { "type": "array", "items": { "type": "string" }, "description": "Tools available to the child agent." }
                        },
                        "required": []
                    }),
                );
                reg.register_intercepted(
                    "delegate_task",
                    dt_schema,
                    std::sync::Arc::new(move |_args| {
                        Box::pin(async move {
                            // Stub: full delegation wiring via DelegateTaskTool is in Plan 4.
                            Ok(r#"{"error":"not_wired","reason":"delegate_task intercept stub — full wiring in Plan 4"}"#.to_string())
                        })
                    }),
                );
            }

            if let Some(ts) = todo_state {
                let read_state = ts.clone();
                reg.register_intercepted(
                    "todo_read",
                    ironhermes_tools::todo_read_schema(),
                    std::sync::Arc::new(move |_args| {
                        let st = read_state.clone();
                        Box::pin(async move {
                            let g = st.lock().await;
                            Ok(serde_json::json!({"items": g.clone()}).to_string())
                        })
                    }),
                );
                let write_state = ts.clone();
                reg.register_intercepted(
                    "todo_write",
                    ironhermes_tools::todo_write_schema(),
                    std::sync::Arc::new(move |args| {
                        let st = write_state.clone();
                        Box::pin(async move {
                            let items: Vec<String> = serde_json::from_value(
                                args.get("items").cloned().unwrap_or(serde_json::json!([])),
                            )
                            .unwrap_or_default();
                            let mut g = st.lock().await;
                            *g = items.clone();
                            Ok(serde_json::json!({"replaced_with": items}).to_string())
                        })
                    }),
                );
            }
        } // drop write guard
        self
    }

    pub fn with_compression(mut self, context_length: usize, threshold: f64) -> Self {
        self.compressor = Some(Mutex::new(ContextCompressor::new(
            context_length,
            threshold,
        )));
        self
    }

    pub fn with_streaming(mut self, callback: StreamCallback) -> Self {
        self.streaming = true;
        self.stream_callback = Some(callback);
        self
    }

    pub fn with_tool_progress(mut self, callback: ToolProgressCallback) -> Self {
        self.tool_progress_callback = Some(callback);
        self
    }

    /// Phase 22.4 D-17 / CR-02 gap closure. Callback fires after every tool
    /// completion with `(tool_name, success)`. Use to drive UI elements that
    /// surface per-call success/failure (e.g. tui_rata REPL's ToolResult
    /// StreamEvent). Parallel to `with_tool_progress` (which fires pre-execution).
    pub fn with_tool_result(mut self, callback: ToolResultCallback) -> Self {
        self.tool_result_callback = Some(callback);
        self
    }

    pub fn with_hook_registry(mut self, registry: Arc<HookRegistry>) -> Self {
        self.hook_registry = Some(registry);
        self
    }

    /// Set this agent loop's own iteration budget handle (PROV-09, D-15).
    ///
    /// Plan 21.7-05: accepts [`BudgetHandle`] rather than a bare
    /// `Arc<AtomicUsize>`. The handle's `consume()` is called at the top of
    /// every turn (Stop100 → clean-stop via `AgentResult::budget_exhausted`);
    /// `pressure()` drives the advisory-injection ladder (Caution70/Warning90).
    ///
    /// Plan 35-02 (D-01/D-04): each call to `run_child` now passes a FRESH
    /// `BudgetHandle::new(max_iterations)` — not a clone of the parent handle.
    pub fn with_budget(mut self, budget: BudgetHandle) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Get this agent's own budget handle (PROV-09, D-15).
    ///
    /// Note: since Plan 35-02 (D-01/D-04), this getter is no longer used to
    /// hand a shared handle to child agents — each child receives a fresh
    /// `BudgetHandle::new(max_iterations)` from `run_child` directly.
    pub fn budget(&self) -> Option<BudgetHandle> {
        self.budget.clone()
    }

    /// Set a fallback client for one-shot provider switching (PROV-07).
    /// When the primary client fails with 429/5xx/401, the fallback client
    /// is swapped in and retries reset. Only fires once per agent run.
    pub fn with_fallback(mut self, client: AnyClient) -> Self {
        self.fallback_client = Some(client);
        self
    }

    /// Phase 22.4 — minimal test-only constructor for tui_rata snapshot tests.
    /// The returned value MUST NOT drive a real LLM turn; snapshot tests render
    /// state only and never call run_agent_turn.
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_tests() -> Self {
        use ironhermes_tools::ToolRegistry;
        let client = AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "http://localhost:11434",
            "test-key",
            "test-model",
        ));
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        Self::new(client, registry, 1)
    }

    // ── Phase 32.1 Plan 02: activity tracker public API ────────────────────

    /// Return a live snapshot of the most-recent observable agent activity.
    ///
    /// Non-async — uses `std::sync::Mutex` so it can be called from any
    /// tokio task (including the cron runner's polling loop) without holding
    /// an async lock that could interact with the agent's own awaits.
    pub fn activity_summary(&self) -> ActivitySummary {
        let last = *self.activity_last.lock().expect("activity_last poisoned");
        let kind = *self.activity_kind.lock().expect("activity_kind poisoned");
        let tool = self.current_tool.lock().expect("current_tool poisoned").clone();
        ActivitySummary {
            seconds_since: last.elapsed().as_secs_f64(),
            last_kind: kind,
            current_tool: tool,
        }
    }

    /// Phase 32.3 Plan 02 (D-04 / RESEARCH A1): return a clonable shared handle
    /// to the live `activity_last` clock. The returned `Arc<Mutex<Instant>>` is
    /// the SAME instance bumped by the four `mark_activity` sites (ApiCall,
    /// StreamToken, ToolCall before, ToolCall after) — no new bump points are
    /// added. Consumers (`SubagentInfo.activity_last`) clone this handle and
    /// call `.lock().elapsed()` at registry-read time for live staleness.
    ///
    /// Zero-copy live clock — no push updates, no periodic polling.
    pub fn activity_last_arc(&self) -> Arc<std::sync::Mutex<std::time::Instant>> {
        self.activity_last.clone()
    }

    /// Interrupt the agent by cancelling its `CancellationToken`.
    ///
    /// Non-async, safe to call from any context. When the token fires, the
    /// next `token.cancelled()` await in `run()` resolves and the loop returns
    /// early with `StopReason::Cancelled`. If no token is configured (e.g. in
    /// unit tests), this is a no-op with a warning log.
    pub fn interrupt(&self, reason: &str) {
        tracing::warn!(reason = %reason, "AgentLoop interrupted");
        if let Some(ref token) = self.cancel_token {
            token.cancel();
        } else {
            tracing::warn!("AgentLoop::interrupt called but no CancellationToken is set — no-op");
        }
    }

    /// Return `true` when the underlying `CancellationToken` has been cancelled.
    /// Returns `false` when no token is configured.
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token
            .as_ref()
            .map(|t| t.is_cancelled())
            .unwrap_or(false)
    }

    /// Bump the activity tracker to `kind` with an optional in-flight tool name.
    ///
    /// Called from three sites inside `run()` and `call_llm_streaming()`:
    ///   1. Before each LLM API call — `(ApiCall, None)`
    ///   2. Before each tool dispatch — `(ToolCall, Some(name))`
    ///   3. After each tool dispatch — `(ToolCall, None)` (clears in-flight name)
    ///   4. On each streamed content delta — `(StreamToken, None)`
    fn mark_activity(&self, kind: ActivityKind, current_tool: Option<String>) {
        *self.activity_last.lock().expect("activity_last poisoned") = std::time::Instant::now();
        *self.activity_kind.lock().expect("activity_kind poisoned") = kind;
        *self.current_tool.lock().expect("current_tool poisoned") = current_tool;
    }

    /// Test-only shim for `mark_activity`. Exposes the internal helper so
    /// tests can verify the bump → `activity_summary()` round-trip without
    /// exercising a full `run()`.
    #[cfg(test)]
    pub fn mark_activity_for_test(&self, kind: ActivityKind, current_tool: Option<String>) {
        self.mark_activity(kind, current_tool);
    }

    /// Check the current pressure tier and return advisory text if this
    /// turn crosses into a new tier (Plan 21.7-05 / D-15 / T-21.7-05-02).
    ///
    /// Returns `None` when:
    /// - No budget is wired.
    /// - Current tier equals the last tier seen (steady-state, no spam).
    /// - Tier is `None` or `Stop100` (no advisory text — Stop100 ends the loop
    ///   before the next provider call, so there is nothing to inject).
    ///
    /// Side effect: advances `self.last_pressure_tier_seen` when a new tier is
    /// observed. This is the ONLY place the tier-seen cell is mutated to
    /// guarantee exactly-once injection per tier crossing.
    fn check_budget_threshold(&mut self) -> Option<&'static str> {
        let budget = self.budget.as_ref()?;
        let tier = budget.pressure();
        if tier == self.last_pressure_tier_seen {
            return None;
        }
        self.last_pressure_tier_seen = tier;
        advisory_text(tier)
    }

    /// Extract an HTTP status code from an error string.
    ///
    /// Recognises both production formats:
    ///   - "(400 Bad Request)"  — `bail!("… ({status}): …")` in client.rs / anthropic_client.rs
    ///   - "status: 429 …"      — synthetic test format kept for regression coverage
    fn extract_http_status(err_str: &str) -> Option<u16> {
        if let Some(open) = err_str.find('(') {
            let rest = &err_str[open + 1..];
            if rest.len() >= 4 {
                let (digits, tail) = rest.split_at(3);
                if tail.starts_with(' ') {
                    if let Ok(code) = digits.parse::<u16>() {
                        return Some(code);
                    }
                }
            }
        }
        if let Some(idx) = err_str.find("status: ") {
            let rest = &err_str[idx + "status: ".len()..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() {
                return digits.parse().ok();
            }
        }
        None
    }

    /// Detect transport-level failures in a formatted error chain.
    ///
    /// Called only when `extract_http_status` returns `None` (i.e. no HTTP status
    /// code was found in the error string). Matches a conservative case-insensitive
    /// allowlist of reqwest / hyper-util / OS transport-error substrings. Returns
    /// `true` when the error chain indicates the provider was unreachable or the
    /// request was never delivered — the caller should then return `(true, true)` to
    /// activate the fallback chain. Unrecognised errors continue to return `(true, false)`.
    ///
    /// Needles are verified against pinned source (reqwest 0.12.28, hyper-util 0.1.20,
    /// anyhow 1.0.100). See phase 27.1.4.1.1 RESEARCH.md — Validated Allowlist (D-01 / D-01a).
    fn is_transport_failure(err_str: &str) -> bool {
        const TRANSPORT_MARKERS: &[&str] = &[
            "error sending request for url",
            "connection refused",
            "dns error",
            "failed to lookup address",
            "connection reset",
            "operation timed out",
            "tcp connect error",
        ];
        let lower = err_str.to_lowercase();
        TRANSPORT_MARKERS.iter().any(|m| lower.contains(m))
    }

    /// Classify an error for fallback decision-making.
    /// Returns (should_retry, should_fallback).
    fn classify_llm_error(err: &anyhow::Error) -> (bool, bool) {
        // Alternate Display walks the full anyhow context chain joined with ": ".
        // Plain Display only shows the outermost context — production errors are
        // wrapped at agent_loop.rs:1030 with `.context("Streaming LLM call failed")`,
        // hiding the underlying `(400 Bad Request)` from the substring scan.
        let err_str = format!("{err:#}");
        let Some(code) = Self::extract_http_status(&err_str) else {
            return (true, Self::is_transport_failure(&err_str));
        };
        match code {
            // Transient — retry first, then fall back if all retries fail.
            429 | 500 | 502 | 503 | 504 => (true, true),
            // Permanent client errors — retrying the same call against the
            // same provider won't help. Skip retry; fall back. 400 is
            // included because OpenRouter returns 400 for invalid model IDs,
            // which a sibling provider (e.g. local ollama) may accept.
            400 | 401 | 403 | 404 => (false, true),
            _ => (true, false),
        }
    }

    /// Phase 18 Plan 06: pre-chat compression + transient-drain block.
    /// Extracted for direct unit testing without spinning the full `run()` loop.
    pub(crate) async fn pre_chat_compress(&mut self, messages: &mut Vec<ChatMessage>) {
        let engine = match &self.context_engine {
            Some(e) => e.clone(),
            None => return,
        };

        // Drain transient pressure message (D-24 channel 3) BEFORE compression so it
        // sits on the same pre-chat message vector the LLM is about to see.
        if let (Some(tracker), Some(sid)) = (&self.pressure_tracker, &self.session_id) {
            if let Some(transient) = tracker.take_transient(sid) {
                messages.push(ChatMessage::system(transient));
            }
        }

        // Belt-and-suspenders: snapshot caller's vec before invoking the engine
        // so that any future engine implementation that mutates `messages` and
        // then returns Err cannot leak a corrupted vec to the LLM.
        let pre_compress_snapshot = messages.clone();

        let estimated = estimate_messages_tokens(messages);
        let ratio = estimated as f32 / self.context_length.max(1) as f32;
        let threshold = engine.threshold();
        let stats = ContextStats {
            context_length: self.context_length,
            estimated_tokens: estimated,
            protect_first_n: 3,
            protect_last_tokens: 20_000.min(self.context_length / 4),
            compression_count: self.compression_count,
            prior_summary: None,
        };
        if ratio >= threshold {
            info!(ratio, threshold, "context compression check");
            match engine.compress(messages, stats).await {
                Ok(outcome) if outcome.compressed => {
                    self.compression_count += 1;
                    info!(
                        compression_count = self.compression_count,
                        "pre_chat_compress: compression fired"
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    *messages = pre_compress_snapshot;
                    tracing::error!(
                        error = ?e,
                        "pre_chat_compress: compression failed, rolled back to pre-compress state"
                    );
                }
            }
        } else {
            debug!(ratio, threshold, "context compression check");
            engine.check_pressure(&stats).await;
        }
    }

    /// Fire a hook event fire-and-forget. No-op if no hook registry is configured.
    fn fire_hook(&self, kind: HookEventKind) {
        if let Some(ref registry) = self.hook_registry {
            let event = HookEvent::new(&self.request_id, kind);
            registry.fire(event);
        }
    }

    /// Run the agent loop with the given messages.
    ///
    /// The loop continues until:
    /// - The LLM produces a response with no tool calls (natural completion)
    /// - Max iterations are reached
    /// - An unrecoverable error occurs
    pub async fn run(&mut self, mut messages: Vec<ChatMessage>) -> Result<AgentResult> {
        let mut tool_schemas = self.registry.read().await.get_definitions(None);
        // Phase 25 D-14: session_search schema flows through registry.get_definitions()
        // because register_intercepted("session_search", ...) was called in with_intercepts().
        // Phase 21.5: Add memory provider tool schemas (e.g. memory_recall).
        // These are tools declared by the provider via get_tool_schemas() —
        // distinct from the built-in "memory" tool which handles add/replace/remove.
        if let Some(ref mgr) = self.memory_manager {
            let guard = mgr.lock().await;
            let schemas = guard.get_tool_schemas().await;
            for s in &schemas {
                self.memory_provider_tool_names
                    .insert(s.function.name.clone());
            }
            tool_schemas.extend(schemas);
        }
        let tools_option = if tool_schemas.is_empty() {
            None
        } else {
            Some(tool_schemas)
        };

        let mut turns_used = 0;
        let mut total_usage = AggregatedUsage::default();
        let mut final_response = None;
        // Phase 25.1 GAP-7 follow-up: track messages appended by THIS run so the
        // gateway/REPL persistence path can include matching tool results without
        // a Role-based filter (which would drop Role::Tool and re-persist prior
        // assistants on subsequent turns). Excludes transient pressure-tier
        // system advisories (lines 738/576) — those are one-shot signals, not
        // durable history.
        let mut appended: Vec<ChatMessage> = Vec::new();

        info!(max_iterations = self.max_iterations, "Starting agent loop");

        // Note: MessageReceived is NOT fired here. It is fired by the platform layer
        // (handler.rs for Telegram, runner.rs for cron) which knows the real platform
        // and chat_id. Firing it here would produce duplicate events (Issue #4 fix).

        loop {
            // D-21: Check cancellation token before each iteration
            if let Some(ref token) = self.cancel_token {
                if token.is_cancelled() {
                    info!(turns = turns_used, "Agent loop cancelled by parent");
                    return Ok(AgentResult {
                        messages,
                        appended,
                        turns_used,
                        finished_naturally: false,
                        final_response: Some("Cancelled by parent".to_string()),
                        total_usage,
                        compression_count_after: self.compression_count,
                        stop_reason: StopReason::Cancelled,
                    });
                }
            }

            if turns_used >= self.max_iterations {
                warn!(turns = turns_used, "Max iterations reached");
                break;
            }

            // Plan 21.7-05 / D-15 / G-01: top-of-turn shared BudgetHandle check.
            // consume() is SeqCst decrement; None means Stop100 → clean terminate
            // (NEVER panic, NEVER process::exit — this is the unskippable yolo
            // guardrail). Parent + child subagents share the same counter.
            if let Some(ref handle) = self.budget {
                if handle.consume().is_none() {
                    info!(
                        target: "ironhermes_agent::budget",
                        used = handle.used(), max = handle.max(),
                        "iteration budget exhausted; stopping cleanly (Stop100)"
                    );
                    return Ok(AgentResult {
                        messages,
                        appended,
                        turns_used,
                        finished_naturally: false,
                        final_response: None,
                        total_usage,
                        compression_count_after: self.compression_count,
                        stop_reason: StopReason::BudgetExhausted,
                    });
                }
            }

            // Phase 34a D-02/D-03/D-08: evict any prior-turn recall injection
            // BEFORE compression. Recall is ephemeral and re-fetched fresh each
            // turn; running this after compression lets a stale <memory-context>
            // block be folded into the persisted [CONTEXT HISTORY] summary on the
            // summarizing-engine path (D-03 violation). Evicting first also keeps
            // the insert-index scan below correct (D-02 / Pitfall 3). The legacy
            // ContextCompressor still has its own step-0 eviction; this guards the
            // active context-engine path which has none.
            messages.retain(|m| !m.is_recall_context);

            // Phase 18 Plan 06: pre-chat context engine path (replaces legacy compressor
            // when wired). Drain transient pressure message, then compress at >= threshold
            // or run pressure check only when below.
            if self.context_engine.is_some() {
                self.pre_chat_compress(&mut messages).await;
            } else if let Some(ref compressor) = self.compressor {
                let mut comp = compressor.lock().await;
                comp.compress(&mut messages);
            }

            // Phase 34a D-08: pre-turn recall injection.
            // Fetch query-scoped recall and inject BEFORE the last user
            // message (only when memory_manager is wired).
            if let Some(ref mgr) = self.memory_manager {
                let session_id = self.session_id.as_deref().unwrap_or("");
                let user_msg_text = messages
                    .iter()
                    .rev()
                    .find(|m| m.role == ironhermes_core::Role::User)
                    .and_then(|m| m.content_text().map(|s| s.to_string()))
                    .unwrap_or_default();

                if !user_msg_text.is_empty() {
                    // Scoped block drops the MutexGuard before messages.insert() (Pitfall 1).
                    let raw = {
                        let guard = mgr.lock().await;
                        guard.prefetch_with_query(&user_msg_text, session_id).await
                    };
                    if let Ok(raw) = raw {
                        if let Some(block) =
                            crate::memory_context::build_memory_context_block(&raw)
                        {
                            // D-08: insert only when recall is non-empty.
                            let insert_idx = messages
                                .iter()
                                .rposition(|m| m.role == ironhermes_core::Role::User)
                                .unwrap_or(messages.len());
                            messages.insert(
                                insert_idx,
                                ChatMessage::recall_system(block),
                            );
                        }
                        // D-08: when build returns None (empty recall), do NOT insert.
                        // The retain above already evicted the prior recall message, so
                        // a file-provider-only session's buffer is byte-identical to pre-34a.
                    }
                }
            }

            turns_used += 1;
            debug!(
                turn = turns_used,
                messages = messages.len(),
                "Agent loop turn"
            );

            // Plan 21.7-05 / D-15 / T-21.7-05-02: inject pressure-tier advisory
            // EXACTLY ONCE per tier crossing — never on steady-state turns.
            // check_budget_threshold() mutates last_pressure_tier_seen to guarantee
            // no-spam semantics.
            if let Some(advisory) = self.check_budget_threshold() {
                let prev_tier = match self.budget.as_ref() {
                    Some(h) => h.pressure(),
                    None => PressureTier::None,
                };
                info!(
                    target: "ironhermes_agent::budget",
                    to = ?prev_tier,
                    "pressure tier crossed; injecting advisory"
                );
                messages.push(ChatMessage::system(advisory));
            }

            // Phase 32.1 Plan 02 Task 2: bump activity tracker before every LLM API call.
            // Placed here (outside the retry loop) so the tracker updates on the initial
            // attempt and each retry — any of which may be the last visible activity before
            // the cron runner's inactivity poller fires.
            self.mark_activity(ActivityKind::ApiCall, None);

            // Call LLM with retry and fallback support
            const MAX_RETRIES: usize = 3;
            let mut retry_count = 0;

            let (assistant_message, usage) = loop {
                let llm_result = if let Some(ref token) = self.cancel_token {
                    tokio::select! {
                        result = async {
                            if self.streaming {
                                self.call_llm_streaming(&messages, tools_option.as_deref()).await
                            } else {
                                self.call_llm(&messages, tools_option.as_deref()).await
                            }
                        } => result,
                        _ = token.cancelled() => {
                            info!(turns = turns_used, "Agent loop cancelled during LLM call");
                            return Ok(AgentResult {
                                messages,
                                appended,
                                turns_used,
                                finished_naturally: false,
                                final_response: Some("Cancelled by parent".to_string()),
                                total_usage,
                                compression_count_after: self.compression_count,
                                stop_reason: StopReason::Cancelled,
                            });
                        }
                    }
                } else if self.streaming {
                    self.call_llm_streaming(&messages, tools_option.as_deref())
                        .await
                } else {
                    self.call_llm(&messages, tools_option.as_deref()).await
                };

                match llm_result {
                    Ok(result) => break result,
                    Err(err) => {
                        let (should_retry, should_fallback) = Self::classify_llm_error(&err);

                        // Try fallback if available and not already activated (PROV-07, D-11)
                        if should_fallback && !self.fallback_activated {
                            if let Some(fallback) = self.fallback_client.take() {
                                warn!("Primary LLM failed, activating fallback provider: {err}");
                                self.client = fallback;
                                self.fallback_activated = true;
                                retry_count = 0;
                                continue;
                            }
                        }

                        // Retry transient errors with backoff
                        if should_retry && retry_count < MAX_RETRIES {
                            retry_count += 1;
                            warn!(retry = retry_count, "LLM call failed, retrying: {err:#}");
                            tokio::time::sleep(tokio::time::Duration::from_millis(
                                500 * retry_count as u64,
                            ))
                            .await;
                            continue;
                        }

                        // Exhausted retries and fallback
                        return Err(err);
                    }
                }
            };

            if let Some(ref usage) = usage {
                total_usage.add(usage);
            }

            // Check for tool calls
            let has_tool_calls = assistant_message
                .tool_calls
                .as_ref()
                .is_some_and(|tc| !tc.is_empty());

            // Extract text response
            if let Some(text) = assistant_message.content_text()
                && !text.is_empty()
            {
                final_response = Some(text.to_string());
            }

            messages.push(assistant_message.clone());
            appended.push(assistant_message.clone());

            if !has_tool_calls {
                debug!(
                    turn = turns_used,
                    "Agent completed naturally (no tool calls)"
                );
                // Plan 20-02: fire the provider's `queue_prefetch` hook on the
                // natural-end break so the primary can warm its cache for the
                // next turn. The query is the most recent user message content.
                if let Some(ref mgr) = self.memory_manager {
                    let query = messages
                        .iter()
                        .rev()
                        .find(|m| m.role == ironhermes_core::Role::User)
                        .and_then(|m| m.content_text().map(|s| s.to_string()))
                        .unwrap_or_default();
                    if !query.is_empty() {
                        let mgr = Arc::clone(mgr);
                        tokio::spawn(async move {
                            let guard = mgr.lock().await;
                            if let Err(e) = guard.queue_prefetch(&query).await {
                                warn!(
                                    error = ?e,
                                    "queue_prefetch failed after natural-end break"
                                );
                            }
                        });
                    }
                }
                break;
            }

            // Execute tool calls
            let tool_calls = assistant_message.tool_calls.as_ref().unwrap();
            debug!(count = tool_calls.len(), "Executing tool calls");

            for tool_call in tool_calls {
                let result = self.execute_tool_call(tool_call).await;
                let tool_msg = ChatMessage::tool_result(&tool_call.id, result);
                messages.push(tool_msg.clone());
                appended.push(tool_msg);
            }

            // Phase 25.3 D-T-1: increment per-turn counter AFTER tool calls execute.
            // The trajectory append site reads this with Ordering::Relaxed during
            // execute_tool_call, so each tool call within a turn shares the same
            // turn_index — incrementing here means the NEXT turn's tool calls see
            // the next index. Phase 25.4 Curator can correlate calls within a turn.
            self.turn_index
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }

        // Note: ResponseSent is NOT fired here. It is fired by the platform layer
        // (handler.rs for Telegram, runner.rs for cron) which knows the real platform
        // and chat_id. Firing it here would produce duplicate events (Issue #4 fix).

        let finished_naturally = turns_used < self.max_iterations;
        let stop_reason = if finished_naturally {
            StopReason::Natural
        } else {
            StopReason::MaxIterations
        };
        Ok(AgentResult {
            messages,
            appended,
            turns_used,
            finished_naturally,
            final_response,
            total_usage,
            compression_count_after: self.compression_count,
            stop_reason,
        })
    }

    /// Call LLM without streaming.
    async fn call_llm(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[ToolSchema]>,
    ) -> Result<(ChatMessage, Option<Usage>)> {
        let response: ChatResponse = self
            .client
            .chat_completion(messages, tools, None, None, None, None)
            .await
            .context("LLM call failed")?;

        let mut message = response
            .choices
            .into_iter()
            .next()
            .context("No choices in LLM response")?
            .message;

        // Phase 34a CR-02: scrub any model-echoed <memory-context> / [System note]
        // before the message is returned, persisted, and replayed next turn.
        if let Some(ironhermes_core::MessageContent::Text(text)) = message.content.as_ref() {
            let sanitized = crate::memory_context::sanitize_context(text);
            message.content = if sanitized.is_empty() {
                None
            } else {
                Some(ironhermes_core::MessageContent::Text(sanitized))
            };
        }

        Ok((message, response.usage))
    }

    /// Call LLM with streaming, forwarding content deltas to the callback.
    async fn call_llm_streaming(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[ToolSchema]>,
    ) -> Result<(ChatMessage, Option<Usage>)> {
        let mut rx = self
            .client
            .chat_completion_stream(messages, tools, None, None, None, None)
            .await
            .context("Streaming LLM call failed")?;

        let mut content = String::new();
        let mut tool_call_deltas: Vec<ToolCallDelta> = Vec::new();
        let mut usage = None;

        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::ContentDelta(delta) => {
                    // Phase 32.1 Plan 02 Task 2: bump activity tracker once per delta event,
                    // not per byte — avoids Mutex::lock thrash on the hot streaming path.
                    self.mark_activity(ActivityKind::StreamToken, None);
                    if let Some(ref cb) = self.stream_callback {
                        cb(&delta);
                    }
                    content.push_str(&delta);
                }
                StreamEvent::ToolCallDelta {
                    index,
                    id,
                    name,
                    arguments,
                } => {
                    tool_call_deltas.push((index, id, name, arguments));
                }
                StreamEvent::Usage(u) => {
                    usage = Some(u);
                }
                StreamEvent::Done(_) => break,
            }
        }

        // Assemble the message
        let tool_calls = if tool_call_deltas.is_empty() {
            None
        } else {
            Some(crate::client::assemble_tool_calls_from_stream(
                &tool_call_deltas,
            ))
        };

        // Phase 34a CR-02: the stream callback scrubs only the *displayed* deltas.
        // The accumulated `content` is the RAW model output and is what gets
        // persisted + replayed next turn, so a model-echoed <memory-context> /
        // [System note] block must be stripped here too or it defeats the
        // recall-boundary guarantee. sanitize_context runs over the full assembled
        // text (no chunk-boundary concern at this point).
        let content = crate::memory_context::sanitize_context(&content);

        let message = ChatMessage {
            role: ironhermes_core::Role::Assistant,
            content: if content.is_empty() {
                None
            } else {
                Some(ironhermes_core::MessageContent::Text(content))
            },
            tool_calls,
            tool_call_id: None,
            name: None,
            is_recall_context: false,
        };

        Ok((message, usage))
    }

    /// Execute a single tool call and return the result string.
    ///
    /// 07.4 D-05 ordering:
    ///   1. Check guardrails FIRST (no execution yet).
    ///   2. On Block: fire ToolCompleted{success:false, result_preview=<formatted block error>}.
    ///      Do NOT fire ToolCalled. Return the formatted block error as the tool result so
    ///      the LLM sees the same error string it saw pre-07.4.
    ///   3. On Allow or Warn: fire ToolCalled, then call registry.execute_tool(), then fire
    ///      ToolCompleted with success/failure based on the execution result.
    ///
    /// Warn counts as Allow for event firing (D-08): the tool executes and both hook events
    /// fire. The tracing::warn! side-effect is owned by ToolRegistry::check_guardrails.
    async fn execute_tool_call(&self, tool_call: &ToolCall) -> String {
        use ironhermes_hooks::GuardrailDecision;

        let name = &tool_call.function.name;
        let args_str = &tool_call.function.arguments;

        if let Some(ref cb) = self.tool_progress_callback {
            let preview = if args_str.len() > 100 {
                let mut end = 100;
                while !args_str.is_char_boundary(end) {
                    end -= 1;
                }
                format!("{}...", &args_str[..end])
            } else {
                args_str.clone()
            };
            cb(name, &preview);
        }

        debug!(tool = %name, "Executing tool call");

        // Parse args BEFORE firing any hook (pre-07.4 behavior: bad args short-circuits
        // with the same error message and does NOT fire ToolCalled/ToolCompleted).
        let args: serde_json::Value = match serde_json::from_str(args_str) {
            Ok(v) => v,
            Err(e) => {
                let err_msg = format!("Failed to parse tool arguments: {}", e);
                warn!(tool = %name, error = %err_msg);
                return err_msg;
            }
        };

        // SKILL-06 / D-04..D-09: enforce allowed_tools from active skills
        {
            let skills = self.active_skills.lock().unwrap_or_else(|e| e.into_inner());
            // D-04: only enforce when at least one skill has allowed_tools
            let restricting_skills: Vec<&ironhermes_core::SkillRecord> = skills
                .iter()
                .filter(|s| s.allowed_tools.is_some())
                .collect();

            if !restricting_skills.is_empty() {
                // D-05: union of all allowed_tools lists
                let mut allowed: std::collections::HashSet<&str> = std::collections::HashSet::new();
                for skill in &restricting_skills {
                    if let Some(ref tools) = skill.allowed_tools {
                        for t in tools {
                            allowed.insert(t.as_str());
                        }
                    }
                }
                // D-07: skills tool is always permitted
                allowed.insert("skills");

                if !allowed.contains(name.as_str()) {
                    // D-09: actionable error message
                    let mut allowed_list: Vec<&str> = allowed.into_iter().collect();
                    allowed_list.sort();
                    let err_msg = format!(
                        "Tool '{}' is not permitted by the active skill set. Allowed tools: [{}]. \
                         Activate a skill that permits '{}' or deactivate the restricting skill.",
                        name,
                        allowed_list.join(", "),
                        name,
                    );
                    warn!(tool = %name, "Skill enforcement blocked tool call");

                    // D-08: same pattern as guardrail block — fire ToolCompleted{success:false},
                    // do NOT fire ToolCalled
                    self.fire_hook(HookEventKind::ToolCompleted {
                        tool_name: name.to_string(),
                        success: false,
                        result_preview: ironhermes_hooks::event::preview(&err_msg, 200),
                        duration_ms: 0,
                    });
                    if let Some(ref cb) = self.tool_result_callback {
                        cb(name, false);
                    }

                    return err_msg;
                }
            }
        }

        // D-05 Step 1: check guardrails WITHOUT executing the tool.
        let decision = self.registry.read().await.check_guardrails(name, &args);

        match decision {
            GuardrailDecision::Block { reason } => {
                // D-05 / D-07: do NOT fire ToolCalled. Format the error via the same
                // format_guardrail_error path that ToolRegistry::dispatch uses, so the
                // block error respects ErrorDetailLevel and looks identical to the
                // pre-07.4 tool_result string that the LLM sees.
                let err_detail = self.registry.read().await.guardrail_error_detail().clone();
                let err_msg = ironhermes_hooks::format_guardrail_error(
                    name,
                    &reason,
                    "guardrail",
                    &err_detail,
                );
                warn!(tool = %name, "Tool blocked by guardrail: {}", err_msg);

                // D-05 Step 2: fire ToolCompleted ONLY (no ToolCalled before it).
                self.fire_hook(HookEventKind::ToolCompleted {
                    tool_name: name.to_string(),
                    success: false,
                    result_preview: ironhermes_hooks::event::preview(&err_msg, 200),
                    duration_ms: 0,
                });
                if let Some(ref cb) = self.tool_result_callback {
                    cb(name, false);
                }

                // Return the formatted error as the tool_result so the LLM sees the
                // same error-shaped string it saw pre-07.4.
                err_msg
            }
            GuardrailDecision::Allow | GuardrailDecision::Warn { .. } => {
                // D-05 Step 3: fire ToolCalled FIRST (this is the post-fix ordering).
                // D-08: Warn counts as Allow for event firing — do not skip ToolCalled.
                self.fire_hook(HookEventKind::ToolCalled {
                    tool_name: name.to_string(),
                    args_preview: ironhermes_hooks::event::preview(args_str, 200),
                });

                // Phase 25 D-12: single intercept dispatch path replaces hardcoded session_search match.
                // dispatch_intercepts returns Some(result) for intercepted tools, None to fall through.
                {
                    let reg = self.registry.read().await;
                    if let Some(result) = reg.dispatch_intercepts(name, args.clone()).await {
                        return match result {
                            Ok(s) => s,
                            Err(e) => format!(
                                r#"{{"error":"intercept_failed","reason":"{}"}}"#,
                                e.to_string().replace('"', "'")
                            ),
                        };
                    }
                }
                // fall through to existing dispatch path (registry.dispatch / guardrail chain)

                // Save path for subdirectory discovery before args is moved
                let tool_path_arg = args.get("path").and_then(|v| v.as_str()).map(String::from);

                let tool_start = std::time::Instant::now();

                // Phase 21.5: Intercept memory provider tools (e.g. memory_recall).
                // Route to MemoryManager.handle_tool_call which delegates to the primary provider.
                if self.memory_provider_tool_names.contains(name.as_str()) {
                    if let Some(ref mgr) = self.memory_manager {
                        let mgr_clone = mgr.clone();
                        let name_owned = name.clone();
                        let args_clone = args.clone();
                        let result = async move {
                            let guard = mgr_clone.lock().await;
                            guard.handle_tool_call(&name_owned, args_clone).await
                        }
                        .await;

                        let tool_duration = tool_start.elapsed().as_millis() as u64;
                        return match result {
                            Ok(s) => {
                                self.fire_hook(HookEventKind::ToolCompleted {
                                    tool_name: name.to_string(),
                                    success: true,
                                    result_preview: ironhermes_hooks::event::preview(&s, 200),
                                    duration_ms: tool_duration,
                                });
                                if let Some(ref cb) = self.tool_result_callback {
                                    cb(name, true);
                                }
                                s
                            }
                            Err(e) => {
                                self.fire_hook(HookEventKind::ToolCompleted {
                                    tool_name: name.to_string(),
                                    success: false,
                                    result_preview: ironhermes_hooks::event::preview(&e, 200),
                                    duration_ms: tool_duration,
                                });
                                if let Some(ref cb) = self.tool_result_callback {
                                    cb(name, false);
                                }
                                e
                            }
                        };
                    }
                    return format!(
                        r#"{{"error":"unavailable","reason":"memory manager not configured"}}"#
                    );
                }

                // Phase 32.1 Plan 02 Task 2: bump activity tracker before tool dispatch.
                // Before: record the in-flight tool name so the cron poller can see it.
                // After: reset current_tool to None so a subsequent activity_summary()
                // reflects that dispatch is finished.
                self.mark_activity(ActivityKind::ToolCall, Some(name.clone()));

                // Take an `args` snapshot for the trajectory append site BEFORE
                // execute_tool consumes the original. Cheap clone — Plan 9 trades
                // a JSON Value clone for a redaction seam (Plan 5 Tool::redact_args).
                let raw_args_for_traj = args.clone();
                let dispatch_result = self.registry.read().await.execute_tool(name, args).await;
                // Phase 32.1 Plan 02 Task 2: reset current_tool after dispatch completes.
                self.mark_activity(ActivityKind::ToolCall, None);
                let duration_ms = tool_start.elapsed().as_millis() as u64;

                match dispatch_result {
                    Ok(result) => {
                        // Phase 25.3 D-T-1 / D-T-3: append trajectory entry (success path).
                        // Best-effort: append failure logs warn! and does NOT abort the turn.
                        // Plan 6 cycle-break: trajectory_writer is Arc<dyn TrajectoryWriterHandle>.
                        // We serialize the entry to a single JSONL line and hand it to the trait;
                        // the impl (TrajectoryWriterHandleImpl in ironhermes-trajectory) handles
                        // locking, write, and fsync.
                        if let Some(ref handle) = self.trajectory_writer {
                            // Look up the tool to call its redact_args override (Plan 5 trait extension).
                            let registry_guard = self.registry.read().await;
                            let redacted = if let Some(t) = registry_guard.get(name.as_str()) {
                                t.redact_args(&raw_args_for_traj)
                            } else {
                                raw_args_for_traj.clone()
                            };
                            drop(registry_guard);
                            let entry = TrajectoryEntry::success(
                                name.as_str(),
                                redacted,
                                result.clone(),
                                duration_ms,
                                classify_impact_level(name.as_str()),
                                self.turn_index.load(std::sync::atomic::Ordering::Relaxed),
                                tool_call.id.clone(),
                            );
                            match serde_json::to_string(&entry) {
                                Ok(line) => {
                                    if let Err(e) = handle.append_json_line(&line) {
                                        tracing::warn!(error = %e, tool = %name,
                                            "Phase 25.3: trajectory append failed; ledger entry lost for this call");
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(error = %e, tool = %name,
                                        "Phase 25.3: trajectory entry serialize failed; ledger entry skipped");
                                }
                            }
                        }

                        self.fire_hook(HookEventKind::ToolCompleted {
                            tool_name: name.to_string(),
                            success: true,
                            result_preview: ironhermes_hooks::event::preview(&result, 200),
                            duration_ms,
                        });
                        if let Some(ref cb) = self.tool_result_callback {
                            cb(name, true);
                        }

                        // CTX-03/CTX-04: progressive subdirectory discovery for file-access tools
                        let mut final_result = result;
                        const FILE_ACCESS_TOOLS: &[&str] =
                            &["read_file", "write_file", "patch", "search_files"];
                        if FILE_ACCESS_TOOLS.contains(&name.as_str()) {
                            if let Some(ref disc) = self.subdir_discovery {
                                if let Some(ref path_str) = tool_path_arg {
                                    let path = std::path::Path::new(path_str);
                                    if let Ok(mut discovery) = disc.lock() {
                                        if let Some(ctx) = discovery.check_path(path) {
                                            debug!(tool = %name, path = %path_str, "Subdirectory context discovered");
                                            final_result.push_str(&ctx);
                                        }
                                    }
                                }
                            }
                        }
                        final_result
                    }
                    Err(e) => {
                        let err_msg = format!("Tool '{}' failed: {}", name, e);
                        warn!(%err_msg);

                        // Phase 25.3 D-T-1 / D-T-3: append trajectory entry (failure path).
                        // Plan 6 cycle-break: serialize then hand to TrajectoryWriterHandle.
                        if let Some(ref handle) = self.trajectory_writer {
                            let registry_guard = self.registry.read().await;
                            let redacted = if let Some(t) = registry_guard.get(name.as_str()) {
                                t.redact_args(&raw_args_for_traj)
                            } else {
                                raw_args_for_traj.clone()
                            };
                            drop(registry_guard);
                            let entry = TrajectoryEntry::failure(
                                name.as_str(),
                                redacted,
                                err_msg.clone(),
                                duration_ms,
                                classify_impact_level(name.as_str()),
                                self.turn_index.load(std::sync::atomic::Ordering::Relaxed),
                                tool_call.id.clone(),
                            );
                            match serde_json::to_string(&entry) {
                                Ok(line) => {
                                    if let Err(append_err) = handle.append_json_line(&line) {
                                        tracing::warn!(error = %append_err, tool = %name,
                                            "Phase 25.3: trajectory append failed (failure path)");
                                    }
                                }
                                Err(serr) => {
                                    tracing::warn!(error = %serr, tool = %name,
                                        "Phase 25.3: trajectory entry serialize failed (failure path); ledger entry skipped");
                                }
                            }
                        }

                        self.fire_hook(HookEventKind::ToolCompleted {
                            tool_name: name.to_string(),
                            success: false,
                            result_preview: ironhermes_hooks::event::preview(&err_msg, 200),
                            duration_ms,
                        });
                        if let Some(ref cb) = self.tool_result_callback {
                            cb(name, false);
                        }
                        err_msg
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 25.3 Plan 9 (D-T-1): tool-name -> ImpactLevel classifier.
// ---------------------------------------------------------------------------

/// Phase 25.3 D-T-1: classify a tool name into an `ImpactLevel` for the trajectory ledger.
///
/// Used by `AgentLoop`'s trajectory append site to populate
/// `TrajectoryEntry.impact_level`. Phase 25.4 Curator's heuristic gate (D-C-2)
/// sums these weights to decide curation eligibility.
///
/// Classification rules:
/// - **Read (0)**: pure read operations (read_file, search_files, web_search,
///   web_read, web_extract, session_search, status checks).
/// - **Write (5)**: persistent state changes that don't run untrusted code or
///   invoke external systems (write_file, patch, patch_file, MEMORY.md writes).
/// - **SystemChange (10)**: code execution, shell, MCP, anything that can have
///   side effects beyond the local filesystem (terminal, execute_code, mcp_*,
///   browser_* navigation/actions).
///
/// Conservative defaults:
/// - Unknown tools default to **Write (5)**: not a no-op, not a code-execution risk.
/// - MCP tools (any name starting with `mcp_` or `mcp__`) default to
///   **SystemChange (10)**: operator-defined, treat as untrusted code path.
pub(crate) fn classify_impact_level(tool_name: &str) -> ImpactLevel {
    // Read-only tools (heuristic by name + known catalog).
    const READ_TOOLS: &[&str] = &[
        "read_file",
        "search_files",
        "web_search",
        "web_read",
        "web_extract",
        "session_search",
        "status",
        "list_files",
        "browser_snapshot",
        "browser_get_images",
        "browser_console",
        "memory_search",
    ];
    // Write-but-local tools.
    const WRITE_TOOLS: &[&str] = &[
        "write_file",
        "patch",
        "patch_file",
        "create_file",
        "delete_file",
        "memory_write",
        "skill_install",
        "skill_remove",
    ];
    // System-changing tools.
    const SYSTEM_CHANGE_TOOLS: &[&str] = &[
        "terminal",
        "execute_code",
        "delegate_task",
        "browser_click",
        "browser_navigate",
        "browser_type",
        "browser_press",
        "browser_scroll",
        "browser_back",
        "browser_close",
    ];

    if READ_TOOLS.contains(&tool_name) {
        ImpactLevel::Read
    } else if WRITE_TOOLS.contains(&tool_name) {
        ImpactLevel::Write
    } else if SYSTEM_CHANGE_TOOLS.contains(&tool_name)
        || tool_name.starts_with("mcp_")
        || tool_name.starts_with("mcp__")
    {
        ImpactLevel::SystemChange
    } else {
        // Conservative default for unknown tools — Curator heuristic D-C-2 weights this as 5.
        ImpactLevel::Write
    }
}

// ---------------------------------------------------------------------------
// Tests: hook ordering and duplicate-event prevention (07.4-02)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod hooks_ordering_tests {
    use super::*;
    use async_trait::async_trait;
    use ironhermes_core::ToolSchema;
    use ironhermes_hooks::{
        BlocklistGuardrail, GuardrailDecision, GuardrailHook, HookEvent, HookEventKind,
        HookRegistry, HooksConfig,
    };
    use ironhermes_tools::{Tool, ToolRegistry};
    use std::sync::{Arc, Mutex};

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn capture_registry() -> (Arc<HookRegistry>, Arc<Mutex<Vec<HookEvent>>>) {
        let mut registry = HookRegistry::new(HooksConfig::default());
        let captured: Arc<Mutex<Vec<HookEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let cap = captured.clone();
        registry.add_listener(Arc::new(move |event: HookEvent| {
            cap.lock().unwrap().push(event);
        }));
        (Arc::new(registry), captured)
    }

    // -----------------------------------------------------------------------
    // Mock tools
    // -----------------------------------------------------------------------

    struct OkMockTool;

    #[async_trait]
    impl Tool for OkMockTool {
        fn name(&self) -> &str {
            "mock"
        }
        fn toolset(&self) -> &str {
            "test"
        }
        fn description(&self) -> &str {
            "ok mock"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                "mock",
                "ok mock",
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("mock result".to_string())
        }
    }

    struct FailMockTool;

    #[async_trait]
    impl Tool for FailMockTool {
        fn name(&self) -> &str {
            "failmock"
        }
        fn toolset(&self) -> &str {
            "test"
        }
        fn description(&self) -> &str {
            "fail mock"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                "failmock",
                "fail mock",
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Err(anyhow::anyhow!("boom"))
        }
    }

    // -----------------------------------------------------------------------
    // Warn guardrail
    // -----------------------------------------------------------------------

    struct WarnGuardrail;

    impl GuardrailHook for WarnGuardrail {
        fn check(&self, _name: &str, _args: &serde_json::Value) -> GuardrailDecision {
            GuardrailDecision::Warn {
                reason: "always warn".to_string(),
            }
        }
        fn name(&self) -> &str {
            "warn-always"
        }
    }

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    fn tool_call(name: &str) -> ToolCall {
        ToolCall {
            id: "call-1".to_string(),
            call_type: "function".to_string(),
            function: ironhermes_core::FunctionCall {
                name: name.to_string(),
                arguments: "{}".to_string(),
            },
        }
    }

    fn build_agent(tool_registry: ToolRegistry, hook_registry: Arc<HookRegistry>) -> AgentLoop {
        let client = AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "http://localhost".to_string(),
            "".to_string(),
            "mock-model",
        ));
        AgentLoop::new(client, Arc::new(RwLock::new(tool_registry)), 4)
            .with_hook_registry(hook_registry)
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    /// D-05 / D-07 / audit warning #3: a blocked tool must emit zero ToolCalled
    /// and exactly one ToolCompleted{success:false} whose result_preview contains
    /// the block reason.
    #[tokio::test]
    async fn test_blocked_tool_no_tool_called_event() {
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Box::new(OkMockTool));
        tool_registry.add_guardrail(Box::new(BlocklistGuardrail::new(vec!["mock".to_string()])));

        let (hook_registry, captured) = capture_registry();
        let agent = build_agent(tool_registry, hook_registry);

        let result = agent.execute_tool_call(&tool_call("mock")).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let events = captured.lock().unwrap();
        let tool_called_count = events
            .iter()
            .filter(|e| matches!(e.kind, HookEventKind::ToolCalled { .. }))
            .count();
        let tool_completed: Vec<_> = events
            .iter()
            .filter_map(|e| match &e.kind {
                HookEventKind::ToolCompleted {
                    success,
                    result_preview,
                    ..
                } => Some((*success, result_preview.clone())),
                _ => None,
            })
            .collect();

        assert_eq!(
            tool_called_count, 0,
            "blocked tool must not emit ToolCalled (audit warning #3)"
        );
        assert_eq!(
            tool_completed.len(),
            1,
            "blocked tool must emit exactly one ToolCompleted"
        );
        assert_eq!(
            tool_completed[0].0, false,
            "blocked tool ToolCompleted must have success=false"
        );
        assert!(
            tool_completed[0].1.contains("blocked")
                || tool_completed[0].1.contains("security policy")
                || tool_completed[0].1.contains("blocklist"),
            "ToolCompleted.result_preview must contain block reason: {:?}",
            tool_completed[0].1
        );
        assert!(
            result.contains("blocked")
                || result.contains("security policy")
                || result.contains("blocklist"),
            "tool_result returned to LLM must be the formatted block error: {result}"
        );
    }

    /// Allowed tool fires ToolCalled then ToolCompleted{success:true} in order.
    #[tokio::test]
    async fn test_allowed_tool_fires_tool_called_then_tool_completed() {
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Box::new(OkMockTool));
        let (hook_registry, captured) = capture_registry();
        let agent = build_agent(tool_registry, hook_registry);

        let result = agent.execute_tool_call(&tool_call("mock")).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert_eq!(result, "mock result");
        let events = captured.lock().unwrap();
        assert_eq!(
            events.len(),
            2,
            "expected ToolCalled + ToolCompleted, got {:?}",
            *events
        );
        assert!(
            matches!(events[0].kind, HookEventKind::ToolCalled { .. }),
            "first event must be ToolCalled"
        );
        assert!(
            matches!(
                events[1].kind,
                HookEventKind::ToolCompleted { success: true, .. }
            ),
            "second event must be ToolCompleted{{success:true}}"
        );
    }

    /// D-08: warn counts as allow for event firing — ToolCalled + ToolCompleted both fire.
    #[tokio::test]
    async fn test_warn_guardrail_fires_tool_called_and_tool_completed() {
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Box::new(OkMockTool));
        tool_registry.add_guardrail(Box::new(WarnGuardrail));
        let (hook_registry, captured) = capture_registry();
        let agent = build_agent(tool_registry, hook_registry);

        let _ = agent.execute_tool_call(&tool_call("mock")).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let events = captured.lock().unwrap();
        assert_eq!(
            events.len(),
            2,
            "warn must still emit ToolCalled + ToolCompleted"
        );
        assert!(matches!(events[0].kind, HookEventKind::ToolCalled { .. }));
        assert!(matches!(
            events[1].kind,
            HookEventKind::ToolCompleted { success: true, .. }
        ));
    }

    /// Execution errors on an allowed tool still emit ToolCalled + ToolCompleted{success:false}.
    #[tokio::test]
    async fn test_allowed_tool_execution_failure_still_fires_tool_called() {
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Box::new(FailMockTool));
        let (hook_registry, captured) = capture_registry();
        let agent = build_agent(tool_registry, hook_registry);

        let _ = agent.execute_tool_call(&tool_call("failmock")).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let events = captured.lock().unwrap();
        assert_eq!(
            events.len(),
            2,
            "failed allowed tool must still emit both events"
        );
        assert!(matches!(events[0].kind, HookEventKind::ToolCalled { .. }));
        assert!(matches!(
            events[1].kind,
            HookEventKind::ToolCompleted { success: false, .. }
        ));
    }

    /// D-01 / audit warning #4: agent_loop.rs must not fire MessageReceived or ResponseSent.
    /// Uses include_str! as a compile-time regression guard — future edits that reintroduce
    /// these forbidden fires will trip this test without needing a mock LlmClient.
    ///
    /// We search for the fire_hook call patterns specifically so the assertion strings
    /// themselves (which mention the type names for documentation purposes) do not
    /// cause false positives.
    // -----------------------------------------------------------------------
    // Skill enforcement tests (SKILL-06 / 07.5-01 Task 2)
    // -----------------------------------------------------------------------

    fn make_skill_record(
        name: &str,
        allowed_tools: Option<Vec<&str>>,
    ) -> ironhermes_core::SkillRecord {
        ironhermes_core::SkillRecord {
            name: name.to_string(),
            description: format!("{} skill", name),
            path: std::path::PathBuf::from("/tmp/fake"),
            platforms: None,
            compatibility: None,
            allowed_tools: allowed_tools.map(|v| v.into_iter().map(|s| s.to_string()).collect()),
            metadata: None,
            // Phase 19 Plan 01 added typed hermes_metadata + source fields.
            hermes_metadata: None,
            source: ironhermes_core::SkillSource::Builtin,
        }
    }

    #[tokio::test]
    async fn test_skill_enforcement_blocks_unlisted_tool() {
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Box::new(OkMockTool));
        let (hook_registry, _captured) = capture_registry();
        let agent = build_agent(tool_registry, hook_registry);

        // Pre-populate active_skills with a restrictive skill
        {
            let mut skills = agent.active_skills.lock().unwrap();
            skills.push(make_skill_record("focus", Some(vec!["web_read"])));
        }

        let result = agent.execute_tool_call(&tool_call("mock")).await;
        assert!(
            result.contains("not permitted by the active skill set"),
            "blocked tool should get enforcement error, got: {result}"
        );
        assert!(
            result.contains("Allowed tools"),
            "error should list allowed tools, got: {result}"
        );
    }

    #[tokio::test]
    async fn test_skill_enforcement_allows_listed_tool() {
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Box::new(OkMockTool));
        let (hook_registry, _captured) = capture_registry();
        let agent = build_agent(tool_registry, hook_registry);

        {
            let mut skills = agent.active_skills.lock().unwrap();
            skills.push(make_skill_record("focus", Some(vec!["mock"])));
        }

        let result = agent.execute_tool_call(&tool_call("mock")).await;
        assert_eq!(result, "mock result", "listed tool should execute normally");
    }

    #[tokio::test]
    async fn test_skill_enforcement_inactive_means_all_allowed() {
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Box::new(OkMockTool));
        let (hook_registry, _captured) = capture_registry();
        let agent = build_agent(tool_registry, hook_registry);

        // No active skills — everything is allowed
        let result = agent.execute_tool_call(&tool_call("mock")).await;
        assert_eq!(
            result, "mock result",
            "no active skills = all tools allowed"
        );
    }

    #[tokio::test]
    async fn test_skill_enforcement_skills_tool_always_allowed() {
        let mut tool_registry = ToolRegistry::new();
        // Register a mock tool named "skills" to simulate the skills tool
        struct SkillsMockTool;
        #[async_trait]
        impl Tool for SkillsMockTool {
            fn name(&self) -> &str {
                "skills"
            }
            fn toolset(&self) -> &str {
                "test"
            }
            fn description(&self) -> &str {
                "mock skills"
            }
            fn schema(&self) -> ToolSchema {
                ToolSchema::new(
                    "skills",
                    "mock skills",
                    serde_json::json!({"type": "object", "properties": {}}),
                )
            }
            async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
                Ok("skills result".to_string())
            }
        }
        tool_registry.register(Box::new(SkillsMockTool));
        let (hook_registry, _captured) = capture_registry();
        let agent = build_agent(tool_registry, hook_registry);

        {
            let mut skills = agent.active_skills.lock().unwrap();
            skills.push(make_skill_record("focus", Some(vec!["web_read"])));
        }

        let result = agent.execute_tool_call(&tool_call("skills")).await;
        assert_eq!(
            result, "skills result",
            "skills tool must always be permitted (D-07)"
        );
    }

    #[tokio::test]
    async fn test_skill_enforcement_union_of_multiple_skills() {
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Box::new(OkMockTool)); // name = "mock"
        let (hook_registry, _captured) = capture_registry();
        let agent = build_agent(tool_registry, hook_registry);

        {
            let mut skills = agent.active_skills.lock().unwrap();
            skills.push(make_skill_record("skill-a", Some(vec!["web_read"])));
            skills.push(make_skill_record("skill-b", Some(vec!["memory"])));
        }

        // "mock" is not in the union {web_read, memory, skills} -> should be blocked
        let result = agent.execute_tool_call(&tool_call("mock")).await;
        assert!(
            result.contains("not permitted"),
            "tool not in union should be blocked, got: {result}"
        );
    }

    #[tokio::test]
    async fn test_skill_enforcement_none_allowed_tools_ignored() {
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Box::new(OkMockTool));
        let (hook_registry, _captured) = capture_registry();
        let agent = build_agent(tool_registry, hook_registry);

        {
            let mut skills = agent.active_skills.lock().unwrap();
            // Skill with None allowed_tools — does not restrict (D-06)
            skills.push(make_skill_record("non-restricting", None));
            // Skill with Some allowed_tools — restricts to web_read only
            skills.push(make_skill_record("restricting", Some(vec!["web_read"])));
        }

        // "mock" is not in allowed set -> blocked because the restricting skill is active
        let result = agent.execute_tool_call(&tool_call("mock")).await;
        assert!(
            result.contains("not permitted"),
            "tool should be blocked when any skill restricts, got: {result}"
        );
    }

    // -----------------------------------------------------------------------
    // CancellationToken tests (09-04 Task 1)
    // -----------------------------------------------------------------------

    #[test]
    fn test_agent_loop_with_cancellation_token_sets_token() {
        use tokio_util::sync::CancellationToken;
        let client = AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "http://localhost".to_string(),
            "".to_string(),
            "mock-model",
        ));
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let token = CancellationToken::new();
        let agent = AgentLoop::new(client, registry, 4).with_cancellation_token(token.clone());
        // Verify the token is set (it exists on the struct)
        assert!(
            agent.cancel_token.is_some(),
            "cancel_token should be set after with_cancellation_token"
        );
    }

    #[tokio::test]
    async fn test_agent_loop_run_returns_early_when_cancelled_before_first_iteration() {
        use tokio_util::sync::CancellationToken;
        let client = AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "http://localhost".to_string(),
            "".to_string(),
            "mock-model",
        ));
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let token = CancellationToken::new();
        // Cancel BEFORE run
        token.cancel();
        let mut agent = AgentLoop::new(client, registry, 4).with_cancellation_token(token);
        let messages = vec![ChatMessage::user("hello")];
        let result = agent.run(messages).await.unwrap();
        assert!(
            !result.finished_naturally,
            "should not finish naturally when cancelled"
        );
        assert_eq!(
            result.final_response.as_deref(),
            Some("Cancelled by parent")
        );
        assert_eq!(
            result.turns_used, 0,
            "should use 0 turns when cancelled before first iteration"
        );
    }

    #[tokio::test]
    async fn test_agent_loop_source_has_no_message_received_or_response_sent_fires() {
        let src = include_str!("agent_loop.rs");
        // Search for the actual fire_hook invocation patterns, not bare type references.
        // The assertion strings below intentionally avoid containing these exact patterns.
        let msg_rcvd_fire = concat!("fire_hook(HookEventKind::", "MessageReceived");
        let resp_sent_fire = concat!("fire_hook(HookEventKind::", "ResponseSent");
        assert!(
            !src.contains(msg_rcvd_fire),
            "agent_loop.rs must not call fire_hook for MessageReceived (D-01, audit warning #4)"
        );
        assert!(
            !src.contains(resp_sent_fire),
            "agent_loop.rs must not call fire_hook for ResponseSent (D-01, audit warning #4)"
        );
    }

    // -----------------------------------------------------------------------
    // Phase 25.3 D-T-1 / D-T-3: trajectory append tests (Plan 9 Task 2)
    // -----------------------------------------------------------------------

    /// Build a minimal AgentLoop with the given trajectory writer attached
    /// AND `OkMockTool` registered, for trajectory append tests.
    fn build_agent_with_trajectory(
        trajectory: Arc<dyn ironhermes_core::commands::context::TrajectoryWriterHandle>,
    ) -> AgentLoop {
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Box::new(OkMockTool));
        let (hook_registry, _captured) = capture_registry();
        build_agent(tool_registry, hook_registry).with_trajectory_writer(trajectory)
    }

    fn build_agent_with_trajectory_and_failmock(
        trajectory: Arc<dyn ironhermes_core::commands::context::TrajectoryWriterHandle>,
    ) -> AgentLoop {
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Box::new(FailMockTool));
        let (hook_registry, _captured) = capture_registry();
        build_agent(tool_registry, hook_registry).with_trajectory_writer(trajectory)
    }

    /// Phase 25.3 D-T-1 / D-T-3: a successful tool call appends exactly one
    /// JSONL entry to the trajectory file with name="mock", result populated,
    /// error None, and impact_level matching the classifier's verdict for the
    /// unknown tool name "mock" (default Write).
    #[tokio::test]
    async fn execute_tool_call_appends_trajectory_entry_on_success() {
        use ironhermes_trajectory::{
            ImpactLevel, TrajectoryReader, TrajectoryWriter, TrajectoryWriterHandleImpl,
        };

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("trajectories.jsonl");
        let writer = std::sync::Arc::new(std::sync::Mutex::new(
            TrajectoryWriter::open(&path).expect("open writer"),
        ));
        let handle: std::sync::Arc<dyn ironhermes_core::commands::context::TrajectoryWriterHandle> =
            std::sync::Arc::new(TrajectoryWriterHandleImpl::new(writer.clone()));

        let agent = build_agent_with_trajectory(handle);
        let _ = agent.execute_tool_call(&tool_call("mock")).await;

        // Drop any owned writer references and force the Mutex<Writer> to flush
        // by reading the file (TrajectoryWriter calls sync_data per append).
        let reader = TrajectoryReader::open(&path);
        let entries = reader.read_all().expect("read trajectory file");
        assert_eq!(
            entries.len(),
            1,
            "execute_tool_call must append exactly one trajectory entry; got {}",
            entries.len()
        );
        assert_eq!(
            entries[0].name, "mock",
            "trajectory entry name must match tool name"
        );
        assert!(
            entries[0].result.is_some(),
            "success entry must have result populated"
        );
        assert!(
            entries[0].error.is_none(),
            "success entry must have error None"
        );
        // "mock" is an unknown tool name to classify_impact_level → Write (5) default.
        assert_eq!(
            entries[0].impact_level,
            ImpactLevel::Write,
            "unknown tool name 'mock' must default to ImpactLevel::Write"
        );
        assert_eq!(
            entries[0].turn_index, 0,
            "default turn_index from a fresh AgentLoop must be 0"
        );
        assert_eq!(
            entries[0].tool_call_id, "call-1",
            "tool_call_id must reflect the LLM-supplied ToolCall.id"
        );
    }

    /// Phase 25.3 D-T-1 / D-T-3: a failing tool call appends one entry with
    /// `error` populated and `result` None.
    #[tokio::test]
    async fn execute_tool_call_appends_trajectory_entry_on_failure() {
        use ironhermes_trajectory::{
            TrajectoryReader, TrajectoryWriter, TrajectoryWriterHandleImpl,
        };

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("trajectories.jsonl");
        let writer = std::sync::Arc::new(std::sync::Mutex::new(
            TrajectoryWriter::open(&path).expect("open writer"),
        ));
        let handle: std::sync::Arc<dyn ironhermes_core::commands::context::TrajectoryWriterHandle> =
            std::sync::Arc::new(TrajectoryWriterHandleImpl::new(writer.clone()));

        let agent = build_agent_with_trajectory_and_failmock(handle);
        let _ = agent.execute_tool_call(&tool_call("failmock")).await;

        let reader = TrajectoryReader::open(&path);
        let entries = reader.read_all().expect("read trajectory file");
        assert_eq!(
            entries.len(),
            1,
            "execute_tool_call must append exactly one trajectory entry on failure"
        );
        assert_eq!(entries[0].name, "failmock");
        assert!(
            entries[0].result.is_none(),
            "failure entry must have result None"
        );
        assert!(
            entries[0].error.is_some(),
            "failure entry must have error populated"
        );
    }

    /// Phase 25.3 D-T-3: when no trajectory writer is attached, execute_tool_call
    /// works without panicking and produces no entries (Option<...> field is None).
    #[tokio::test]
    async fn execute_tool_call_without_trajectory_writer_is_a_noop_for_ledger() {
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Box::new(OkMockTool));
        let (hook_registry, _captured) = capture_registry();
        let agent = build_agent(tool_registry, hook_registry);
        // Sanity: no trajectory writer attached.
        assert!(
            agent.trajectory_writer.is_none(),
            "default AgentLoop must have trajectory_writer None"
        );
        // Should not panic, should return the tool result text.
        let result = agent.execute_tool_call(&tool_call("mock")).await;
        assert_eq!(
            result, "mock result",
            "tool result must pass through unchanged"
        );
    }
}

// ---------------------------------------------------------------------------
// Tests: iteration budget (12-03 PROV-09, PROV-10)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod budget_tests {
    use super::*;
    use crate::budget::{BudgetHandle, CAUTION_ADVISORY, WARNING_ADVISORY};
    use std::sync::Arc;

    fn make_agent(max_iterations: usize) -> AgentLoop {
        let client = AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "http://localhost".to_string(),
            "".to_string(),
            "test",
        ));
        let registry = Arc::new(RwLock::new(ironhermes_tools::ToolRegistry::new()));
        AgentLoop::new(client, registry, max_iterations)
    }

    /// Drive a handle to a target `used` count (used by the tier tests below).
    fn drive_used(handle: &BudgetHandle, used: usize) {
        for _ in 0..used {
            handle.consume();
        }
    }

    #[test]
    fn test_budget_threshold_below_70() {
        let handle = BudgetHandle::new(10);
        drive_used(&handle, 5); // 50% → None tier → no advisory
        let mut agent = make_agent(10).with_budget(handle);
        assert_eq!(agent.check_budget_threshold(), None);
    }

    #[test]
    fn test_budget_threshold_at_70() {
        let handle = BudgetHandle::new(10);
        drive_used(&handle, 7); // 70% → Caution70 → first observation injects
        let mut agent = make_agent(10).with_budget(handle);
        let result = agent.check_budget_threshold();
        assert_eq!(
            result,
            Some(CAUTION_ADVISORY),
            "expected CAUTION_ADVISORY at 70%"
        );
        // Exactly-once: second call at steady-state returns None (no spam).
        assert_eq!(
            agent.check_budget_threshold(),
            None,
            "steady-state at Caution70 must NOT re-inject"
        );
    }

    #[test]
    fn test_budget_threshold_at_90() {
        let handle = BudgetHandle::new(10);
        drive_used(&handle, 9); // 90% → Warning90 → first observation injects
        let mut agent = make_agent(10).with_budget(handle);
        let result = agent.check_budget_threshold();
        assert_eq!(
            result,
            Some(WARNING_ADVISORY),
            "expected WARNING_ADVISORY at 90%"
        );
    }

    #[test]
    fn test_shared_budget_increment() {
        // BudgetHandle::clone shares the same underlying counter (Arc<AtomicUsize>).
        // This is still used by gateway/CommandContext and reset() visibility.
        let parent = BudgetHandle::new(10);
        let child = parent.clone();
        for _ in 0..5 {
            parent.consume();
        }
        for _ in 0..3 {
            child.consume();
        }
        assert_eq!(parent.used(), 8);
        assert_eq!(child.used(), 8, "clones share the same counter");
    }

    /// D-07.1 independence regression test (Plan 35-02).
    ///
    /// Two distinct `BudgetHandle::new(max)` instances do NOT share a counter.
    /// This models the parent agent loop's budget and the fresh per-child budget
    /// produced by `AgentSubagentRunner::run_child` (which now calls
    /// `BudgetHandle::new(max_iterations)` rather than cloning the parent handle).
    ///
    /// Draining the child to exhaustion must leave the parent budget unchanged.
    /// This is the inversion of the old PROV-10 shared-counter assumption.
    #[test]
    fn test_independent_budget_child_drain_does_not_affect_parent() {
        let max = 10;
        // Two separate handles — two distinct Arc<AtomicUsize> instances.
        let parent = BudgetHandle::new(max);
        let child = BudgetHandle::new(max);

        // Both start full.
        assert_eq!(parent.remaining(), max, "parent starts at max");
        assert_eq!(child.remaining(), max, "child starts at max");

        // Drain the child to exhaustion.
        for _ in 0..max {
            child.consume();
        }
        assert_eq!(child.remaining(), 0, "child is fully drained");

        // Independence guarantee: the parent's counter is untouched.
        assert_eq!(
            parent.remaining(),
            max,
            "child drain must not affect parent remaining() — independence guarantee (D-07.1)"
        );
    }

    #[test]
    fn test_budget_getter_returns_handle() {
        let handle = BudgetHandle::new(10);
        let agent = make_agent(10).with_budget(handle.clone());
        let retrieved = agent.budget();
        assert!(
            retrieved.is_some(),
            "budget() should return Some after with_budget"
        );
        retrieved.unwrap().consume();
        assert_eq!(handle.used(), 1, "retrieved handle shares the same counter");
    }

    // -----------------------------------------------------------------------
    // Plan 21.7-05 additions (D-15 / G-01 / T-21.7-05-02).
    // -----------------------------------------------------------------------

    #[test]
    fn tier_crossing_injects_advisory_exactly_once_then_goes_quiet() {
        // max=10 → Caution70 at used=7.
        let handle = BudgetHandle::new(10);
        let mut agent = make_agent(10).with_budget(handle.clone());
        // Below threshold — no advisory.
        drive_used(&handle, 6);
        assert_eq!(agent.check_budget_threshold(), None);
        // Cross to Caution70 — first observation returns CAUTION_ADVISORY.
        handle.consume();
        assert_eq!(agent.check_budget_threshold(), Some(CAUTION_ADVISORY));
        // Still Caution70 — steady-state returns None (T-21.7-05-02 no-spam).
        assert_eq!(agent.check_budget_threshold(), None);
        // Cross to Warning90 — returns WARNING_ADVISORY once.
        drive_used(&handle, 2);
        assert_eq!(agent.check_budget_threshold(), Some(WARNING_ADVISORY));
        assert_eq!(agent.check_budget_threshold(), None);
    }

    #[test]
    fn stop100_pressure_returns_none_from_check_budget_threshold() {
        // Plan 21.7-05: advisory_text(Stop100) is None by design — the loop
        // terminates before the next provider call, so there is nothing to inject.
        let handle = BudgetHandle::new(2);
        drive_used(&handle, 2);
        let mut agent = make_agent(2).with_budget(handle);
        // Tier is Stop100; we crossed from the last-seen tier, but
        // advisory_text(Stop100)=None → no injection.
        assert_eq!(agent.check_budget_threshold(), None);
    }

    #[test]
    fn budget_exhausted_constructor_sets_expected_fields() {
        let result = AgentResult::budget_exhausted(vec![], 7);
        assert_eq!(result.turns_used, 7);
        assert!(!result.finished_naturally);
        assert!(result.final_response.is_none());
        assert_eq!(result.stop_reason, StopReason::BudgetExhausted);
    }
}

// ---------------------------------------------------------------------------
// Tests: one-shot fallback (12-03 PROV-07, D-11)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod fallback_tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn test_fallback_state_initial() {
        let client = AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "http://localhost".to_string(),
            "".to_string(),
            "test",
        ));
        let registry = Arc::new(RwLock::new(ironhermes_tools::ToolRegistry::new()));
        let agent = AgentLoop::new(client, registry, 10);
        assert!(
            !agent.fallback_activated,
            "fallback_activated should start false"
        );
        assert!(
            agent.fallback_client.is_none(),
            "fallback_client should start None"
        );
    }

    #[test]
    fn test_classify_429_error() {
        let err = anyhow!("HTTP request failed with status: 429 Too Many Requests");
        let (should_retry, should_fallback) = AgentLoop::classify_llm_error(&err);
        assert!(should_retry, "429 should be retryable");
        assert!(should_fallback, "429 should trigger fallback");
    }

    #[test]
    fn test_classify_401_error() {
        let err = anyhow!("HTTP request failed with status: 401 Unauthorized");
        let (should_retry, should_fallback) = AgentLoop::classify_llm_error(&err);
        assert!(!should_retry, "401 should not be retried");
        assert!(should_fallback, "401 should trigger fallback");
    }

    #[test]
    fn test_classify_other_error() {
        let err = anyhow!("unexpected end of JSON input");
        let (should_retry, should_fallback) = AgentLoop::classify_llm_error(&err);
        assert!(should_retry, "generic errors should be retryable");
        assert!(
            !should_fallback,
            "generic errors should not trigger fallback"
        );
    }

    /// Marquee transport case: Ollama not running, full reqwest/hyper-util chain.
    /// `{err:#}` contains `"error sending request for url"` (reqwest Kind::Request)
    /// + `"tcp connect error"` (hyper-util) + `"Connection refused"` (OS errno).
    /// All three needles match; each is independently sufficient.
    #[test]
    fn test_classify_transport_request_send_marker() {
        let err = anyhow!(
            "Streaming LLM call failed: Failed to send streaming request: \
             error sending request for url (http://localhost:11434/v1/chat/completions): \
             tcp connect error: Connection refused (os error 61)"
        );
        let (should_retry, should_fallback) = AgentLoop::classify_llm_error(&err);
        assert!(should_retry, "transport errors should be retryable");
        assert!(
            should_fallback,
            "reqwest Kind::Request error should trigger fallback"
        );
    }

    /// TCP connection refused without the outer reqwest wrapper — independently
    /// exercises the `"connection refused"` and `"tcp connect error"` needles.
    #[test]
    fn test_classify_transport_connection_refused() {
        let err = anyhow!("tcp connect error: Connection refused (os error 61)");
        let (should_retry, should_fallback) = AgentLoop::classify_llm_error(&err);
        assert!(should_retry, "transport errors should be retryable");
        assert!(
            should_fallback,
            "connection refused should trigger fallback"
        );
    }

    /// Connect-phase timeout: exercises `"tcp connect error"` and `"operation timed out"`.
    #[test]
    fn test_classify_transport_connect_timeout() {
        let err = anyhow!(
            "error sending request for url (http://localhost:11434/...): \
             tcp connect error: Operation timed out (os error 60)"
        );
        let (should_retry, should_fallback) = AgentLoop::classify_llm_error(&err);
        assert!(should_retry, "transport errors should be retryable");
        assert!(
            should_fallback,
            "connect timeout should trigger fallback"
        );
    }

    /// DNS resolution failure: exercises `"dns error"` and `"failed to lookup address"`.
    #[test]
    fn test_classify_transport_dns_failure() {
        let err = anyhow!(
            "error sending request for url (http://nope.invalid/...): \
             dns error: failed to lookup address information: \
             nodename nor servname provided, or not known"
        );
        let (should_retry, should_fallback) = AgentLoop::classify_llm_error(&err);
        assert!(should_retry, "transport errors should be retryable");
        assert!(
            should_fallback,
            "DNS failure should trigger fallback"
        );
    }

    /// Connection reset by peer — fed alone (no `"error sending request for url"`)
    /// because mid-stream resets may surface via reqwest `Kind::Body`, not `Kind::Request`.
    /// Exercises the `"connection reset"` needle in isolation.
    #[test]
    fn test_classify_transport_connection_reset() {
        let err = anyhow!("Connection reset by peer (os error 54)");
        let (should_retry, should_fallback) = AgentLoop::classify_llm_error(&err);
        assert!(should_retry, "transport errors should be retryable");
        assert!(
            should_fallback,
            "connection reset should trigger fallback"
        );
    }

    /// Regression guard against Pitfall 1: SSE stream read timeout is NOT a transport
    /// failure — the server was reachable and responded. Bare `"timed out"` is NOT in
    /// the allowlist; only the scoped `"operation timed out"` marker is present.
    #[test]
    fn test_classify_sse_read_timeout_not_transport() {
        let err = anyhow!("SSE stream read timed out after 60s");
        let (should_retry, should_fallback) = AgentLoop::classify_llm_error(&err);
        assert!(should_retry, "SSE read timeout should be retryable");
        assert!(
            !should_fallback,
            "SSE stream read timeout should not trigger fallback (Pitfall 1 guard)"
        );
    }

    /// Regression: production errors are wrapped via
    /// `.context("Streaming LLM call failed")` in agent_loop.rs:1030. Plain
    /// `err.to_string()` returns only the outermost context — hiding the
    /// inner `(400 Bad Request)` — so the classifier must use alternate
    /// Display (`{err:#}`) to walk the full chain. This test exercises that
    /// exact wrap-and-extract path.
    #[test]
    fn test_classify_walks_anyhow_context_chain() {
        use anyhow::Context;
        let inner: anyhow::Error =
            anyhow!("Streaming chat completion failed (400 Bad Request): {{}}");
        let wrapped = Err::<(), _>(inner)
            .context("Streaming LLM call failed")
            .unwrap_err();
        let (should_retry, should_fallback) = AgentLoop::classify_llm_error(&wrapped);
        assert!(
            !should_retry && should_fallback,
            "wrapped 400 must trigger fallback (no retry); plain to_string() would lose the inner (400 …) frame"
        );
    }

    /// Regression: production errors use the `(NNN Reason)` format from
    /// `bail!("Streaming chat completion failed ({status}): {body}")` in
    /// client.rs:170. Earlier versions of `classify_llm_error` only matched
    /// the synthetic `"status: NNN"` format, so the fallback path silently
    /// never fired in production. This test locks the production format.
    #[test]
    fn test_classify_production_error_format() {
        let err = anyhow!(
            "Streaming chat completion failed (400 Bad Request): \
             {{\"error\":{{\"message\":\"openai/sgpt-4o-mini is not a valid model ID\"}}}}"
        );
        let (should_retry, should_fallback) = AgentLoop::classify_llm_error(&err);
        assert!(!should_retry, "400 should skip retry");
        assert!(should_fallback, "400 should trigger fallback");

        let err_404 = anyhow!("Streaming chat completion failed (404 Not Found): {{}}");
        let (_, fb_404) = AgentLoop::classify_llm_error(&err_404);
        assert!(fb_404, "404 in production format should trigger fallback");

        let err_429 = anyhow!("Streaming chat completion failed (429 Too Many Requests): {{}}");
        let (retry_429, fb_429) = AgentLoop::classify_llm_error(&err_429);
        assert!(retry_429 && fb_429, "429 should retry AND fall back");

        let err_500 = anyhow!("Streaming chat completion failed (500 Internal Server Error): {{}}");
        let (retry_500, fb_500) = AgentLoop::classify_llm_error(&err_500);
        assert!(retry_500 && fb_500, "500 should retry AND fall back");
    }

    #[test]
    fn test_fallback_activated_prevents_refire() {
        let primary = AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "http://primary".to_string(),
            "key1".to_string(),
            "model1",
        ));
        let fallback = AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "http://fallback".to_string(),
            "key2".to_string(),
            "model2",
        ));
        let registry = Arc::new(RwLock::new(ironhermes_tools::ToolRegistry::new()));
        let mut agent = AgentLoop::new(primary, registry, 10).with_fallback(fallback);

        assert!(!agent.fallback_activated);
        assert!(agent.fallback_client.is_some());

        // Manually activate fallback (as the run() loop would)
        if let Some(fb) = agent.fallback_client.take() {
            agent.client = fb;
            agent.fallback_activated = true;
        }

        assert!(agent.fallback_activated);
        assert!(
            agent.fallback_client.is_none(),
            "take() should leave None — one-shot guarantee"
        );
    }

    #[test]
    fn test_classify_500_error() {
        let err = anyhow!("HTTP request failed with status: 500 Internal Server Error");
        let (should_retry, should_fallback) = AgentLoop::classify_llm_error(&err);
        assert!(should_retry, "500 should be retryable");
        assert!(should_fallback, "500 should trigger fallback");
    }
}

// ---------------------------------------------------------------------------
// Tests: Phase 18 Plan 06 pre-chat compression wiring
// ---------------------------------------------------------------------------

#[cfg(test)]
mod plan_18_06_tests {
    use super::*;
    use crate::context_engine::{CompressionMode, CompressionOutcome, ContextError};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    /// Recording fake engine — captures call order without real LLM work.
    struct RecordingEngine {
        threshold: f32,
        compress_calls: Arc<AtomicUsize>,
        pressure_calls: Arc<AtomicUsize>,
        event_log: Arc<std::sync::Mutex<Vec<&'static str>>>,
    }

    #[async_trait]
    impl ContextEngine for RecordingEngine {
        async fn compress(
            &self,
            _messages: &mut Vec<ChatMessage>,
            _stats: ContextStats,
        ) -> Result<CompressionOutcome, ContextError> {
            self.compress_calls.fetch_add(1, AtomicOrdering::SeqCst);
            self.event_log.lock().unwrap().push("compress");
            Ok(CompressionOutcome {
                compressed: true,
                ..CompressionOutcome::default()
            })
        }
        fn threshold(&self) -> f32 {
            self.threshold
        }
        fn mode(&self) -> CompressionMode {
            CompressionMode::Hard
        }
        async fn check_pressure(&self, _stats: &ContextStats) -> bool {
            self.pressure_calls.fetch_add(1, AtomicOrdering::SeqCst);
            self.event_log.lock().unwrap().push("check_pressure");
            false
        }
    }

    fn make_engine(
        threshold: f32,
    ) -> (
        Arc<RecordingEngine>,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
        Arc<std::sync::Mutex<Vec<&'static str>>>,
    ) {
        let c = Arc::new(AtomicUsize::new(0));
        let p = Arc::new(AtomicUsize::new(0));
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let engine = Arc::new(RecordingEngine {
            threshold,
            compress_calls: c.clone(),
            pressure_calls: p.clone(),
            event_log: log.clone(),
        });
        (engine, c, p, log)
    }

    fn make_agent_with_engine(engine: Arc<dyn ContextEngine>, ctx_len: usize) -> AgentLoop {
        let client = AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "http://localhost".to_string(),
            "".to_string(),
            "test",
        ));
        let registry = Arc::new(RwLock::new(ironhermes_tools::ToolRegistry::new()));
        AgentLoop::new(client, registry, 10)
            .with_context_engine(engine, ctx_len)
            .with_session_id("sess-test")
    }

    fn filler_messages(n: usize) -> Vec<ChatMessage> {
        (0..n)
            .map(|i| ChatMessage::user(format!("message {i} ").repeat(20)))
            .collect()
    }

    /// Agent loop compresses at >= threshold ratio and runs check_pressure below it.
    #[tokio::test]
    async fn dual_mode_thresholds() {
        // Below threshold: filler messages keep ratio < 0.5.
        let (engine, compress_calls, pressure_calls, _log) = make_engine(0.5);
        let mut agent = make_agent_with_engine(engine.clone(), 1_000_000);
        let mut messages = filler_messages(5);
        agent.pre_chat_compress(&mut messages).await;
        assert_eq!(
            compress_calls.load(AtomicOrdering::SeqCst),
            0,
            "below threshold must not compress"
        );
        assert_eq!(
            pressure_calls.load(AtomicOrdering::SeqCst),
            1,
            "below threshold must run pressure check"
        );

        // Above threshold: tiny ctx_len forces ratio > 0.5.
        let (engine2, compress_calls2, pressure_calls2, _log2) = make_engine(0.5);
        let mut agent2 = make_agent_with_engine(engine2.clone(), 100);
        let mut msgs2 = filler_messages(20);
        agent2.pre_chat_compress(&mut msgs2).await;
        assert_eq!(
            compress_calls2.load(AtomicOrdering::SeqCst),
            1,
            "above threshold must compress"
        );
        assert_eq!(
            pressure_calls2.load(AtomicOrdering::SeqCst),
            0,
            "above threshold must NOT also run pressure check"
        );
    }

    /// Transient pressure message is drained and injected as a system message
    /// before compression runs; subsequent drain returns None (consumed).
    #[tokio::test]
    async fn agent_loop_injects_transient_pressure_message() {
        let tracker = Arc::new(PressureTracker::new());
        // Seed a transient via check_and_maybe_emit: ratio above 85% of threshold.
        let fired = tracker
            .check_and_maybe_emit("sess-test", 0.5, 50, 100, "hard", None)
            .await;
        assert!(fired, "precondition: tracker must fire the transient");

        let (engine, _c, _p, _log) = make_engine(0.5);
        let mut agent =
            make_agent_with_engine(engine, 1_000_000).with_pressure_tracker(tracker.clone());

        let mut messages = filler_messages(3);
        let before_len = messages.len();
        agent.pre_chat_compress(&mut messages).await;

        assert_eq!(messages.len(), before_len + 1, "transient must be appended");
        let injected = messages.last().unwrap();
        assert_eq!(injected.role, ironhermes_core::Role::System);
        assert!(
            injected
                .content_text()
                .unwrap_or("")
                .contains("CONTEXT PRESSURE HIGH")
        );

        // Transient is one-shot — subsequent drain returns None.
        assert!(tracker.take_transient("sess-test").is_none());
    }

    /// Belt-and-suspenders rollback: a faulty engine that mutates `messages`
    /// AND returns Err must not leak corruption to the caller. The agent loop
    /// snapshots the pre-compress vec and restores it on Err.
    #[tokio::test]
    async fn pre_chat_compress_rolls_back_on_engine_error() {
        struct CorruptingEngine;
        #[async_trait]
        impl ContextEngine for CorruptingEngine {
            async fn compress(
                &self,
                messages: &mut Vec<ChatMessage>,
                _stats: ContextStats,
            ) -> Result<CompressionOutcome, ContextError> {
                // Mutate then fail — exactly the bug class the snapshot guards.
                messages.clear();
                messages.push(ChatMessage::system("CORRUPTED"));
                Err(ContextError::OrphanedToolPair)
            }
            fn threshold(&self) -> f32 {
                0.0 // always above threshold so compress() runs
            }
            fn mode(&self) -> CompressionMode {
                CompressionMode::Hard
            }
        }

        let engine: Arc<dyn ContextEngine> = Arc::new(CorruptingEngine);
        let mut agent = make_agent_with_engine(engine, 100);
        let mut messages = filler_messages(5);
        let snapshot = messages.clone();

        agent.pre_chat_compress(&mut messages).await;

        assert_eq!(
            messages.len(),
            snapshot.len(),
            "messages restored after engine returned Err"
        );
        for (a, b) in messages.iter().zip(snapshot.iter()) {
            assert_eq!(a.content_text(), b.content_text());
        }
        assert!(
            messages
                .iter()
                .all(|m| { m.content_text().map(|t| t != "CORRUPTED").unwrap_or(true) }),
            "corruption sentinel must not appear in restored vec"
        );
    }

    /// Compression fires BEFORE any client call — since pre_chat_compress returns
    /// before the LLM is touched, its completion is a strict happens-before for chat.
    /// Verified by event_log ordering: compress recorded, no chat marker yet.
    #[tokio::test]
    async fn agent_loop_compression_before_chat() {
        let (engine, _c, _p, log) = make_engine(0.5);
        let mut agent = make_agent_with_engine(engine, 100);
        let mut messages = filler_messages(20);
        agent.pre_chat_compress(&mut messages).await;
        let final_log = log.lock().unwrap().clone();
        assert_eq!(
            final_log,
            vec!["compress"],
            "compress must be the pre-chat event"
        );
    }
}

// ---------------------------------------------------------------------------
// Tests: Phase 21.5 memory provider tool wiring
// ---------------------------------------------------------------------------

#[cfg(test)]
mod memory_provider_wiring_tests {
    use super::*;

    fn make_agent() -> AgentLoop {
        let client = AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "http://localhost".to_string(),
            "".to_string(),
            "mock-model",
        ));
        let registry = Arc::new(RwLock::new(ironhermes_tools::ToolRegistry::new()));
        AgentLoop::new(client, registry, 4)
    }

    #[tokio::test]
    async fn memory_provider_tool_names_populated_from_manager() {
        // Verify that the memory_provider_tool_names field is empty
        // when no memory_manager is configured.
        let agent = make_agent();
        assert!(
            agent.memory_provider_tool_names.is_empty(),
            "should be empty when no memory_manager is configured"
        );
    }

    #[test]
    fn memory_recall_wiring_regression() {
        // Static grep: verify the memory_recall wiring exists in agent_loop.rs
        let source = include_str!("agent_loop.rs");
        assert!(
            source.contains("memory_provider_tool_names"),
            "agent_loop.rs must contain memory_provider_tool_names field"
        );
        assert!(
            source.contains("get_tool_schemas().await"),
            "agent_loop.rs must call get_tool_schemas on memory_manager in run()"
        );
        assert!(
            source.contains("memory_provider_tool_names.contains"),
            "agent_loop.rs must intercept tools by name in execute_tool_call()"
        );
    }

    // -----------------------------------------------------------------------
    // Phase 25 Plan 03 Task 3: with_intercepts builder + session_search migration tests
    // -----------------------------------------------------------------------

    /// Test: Build AgentLoop, call with_intercepts with a state_store; assert session_search
    /// is registered via dispatch_intercepts.
    #[tokio::test]
    async fn agent_loop_with_intercepts_registers_session_search() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("state.db");
        let store = ironhermes_state::StateStore::new(db_path).unwrap();
        let state = std::sync::Arc::new(std::sync::Mutex::new(store));

        let client = AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "http://localhost".to_string(),
            "".to_string(),
            "mock-model",
        ));
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let agent = AgentLoop::new(client, registry, 1).with_intercepts(
            None,
            Some(state.clone()),
            None,
            None,
            None,
        );

        let reg = agent.registry.read().await;
        let result = reg
            .dispatch_intercepts("session_search", serde_json::json!({"query": "test"}))
            .await;
        assert!(
            result.is_some(),
            "session_search must be registered as intercept via with_intercepts"
        );
    }

    /// Test: Build AgentLoop, call with_intercepts with a todo_state; assert both
    /// todo_write and todo_read are registered.
    #[tokio::test]
    async fn agent_loop_with_intercepts_registers_todo_pair() {
        let client = AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "http://localhost".to_string(),
            "".to_string(),
            "mock-model",
        ));
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let todo_state: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let agent = AgentLoop::new(client, registry, 1).with_intercepts(
            None,
            None,
            None,
            Some(todo_state),
            None,
        );

        let reg = agent.registry.read().await;
        assert!(
            reg.dispatch_intercepts("todo_write", serde_json::json!({"items": []}))
                .await
                .is_some(),
            "todo_write must be registered via with_intercepts"
        );
        assert!(
            reg.dispatch_intercepts("todo_read", serde_json::json!({}))
                .await
                .is_some(),
            "todo_read must be registered via with_intercepts"
        );
    }

    /// Source-text invariant: session_search schema injection block is deleted (D-14).
    /// The push call that injects session_search_schema() into tool_schemas must be absent.
    #[test]
    fn agent_loop_session_search_schema_injection_removed() {
        let source = include_str!("agent_loop.rs");
        // Build the forbidden string at runtime to avoid it appearing in test source.
        let forbidden_push: String = [
            "tool_schemas.push(crate::session_search::",
            "session_search_schema())",
        ]
        .concat();
        assert!(
            !source.contains(&forbidden_push),
            "D-14: session_search schema push injection must be deleted from run(); \
             schema flows through registry.get_definitions() via register_intercepted. \
             Found injection at unexpected location in agent_loop.rs."
        );
    }

    /// Source-text invariant: hardcoded session_search dispatch block is deleted (D-12).
    /// The name-equality check for session_search in execute_tool_call must be absent.
    #[test]
    fn agent_loop_session_search_match_block_removed() {
        let source = include_str!("agent_loop.rs");
        // Build the forbidden string at runtime to avoid it appearing in test source.
        let forbidden_match: String = ["if name == ", "\"session_search\""].concat();
        assert!(
            !source.contains(&forbidden_match),
            "D-12: hardcoded session_search name-equality block must be deleted \
             from execute_tool_call(); replaced by dispatch_intercepts call. \
             Found hardcoded match at unexpected location in agent_loop.rs."
        );
    }

    /// D-26 Test 3 live-handler version: all 6 intercepted tools appear exactly once
    /// in get_definitions(None) after wiring via with_intercepts.
    #[tokio::test]
    async fn intercepted_no_duplicate_with_real_handlers() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("state.db");
        let store = ironhermes_state::StateStore::new(db_path).unwrap();
        let state = std::sync::Arc::new(std::sync::Mutex::new(store));
        let todo_state: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let client = AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "http://localhost".to_string(),
            "".to_string(),
            "mock-model",
        ));
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let agent = AgentLoop::new(client, registry, 1).with_intercepts(
            None,
            Some(state),
            None,
            Some(todo_state),
            None,
        );

        let reg = agent.registry.read().await;
        let schemas = reg.get_definitions(None);
        let names: Vec<String> = schemas.iter().map(|s| s.function.name.clone()).collect();

        // session_search, todo_write, todo_read should be wired (memory needs handle)
        for expected in &["session_search", "todo_write", "todo_read"] {
            let count = names.iter().filter(|n| n.as_str() == *expected).count();
            assert_eq!(
                count, 1,
                "intercepted tool '{}' must appear exactly once in get_definitions(None); \
                 all: {:?}",
                expected, names
            );
        }
    }

    // -----------------------------------------------------------------------
    // Phase 25.1 D-03/D-17: browser_session field + with_browser_session builder
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn agent_loop_with_browser_session_sets_field() {
        use ironhermes_tools::browser_session::BrowserSession;
        let arc = std::sync::Arc::new(tokio::sync::Mutex::new(None::<BrowserSession>));
        let client = AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "http://localhost".to_string(),
            "".to_string(),
            "mock-model",
        ));
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let agent = AgentLoop::new(client, registry, 1).with_browser_session(arc.clone());
        assert!(
            agent.browser_session.is_some(),
            "Phase 25.1 D-17: with_browser_session MUST populate the field"
        );
    }

    #[test]
    fn agent_loop_browser_session_wiring_invariants() {
        let source = include_str!("agent_loop.rs");
        assert!(
            source.contains("browser_session: Option<"),
            "AgentLoop MUST have browser_session field"
        );
        assert!(
            source.contains("pub fn with_browser_session"),
            "AgentLoop MUST expose with_browser_session builder"
        );
        assert!(
            source.contains("browser_session: None"),
            "AgentLoop::new MUST initialize browser_session to None"
        );
    }
}

// ---------------------------------------------------------------------------
// Phase 32.1 Plan 02: activity tracker types, accessors, and tests
// ---------------------------------------------------------------------------

/// Classifies the most-recent observable event inside `AgentLoop::run`.
/// Used by the cron runner to distinguish idle LLM-wait from active tool execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityKind {
    /// The agent just issued (or is waiting on) an LLM API call.
    /// This is also the sentinel "no activity yet" value — it is the first
    /// observable event in any `run()` so initialising to it is correct.
    ApiCall,
    /// A tool call was dispatched (or is in-flight).
    ToolCall,
    /// A streamed content delta was received from the LLM.
    StreamToken,
}

/// Snapshot of the most-recent observable agent activity.
/// Returned by `AgentLoop::activity_summary()` and consumed by the cron runner
/// to implement the inactivity-tracked timeout (CONTEXT.md §Timeout & resilience).
#[derive(Debug, Clone)]
pub struct ActivitySummary {
    /// Seconds elapsed since the last activity bump.
    /// Computed as `Instant::now() - last_bump` at the moment of the call — not
    /// a stored elapsed value — so successive calls yield monotonically advancing
    /// readings without any additional state.
    pub seconds_since: f64,
    /// Kind of the most-recent observable event.
    pub last_kind: ActivityKind,
    /// Name of the tool currently in-flight, if any.
    /// `None` when no tool call is currently being dispatched.
    pub current_tool: Option<String>,
}

// ---------------------------------------------------------------------------
// Phase 25.3 Plan 9 (D-T-1 / D-T-3): trajectory wireup tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod trajectory_wireup_tests {
    use super::*;
    use ironhermes_trajectory::ImpactLevel;

    #[test]
    fn classify_impact_level_known_categories() {
        assert_eq!(classify_impact_level("read_file"), ImpactLevel::Read);
        assert_eq!(classify_impact_level("web_extract"), ImpactLevel::Read);
        assert_eq!(classify_impact_level("write_file"), ImpactLevel::Write);
        assert_eq!(classify_impact_level("patch"), ImpactLevel::Write);
        assert_eq!(classify_impact_level("terminal"), ImpactLevel::SystemChange);
        assert_eq!(
            classify_impact_level("execute_code"),
            ImpactLevel::SystemChange
        );
    }

    #[test]
    fn classify_impact_level_mcp_default_system_change() {
        assert_eq!(
            classify_impact_level("mcp__github_create_issue"),
            ImpactLevel::SystemChange
        );
        assert_eq!(
            classify_impact_level("mcp_filesystem_write"),
            ImpactLevel::SystemChange
        );
    }

    #[test]
    fn classify_impact_level_unknown_default_write() {
        assert_eq!(
            classify_impact_level("brand_new_tool_2030"),
            ImpactLevel::Write
        );
        assert_eq!(classify_impact_level(""), ImpactLevel::Write);
    }

    #[test]
    fn agent_loop_trajectory_writer_default_none() {
        let client = AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "http://localhost".to_string(),
            "".to_string(),
            "mock-model",
        ));
        let registry = Arc::new(RwLock::new(ironhermes_tools::ToolRegistry::new()));
        let agent = AgentLoop::new(client, registry, 4);
        assert!(
            agent.trajectory_writer.is_none(),
            "Phase 25.3 D-T-3: AgentLoop::new MUST default trajectory_writer to None"
        );
    }
}

// ---------------------------------------------------------------------------
// Phase 32.1 Plan 02 Task 1: activity tracker unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod activity_tracker_tests {
    use super::*;
    use std::time::Duration;

    /// Test 1: `AgentLoop::for_tests()` constructs an instance whose
    /// `activity_summary()` returns a fresh `ActivitySummary` with
    /// `seconds_since` < 1.0, `last_kind == ActivityKind::ApiCall` (sentinel),
    /// and `current_tool == None`.
    #[test]
    fn activity_summary_initial_state() {
        let agent = AgentLoop::for_tests();
        let summary = agent.activity_summary();
        assert!(
            summary.seconds_since < 1.0,
            "seconds_since should be near-zero after construction, got {}",
            summary.seconds_since
        );
        assert_eq!(
            summary.last_kind,
            ActivityKind::ApiCall,
            "initial sentinel kind should be ApiCall"
        );
        assert_eq!(
            summary.current_tool, None,
            "initial current_tool should be None"
        );
    }

    /// Test 2: Calling `activity_summary()` twice with a 50ms sleep between
    /// them yields a second `seconds_since` greater than the first, proving
    /// the timestamp is computed live from `Instant::now()` rather than a
    /// frozen elapsed value.
    #[test]
    fn activity_summary_seconds_since_advances() {
        let agent = AgentLoop::for_tests();
        let first = agent.activity_summary().seconds_since;
        std::thread::sleep(Duration::from_millis(50));
        let second = agent.activity_summary().seconds_since;
        assert!(
            second > first,
            "second seconds_since ({second}) must be > first ({first}) after 50ms sleep"
        );
    }

    /// Test 3: `interrupt("inactivity timeout")` cancels the agent's underlying
    /// `CancellationToken`, which is reflected by `is_cancelled()` returning `true`.
    #[test]
    fn interrupt_cancels_token() {
        let token = CancellationToken::new();
        let agent = AgentLoop::for_tests().with_cancellation_token(token.clone());
        assert!(!agent.is_cancelled(), "should not be cancelled before interrupt");
        agent.interrupt("inactivity timeout");
        assert!(
            agent.is_cancelled(),
            "is_cancelled() should return true after interrupt()"
        );
        assert!(
            token.is_cancelled(),
            "the underlying CancellationToken should also be cancelled"
        );
    }

    /// Test 4: `mark_activity_for_test` bumps the tracker — verified by reading
    /// `activity_summary()` afterwards and asserting the updated kind and tool name.
    #[test]
    fn mark_activity_for_test_updates_summary() {
        let agent = AgentLoop::for_tests();
        // Start: sentinel ApiCall / None
        assert_eq!(agent.activity_summary().last_kind, ActivityKind::ApiCall);
        assert_eq!(agent.activity_summary().current_tool, None);

        // Bump to ToolCall with a named tool
        agent.mark_activity_for_test(ActivityKind::ToolCall, Some("web_read".into()));
        let summary = agent.activity_summary();
        assert_eq!(
            summary.last_kind,
            ActivityKind::ToolCall,
            "last_kind should reflect the bumped kind"
        );
        assert_eq!(
            summary.current_tool,
            Some("web_read".to_string()),
            "current_tool should hold the in-flight tool name"
        );
        assert!(
            summary.seconds_since < 1.0,
            "seconds_since should be near-zero just after bump, got {}",
            summary.seconds_since
        );

        // Post-dispatch reset: clear current_tool
        agent.mark_activity_for_test(ActivityKind::ToolCall, None);
        let after = agent.activity_summary();
        assert_eq!(after.last_kind, ActivityKind::ToolCall);
        assert_eq!(after.current_tool, None, "current_tool should be cleared after reset");
    }

    /// Test 5: `interrupt()` on an agent without a CancellationToken is a no-op
    /// (does not panic). `is_cancelled()` returns `false`.
    #[test]
    fn interrupt_without_token_is_noop() {
        let agent = AgentLoop::for_tests(); // no with_cancellation_token()
        agent.interrupt("should not panic");
        assert!(
            !agent.is_cancelled(),
            "is_cancelled() should be false when no token is configured"
        );
    }
}

// ---------------------------------------------------------------------------
// Phase 32.1 Plan 02 Task 2: run-wiring tests
// Verify that mark_activity is wired at the three observable sites in run()
// and execute_tool_call().
// ---------------------------------------------------------------------------

#[cfg(test)]
mod activity_tracker_run_wiring {
    use super::*;
    use async_trait::async_trait;
    use ironhermes_core::ToolSchema;
    use ironhermes_tools::{Tool, ToolRegistry};
    use std::sync::Arc;
    use tokio::sync::RwLock;

    // -----------------------------------------------------------------------
    // Test 1 (source): mark_activity(ActivityKind::ApiCall) is wired in run()
    // -----------------------------------------------------------------------

    /// Source-text assertion: `ActivityKind::ApiCall` appears in run() body
    /// (at the API-call bump site). Accepts at least 2 occurrences: one in
    /// `new()` initialisation and at least one in the run loop.
    #[test]
    fn source_has_api_call_site_in_run() {
        let src = include_str!("agent_loop.rs");
        let count = src.matches("ActivityKind::ApiCall").count();
        assert!(
            count >= 2,
            "Expected at least 2 occurrences of ActivityKind::ApiCall (init + run site), found {}",
            count
        );
    }

    /// Source-text assertion: `ActivityKind::ToolCall` appears at least twice
    /// inside execute_tool_call (pre-dispatch bump + post-dispatch reset).
    #[test]
    fn source_has_tool_call_sites() {
        let src = include_str!("agent_loop.rs");
        let count = src.matches("ActivityKind::ToolCall").count();
        assert!(
            count >= 2,
            "Expected at least 2 occurrences of ActivityKind::ToolCall (pre + post dispatch), found {}",
            count
        );
    }

    /// Source-text assertion: `ActivityKind::StreamToken` appears in
    /// call_llm_streaming (ContentDelta arm).
    #[test]
    fn source_has_stream_token_site() {
        let src = include_str!("agent_loop.rs");
        let count = src.matches("ActivityKind::StreamToken").count();
        assert!(
            count >= 1,
            "Expected at least 1 occurrence of ActivityKind::StreamToken, found {}",
            count
        );
    }

    /// Source-text assertion: `self.mark_activity(` appears at least 4 times
    /// (1 ApiCall site + 2 ToolCall sites + 1 StreamToken site).
    #[test]
    fn source_has_minimum_mark_activity_call_count() {
        let src = include_str!("agent_loop.rs");
        let count = src.matches("self.mark_activity(").count();
        assert!(
            count >= 4,
            "Expected at least 4 self.mark_activity( calls, found {}",
            count
        );
    }

    // -----------------------------------------------------------------------
    // Test 2 (behavioral): execute_tool_call bumps ToolCall kind
    // -----------------------------------------------------------------------

    struct TrackingMockTool;

    #[async_trait]
    impl Tool for TrackingMockTool {
        fn name(&self) -> &str { "track_mock" }
        fn toolset(&self) -> &str { "test" }
        fn description(&self) -> &str { "tracking mock tool" }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                "track_mock",
                "tracking mock",
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("tracked".to_string())
        }
    }

    fn make_tool_call(name: &str) -> ironhermes_core::ToolCall {
        ironhermes_core::ToolCall {
            id: "tc-wiring-test".to_string(),
            call_type: "function".to_string(),
            function: ironhermes_core::FunctionCall {
                name: name.to_string(),
                arguments: "{}".to_string(),
            },
        }
    }

    fn make_agent_with_tool(tool: Box<dyn ironhermes_tools::Tool>) -> AgentLoop {
        let mut registry = ToolRegistry::new();
        registry.register(tool);
        let client = AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "http://localhost:11434",
            "test-key",
            "test-model",
        ));
        AgentLoop::new(client, Arc::new(RwLock::new(registry)), 1)
    }

    /// Behavioral test: calling execute_tool_call bumps last_kind to ToolCall
    /// and clears current_tool to None after dispatch completes.
    #[tokio::test]
    async fn execute_tool_call_bumps_activity_tracker() {
        let agent = make_agent_with_tool(Box::new(TrackingMockTool));
        // Pre-call: sentinel kind is ApiCall
        assert_eq!(agent.activity_summary().last_kind, ActivityKind::ApiCall);

        let tc = make_tool_call("track_mock");
        let result = agent.execute_tool_call(&tc).await;
        assert_eq!(result, "tracked", "tool should have executed successfully");

        // Post-call: kind should be ToolCall (post-dispatch reset bumps ToolCall+None)
        let summary = agent.activity_summary();
        assert_eq!(
            summary.last_kind,
            ActivityKind::ToolCall,
            "last_kind should be ToolCall after execute_tool_call"
        );
        assert_eq!(
            summary.current_tool,
            None,
            "current_tool should be None after post-dispatch reset"
        );
    }

    /// Test: after a successful run() that does not need a live LLM, the bounded
    /// seconds_since is reasonable (< 5.0s), confirming the tracker was updated
    /// near the end of the run (not left at construction time). We test this by
    /// bumping the tracker manually and reading back a fresh summary.
    #[test]
    fn activity_summary_seconds_since_bounded_after_bump() {
        let agent = AgentLoop::for_tests();
        // Simulate a tracker bump as run() would do
        agent.mark_activity_for_test(ActivityKind::ApiCall, None);
        let summary = agent.activity_summary();
        assert!(
            summary.seconds_since < 5.0,
            "seconds_since after a fresh bump must be < 5.0s, got {}",
            summary.seconds_since
        );
    }
}

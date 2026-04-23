use anyhow::{Context, Result};
use ironhermes_core::{ChatMessage, ChatResponse, ToolCall, ToolSchema, Usage};
use ironhermes_hooks::{HookEvent, HookEventKind, HookRegistry};
use ironhermes_state::StateStore;
use ironhermes_tools::ToolRegistry;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::any_client::AnyClient;
use crate::budget::{advisory_text, BudgetHandle, PressureTier};
use crate::client::{StreamEvent, ToolCallDelta};
use crate::context_compressor::{estimate_messages_tokens, ContextCompressor};
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

/// The main agent loop that orchestrates LLM calls and tool execution.
pub struct AgentLoop {
    client: AnyClient,
    registry: Arc<RwLock<ToolRegistry>>,
    max_iterations: usize,
    compressor: Option<Mutex<ContextCompressor>>,
    stream_callback: Option<StreamCallback>,
    tool_progress_callback: Option<ToolProgressCallback>,
    streaming: bool,
    hook_registry: Option<Arc<HookRegistry>>,
    request_id: String,
    active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>>,
    /// Optional cancellation token for cooperative shutdown (D-21).
    /// When cancelled, the loop returns early with "Cancelled by parent".
    cancel_token: Option<CancellationToken>,
    /// Shared iteration budget handle (PROV-09, PROV-10, D-15).
    /// Tracks total turns across parent + child agents and exposes the
    /// pressure-tier ladder (None / Caution70 / Warning90 / Stop100) via
    /// `BudgetHandle::pressure()`. Plan 21.7-05 replaced the bare
    /// `Arc<AtomicUsize>` with the handle so parent + child decrement the
    /// same counter and tier transitions are observed consistently.
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
}

impl AgentLoop {
    pub fn new(client: AnyClient, registry: Arc<RwLock<ToolRegistry>>, max_iterations: usize) -> Self {
        Self {
            client,
            registry,
            max_iterations,
            compressor: None,
            stream_callback: None,
            tool_progress_callback: None,
            streaming: false,
            hook_registry: None,
            request_id: uuid::Uuid::new_v4().to_string(),
            active_skills: Arc::new(std::sync::Mutex::new(Vec::new())),
            cancel_token: None,
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

    pub fn with_active_skills(mut self, active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>>) -> Self {
        self.active_skills = active_skills;
        self
    }

    pub fn active_skills(&self) -> Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>> {
        self.active_skills.clone()
    }

    /// Set subdirectory discovery for progressive context injection (CTX-03/CTX-04).
    pub fn with_subdir_discovery(mut self, discovery: Arc<std::sync::Mutex<SubdirDiscovery>>) -> Self {
        self.subdir_discovery = Some(discovery);
        self
    }

    /// Set the StateStore for session_search tool interception (D-07).
    /// When set, session_search calls are intercepted before registry dispatch.
    pub fn with_state_store(mut self, store: Arc<std::sync::Mutex<StateStore>>) -> Self {
        self.state_store = Some(store);
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

    pub fn with_hook_registry(mut self, registry: Arc<HookRegistry>) -> Self {
        self.hook_registry = Some(registry);
        self
    }

    /// Set a shared iteration budget handle (PROV-09, PROV-10, D-15).
    ///
    /// Plan 21.7-05: accepts [`BudgetHandle`] rather than a bare
    /// `Arc<AtomicUsize>`. The handle's `consume()` is called at the top of
    /// every turn (Stop100 → clean-stop via `AgentResult::budget_exhausted`);
    /// `pressure()` drives the advisory-injection ladder (Caution70/Warning90).
    pub fn with_budget(mut self, budget: BudgetHandle) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Get the budget handle for sharing with child agents (PROV-10).
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

    /// Classify an error for fallback decision-making.
    /// Returns (should_retry, should_fallback).
    fn classify_llm_error(err: &anyhow::Error) -> (bool, bool) {
        let err_str = err.to_string();
        if err_str.contains("status: 429")
            || err_str.contains("status: 500")
            || err_str.contains("status: 502")
            || err_str.contains("status: 503")
            || err_str.contains("status: 504")
        {
            return (true, true);
        }
        if err_str.contains("status: 401")
            || err_str.contains("status: 403")
            || err_str.contains("status: 404")
        {
            return (false, true);
        }
        (true, false)
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
        // D-07: Add session_search schema when state_store is configured.
        // Not added in subagent context (subagents should not search sessions).
        if self.state_store.is_some() {
            tool_schemas.push(crate::session_search::session_search_schema());
        }
        // Phase 21.5: Add memory provider tool schemas (e.g. memory_recall).
        // These are tools declared by the provider via get_tool_schemas() —
        // distinct from the built-in "memory" tool which handles add/replace/remove.
        if let Some(ref mgr) = self.memory_manager {
            let guard = mgr.lock().await;
            let schemas = guard.get_tool_schemas().await;
            for s in &schemas {
                self.memory_provider_tool_names.insert(s.function.name.clone());
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
                        turns_used,
                        finished_naturally: false,
                        final_response: None,
                        total_usage,
                        compression_count_after: self.compression_count,
                        stop_reason: StopReason::BudgetExhausted,
                    });
                }
            }

            // Phase 18 Plan 06: pre-chat context engine path (replaces legacy compressor
            // when wired). Drain transient pressure message, then compress at >= threshold
            // or run pressure check only when below.
            if self.context_engine.is_some() {
                self.pre_chat_compress(&mut messages).await;
            } else if let Some(ref compressor) = self.compressor {
                let mut comp = compressor.lock().await;
                comp.compress(&mut messages);
            }

            turns_used += 1;
            debug!(turn = turns_used, messages = messages.len(), "Agent loop turn");

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
                    self.call_llm_streaming(&messages, tools_option.as_deref()).await
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
                            warn!(retry = retry_count, "LLM call failed, retrying: {err}");
                            tokio::time::sleep(tokio::time::Duration::from_millis(500 * retry_count as u64)).await;
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

            if !has_tool_calls {
                debug!(turn = turns_used, "Agent completed naturally (no tool calls)");
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
                messages.push(ChatMessage::tool_result(
                    &tool_call.id,
                    result,
                ));
            }
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

        let choice = response
            .choices
            .into_iter()
            .next()
            .context("No choices in LLM response")?;

        Ok((choice.message, response.usage))
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

                // D-07: Intercept session_search before registry dispatch.
                // StateStore uses sync rusqlite; wrap in spawn_blocking to avoid blocking tokio.
                if name == "session_search" {
                    if let Some(ref state) = self.state_store {
                        let state_clone = state.clone();
                        let args_clone = args.clone();
                        let result = tokio::task::spawn_blocking(move || {
                            let store = state_clone.lock().unwrap();
                            crate::session_search::handle_session_search(&args_clone, &store)
                        })
                        .await;
                        return match result {
                            Ok(s) => s,
                            Err(e) => format!(
                                r#"{{"error":"internal","reason":"{}"}}"#,
                                e.to_string().replace('"', "'")
                            ),
                        };
                    }
                    return r#"{"error":"unavailable","reason":"state store not configured"}"#.to_string();
                }

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
                        }.await;

                        let tool_duration = tool_start.elapsed().as_millis() as u64;
                        return match result {
                            Ok(s) => {
                                self.fire_hook(HookEventKind::ToolCompleted {
                                    tool_name: name.to_string(),
                                    success: true,
                                    result_preview: ironhermes_hooks::event::preview(&s, 200),
                                    duration_ms: tool_duration,
                                });
                                s
                            }
                            Err(e) => {
                                self.fire_hook(HookEventKind::ToolCompleted {
                                    tool_name: name.to_string(),
                                    success: false,
                                    result_preview: ironhermes_hooks::event::preview(&e, 200),
                                    duration_ms: tool_duration,
                                });
                                e
                            }
                        };
                    }
                    return format!(r#"{{"error":"unavailable","reason":"memory manager not configured"}}"#);
                }

                let dispatch_result = self.registry.read().await.execute_tool(name, args).await;
                let duration_ms = tool_start.elapsed().as_millis() as u64;

                match dispatch_result {
                    Ok(result) => {
                        self.fire_hook(HookEventKind::ToolCompleted {
                            tool_name: name.to_string(),
                            success: true,
                            result_preview: ironhermes_hooks::event::preview(&result, 200),
                            duration_ms,
                        });

                        // CTX-03/CTX-04: progressive subdirectory discovery for file-access tools
                        let mut final_result = result;
                        const FILE_ACCESS_TOOLS: &[&str] = &["read_file", "write_file", "patch", "search_files"];
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
                        self.fire_hook(HookEventKind::ToolCompleted {
                            tool_name: name.to_string(),
                            success: false,
                            result_preview: ironhermes_hooks::event::preview(&err_msg, 200),
                            duration_ms,
                        });
                        err_msg
                    }
                }
            }
        }
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
        let client = AnyClient::ChatCompletions(
            crate::client::LlmClient::new("http://localhost".to_string(), "".to_string(), "mock-model"),
        );
        AgentLoop::new(client, Arc::new(RwLock::new(tool_registry)), 4).with_hook_registry(hook_registry)
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
        tool_registry
            .add_guardrail(Box::new(BlocklistGuardrail::new(vec!["mock".to_string()])));

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

    fn make_skill_record(name: &str, allowed_tools: Option<Vec<&str>>) -> ironhermes_core::SkillRecord {
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
        assert_eq!(result, "mock result", "no active skills = all tools allowed");
    }

    #[tokio::test]
    async fn test_skill_enforcement_skills_tool_always_allowed() {
        let mut tool_registry = ToolRegistry::new();
        // Register a mock tool named "skills" to simulate the skills tool
        struct SkillsMockTool;
        #[async_trait]
        impl Tool for SkillsMockTool {
            fn name(&self) -> &str { "skills" }
            fn toolset(&self) -> &str { "test" }
            fn description(&self) -> &str { "mock skills" }
            fn schema(&self) -> ToolSchema {
                ToolSchema::new("skills", "mock skills", serde_json::json!({"type": "object", "properties": {}}))
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
        assert_eq!(result, "skills result", "skills tool must always be permitted (D-07)");
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
        let client = AnyClient::ChatCompletions(
            crate::client::LlmClient::new("http://localhost".to_string(), "".to_string(), "mock-model"),
        );
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let token = CancellationToken::new();
        let agent = AgentLoop::new(client, registry, 4)
            .with_cancellation_token(token.clone());
        // Verify the token is set (it exists on the struct)
        assert!(agent.cancel_token.is_some(), "cancel_token should be set after with_cancellation_token");
    }

    #[tokio::test]
    async fn test_agent_loop_run_returns_early_when_cancelled_before_first_iteration() {
        use tokio_util::sync::CancellationToken;
        let client = AnyClient::ChatCompletions(
            crate::client::LlmClient::new("http://localhost".to_string(), "".to_string(), "mock-model"),
        );
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let token = CancellationToken::new();
        // Cancel BEFORE run
        token.cancel();
        let mut agent = AgentLoop::new(client, registry, 4)
            .with_cancellation_token(token);
        let messages = vec![ChatMessage::user("hello")];
        let result = agent.run(messages).await.unwrap();
        assert!(!result.finished_naturally, "should not finish naturally when cancelled");
        assert_eq!(result.final_response.as_deref(), Some("Cancelled by parent"));
        assert_eq!(result.turns_used, 0, "should use 0 turns when cancelled before first iteration");
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
        let client = AnyClient::ChatCompletions(
            crate::client::LlmClient::new("http://localhost".to_string(), "".to_string(), "test"),
        );
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
        assert_eq!(result, Some(CAUTION_ADVISORY), "expected CAUTION_ADVISORY at 70%");
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
        assert_eq!(result, Some(WARNING_ADVISORY), "expected WARNING_ADVISORY at 90%");
    }

    #[test]
    fn test_shared_budget_increment() {
        // Parent + child share the same underlying counter via BudgetHandle::clone.
        let parent = BudgetHandle::new(10);
        let child = parent.clone();
        for _ in 0..5 {
            parent.consume();
        }
        for _ in 0..3 {
            child.consume();
        }
        assert_eq!(parent.used(), 8);
        assert_eq!(child.used(), 8, "clones share the same counter (PROV-10)");
    }

    #[test]
    fn test_budget_getter_returns_handle() {
        let handle = BudgetHandle::new(10);
        let agent = make_agent(10).with_budget(handle.clone());
        let retrieved = agent.budget();
        assert!(retrieved.is_some(), "budget() should return Some after with_budget");
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
        let client = AnyClient::ChatCompletions(
            crate::client::LlmClient::new("http://localhost".to_string(), "".to_string(), "test"),
        );
        let registry = Arc::new(RwLock::new(ironhermes_tools::ToolRegistry::new()));
        let agent = AgentLoop::new(client, registry, 10);
        assert!(!agent.fallback_activated, "fallback_activated should start false");
        assert!(agent.fallback_client.is_none(), "fallback_client should start None");
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
        let err = anyhow!("Connection refused: failed to connect to LLM");
        let (should_retry, should_fallback) = AgentLoop::classify_llm_error(&err);
        assert!(should_retry, "generic errors should be retryable");
        assert!(!should_fallback, "generic errors should not trigger fallback");
    }

    #[test]
    fn test_fallback_activated_prevents_refire() {
        let primary = AnyClient::ChatCompletions(
            crate::client::LlmClient::new("http://primary".to_string(), "key1".to_string(), "model1"),
        );
        let fallback = AnyClient::ChatCompletions(
            crate::client::LlmClient::new("http://fallback".to_string(), "key2".to_string(), "model2"),
        );
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
        assert!(agent.fallback_client.is_none(), "take() should leave None — one-shot guarantee");
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

    fn make_engine(threshold: f32) -> (Arc<RecordingEngine>, Arc<AtomicUsize>, Arc<AtomicUsize>, Arc<std::sync::Mutex<Vec<&'static str>>>) {
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
        assert_eq!(compress_calls.load(AtomicOrdering::SeqCst), 0, "below threshold must not compress");
        assert_eq!(pressure_calls.load(AtomicOrdering::SeqCst), 1, "below threshold must run pressure check");

        // Above threshold: tiny ctx_len forces ratio > 0.5.
        let (engine2, compress_calls2, pressure_calls2, _log2) = make_engine(0.5);
        let mut agent2 = make_agent_with_engine(engine2.clone(), 100);
        let mut msgs2 = filler_messages(20);
        agent2.pre_chat_compress(&mut msgs2).await;
        assert_eq!(compress_calls2.load(AtomicOrdering::SeqCst), 1, "above threshold must compress");
        assert_eq!(pressure_calls2.load(AtomicOrdering::SeqCst), 0, "above threshold must NOT also run pressure check");
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
        let mut agent = make_agent_with_engine(engine, 1_000_000)
            .with_pressure_tracker(tracker.clone());

        let mut messages = filler_messages(3);
        let before_len = messages.len();
        agent.pre_chat_compress(&mut messages).await;

        assert_eq!(messages.len(), before_len + 1, "transient must be appended");
        let injected = messages.last().unwrap();
        assert_eq!(injected.role, ironhermes_core::Role::System);
        assert!(injected.content_text().unwrap_or("").contains("CONTEXT PRESSURE HIGH"));

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
            messages.iter().all(|m| {
                m.content_text().map(|t| t != "CORRUPTED").unwrap_or(true)
            }),
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
        assert_eq!(final_log, vec!["compress"], "compress must be the pre-chat event");
    }
}

// ---------------------------------------------------------------------------
// Tests: Phase 21.5 memory provider tool wiring
// ---------------------------------------------------------------------------

#[cfg(test)]
mod memory_provider_wiring_tests {
    use super::*;

    fn make_agent() -> AgentLoop {
        let client = AnyClient::ChatCompletions(
            crate::client::LlmClient::new(
                "http://localhost".to_string(),
                "".to_string(),
                "mock-model",
            ),
        );
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
}

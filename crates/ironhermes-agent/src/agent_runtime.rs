//! `AgentRuntime` — the channel-facing agent API.
//!
//! One `AgentRuntime` per logical agent (per gateway process, per web server,
//! per CLI/TUI session). It owns the durable agent resources — the tool
//! registry, skills, browser session, hook registry, the model client, and
//! crucially the shared `BudgetHandle` — and exposes a single `run_turn` entry
//! point. Channels build one runtime via `from_config` and call `run_turn` per
//! user turn; they no longer construct `BudgetHandle`s, build `AgentLoop`s by
//! hand, or manage budget lifecycle.
//!
//! ## Why this exists
//!
//! Before this type, every channel constructed its own `BudgetHandle` at
//! startup and threaded it into both the per-request `AgentLoop` and the
//! subagent runner. Nothing reset it, so a long-lived server latched at
//! `Stop100` after the first budget-exhausting conversation. Centralizing the
//! budget here — created once, **reset at the `run_turn` boundary** — fixes that
//! for every channel and removes four copies of the same wiring. See
//! `docs/AGENT-RUNTIME-DESIGN.md`.
//!
//! ## Budget (top-level / interactive, D-15)
//!
//! `from_config` creates the `BudgetHandle` for the TOP-LEVEL interactive
//! agent loop and passes a clone to `AgentSubagentRunner::new` for storage.
//! `run_turn` resets that handle before each user turn so a long-lived runtime
//! never latches at Stop100.
//!
//! Plan 35-02 (D-01/D-04): PROV-10 shared parent↔child counter is RETIRED.
//! `AgentSubagentRunner::run_child` now gives each child its own fresh
//! `BudgetHandle::new(max_iterations)` — children no longer clone the stored
//! runner budget. The stored field is retained for the `new` signature and grep
//! invariants (see `AgentSubagentRunner` field doc).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{Mutex as TokioMutex, RwLock};
use tokio_util::sync::CancellationToken;

use ironhermes_core::{ChatMessage, Config, ProviderResolver, SkillRecord, SkillRegistry};
use ironhermes_cron::JobStore;
use ironhermes_exec::process_registry::ProcessRegistry;
use ironhermes_hooks::{HookRegistry, HooksConfig};
use ironhermes_state::StateStore;
use ironhermes_tools::browser_session::BrowserSession;
use ironhermes_tools::delegate_task::SubagentProgressCallback;
use ironhermes_tools::memory_tool::SharedMemoryManager;

use crate::agent_wiring::attach_context_engine;
use crate::any_client::{build_main_client, wire_fallback_if_configured};
use crate::context_refs::preprocess_context_references_async;
use crate::app_runtime_factory::{
    AppRuntimeBundle, AppRuntimeFactoryInput, DelegateTaskWiring, build_app_runtime_bundle,
};
use crate::agent_loop::{StreamCallback, ToolProgressCallback, ToolResultCallback};
use crate::budget::BudgetHandle;
use crate::memory::MemoryManager;
use crate::pressure_warning::PressureTracker;
use crate::subagent_registry::SubagentRegistry;
use crate::subagent_runner::AgentSubagentRunner;
use crate::{AgentLoop, AgentResult, AnyClient};

/// Construction inputs for [`AgentRuntime::from_config`]. Carries the config and
/// the small set of channel-specific knobs needed to build the subagent runner
/// (decision A in the design doc); the budget and the runner are built here so
/// channels stop constructing them.
pub struct AgentRuntimeInput {
    pub config: Arc<Config>,
    pub resolver: Arc<ProviderResolver>,
    pub cwd: PathBuf,
    pub process_registry: Arc<RwLock<ProcessRegistry>>,
    /// Concrete memory manager (also down-cast to `SharedMemoryManager` for the
    /// tool registry). `None` disables memory wiring.
    pub memory_manager: Option<Arc<TokioMutex<MemoryManager>>>,
    pub hooks_config: HooksConfig,
    pub emit_mcp_startup_logs: bool,

    // ── subagent runner knobs (channel-specific) ──────────────────────────
    pub subagent_registry: Arc<RwLock<SubagentRegistry>>,
    /// `(hermes_home, transcript_scope_label)` — the runner writes per-subagent
    /// transcripts under `hermes_home` keyed by this scope (e.g. the session id
    /// or "web-ui").
    pub transcript_scope: (PathBuf, String),
    pub subagent_progress_callback: Option<SubagentProgressCallback>,
    pub subagent_cancel_token: Option<CancellationToken>,
}

/// Everything that legitimately varies turn-to-turn. The channel builds the
/// message vector (session stores differ per channel) and supplies the per-turn
/// callbacks + identifiers.
#[derive(Default)]
pub struct TurnRequest {
    pub messages: Vec<ChatMessage>,
    pub session_id: String,
    pub cancel_token: Option<CancellationToken>,
    pub stream: Option<StreamCallback>,
    pub tool_progress: Option<ToolProgressCallback>,
    pub tool_result: Option<ToolResultCallback>,
    /// Per-session trajectory writer (gateway). `None` = no trajectory capture.
    pub trajectory_writer:
        Option<Arc<dyn ironhermes_core::commands::context::TrajectoryWriterHandle>>,
    /// Pre-built pressure tracker; `None` lets `attach_context_engine` make a
    /// fresh one for the turn.
    pub pressure_tracker: Option<Arc<PressureTracker>>,
    /// State store for `session_search` tool interception (web). `None` skips it.
    pub state_store: Option<Arc<std::sync::Mutex<StateStore>>>,
    /// Compression-count carry-over for multi-turn sessions (default 0).
    pub compression_count: usize,
}

/// Durable, channel-agnostic agent unit. Build once via [`from_config`], then
/// call [`run_turn`] per top-level user turn.
///
/// [`from_config`]: AgentRuntime::from_config
/// [`run_turn`]: AgentRuntime::run_turn
pub struct AgentRuntime {
    config: Arc<Config>,
    resolver: Arc<ProviderResolver>,
    client: AnyClient,
    bundle: AppRuntimeBundle,
    budget: BudgetHandle,
    memory_manager: Option<Arc<TokioMutex<MemoryManager>>>,
    subagent_registry: Arc<RwLock<SubagentRegistry>>,
    max_iterations: usize,
    /// Working directory for `@`-ref expansion (D-05: fixed to cwd at startup,
    /// used as both `cwd` and `allowed_root` in `preprocess_context_references_async`).
    cwd: PathBuf,
}

impl AgentRuntime {
    /// Build the runtime: create the shared budget from
    /// `config.agent.max_iterations`, construct the subagent runner with a clone
    /// of it (so parent + children share one counter), then assemble the tool
    /// registry / skills / browser bundle around that runner.
    pub async fn from_config(input: AgentRuntimeInput) -> Result<Self> {
        let AgentRuntimeInput {
            config,
            resolver,
            cwd,
            process_registry,
            memory_manager,
            hooks_config,
            emit_mcp_startup_logs,
            subagent_registry,
            transcript_scope,
            subagent_progress_callback,
            subagent_cancel_token,
        } = input;

        let max_iterations = config.agent.max_iterations;
        let budget = BudgetHandle::new(max_iterations);

        let client = build_main_client(&resolver)?;

        // Build the subagent runner, passing the budget clone for storage (field-kept
        // per Plan 35-02 field-disposition). Children no longer clone this stored
        // budget; each child gets a fresh BudgetHandle::new(max_iterations) in run_child.
        let (transcript_home, transcript_scope_label) = transcript_scope;
        let subagent_runner = Arc::new(
            AgentSubagentRunner::new(client.clone(), (*resolver).clone(), Some(budget.clone()))
                .with_subagent_registry(subagent_registry.clone())
                .with_transcript_scope(transcript_home, transcript_scope_label),
        );

        let semaphore = Arc::new(tokio::sync::Semaphore::new(
            config.delegation.max_concurrent_children,
        ));

        let shared_memory: Option<SharedMemoryManager> = memory_manager
            .clone()
            .map(|m| m as SharedMemoryManager);

        let cwd_stored = cwd.clone();
        let bundle = build_app_runtime_bundle(AppRuntimeFactoryInput {
            config: config.clone(),
            resolver: resolver.clone(),
            cwd,
            process_registry,
            memory_manager: shared_memory,
            delegate_task: Some(DelegateTaskWiring {
                runner: subagent_runner,
                semaphore,
                config: config.delegation.clone(),
                cancel_token: subagent_cancel_token,
                progress_callback: subagent_progress_callback,
            }),
            hooks_config,
            emit_mcp_startup_logs,
        })
        .await?;

        Ok(Self {
            config,
            resolver,
            client,
            bundle,
            budget,
            memory_manager,
            subagent_registry,
            max_iterations,
            cwd: cwd_stored,
        })
    }

    /// Run one top-level agent turn. This is the budget lifecycle boundary:
    /// the top-level `BudgetHandle` is reset to full here so a long-lived runtime
    /// never latches at `Stop100`. Plan 35-02 (D-01/D-04): subagents spawned
    /// during the turn each receive their own fresh `BudgetHandle::new(max_iterations)`
    /// in `run_child`; they no longer decrement the top-level counter.
    pub async fn run_turn(&self, mut req: TurnRequest) -> Result<AgentResult> {
        // ── budget lifecycle: refill before the turn ──────────────────────
        self.budget.reset();

        let context_length = self.resolver.resolve_for_main().context_length();

        // ── Phase 34b D-09/D-11: centralized @-ref preprocessing ─────────
        // Runs ONCE here, BEFORE attach_context_engine/agent.run, over the
        // latest user message. Never called per-surface (centralization invariant).
        // D-05: allowed_root = cwd (fixed at startup, no config escape hatch — D-04).
        let context_warnings: Vec<String> = {
            // Find the latest user-role message index.
            let last_user_idx = req
                .messages
                .iter()
                .enumerate()
                .rev()
                .find(|(_, m)| m.role == ironhermes_core::Role::User)
                .map(|(i, _)| i);

            if let Some(idx) = last_user_idx {
                if let Some(text) = req.messages[idx].content_text().map(|s| s.to_string()) {
                    // Production UrlFetcher: WebExtractTool with use_llm_processing:true (D-01).
                    // Raw fallback on LLM failure is handled inside the fetcher closure (D-02).
                    let url_fetcher: crate::context_refs::UrlFetcher = {
                        let registry = self.bundle.registry.clone();
                        Box::new(move |url: String| {
                            let registry = registry.clone();
                            Box::pin(async move {
                                // Call web_extract tool via the registry with use_llm_processing:true.
                                let args = serde_json::json!({
                                    "urls": [url],
                                    "use_llm_processing": true,
                                });
                                let reg = registry.read().await;
                                match reg.execute_tool("web_extract", args).await {
                                    Ok(result_str) => {
                                        // Parse ExtractionResult array from web_extract output.
                                        if let Ok(results) = serde_json::from_str::<Vec<serde_json::Value>>(&result_str) {
                                            if let Some(first) = results.first() {
                                                if let Some(content) = first.get("content").and_then(|v| v.as_str()) {
                                                    if !content.is_empty() {
                                                        return Ok(content.to_string());
                                                    }
                                                }
                                                // D-02: fall back to raw content on LLM-processing failure.
                                                if let Some(err) = first.get("error").and_then(|v| v.as_str()) {
                                                    return Err(format!("web_extract error: {}", err));
                                                }
                                            }
                                        }
                                        Err("web_extract returned no content".to_string())
                                    }
                                    Err(e) => Err(format!("web_extract failed: {}", e)),
                                }
                            })
                        })
                    };

                    let ctx_result = preprocess_context_references_async(
                        &text,
                        &self.cwd,
                        context_length,
                        Some(&url_fetcher),
                        None, // allowed_root defaults to cwd (D-04/D-05)
                    )
                    .await;

                    // Replace the latest user message text with the expanded version.
                    if ctx_result.expanded || ctx_result.blocked {
                        if let Some(msg) = req.messages.get_mut(idx) {
                            msg.content = Some(ironhermes_core::MessageContent::Text(
                                ctx_result.message.clone(),
                            ));
                        }
                    }

                    // Log warnings centrally (D-11 carrier).
                    for w in &ctx_result.warnings {
                        tracing::warn!(target: "ironhermes_agent::context_refs", warning = %w, "@ context expansion warning");
                    }

                    ctx_result.warnings
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            }
        };

        let mut agent = AgentLoop::new(
            self.client.clone(),
            self.bundle.registry.clone(),
            self.max_iterations,
        )
        .with_budget(self.budget.clone())
        .with_hook_registry(self.bundle.hook_registry.clone())
        .with_browser_session(self.bundle.browser_session.clone())
        .with_active_skills(self.bundle.active_skills.clone())
        .with_compression(context_length, self.config.agent.context_compression)
        .with_compression_count(req.compression_count);

        if let Some(ref mgr) = self.memory_manager {
            agent = agent.with_memory_manager(mgr.clone());
        }

        agent = wire_fallback_if_configured(agent, &self.resolver);

        // ── per-turn / channel-specific wiring ────────────────────────────
        if let Some(cb) = req.stream {
            agent = agent.with_streaming(cb);
        }
        if let Some(cb) = req.tool_progress {
            agent = agent.with_tool_progress(cb);
        }
        if let Some(cb) = req.tool_result {
            agent = agent.with_tool_result(cb);
        }
        if let Some(token) = req.cancel_token {
            agent = agent.with_cancellation_token(token);
        }
        if let Some(tw) = req.trajectory_writer {
            agent = agent.with_trajectory_writer(tw);
        }
        if let Some(store) = req.state_store {
            agent = agent.with_intercepts(None, Some(store), None, None, None);
        }

        agent = attach_context_engine(
            agent,
            &self.config,
            &self.resolver,
            req.session_id,
            Some(self.bundle.hook_registry.clone()),
            req.pressure_tracker,
            context_length,
            self.memory_manager.clone(),
        );

        // ── Phase 34b Plan 02 (D-07/D-09): central per-turn engine hooks ─────
        // Invoked ONCE here — the single per-turn locus — never per-surface.
        // Grab a handle to the attached engine (None on surfaces that disable
        // compression). The shipped engines treat both as no-ops; an engine
        // holding durable state can react. update_model is wired definitely
        // this phase (D-07), NOT conditionally.
        let engine_handle = agent.context_engine();
        if let Some(ref engine) = engine_handle {
            // Per-turn model identity: fully resolvable from the same accessor
            // run_turn already used for context_length above (no hedge — D-07).
            let endpoint = self.resolver.resolve_for_main();
            engine.update_model(
                endpoint.default_model.as_str(),
                context_length,
                Some(endpoint.base_url.as_str()),
            );
        }

        // D-11 / WR-01: attach context_warnings from @-ref expansion onto AgentResult.
        // Each surface (CLI, gateway, web) reads this field after run_turn returns and
        // renders the --- Context Warnings --- block out-of-band (not embedded in the
        // model-bound message text — that embedding was removed in Phase 34b Plan 03).
        let mut out = agent.run(req.messages).await?;

        // Phase 34b Plan 02 (D-09): post-run per-turn usage hook. MUST appear
        // AFTER agent.run (asserted in invariants_34b).
        if let Some(ref engine) = engine_handle {
            engine.update_from_response(&out.total_usage);
        }

        out.context_warnings = context_warnings;
        Ok(out)
    }

    // ── accessors for channel-specific surfaces (slash dispatch, /agents,
    //    status, prompt building) ──────────────────────────────────────────
    pub fn budget(&self) -> &BudgetHandle {
        &self.budget
    }
    pub fn registry(&self) -> &Arc<RwLock<ironhermes_tools::ToolRegistry>> {
        &self.bundle.registry
    }
    pub fn hook_registry(&self) -> &Arc<HookRegistry> {
        &self.bundle.hook_registry
    }
    pub fn skill_registry(&self) -> &Arc<SkillRegistry> {
        &self.bundle.skill_registry
    }
    pub fn active_skills(&self) -> &Arc<std::sync::Mutex<Vec<SkillRecord>>> {
        &self.bundle.active_skills
    }
    pub fn browser_session(&self) -> &Arc<TokioMutex<Option<BrowserSession>>> {
        &self.bundle.browser_session
    }
    pub fn job_store(&self) -> &Arc<std::sync::Mutex<JobStore>> {
        &self.bundle.job_store
    }
    pub fn subagent_registry(&self) -> &Arc<RwLock<SubagentRegistry>> {
        &self.subagent_registry
    }
    pub fn client(&self) -> &AnyClient {
        &self.client
    }
    pub fn config(&self) -> &Arc<Config> {
        &self.config
    }
    /// Returns the MCP manager handle built during `from_config`, if any MCP
    /// servers were configured. Used by `run_gateway` to wire the shutdown path
    /// so `ironhermes gateway` exits in bounded time on Ctrl+C.
    pub fn mcp_manager(&self) -> Option<&Arc<ironhermes_mcp::McpManager>> {
        self.bundle.mcp_manager.as_ref()
    }
    /// Returns the merged `ToolsConfig` (config.tools with ALL_TOOLSETS defaults
    /// filled in). Needed by run_gateway to construct the `ToolsetSessionHandle`
    /// from the same baseline the registry filter uses.
    pub fn merged_tools(&self) -> &ironhermes_core::config::ToolsConfig {
        &self.bundle.merged_tools
    }
}

impl AgentRuntime {
    /// Build a minimal `AgentRuntime` for use in unit tests and test fixtures.
    ///
    /// Uses a localhost:0 client (no real LLM endpoint needed), default Config,
    /// and empty registries. `run_turn` will fail to connect if called, but the
    /// runtime's struct fields (budget, registry, etc.) are fully initialised.
    /// This is the cleanest path for test fixtures that need an `Arc<AgentRuntime>`
    /// without a live model endpoint (Phase 28.1-05 D-01).
    ///
    /// `JobStore::open` requires a writable directory; we use a temp dir unique to
    /// the process so parallel test runs don't collide.
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_tests() -> Self {
        use std::sync::Arc;
        use ironhermes_core::{Config, ProviderResolver, SkillRegistry};
        use ironhermes_hooks::HookRegistry;
        use ironhermes_tools::ToolRegistry;
        use tokio::sync::RwLock;
        use crate::app_runtime_factory::AppRuntimeBundle;

        let config = Arc::new(Config::default());
        let resolver = Arc::new(
            ProviderResolver::build(&config)
                .expect("ProviderResolver::build with default Config must succeed in test context"),
        );

        // Use ChatCompletions client pointing to localhost:0 — it won't connect
        // but provides a valid AnyClient for struct construction.
        let client = crate::AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "http://localhost:0",
            "test-key",
            "test-model",
        ));

        let max_iterations = config.agent.max_iterations;
        let budget = crate::budget::BudgetHandle::new(max_iterations);

        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let hook_registry = Arc::new(HookRegistry::new(ironhermes_hooks::HooksConfig::default()));
        // load_with_paths(&[]) produces an empty SkillRegistry without touching disk.
        let skill_registry = Arc::new(SkillRegistry::load_with_paths(&[]));
        let active_skills = Arc::new(std::sync::Mutex::new(Vec::new()));
        let cron_dir = std::env::temp_dir()
            .join(format!("ironhermes_test_cron_{}", std::process::id()));
        let job_store = Arc::new(std::sync::Mutex::new(
            ironhermes_cron::JobStore::open(cron_dir)
                .expect("temp-dir JobStore must succeed in test context"),
        ));
        let browser_session = Arc::new(tokio::sync::Mutex::new(None));

        let bundle = AppRuntimeBundle {
            registry,
            hook_registry,
            skill_registry,
            active_skills,
            job_store,
            browser_session,
            mcp_manager: None,
            merged_tools: ironhermes_core::config::ToolsConfig::default(),
        };

        let subagent_registry = Arc::new(RwLock::new(
            crate::subagent_registry::SubagentRegistry::new(),
        ));

        Self {
            config,
            resolver,
            client,
            bundle,
            budget,
            memory_manager: None,
            subagent_registry,
            max_iterations,
            cwd: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Source text for this file — used by position-guard assertions below.
    const SOURCE: &str = include_str!("agent_runtime.rs");

    /// Regression gate: `run_turn` MUST call `self.budget.reset()` BEFORE
    /// constructing `AgentLoop::new`. If a future refactor drops or relocates
    /// the reset call this test fails, catching the regression at CI time.
    ///
    /// Additionally proves the behavioral invariant: after draining a
    /// `BudgetHandle` to zero, calling the same `reset()` call that `run_turn`
    /// uses returns the budget to full — ensuring a second top-level turn never
    /// inherits a depleted budget (Stop100 latch class of bug, CONTEXT #2).
    ///
    /// Form chosen: direct `BudgetHandle` manipulation via a standalone handle
    /// that mirrors what `run_turn` holds. A full `from_config` round-trip is
    /// impractical in a unit test (it requires a reachable model endpoint and
    /// assembles MCP/tools); the behavioral drain + reset contract is identical
    /// regardless of how the handle was constructed.
    #[test]
    fn budget_resets_between_turns() {
        // ── behavioral assertion ─────────────────────────────────────────────
        // Mirror the runtime's budget: use the same API `run_turn` uses.
        let max = 5_usize;
        let budget = BudgetHandle::new(max);

        // Simulate a budget-exhausting first turn: drain to zero.
        while budget.consume().is_some() {}
        assert_eq!(
            budget.remaining(),
            0,
            "pre-condition: budget must be fully exhausted before reset"
        );

        // Call the exact reset boundary that `run_turn` uses (line ~198).
        budget.reset();

        assert_eq!(
            budget.remaining(),
            max,
            "after reset(), remaining must equal max_iterations (no Stop100 latch)"
        );

        // ── source-include guard: reset call must exist ──────────────────────
        assert!(
            SOURCE.contains("self.budget.reset()"),
            "run_turn must call `self.budget.reset()` — source guard failed; \
             reset was removed or renamed"
        );

        // ── position guard: reset must appear BEFORE AgentLoop::new ─────────
        // Mirrors the `.find()` byte-offset pattern from
        // `crates/ironhermes-cli/tests/invariants_22_4.rs` (INV-22.4-24).
        let reset_pos = SOURCE
            .find("self.budget.reset()")
            .expect("self.budget.reset() must be present in agent_runtime.rs");
        let loop_pos = SOURCE
            .find("AgentLoop::new(")
            .expect("AgentLoop::new( must be present in agent_runtime.rs");
        assert!(
            reset_pos < loop_pos,
            "self.budget.reset() (at byte {reset_pos}) must appear BEFORE \
             AgentLoop::new( (at byte {loop_pos}) in run_turn — budget must be \
             refilled before the loop is constructed"
        );
    }

    /// Regression gate: `from_config` wires the top-level budget into
    /// `AgentSubagentRunner::new` for storage, and `run_child` gives each child
    /// a FRESH `BudgetHandle::new(max_iterations)` — not a clone of the stored
    /// runner budget. PROV-10 shared parent↔child counter is RETIRED (Plan 35-02
    /// D-04); this test documents the new independence contract.
    ///
    /// Form chosen: source-include guard. Building a full `AgentRuntime` via
    /// `from_config` in a unit test is impractical (it requires a reachable
    /// model endpoint and assembles the MCP/tool bundle). The storage wiring
    /// (field-kept per Plan 35-02 field-disposition) is verified by asserting
    /// the exact source patterns; the independence behavior is proven by the
    /// D-07.1 test in `agent_loop.rs::budget_tests`.
    #[test]
    fn runner_stores_budget_field_children_get_fresh_handle() {
        // Assert from_config still passes the budget clone for storage in the runner
        // (field-kept so new() signature and grep invariants stay intact).
        assert!(
            SOURCE.contains("Some(budget.clone())"),
            "from_config must pass `Some(budget.clone())` to AgentSubagentRunner::new \
             (field-kept per Plan 35-02) — source guard failed"
        );

        // Assert the top-level budget is stored on Self so run_turn can reset it.
        assert!(
            SOURCE.contains("budget,"),
            "AgentRuntime struct initializer must include `budget,` field — source guard failed; \
             the top-level BudgetHandle must be stored on Self so run_turn can reset it"
        );

        // Assert the runner is built before Self is returned.
        let runner_pos = SOURCE
            .find("Some(budget.clone())")
            .expect("Some(budget.clone()) must be present in agent_runtime.rs");
        let self_ok_pos = SOURCE
            .find("Ok(Self {")
            .expect("Ok(Self { must be present in agent_runtime.rs");
        assert!(
            runner_pos < self_ok_pos,
            "Some(budget.clone()) (at byte {runner_pos}) must appear BEFORE \
             Ok(Self {{ (at byte {self_ok_pos})) — runner must be wired before Self is constructed"
        );

        // Assert run_child gives each child a FRESH budget (independence — D-01/D-04).
        // Use include_str! on subagent_runner.rs to verify the change site.
        let runner_src = include_str!("subagent_runner.rs");
        assert!(
            runner_src.contains("BudgetHandle::new(max_iterations)"),
            "subagent_runner.rs run_child must use BudgetHandle::new(max_iterations) \
             to give each child a fresh independent budget (D-01/D-04) — source guard failed"
        );
        assert!(
            !runner_src.contains("agent = agent.with_budget(budget.clone())"),
            "subagent_runner.rs run_child must NOT clone the parent budget into children \
             (PROV-10 retired, D-04) — source guard failed"
        );
    }
}

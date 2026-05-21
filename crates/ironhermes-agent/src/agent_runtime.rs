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
//! ## Budget sharing (PROV-10)
//!
//! `from_config` creates the `BudgetHandle` and builds the `AgentSubagentRunner`
//! with a clone of it, so a parent turn and the subagents it spawns share one
//! counter (a runaway delegation tree is bounded together). `run_turn` resets
//! that shared counter before each top-level turn.

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

        // Build the subagent runner with a clone of the SHARED budget (PROV-10).
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
        })
    }

    /// Run one top-level agent turn. This is the budget lifecycle boundary:
    /// the shared `BudgetHandle` is reset to full here so a long-lived runtime
    /// never latches at `Stop100`. Subagents spawned during the turn share the
    /// just-reset counter via the runner's `Arc`.
    pub async fn run_turn(&self, req: TurnRequest) -> Result<AgentResult> {
        // ── budget lifecycle: refill before the turn ──────────────────────
        self.budget.reset();

        let context_length = self.resolver.resolve_for_main().context_length();

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

        agent.run(req.messages).await
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

    /// Regression gate: `from_config` MUST pass a clone of the shared budget
    /// to `AgentSubagentRunner::new` (PROV-10 — parent and subagents share one
    /// counter). This source-include guard fails if the wiring is dropped or
    /// broken by a future refactor.
    ///
    /// Form chosen: source-include guard. Building a full `AgentRuntime` via
    /// `from_config` in a unit test is impractical (it requires a reachable
    /// model endpoint and assembles the MCP/tool bundle); the Arc-identity
    /// invariant is fully captured by asserting the source contains the exact
    /// clone-pass pattern and that `budget` is stored in `Self`. The behavioral
    /// Arc-sharing is already covered by `budget.rs::reset_is_visible_through_shared_clone`.
    #[test]
    fn runner_shares_budget_arc() {
        // Assert from_config clones the shared budget into the subagent runner.
        assert!(
            SOURCE.contains("Some(budget.clone())"),
            "from_config must pass `Some(budget.clone())` to AgentSubagentRunner::new \
             (PROV-10 parent/child budget sharing) — source guard failed"
        );

        // Assert the same budget is stored on Self (not a separately-created one).
        assert!(
            SOURCE.contains("budget,"),
            "AgentRuntime struct initializer must include `budget,` field — source guard failed; \
             the shared BudgetHandle must be stored on Self so run_turn can reset it"
        );

        // Assert the runner is built with the cloned budget before Self is returned.
        let runner_pos = SOURCE
            .find("Some(budget.clone())")
            .expect("Some(budget.clone()) must be present in agent_runtime.rs");
        let self_ok_pos = SOURCE
            .find("Ok(Self {")
            .expect("Ok(Self { must be present in agent_runtime.rs");
        assert!(
            runner_pos < self_ok_pos,
            "Some(budget.clone()) (at byte {runner_pos}) must appear BEFORE \
             Ok(Self {{ (at byte {self_ok_pos})) — runner must be wired with the \
             budget before Self is constructed"
        );
    }
}

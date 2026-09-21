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

use crate::agent_loop::{StreamCallback, ToolProgressCallback, ToolResultCallback};
use crate::agent_wiring::attach_context_engine;
use crate::any_client::{build_main_client, build_role_client, wire_fallback_if_configured};
use crate::app_runtime_factory::{
    AppRuntimeBundle, AppRuntimeFactoryInput, DelegateTaskWiring, build_app_runtime_bundle,
};
use crate::budget::BudgetHandle;
use crate::context_refs::preprocess_context_references_async;
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

/// Phase 36.17.7 D-01: lightweight per-turn TTS session descriptor.
///
/// Built by each surface handler (gateway, ws.rs, event_loop.rs) and threaded
/// into `TurnRequest` so `AgentRuntime::run_turn` can call
/// `ToolRegistry::register_tts_tools` on the durable `bundle.registry` before
/// the agent loop is constructed.
///
/// `audio_dispatcher` is `None` for `Platform::Local` (TUI) — `SendAudioTool`'s
/// Local arm handles `rodio` playback directly without a dispatcher Arc.
pub struct TtsPerTurnWiring {
    pub session_key: ironhermes_core::SessionKey,
    pub audio_dispatcher: Option<Arc<dyn ironhermes_tools::AudioDispatcher>>,
}

/// Phase 36.3.8 D-02/D-04/D-05: lightweight per-turn messaging + clarify descriptor.
///
/// Built by each surface handler and threaded into `TurnRequest` so
/// `AgentRuntime::run_turn` can call `ToolRegistry::register_messaging_tools` on
/// the durable `bundle.registry` before the agent loop runs.
///
/// `message_dispatcher: None` — correct for Local platform (stdout in tool).
/// `clarify_dispatcher: None` — drives the stdout numbered fallback in ClarifyTool.
/// `clarify_registry` MUST be the same Arc held by the gateway callback loop
/// (T-36.3.8-ROUTE: one map, one awaiter resolution path).
/// `cancel_token: None` — creates a never-firing arm (WASM-safe via unwrap_or_default).
pub struct MessagingPerTurnWiring {
    pub session_key: ironhermes_core::SessionKey,
    pub message_dispatcher: Option<Arc<dyn ironhermes_tools::MessageDispatcher>>,
    pub clarify_dispatcher: Option<Arc<dyn ironhermes_tools::ClarifyDispatcher>>,
    pub clarify_registry: Arc<ironhermes_tools::clarify_registry::PendingClarifyRegistry>,
    pub cancel_token: Option<tokio_util::sync::CancellationToken>,
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
    /// Phase 36.17.7 D-01: per-turn TTS wiring. `None` skips TTS registration.
    /// `Some(...)` causes `run_turn` to call `register_tts_tools` on
    /// `bundle.registry` at the top of the turn, before the agent loop runs.
    pub tts_wiring: Option<TtsPerTurnWiring>,
    /// Phase 36.3.8 D-02/D-04/D-05: per-turn messaging + clarify wiring.
    /// `None` skips messaging tool registration (backwards-compatible: existing
    /// callers that don't set this field compile with Option::None via Default).
    /// `Some(...)` causes `run_turn` to call `register_messaging_tools` on
    /// `bundle.registry` at the top of the turn, before the agent loop runs.
    pub messaging_wiring: Option<MessagingPerTurnWiring>,
    /// Phase 39.2: black-box recorder run_id. Set by the surface layer before
    /// calling `run_turn`; `None` = recorder generates a fresh UUID for this turn.
    pub turn_id: Option<uuid::Uuid>,
    /// Phase 45 D-11: per-turn approval gate. `None` skips gate injection (the
    /// NeedsApproval arm in AgentLoop is fail-closed: GateUnavailable returned).
    /// Gateway surfaces supply a `GatewayApprovalGate` bound to the coordinator
    /// + chat_id for this turn; other surfaces leave this `None`.
    pub approval_gate: Option<std::sync::Arc<dyn ironhermes_core::ApprovalGate>>,
    /// Phase 45 D-11: per-turn terminal tool intercept handler. When `Some`, the
    /// gateway installs a gated override (via `register_intercepted_or_replace`)
    /// so LLM-issued `terminal` tool calls route through `handle_shell_exec` instead
    /// of the default `TerminalTool::execute`. Other surfaces leave this `None`.
    pub terminal_intercept: Option<ironhermes_tools::registry::InterceptHandler>,
    /// Phase 36.3.12 D-08/D-11: per-turn `execute_code` tool intercept handler.
    /// Mirrors `terminal_intercept` exactly — when `Some`, `run_turn` installs it
    /// via `register_intercepted_or_replace("execute_code", ...)` so LLM-issued
    /// `execute_code` calls route through the caller's gating closure (built around
    /// `ironhermes_hooks::execute_gated_command`, gate-only per D-11 — the Python
    /// script still runs on the local `Sandbox`). `None` skips gating (no surface in
    /// this phase should leave this `None` in production — see
    /// `AgentRuntime::execute_code_tool_arc` for the underlying tool the closure
    /// wraps).
    pub execute_code_intercept: Option<ironhermes_tools::registry::InterceptHandler>,
}

/// Vision auto-routing (fix): true when any message carries inline image content
/// (a `ContentPart::ImageUrl` inside a `Parts` body). Such a turn must run on a
/// vision-capable model, or the provider rejects it with a 400 ("Image content is
/// not supported by this model"). Used by [`AgentRuntime::run_turn`] to route
/// image-bearing turns to the configured `roles.vision` model.
fn messages_contain_image(messages: &[ChatMessage]) -> bool {
    use ironhermes_core::{ContentPart, MessageContent};
    messages.iter().any(|m| {
        matches!(
            &m.content,
            Some(MessageContent::Parts(parts))
                if parts.iter().any(|p| matches!(p, ContentPart::ImageUrl { .. }))
        )
    })
}

/// Durable, channel-agnostic agent unit. Build once via [`from_config`], then
/// call [`run_turn`] per top-level user turn.
///
/// [`from_config`]: AgentRuntime::from_config
/// [`run_turn`]: AgentRuntime::run_turn
pub struct AgentRuntime {
    /// Phase 50.4 (D-08/D-14): hot-swappable — `AgentRuntime::reload_config_and_resolver`
    /// replaces the inner `Arc<Config>` on a live reload so every subsequent
    /// `AgentRuntime::config()` read sees the new config. Mirrors
    /// `AppRuntimeBundle::skill_registry`'s `Arc<std::sync::RwLock<Arc<T>>>` shape
    /// (`std::sync::RwLock`, not tokio's, so the accessor stays callable from sync
    /// contexts; the guard is never held across an `.await`).
    config_handle: Arc<std::sync::RwLock<Arc<Config>>>,
    /// Phase 50.4 (D-08/D-14): same hot-swap shape as `config_handle`.
    resolver_handle: Arc<std::sync::RwLock<Arc<ProviderResolver>>>,
    /// Phase 50.4 (D-14): the cached main client, hot-swappable so a reload can
    /// rebuild it against the NEW resolver — without this rebuild, swapping
    /// `resolver_handle` alone is inert: the cached client was built once, at
    /// construction, from whatever resolver existed then.
    client_handle: Arc<std::sync::RwLock<AnyClient>>,
    bundle: AppRuntimeBundle,
    budget: BudgetHandle,
    memory_manager: Option<Arc<TokioMutex<MemoryManager>>>,
    subagent_registry: Arc<RwLock<SubagentRegistry>>,
    max_iterations: usize,
    /// Working directory for `@`-ref expansion (D-05: fixed to cwd at startup,
    /// used as both `cwd` and `allowed_root` in `preprocess_context_references_async`).
    cwd: PathBuf,
    /// Phase 36.2 CR-04: model name from the immediately previous turn. Used
    /// to fire the cache-break warning when an operator swaps models mid-
    /// session. `None` on the first turn since runtime construction.
    previous_model: std::sync::Mutex<Option<String>>,
    /// Phase 36.2 CR-04: count of turns this runtime has executed. The
    /// model-swap cache-break warning suppresses on turn 0 (the first turn
    /// PICKS a model rather than swapping it). Atomic so per-turn updates
    /// don't need a Mutex acquire.
    session_turn_count: std::sync::atomic::AtomicUsize,
    /// Phase 36.2 CR-04: paths the PressureTracker mtime-snapshots so a
    /// SOUL.md / AGENTS.md / CLAUDE.md edit fires the cache-break warning.
    /// Resolved once at runtime construction (cwd-derived candidates + the
    /// $IRONHERMES_HOME identity files). Empty list = no context-file tracking.
    context_file_paths: Vec<PathBuf>,
    /// Phase 39.2: black-box event recorder. `None` = recording disabled.
    /// Set via `with_bb_recorder()` after `from_config`; initialized by
    /// `app_runtime_factory.rs` for all production surfaces.
    pub bb_recorder: Option<Arc<ironhermes_blackbox::BlackBoxRecorder>>,
    /// Phase 36.3.12 D-08/D-10: the regular-tool `Arc` for `"terminal"`, captured
    /// ONCE here (before any turn ever runs) so every surface's gating closure —
    /// built fresh per turn in `TurnRequest.terminal_intercept` — can invoke the
    /// SAME already-configured tool instance (preserving its `ProcessRegistry`
    /// wiring for `background=true`) on turn 2+, after
    /// `register_intercepted_or_replace` has permanently moved `"terminal"` out of
    /// the registry's regular `tools` map. `None` if the factory never registered a
    /// "terminal" tool (defensive; production always registers one).
    terminal_tool_arc: Option<Arc<dyn ironhermes_tools::registry::Tool>>,
    /// Phase 36.3.12 D-08/D-11: same capture as `terminal_tool_arc`, for
    /// `"execute_code"`.
    execute_code_tool_arc: Option<Arc<dyn ironhermes_tools::registry::Tool>>,
    /// Phase 36.8 CR-01 (D-16): same capture as `terminal_tool_arc`, for
    /// `"write_file"`. The ACP surface's per-turn `write_file` gating closure
    /// (`gate_workspace_write` in `handlers.rs`) needs the SAME already-configured
    /// tool instance on turn 2+, after the name has been moved out of the
    /// registry's regular `tools` map by `register_intercepted_or_replace` on turn
    /// 1 and then swept from the `intercepts` map by the WR-05 end-of-turn cleanup
    /// — without this capture, a per-turn `get_arc("write_file")` lookup returns
    /// `None` on every turn after the first and an approved write silently never
    /// reaches disk (36.8-VERIFICATION.md's CR-01). `Option`, not a bare `Arc`:
    /// `write_file` registration is toolset-skippable.
    write_file_tool_arc: Option<Arc<dyn ironhermes_tools::registry::Tool>>,
    /// Phase 36.8 CR-01 (D-16): same capture as `write_file_tool_arc`, for
    /// `"patch"`.
    patch_tool_arc: Option<Arc<dyn ironhermes_tools::registry::Tool>>,
}

/// What `AgentRuntime::reload_skill_registry` changed, so callers can report it
/// instead of guessing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillRegistryReload {
    /// Skill names present after the reload but not before, sorted.
    pub added: Vec<String>,
    /// Skill names present before the reload but not after, sorted.
    pub removed: Vec<String>,
    /// Total skills in the reloaded catalog.
    pub total: usize,
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

        // Phase 50.4 (D-08/D-14): construct the two config/resolver handles now,
        // at the earliest point the plain values exist, and BEFORE build_main_client
        // and the subagent-runner/bundle construction that follow. `.clone()` here
        // is an Arc clone — it does not move `config`/`resolver`, so every
        // downstream `.clone()` on those same locals (subagent runner, bundle)
        // below is unaffected and stays byte-identical in this plan. Plan 08
        // (wave 2) hands these SAME handle objects to the subagent runner and to
        // the vision/web-extract handle constructors instead of the plain-value
        // clones those consumers still take here — this plan's job is only to
        // make the handles exist here, textually above every consumer, so Plan 08
        // is a propagation rather than a re-ordering of this constructor.
        let config_handle = Arc::new(std::sync::RwLock::new(config.clone()));
        let resolver_handle = Arc::new(std::sync::RwLock::new(resolver.clone()));

        let mut client = build_main_client(&resolver)?;
        // Phase 36.2 CR-09: enable OpenRouter Claude cache_control routing on
        // the streaming send path. No-op for non-OpenRouter providers and
        // non-Claude models (the inner check in `chat_completion_stream`
        // guards via `is_openrouter_claude`). For Anthropic-native this is
        // also a no-op — the AnthropicMessages arm has its own cache wiring.
        client.enable_openrouter_caching(
            resolver.main_provider().to_string(),
            config.prompt_caching.clone(),
        );
        // Phase 50.4 (D-14): wrap the freshly-caching-enabled client in its own
        // handle via a clone — NOT a move — so the subagent-runner construction
        // immediately below (unchanged in this plan; Plan 08's job) can still
        // take its own `client.clone()` of the plain local exactly as it did
        // before this phase.
        let client_handle = Arc::new(std::sync::RwLock::new(client.clone()));

        // Build the subagent runner, passing the budget clone for storage (field-kept
        // per Plan 35-02 field-disposition). Children no longer clone this stored
        // budget; each child gets a fresh BudgetHandle::new(max_iterations) in run_child.
        //
        // Phase 50.4 (D-14, wave 2): the `client.clone()` / `(*resolver).clone()`
        // arguments passed into the constructor below are immediately
        // superseded by the `.with_shared_handles(...)` builder chained onto
        // it — left as-is rather than restructured because `invariants_21_7.rs`
        // greps this exact construction call site by literal substring count
        // and the arguments themselves are cheap. The builder hands the
        // runner the SAME handle objects `reload_config_and_resolver`
        // publishes into, so a delegated child agent spawned after an
        // apply-now reads the post-reload provider and client, not the
        // deep-cloned snapshot the constructor's own arguments produced.
        let (transcript_home, transcript_scope_label) = transcript_scope;
        let subagent_runner = Arc::new(
            AgentSubagentRunner::new(client.clone(), (*resolver).clone(), Some(budget.clone()))
                .with_shared_handles(client_handle.clone(), resolver_handle.clone())
                .with_subagent_registry(subagent_registry.clone())
                .with_transcript_scope(transcript_home, transcript_scope_label),
        );

        let semaphore = Arc::new(tokio::sync::Semaphore::new(
            config.delegation.max_concurrent_children,
        ));

        let shared_memory: Option<SharedMemoryManager> =
            memory_manager.clone().map(|m| m as SharedMemoryManager);

        let cwd_stored = cwd.clone();
        // Phase 36.17.7 Plan 01 (BLOCKER 1 fix for D-05): the startup bundle has no
        // TTS — per-turn TTS now lives in `TurnRequest::tts_wiring` and is registered
        // by `run_turn` via the per-turn block below. Using `..Default::default()` for
        // the residual `Option`-typed fields eliminates the previous deferral literal
        // from this source file so the D-05 negative-assert in Plan 05 Task 6
        // (`invariants_36_17_7.rs`) GREENs.
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
            // Phase 50.4 (D-14, wave 2): hand the factory THIS runtime's own
            // resolver handle, not the plain `resolver` clone above — this is
            // the line that makes the vision/web-extract handle wiring
            // load-bearing rather than theoretical. Without it,
            // `build_app_runtime_bundle` silently takes the derive-a-fresh-
            // handle fallback and neither tool handle ever observes a reload.
            resolver_handle: Some(resolver_handle.clone()),
            ..Default::default()
        })
        .await?;

        // Phase 36.2 CR-04: resolve the context files PressureTracker will
        // mtime-snapshot. Mirrors PromptBuilder's load order: IRONHERMES_HOME
        // identity files + every CONTEXT_CANDIDATES filename under cwd. Paths
        // need not exist at startup — agent_loop tolerates missing files.
        let mut context_file_paths: Vec<PathBuf> = Vec::new();
        let hermes_home = ironhermes_core::get_hermes_home();
        context_file_paths.push(hermes_home.join("SOUL.md"));
        context_file_paths.push(hermes_home.join("AGENTS.md"));
        for filename in crate::context_loader::CONTEXT_CANDIDATES {
            context_file_paths.push(cwd_stored.join(filename));
        }

        // Phase 39.2: take bb_recorder from the bundle so we share a single
        // writer task (one JSONL file, one flush loop) across the runtime lifetime.
        // build_app_runtime_bundle initializes it using get_hermes_home().
        let bb_recorder = bundle.bb_recorder.clone();

        // Phase 36.3.12 D-08/D-10/D-11, extended by Phase 36.8 CR-01 (D-16): capture
        // the regular-tool Arcs for "terminal", "execute_code", "write_file" and
        // "patch" BEFORE any surface ever calls `register_intercepted_or_replace`
        // (which permanently moves the name from the `tools` map into `intercepts`
        // on first use). Captured once here, in a SINGLE registry read-lock
        // acquisition, so every turn's gating closure — built fresh per turn by
        // CLI/TUI/gateway/ACP — can invoke the SAME already-configured tool
        // instance even on turn 2+, when the registry no longer has these names in
        // its regular `tools` map.
        let (terminal_tool_arc, execute_code_tool_arc, write_file_tool_arc, patch_tool_arc) = {
            let reg = bundle.registry.read().await;
            (
                reg.get_arc("terminal"),
                reg.get_arc("execute_code"),
                reg.get_arc("write_file"),
                reg.get_arc("patch"),
            )
        };

        Ok(Self {
            config_handle,
            resolver_handle,
            client_handle,
            bundle,
            budget,
            memory_manager,
            subagent_registry,
            max_iterations,
            cwd: cwd_stored,
            previous_model: std::sync::Mutex::new(None),
            session_turn_count: std::sync::atomic::AtomicUsize::new(0),
            context_file_paths,
            bb_recorder,
            terminal_tool_arc,
            execute_code_tool_arc,
            write_file_tool_arc,
            patch_tool_arc,
        })
    }

    /// Phase 36.3.12 D-08/D-10: the regular `"terminal"` tool instance captured at
    /// construction time, for surfaces building a gating closure (`terminal_intercept`)
    /// that needs to invoke the real dispatch (preserving its `ProcessRegistry`
    /// wiring for `background=true`) after the name has been intercepted.
    pub fn terminal_tool_arc(&self) -> Option<Arc<dyn ironhermes_tools::registry::Tool>> {
        self.terminal_tool_arc.clone()
    }

    /// Phase 36.3.12 D-08/D-11: same as `terminal_tool_arc`, for `"execute_code"`.
    pub fn execute_code_tool_arc(&self) -> Option<Arc<dyn ironhermes_tools::registry::Tool>> {
        self.execute_code_tool_arc.clone()
    }

    /// Phase 36.8 CR-01 (D-16): the regular `"write_file"` tool instance captured at
    /// construction time, for surfaces building a gating closure
    /// (`gate_workspace_write` in ACP's `handlers.rs`) that needs to invoke the real
    /// dispatch after the name has been intercepted and later evicted from the
    /// registry's `intercepts` map by the end-of-turn cleanup.
    pub fn write_file_tool_arc(&self) -> Option<Arc<dyn ironhermes_tools::registry::Tool>> {
        self.write_file_tool_arc.clone()
    }

    /// Phase 36.8 CR-01 (D-16): same as `write_file_tool_arc`, for `"patch"`.
    pub fn patch_tool_arc(&self) -> Option<Arc<dyn ironhermes_tools::registry::Tool>> {
        self.patch_tool_arc.clone()
    }

    /// Phase 36.8 plan 02 task 1: read-only introspection of the context-file
    /// candidate paths resolved at construction time (identity files under
    /// `IRONHERMES_HOME` + `CONTEXT_CANDIDATES` under `cwd`). Exists so the ACP
    /// crate's CLI-08 cwd-isolation contract test can assert on the REAL resolved
    /// paths of a per-session runtime, not a hand-rolled recomputation that would
    /// pass even if a future regression reverted to one shared runtime per process.
    pub fn context_file_paths(&self) -> &[PathBuf] {
        &self.context_file_paths
    }

    /// Run one top-level agent turn. This is the budget lifecycle boundary:
    /// the top-level `BudgetHandle` is reset to full here so a long-lived runtime
    /// never latches at `Stop100`. Plan 35-02 (D-01/D-04): subagents spawned
    /// during the turn each receive their own fresh `BudgetHandle::new(max_iterations)`
    /// in `run_child`; they no longer decrement the top-level counter.
    pub async fn run_turn(&self, mut req: TurnRequest) -> Result<AgentResult> {
        // ── budget lifecycle: refill before the turn ──────────────────────
        self.budget.reset();

        // Phase 50.4 (D-10/D-14): snapshot the reloadable config/resolver/client
        // ONCE for this turn, through the accessors — never a frozen bare field.
        // Matches D-10's guarantee that an in-flight turn finishes on the
        // config/resolver pair it started with: re-reading the accessors
        // repeatedly across this function body could observe a reload landing
        // mid-turn and tear the config, resolver and client apart from each
        // other inconsistently. The NEXT run_turn call re-snapshots and picks
        // up any reload that happened in between (D-10's "observed at the
        // start of the next turn").
        let config = self.config();
        let resolver = self.resolver();
        let client_snapshot = self.client();

        // ── Phase 39.2: black-box turn instrumentation ────────────────────
        let bb_start = std::time::Instant::now();
        let bb_run_id = req.turn_id.unwrap_or_else(uuid::Uuid::new_v4);
        if let Some(ref rec) = self.bb_recorder {
            rec.try_record(ironhermes_blackbox::BlackBoxRecorder::make_event(
                bb_run_id,
                ironhermes_blackbox::Stage::Input,
                "turn_started",
                "hermes-2026-06",
                serde_json::json!({
                    "session_id": &req.session_id,
                    "turn_id": bb_run_id.to_string(),
                }),
            ));
        }

        // Phase 36.17.7 D-01: register TTS tools for this turn's session.
        // `ToolRegistry::register` uses `HashMap::insert` (upsert by name — verified
        // in registry.rs:137) so repeated turns idempotently replace the previous
        // SendAudioTool instance with the current session's SessionKey + dispatcher.
        // No `unregister` call needed.
        if let Some(ref wiring) = req.tts_wiring {
            let mut reg = self.bundle.registry.write().await;
            reg.register_tts_tools(
                wiring.session_key.clone(),
                wiring.audio_dispatcher.clone(),
                config.clone(),
            );
            drop(reg);
        }

        // Phase 36.3.8 D-02/D-04/D-05: register send_message + clarify per turn.
        // Mirrors tts_wiring block above exactly. ToolRegistry::register is an
        // upsert by name so repeated turns idempotently replace the prior instances.
        // The clarify_registry Arc MUST be the same instance held by the gateway
        // callback loop so a button tap resolves the correct awaiter (T-36.3.8-ROUTE).
        if let Some(ref wiring) = req.messaging_wiring {
            let mut reg = self.bundle.registry.write().await;
            reg.register_messaging_tools(
                wiring.session_key.clone(),
                wiring.message_dispatcher.clone(),
                wiring.clarify_dispatcher.clone(),
                wiring.clarify_registry.clone(),
                wiring.cancel_token.clone(),
                config.clone(),
            );
            drop(reg);
        }

        // Phase 36.3.12 WR-05: track whether this turn installs a session-scoped
        // intercept (terminal and/or execute_code) so it can be evicted below once
        // the turn completes, on both the success and failure paths. Without this,
        // `ToolRegistry.intercepts[name].1[session_id]` — holding this turn's
        // captured closure (Arc<Config>, tool Arc, gate Arc) — is retained for the
        // process lifetime once installed by `register_intercepted_or_replace`
        // below; on the gateway's single shared `Arc<AgentRuntime>`, every distinct
        // session that ever ran a gated turn grew this map by one entry forever
        // (CR-01's own cleanup method, `unregister_intercepts_for_session`, was
        // fully implemented and unit-tested but never wired to a caller — WR-05).
        let mut session_intercept_installed: Option<String> = None;

        // Phase 45 D-11: per-turn terminal intercept (gateway surface only).
        // `register_intercepted_or_replace` steals the schema from the regular
        // `terminal` tool if it is already registered, so the LLM still sees the
        // tool — but calls are now routed through `handle_shell_exec` (with the
        // DangerousCommandGuardrail + ApprovalGate) instead of TerminalTool::execute.
        if let Some(handler) = req.terminal_intercept {
            let fallback = ironhermes_core::ToolSchema::new(
                "terminal",
                "Execute a shell command (gateway-gated; dangerous commands require approval)",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Shell command to execute" }
                    },
                    "required": ["command"]
                }),
            );
            let mut reg = self.bundle.registry.write().await;
            reg.register_intercepted_or_replace("terminal", &req.session_id, fallback, handler);
            drop(reg);
            session_intercept_installed = Some(req.session_id.clone());
        }

        // Phase 36.3.12 D-08/D-11: per-turn execute_code intercept — mirrors the
        // terminal_intercept block immediately above. `register_intercepted_or_replace`
        // steals the schema from the regular `execute_code` tool if it is already
        // registered, so the LLM still sees the tool — but calls are now routed
        // through the caller's gating closure (execute_gated_command, gate-only per
        // D-11) instead of `ExecuteCodeTool::execute` directly.
        if let Some(handler) = req.execute_code_intercept {
            let fallback = ironhermes_core::ToolSchema::new(
                "execute_code",
                "Execute a Python script in an isolated sandbox (gated; every resolution is \
                 audited). The script can call agent tools via 'from hermes_tools import \
                 <tool>'. Returns stdout, stderr, and exit code. Set background=true to run as \
                 a tracked background process (no sandbox; returns {process_id, pid} \
                 immediately).",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "code": {
                            "type": "string",
                            "description": "Python script source code to execute."
                        },
                        "background": {
                            "type": "boolean",
                            "description": "When true, spawn the script as a tracked background process (no sandbox, no RPC).",
                            "default": false
                        },
                        "watch_patterns": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Regex patterns to match against stdout/stderr lines (background mode only).",
                            "default": []
                        }
                    },
                    "required": ["code"]
                }),
            );
            let mut reg = self.bundle.registry.write().await;
            reg.register_intercepted_or_replace("execute_code", &req.session_id, fallback, handler);
            drop(reg);
            session_intercept_installed = Some(req.session_id.clone());
        }

        let context_length = resolver.resolve_for_main().context_length();

        // ── Phase 36.15 Plan 04 (PROV-11): per-turn extras resolution ─────
        // D-10: resolve (provider, model) → merged HashMap on every turn so a
        // mid-session /model switch picks up the new per-model override immediately.
        // resolver.main_provider() is the providers: map key; resolve_for_main()
        // .default_model is the wire model string LlmClient uses when None is passed.
        let resolved_extras_for_turn: Option<std::collections::HashMap<String, serde_json::Value>> = {
            let provider_name = resolver.main_provider();
            let model_name = resolver.resolve_for_main().default_model.clone();
            let merged = ironhermes_core::config_extras::resolve_extras(
                &config.providers,
                provider_name,
                &model_name,
            );
            if merged.is_empty() {
                None
            } else {
                Some(merged)
            }
        };

        // Phase 39.2: emit routing stage event after provider/model is resolved.
        if let Some(ref rec) = self.bb_recorder {
            let provider_name = resolver.main_provider();
            let model_name = resolver.resolve_for_main().default_model.clone();
            rec.try_record(ironhermes_blackbox::BlackBoxRecorder::make_event(
                bb_run_id,
                ironhermes_blackbox::Stage::Routing,
                "model_selected",
                "hermes-2026-06",
                serde_json::json!({
                    "provider_name": provider_name,
                    "model_id": model_name,
                }),
            ));
        }

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
                                        if let Ok(results) =
                                            serde_json::from_str::<Vec<serde_json::Value>>(
                                                &result_str,
                                            )
                                            && let Some(first) = results.first()
                                        {
                                            if let Some(content) =
                                                first.get("content").and_then(|v| v.as_str())
                                                && !content.is_empty()
                                            {
                                                return Ok(content.to_string());
                                            }
                                            // D-02: fall back to raw content on LLM-processing failure.
                                            if let Some(err) =
                                                first.get("error").and_then(|v| v.as_str())
                                            {
                                                return Err(format!("web_extract error: {}", err));
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
                    if (ctx_result.expanded || ctx_result.blocked)
                        && let Some(msg) = req.messages.get_mut(idx)
                    {
                        msg.content = Some(ironhermes_core::MessageContent::Text(
                            ctx_result.message.clone(),
                        ));
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

        // Phase 51 Plan 20 (G-51-7): per-turn provider identity — the label
        // that will be written into usage_events.provider and the api-key
        // that will be hashed into usage_events.api_key_hash. Defaulted to
        // the main provider exactly as before this fix, and overwritten
        // ONLY in the vision-routed arm below, in the SAME match arm that
        // swaps `turn_client` — so the label and the client that actually
        // ran can never disagree. This mirrors `with_fallback_named`'s
        // Cause E fix on the failover path; this is the vision-role variant
        // of the same rule (the row names the client that ran).
        let mut turn_provider_name = resolver.main_provider().to_string();
        let mut turn_api_key_for_usage = resolver.resolve_for_main().api_key.clone();

        // Vision auto-routing (fix): when this turn carries image content, run it
        // on the configured `roles.vision` model instead of the active chat model.
        // The active model may not support vision (e.g. kimi-k3), which the
        // provider rejects with a 400 "Image content is not supported by this
        // model". Falls back to the main client when the turn has no image OR no
        // vision role is resolvable — the provider error is then surfaced as-is
        // (now visible via the full-chain error surfacing).
        let (turn_client, vision_routed_to) = if messages_contain_image(&req.messages) {
            // Phase 51 Plan 20 (G-51-7): bind the resolver's named vision
            // resolution ONCE, evaluated only on image-bearing turns (the
            // same cost build_role_client's own resolve_role call already
            // pays below — one additional role resolution per image-bearing
            // turn, negligible next to an LLM call).
            let vision_named = resolver.resolve_role_named("vision");
            match build_role_client(&resolver, "vision") {
                Ok(Some(vision_client)) => {
                    let vm = vision_client.model().to_string();
                    tracing::info!(
                        vision_model = %vm,
                        active_model = %client_snapshot.model(),
                        "vision auto-route: turn carries image content; routing to the vision-role model"
                    );
                    // Phase 51 Plan 20 (G-51-7): overwrite the per-turn
                    // identity with the vision role's OWN provider name and
                    // api key — in this SAME arm that swaps turn_client, so
                    // the label is structurally inseparable from the client
                    // it describes.
                    if let Some((vision_provider_name, vision_endpoint)) = vision_named {
                        turn_provider_name = vision_provider_name;
                        turn_api_key_for_usage = vision_endpoint.api_key;
                    }
                    (vision_client, Some(vm))
                }
                _ => (client_snapshot.clone(), None),
            }
        } else {
            (client_snapshot.clone(), None)
        };

        // Transparency (user request): never switch models silently — surface the
        // auto-routed vision model as a leading note in the turn's own stream so
        // the user sees which model answered. Display-only (emitted through the
        // stream callback; not persisted into conversation history). No-op when
        // the surface supplied no stream callback (e.g. non-streaming callers).
        if let (Some(vm), Some(cb)) = (&vision_routed_to, req.stream.as_ref()) {
            cb(&format!("_[image → vision model `{vm}`]_\n\n"));
        }

        let mut agent = AgentLoop::new(
            turn_client,
            self.bundle.registry.clone(),
            self.max_iterations,
        )
        .with_budget(self.budget.clone())
        .with_hook_registry(self.bundle.hook_registry.clone())
        .with_browser_session(self.bundle.browser_session.clone())
        .with_active_skills(self.bundle.active_skills.clone())
        .with_compression(context_length, config.agent.context_compression)
        .with_compression_count(req.compression_count)
        // Phase 36.3.12 CR-01 (D-08/D-10): give AgentLoop the turn's REAL session_id.
        // Unconditional because `TurnRequest.session_id` is a plain `String`, never an
        // `Option`. Without this, `AgentLoop.session_id` stays `None` on every
        // production `run_turn` path, and every session-scoped intercept lookup added
        // for "terminal"/"execute_code" would miss (fail-closed) on every tool call —
        // Plan 09's own orientation note #5. `req.session_id` is cloned here because it
        // is moved later (into `attach_context_engine`, below).
        .with_session_id(req.session_id.clone());

        if let Some(ref mgr) = self.memory_manager {
            agent = agent.with_memory_manager(mgr.clone());
        }

        agent = wire_fallback_if_configured(agent, &resolver);

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
            // Phase 36.2 Plan 07 fix: `with_intercepts` only registers the
            // `session_search` tool intercept — it does NOT set
            // `AgentLoop::state_store`. Without this `with_state_store` call,
            // the post-LLM-call write site at agent_loop.rs:1018 (gated by
            // `if let Some(store) = &self.state_store`) silently skips on EVERY
            // turn that runs through the runtime — usage_events stays empty
            // and `sessions.input_tokens` / `output_tokens` / cost columns
            // never increment, breaking /usage and the Plan 10 status pills.
            agent = agent.with_state_store(store);
            // NOTE: `with_intercepts(None, Some(store), None, None, None)` was
            // also called here previously, which registered `session_search`
            // as a new tool the model could call. That tool was never wired on
            // the gateway pre-Phase 36.2; re-registering it on every turn (now
            // that all surfaces enable state_store) introduces a tool the
            // model didn't expect and can confuse multi-iteration tool flows.
            // The write site only needs `state_store`, not the intercept, so
            // it is intentionally omitted here. If a future surface needs
            // session_search exposed as a model tool, register it once on
            // AgentRuntime construction — not per-turn in run_turn.
        }

        // Phase 45 D-11: inject approval gate if the surface provided one.
        // Fail-closed: when None, the NeedsApproval arm in AgentLoop returns
        // GateUnavailable (consistent with the headless-CLI and web surfaces
        // that never wire a gate). Moving the gate out of req avoids cloning.
        if let Some(gate) = req.approval_gate {
            agent = agent.with_approval_gate(gate);
        }

        // Phase 36.2 code-review fix CR-02: wire provider name + api-key hash
        // source onto the AgentLoop so the post-LLM-call write site records
        // non-empty `usage_events.provider` and a per-key-derived
        // `api_key_hash`. Without this, every production row was written with
        // provider="" and a constant SHA-256-of-empty-string hash bucket —
        // making /usage --provider filters useless and (worse) collapsing
        // multi-tenant rate-limit tracking into a single shared bucket.
        //
        // Phase 51 Plan 20 (G-51-7): the rule is now "the row names the
        // client that ran," not "the row names the main provider" —
        // `turn_provider_name` / `turn_api_key_for_usage` are the per-turn
        // identity established above, defaulted from main and overwritten
        // only on the vision-routed arm. This is the second promoted
        // variant of the rule `with_fallback_named` (the Cause E fix)
        // already established on the failover path: two routing paths, one
        // shared "the row names the client that ran" contract.
        agent = agent.with_provider_name(turn_provider_name);
        // Phase 36.15 Plan 04 (PROV-11): wire per-turn merged extras resolved above.
        agent = agent.with_resolved_extras(resolved_extras_for_turn);
        if let Some(ref key) = turn_api_key_for_usage {
            agent = agent.with_api_key_for_usage_tracking(key.clone());
        }

        // Phase 36.2 follow-up: load the disk-resident pricing cache and merge
        // it into the per-turn `PricingRegistry`. Without this, every turn's
        // write_usage_success used the default `PricingRegistry::new()` which
        // reads ONLY the bundled `pricing.toml` — the entries operators add
        // via `hermes pricing refresh [--source openrouter]` were silently
        // ignored and `usage_events.cost_usd_micros` stayed at 0 for any model
        // not in the bundled table (notably every OpenRouter slug like
        // `google/gemini-3.5-flash`). Loading per-turn keeps the cache hot —
        // operators can refresh mid-session and the very next turn picks it
        // up without a restart. The load is a small synchronous JSON read
        // (file may not exist → returns default()).
        {
            let mut pricing = ironhermes_core::PricingRegistry::new();
            let cache = ironhermes_core::pricing_cache::PricingCache::load();
            pricing.merge_cache(cache.into_pricing_map());
            agent = agent.with_pricing_registry(std::sync::Arc::new(pricing));
        }

        // Phase 36.2 CR-04: wire the cache-break advisory state. The model-
        // swap warning needs the previous-turn model name plus a
        // "session-has-prior-turns" flag; the context-file-edit warning
        // needs the list of paths to snapshot. Without these, both triggers
        // are dead code — defined and tested but unreachable from any
        // production surface.
        let prior_turns = self
            .session_turn_count
            .load(std::sync::atomic::Ordering::Acquire);
        agent = agent.with_session_has_prior_turns(prior_turns > 0);
        if let Some(prev) = self.previous_model.lock().ok().and_then(|g| g.clone()) {
            agent = agent.with_previous_model(prev);
        }
        if !self.context_file_paths.is_empty() {
            agent = agent.with_context_file_paths(self.context_file_paths.clone());
        }

        agent = attach_context_engine(
            agent,
            &config,
            &resolver,
            req.session_id,
            Some(self.bundle.hook_registry.clone()),
            req.pressure_tracker,
            context_length,
            self.memory_manager.clone(),
        );

        // Phase 39.2: thread bb_recorder + bb_run_id into AgentLoop for Model-stage events.
        if let Some(ref rec) = self.bb_recorder {
            agent = agent.with_bb_recorder(Arc::clone(rec), bb_run_id);
        }

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
            let endpoint = resolver.resolve_for_main();
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
        let run_result = agent.run(req.messages).await;

        // Phase 39.2: emit output or error stage event after run completes.
        match &run_result {
            Ok(result) => {
                if let Some(ref rec) = self.bb_recorder {
                    rec.try_record(ironhermes_blackbox::BlackBoxRecorder::make_event(
                        bb_run_id,
                        ironhermes_blackbox::Stage::Output,
                        "turn_completed",
                        "hermes-2026-06",
                        serde_json::json!({
                            "finished_naturally": result.finished_naturally,
                            "total_latency_ms": bb_start.elapsed().as_millis() as u64,
                        }),
                    ));
                }
            }
            Err(e) => {
                if let Some(ref rec) = self.bb_recorder {
                    rec.try_record(ironhermes_blackbox::BlackBoxRecorder::make_event(
                        bb_run_id,
                        ironhermes_blackbox::Stage::Error,
                        "turn_failed",
                        "hermes-2026-06",
                        serde_json::json!({
                            "error_kind": e.to_string(),
                            "total_latency_ms": bb_start.elapsed().as_millis() as u64,
                        }),
                    ));
                }
            }
        }
        // Phase 36.3.12 WR-05: evict this turn's session-scoped intercept(s) now that
        // the turn has fully completed — `agent.run` above was awaited to completion,
        // so no further intercept dispatch for this session_id can occur until the
        // NEXT `run_turn` call re-installs it at the top of this function. Runs on
        // both the success and failure paths (placed before the `?` below), because
        // a failed turn leaves the closure captured in `ToolRegistry.intercepts` just
        // as a successful one would.
        if let Some(session_id) = session_intercept_installed {
            let mut reg = self.bundle.registry.write().await;
            reg.unregister_intercepts_for_session(&session_id);
            drop(reg);
        }

        let mut out = run_result?;

        // Phase 34b Plan 02 (D-09): post-run per-turn usage hook. MUST appear
        // AFTER agent.run (asserted in invariants_34b).
        if let Some(ref engine) = engine_handle {
            engine.update_from_response(&out.total_usage);
        }

        out.context_warnings = context_warnings;

        // Phase 36.2 CR-04: snapshot the just-run model name + bump the turn
        // counter so the NEXT turn can compare and fire the model-swap cache-
        // break warning if the operator swapped models. Uses the resolver's
        // currently-resolved main model — that is what `agent.client.model()`
        // exposed to the LLM. Stored unconditionally so a fast model-swap →
        // single-call → swap-back pattern still gets the prior name on the
        // intermediate turn.
        if let Ok(mut prev) = self.previous_model.lock() {
            *prev = Some(resolver.resolve_for_main().default_model.clone());
        }
        self.session_turn_count
            .fetch_add(1, std::sync::atomic::Ordering::Release);

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
    /// The current skill catalog. Returns an owned `Arc` rather than a borrow
    /// because the catalog is hot-swappable (see
    /// `AppRuntimeBundle::skill_registry`) — callers that cache the returned
    /// `Arc` across a reload keep observing the OLD catalog, so read it fresh at
    /// each use site instead of holding it in long-lived state.
    pub fn skill_registry(&self) -> Arc<SkillRegistry> {
        match self.bundle.skill_registry.read() {
            Ok(guard) => guard.clone(),
            // A panic in another reader/writer must not take the catalog down
            // with it — the value behind the lock is a plain `Arc` and is always
            // consistent, so recovering the poisoned inner value is safe.
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// The shared swappable handle itself, for surfaces that must observe
    /// reloads without going back through `AgentRuntime`.
    pub fn skill_registry_handle(&self) -> &Arc<std::sync::RwLock<Arc<SkillRegistry>>> {
        &self.bundle.skill_registry
    }

    /// Re-scan the configured skill search paths and swap the result in.
    ///
    /// Call after any write that changes what is on disk (install, create, fork,
    /// SKILL.md edit, delete). Returns what changed so callers can report it.
    /// The `skills` tool is re-registered against the new catalog so the agent
    /// can actually invoke a newly installed skill in the same process.
    pub async fn reload_skill_registry(
        &self,
        skills_config: &ironhermes_core::config::SkillsConfig,
    ) -> SkillRegistryReload {
        let fresh = Arc::new(SkillRegistry::load_with_config(
            &self.bundle.skills_cwd,
            skills_config,
        ));

        let before: std::collections::HashSet<String> = self
            .skill_registry()
            .list()
            .iter()
            .map(|r| r.name.clone())
            .collect();
        let after: std::collections::HashSet<String> =
            fresh.list().iter().map(|r| r.name.clone()).collect();
        let mut added: Vec<String> = after.difference(&before).cloned().collect();
        let mut removed: Vec<String> = before.difference(&after).cloned().collect();
        added.sort();
        removed.sort();
        let total = fresh.list().len();

        // Swap first, so any concurrent reader that beats the tool re-registration
        // still sees the new catalog rather than the old one.
        match self.bundle.skill_registry.write() {
            Ok(mut guard) => *guard = fresh.clone(),
            Err(poisoned) => *poisoned.into_inner() = fresh.clone(),
        }

        // Re-register the `skills` tool against the replacement. It captured an
        // `Arc<SkillRegistry>` by value at boot, so without this the agent would
        // keep listing and reading the pre-reload catalog even though every
        // display surface had moved on.
        {
            let mut tools = self.bundle.registry.write().await;
            tools.register_skills_tool(
                fresh,
                self.bundle.active_skills.clone(),
                self.bundle.skills_credential_dir.clone(),
                std::collections::HashMap::new(),
            );
        }

        SkillRegistryReload {
            added,
            removed,
            total,
        }
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
    /// The current cached main client. Returns an owned clone rather than a
    /// borrow because the client is hot-swappable (Phase 50.4 D-14) — callers
    /// that cache the returned value across a reload keep observing the OLD
    /// client, so read it fresh at each use site instead of holding it in
    /// long-lived state.
    pub fn client(&self) -> AnyClient {
        match self.client_handle.read() {
            Ok(guard) => guard.clone(),
            // A panic in another reader/writer must not take the client down
            // with it — the value behind the lock is a plain `AnyClient` and
            // is always consistent, so recovering the poisoned inner value
            // is safe.
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// The current config. Returns an owned `Arc` rather than a borrow
    /// because the config is hot-swappable (Phase 50.4 D-08/D-14) — callers
    /// that cache the returned `Arc` across a reload keep observing the OLD
    /// config, so read it fresh at each use site instead of holding it in
    /// long-lived state.
    pub fn config(&self) -> Arc<Config> {
        match self.config_handle.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// The current provider resolver. Same hot-swap contract as [`Self::config`].
    pub fn resolver(&self) -> Arc<ProviderResolver> {
        match self.resolver_handle.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Re-resolve `config`/`resolver` and swap them in, rebuilding the cached
    /// main client against the NEW resolver so the next `run_turn` reads the
    /// new provider/model/credential all the way through to the outbound LLM
    /// call (Phase 50.4 D-08/D-14).
    ///
    /// DEVIATES from [`Self::reload_skill_registry`]'s swap-first ordering,
    /// deliberately: that method's derived-state rebuild (re-registering the
    /// `skills` tool) is infallible, so swapping first is safe there. Here the
    /// derived state is `build_main_client(&resolver)`, which is fallible, and
    /// D-09 forbids a half-swap — a config/resolver pair that cannot produce a
    /// client must leave the previously-running config, resolver AND client
    /// all untouched. So: build the new client into a LOCAL first and
    /// propagate its error with `?` before touching any lock; only once the
    /// client exists do we take the three write guards and publish. Do not
    /// "fix" this back to `reload_skill_registry`'s order — that would
    /// reintroduce the half-swap D-09 exists to prevent.
    pub async fn reload_config_and_resolver(
        &self,
        config: Arc<Config>,
        resolver: Arc<ProviderResolver>,
    ) -> anyhow::Result<()> {
        // Build the new client and re-arm OpenRouter Claude cache_control
        // routing on it — the SAME pairing `from_config` performs — into a
        // local `mut client`, entirely before any lock is touched. A reload
        // that rebuilds the client but skips `enable_openrouter_caching`
        // would produce a fully valid, fully functional client that silently
        // serves a different caching posture than the one the operator was
        // running before they clicked APPLY NOW (D-14) — correct-looking,
        // green, and wrong.
        let mut client = build_main_client(&resolver)?;
        client.enable_openrouter_caching(
            resolver.main_provider().to_string(),
            config.prompt_caching.clone(),
        );

        // Nothing above this point touched a lock — a `build_main_client`
        // failure returns via `?` with the previously-running config,
        // resolver and client all still in place (D-09).
        match self.config_handle.write() {
            Ok(mut guard) => *guard = config,
            Err(poisoned) => *poisoned.into_inner() = config,
        }
        match self.resolver_handle.write() {
            Ok(mut guard) => *guard = resolver,
            Err(poisoned) => *poisoned.into_inner() = resolver,
        }
        match self.client_handle.write() {
            Ok(mut guard) => *guard = client,
            Err(poisoned) => *poisoned.into_inner() = client,
        }

        Ok(())
    }
    /// Returns the MCP manager handle built during `from_config`, if any MCP
    /// servers were configured. Used by `run_gateway` to wire the shutdown path
    /// so `ironhermes gateway` exits in bounded time on Ctrl+C.
    pub fn mcp_manager(&self) -> Option<&Arc<ironhermes_mcp::McpManager>> {
        self.bundle.mcp_manager.as_ref()
    }
    /// Phase 39.2: attach a black-box recorder. Called by `app_runtime_factory`
    /// after `from_config`; `None` by default (recording disabled).
    pub fn with_bb_recorder(
        mut self,
        recorder: Arc<ironhermes_blackbox::BlackBoxRecorder>,
    ) -> Self {
        self.bb_recorder = Some(recorder);
        self
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
        Self::for_tests_with_base_url("http://localhost:0")
    }

    /// Like [`Self::for_tests`], but the `AnyClient` points at `base_url`
    /// instead of the unreachable `localhost:0` sink (Phase 47.6 Plan 09).
    ///
    /// This lets an integration test stand up a `wiremock::MockServer`
    /// serving a canned chat-completions SSE stream and drive a REAL
    /// `AgentRuntime::run_turn` end-to-end — real `AgentLoop`, real tool
    /// registry, real hook registry — against that canned response, instead
    /// of hand-driving the production composition (the approach
    /// `telegram_media_delivery.rs` uses when a network response cannot be
    /// simulated; see that file's module doc for why a `Fake` `AnyClient`
    /// variant or an invasive delta-injection point was rejected there).
    /// Still gated `#[cfg(any(test, feature = "test-support"))]` — this adds
    /// no new non-test code path.
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_tests_with_base_url(base_url: impl Into<String>) -> Self {
        Self::for_tests_inner(base_url.into())
    }

    #[cfg(any(test, feature = "test-support"))]
    fn for_tests_inner(base_url: String) -> Self {
        use crate::app_runtime_factory::AppRuntimeBundle;
        use ironhermes_core::{Config, ProviderResolver, SkillRegistry};
        use ironhermes_hooks::HookRegistry;
        use ironhermes_tools::ToolRegistry;
        use std::sync::Arc;
        use tokio::sync::RwLock;

        let config = Arc::new(Config::default());
        let resolver = Arc::new(
            ProviderResolver::build(&config)
                .expect("ProviderResolver::build with default Config must succeed in test context"),
        );

        // Use ChatCompletions client pointing to `base_url`. The zero-arg
        // `for_tests()` passes "http://localhost:0" (won't connect, but
        // provides a valid AnyClient for struct construction);
        // `for_tests_with_base_url` lets a caller point this at a real
        // (test-only) HTTP endpoint, e.g. a `wiremock::MockServer`.
        let client = crate::AnyClient::ChatCompletions(crate::client::LlmClient::new(
            base_url,
            "test-key",
            "test-model",
        ));

        let max_iterations = config.agent.max_iterations;
        let budget = crate::budget::BudgetHandle::new(max_iterations);

        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let hook_registry = Arc::new(HookRegistry::new(ironhermes_hooks::HooksConfig::default()));
        // load_with_paths(&[]) produces an empty SkillRegistry without touching disk.
        let skill_registry = Arc::new(std::sync::RwLock::new(Arc::new(
            SkillRegistry::load_with_paths(&[]),
        )));
        let active_skills = Arc::new(std::sync::Mutex::new(Vec::new()));
        let cron_dir =
            std::env::temp_dir().join(format!("ironhermes_test_cron_{}", std::process::id()));
        let job_store = Arc::new(std::sync::Mutex::new(
            ironhermes_cron::JobStore::open(cron_dir)
                .expect("temp-dir JobStore must succeed in test context"),
        ));
        let browser_session = Arc::new(tokio::sync::Mutex::new(None));

        let bundle = AppRuntimeBundle {
            registry,
            hook_registry,
            skill_registry,
            skills_cwd: std::env::temp_dir(),
            skills_credential_dir: std::env::temp_dir(),
            active_skills,
            job_store,
            browser_session,
            mcp_manager: None,
            merged_tools: ironhermes_core::config::ToolsConfig::default(),
            bb_recorder: None, // Phase 39.2: disabled in test helpers
        };

        let subagent_registry = Arc::new(RwLock::new(
            crate::subagent_registry::SubagentRegistry::new(),
        ));

        Self {
            config_handle: Arc::new(std::sync::RwLock::new(config)),
            resolver_handle: Arc::new(std::sync::RwLock::new(resolver)),
            client_handle: Arc::new(std::sync::RwLock::new(client)),
            bundle,
            budget,
            memory_manager: None,
            subagent_registry,
            max_iterations,
            cwd: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            previous_model: std::sync::Mutex::new(None),
            session_turn_count: std::sync::atomic::AtomicUsize::new(0),
            context_file_paths: Vec::new(),
            bb_recorder: None, // Phase 39.2: disabled in test helpers
            // Phase 36.3.12 D-08: this test helper builds a minimal registry with
            // no "terminal"/"execute_code" tools registered — no capture needed.
            terminal_tool_arc: None,
            execute_code_tool_arc: None,
            write_file_tool_arc: None,
            patch_tool_arc: None,
        }
    }

    /// Phase 36.15 Plan 04 (PROV-11): test-only helper that re-derives
    /// `(provider, model) → merged extras` using the same logic as `run_turn`.
    ///
    /// Allows unit tests to assert on the extras resolution result without
    /// running a full async `run_turn` (which requires a live LLM endpoint).
    #[cfg(test)]
    pub(crate) fn resolved_extras_for_test_turn(
        &self,
    ) -> Option<std::collections::HashMap<String, serde_json::Value>> {
        let resolver = self.resolver();
        let config = self.config();
        let provider_name = resolver.main_provider();
        let model_name = resolver.resolve_for_main().default_model.clone();
        let merged = ironhermes_core::config_extras::resolve_extras(
            &config.providers,
            provider_name,
            &model_name,
        );
        if merged.is_empty() {
            None
        } else {
            Some(merged)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Source text for this file — used by position-guard assertions below.
    const SOURCE: &str = include_str!("agent_runtime.rs");

    /// Vision auto-routing (fix): the image detector fires only for a message
    /// that actually carries a `ContentPart::ImageUrl` in a `Parts` body.
    #[test]
    fn messages_contain_image_detects_image_parts() {
        use ironhermes_core::{ChatMessage, ContentPart, ImageUrl, MessageContent, Role};
        let mk = |content: MessageContent| ChatMessage {
            role: Role::User,
            content: Some(content),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            is_recall_context: false,
        };
        // text-only (both the plain-Text and a Parts-of-only-text form) → no image
        assert!(!messages_contain_image(&[mk(MessageContent::Text(
            "hi".into()
        ))]));
        assert!(!messages_contain_image(&[mk(MessageContent::Parts(vec![
            ContentPart::Text { text: "x".into() },
        ]))]));
        // Parts carrying an ImageUrl → image present
        assert!(messages_contain_image(&[mk(MessageContent::Parts(vec![
            ContentPart::Text {
                text: "look".into(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "data:image/png;base64,AAAA".into(),
                    detail: None,
                },
            },
        ]))]));
    }

    /// Wiring guard (mirrors the budget-reset position guard): `run_turn` MUST
    /// select the `roles.vision` client for image-bearing turns, gated on
    /// `messages_contain_image`, BEFORE constructing `AgentLoop::new`. Prevents
    /// the auto-route from silently going inert in a future refactor.
    #[test]
    fn run_turn_routes_image_turns_to_vision_role() {
        // Phase 50.4 (D-14): the field was renamed to `resolver_handle`, so the
        // call site is now the accessor-derived local `resolver`, not a bare
        // `self.resolver` field — update the needle to match, per this plan's
        // own instruction not to delete this test.
        let route_pos = SOURCE
            .find("build_role_client(&resolver, \"vision\")")
            .expect("run_turn must route image turns via build_role_client(.., \"vision\")");
        let loop_pos = SOURCE
            .find("AgentLoop::new(")
            .expect("AgentLoop::new( must be present in run_turn");
        assert!(
            route_pos < loop_pos,
            "vision auto-route client selection must occur BEFORE AgentLoop::new"
        );
        assert!(
            SOURCE.contains("messages_contain_image(&req.messages)"),
            "vision routing must be gated on messages_contain_image(&req.messages)"
        );
    }

    /// INV-51-20 (G-51-7): a vision-auto-routed turn must be written into
    /// `usage_events` under the CLIENT THAT ACTUALLY RAN, not the main
    /// provider's name. UAT test 6 observed `run_turn` route to the vision
    /// role's client yet stamp every usage row `provider=openrouter` — the
    /// main provider — an internally contradictory row (openrouter rejects
    /// the vision model id with a 400). This mirrors the already-fixed
    /// failover case (`with_fallback_named`, the Cause E fix).
    ///
    /// `SOURCE` is `include_str!` of this ENTIRE file, tests included. A
    /// negative assertion built from a contiguous string literal written
    /// inside this test would match its own source line and could never go
    /// red-to-green — the self-invalidation hazard documented at
    /// `crates/ironhermes-kanban/src/dispatcher.rs:1936`. Every needle below
    /// is therefore assembled at RUNTIME from fragments, never a contiguous
    /// literal anywhere in this file. Do not "simplify" this back into a
    /// plain string literal.
    #[test]
    fn inv_51_20_vision_routed_turn_is_labelled_by_the_client_that_ran() {
        let non_comment: String = SOURCE
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        // (1) The singular assumption — labelling the turn with the
        // resolver's main-provider accessor DIRECTLY as the wiring
        // argument, regardless of which client actually ran — must be gone
        // from non-comment source.
        let singular_provider_label_needle = format!(
            "with_provider_name({}{}{})",
            "resolver.main", "_provider", "()"
        );
        assert!(
            !non_comment.contains(singular_provider_label_needle.as_str()),
            "Phase 51 Plan 20 (G-51-7): run_turn must NOT label the turn with \
             resolver.main_provider() directly — the usage_events row must \
             name the client that actually ran the turn (vision-routed or \
             not), the same rule with_fallback_named already enforces on \
             the failover path."
        );

        // (2) The Phase 36.2 CR-02 wiring calls must still exist verbatim —
        // this fix changes their ARGUMENTS, never removes or conditionally
        // skips either call site.
        let provider_name_call_needle = format!("agent.{}{}(", "with_provider", "_name");
        let api_key_call_needle = format!("agent.{}{}(", "with_api_key_for_usage", "_tracking");
        assert!(
            non_comment.contains(provider_name_call_needle.as_str()),
            "Phase 36.2 CR-02 must still hold: agent.with_provider_name(...) call site required."
        );
        assert!(
            non_comment.contains(api_key_call_needle.as_str()),
            "Phase 36.2 CR-02 must still hold: agent.with_api_key_for_usage_tracking(...) call site required."
        );

        // (3) The per-turn provider-name identity must be established BEFORE
        // AgentLoop::new(, exactly like the sibling vision-route guard
        // (`run_turn_routes_image_turns_to_vision_role`) asserts for the
        // client selection itself — so the label and the client can never
        // disagree about which arm produced them.
        let turn_identity_needle = format!("{}_{}", "turn_provider", "name");
        let identity_pos = SOURCE.find(turn_identity_needle.as_str()).expect(
            "run_turn must introduce a per-turn provider-name identity binding \
             (turn_provider_name) sourced from the client that actually ran",
        );
        let loop_pos = SOURCE
            .find("AgentLoop::new(")
            .expect("AgentLoop::new( must be present in run_turn");
        assert!(
            identity_pos < loop_pos,
            "the per-turn provider-name identity must be established before AgentLoop::new"
        );
    }

    /// Phase 51 Plan 20 (G-51-7) Task 2(a) — non-vision regression control.
    /// A turn with NO image content must leave the per-turn identity at the
    /// main provider's name and the main endpoint's api key —
    /// byte-identical to pre-Plan-20 behavior. Constructing a real
    /// `AgentRuntime` with a live client to drive an actual non-image
    /// `run_turn` is not available in this test module (the same
    /// constraint `run_turn_reads_the_reloadable_handles_not_frozen_fields`
    /// and its siblings face), so this is proven at two levels instead:
    /// (1) a resolver-level equality, since `resolve_for_main()`'s endpoint
    /// IS what a non-routed turn's identity is derived from —
    /// `resolve_for_main()` looks up `self.endpoints[self.main_provider]`,
    /// so asserting `resolve(main_provider())` agrees with
    /// `resolve_for_main()` pins that the two accessors run_turn's default
    /// bindings compose from can never drift apart; (2) a source-position
    /// assertion that the default bindings are established BEFORE the
    /// image-gated vision block that could overwrite them, so the common
    /// (non-image) path provably reaches `AgentLoop::new` with the
    /// defaults untouched.
    #[test]
    fn non_vision_turn_identity_matches_main_provider_established_before_vision_arm() {
        let mut config = Config::default();
        config.model.roles.insert(
            "vision".to_string(),
            ironhermes_core::ModelRoleConfig {
                provider: "anthropic".to_string(),
                model: Some("claude-vision".to_string()),
            },
        );
        let resolver = ProviderResolver::build(&config).expect("build");

        // (1) Resolver-level equality: the endpoint run_turn's default
        // identity is DERIVED from (resolve_for_main()) can never disagree
        // with a direct lookup keyed by main_provider() — the same
        // accessor run_turn's default provider-name binding calls.
        let main_ep = resolver.resolve_for_main();
        let via_name = resolver
            .resolve(resolver.main_provider())
            .expect("main_provider() must resolve to a real endpoint");
        assert_eq!(
            main_ep.base_url, via_name.base_url,
            "resolve_for_main() must agree with resolve(main_provider()) — the two \
             accessors a non-routed turn's default identity composes from"
        );
        assert_eq!(main_ep.api_key, via_name.api_key);

        // (2) Source position: the default bindings must be established
        // BEFORE the image-gated vision block, so a non-image turn's
        // identity is never touched by the vision-routing arm at all.
        let default_binding_needle = format!("{}_{}", "turn_provider", "name");
        let default_pos = SOURCE
            .find(default_binding_needle.as_str())
            .expect("the default turn_provider_name binding must exist in run_turn");
        let image_gate_pos = SOURCE
            .find("messages_contain_image(&req.messages)")
            .expect("the image-content gate must exist in run_turn");
        assert!(
            default_pos < image_gate_pos,
            "the default per-turn identity must be established BEFORE the \
             image-content gate, so a non-image turn's identity is never \
             touched by the vision-routing arm"
        );
    }

    /// Phase 50.4 (D-14, Test 4): `run_turn` must snapshot the reloadable
    /// config/resolver through the `AgentRuntime::config()` / `::resolver()`
    /// accessors, not a frozen bare field — an implementation that reverted to
    /// direct `self.resolver`/`self.config` field reads would silently keep
    /// serving the pre-reload provider for the whole life of the runtime,
    /// because `run_turn` only ever reads what it snapshots at turn start.
    #[test]
    fn run_turn_reads_the_reloadable_handles_not_frozen_fields() {
        assert!(
            SOURCE.contains("let resolver = self.resolver();"),
            "run_turn must read the resolver through the AgentRuntime::resolver() accessor \
             (a snapshot local named `resolver`), not a frozen self.resolver field, so a \
             mid-session reload is observed starting on the next turn (D-14)."
        );
        assert!(
            SOURCE.contains("let config = self.config();"),
            "run_turn must read the config through the AgentRuntime::config() accessor \
             (a snapshot local named `config`), not a frozen self.config field, so a \
             mid-session reload is observed starting on the next turn (D-14)."
        );
    }

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

    /// INV-36.2-07-RUNTIME: Phase 36.2 Plan 07 regression net.
    /// When `req.state_store` is `Some(...)`, `run_turn` MUST call
    /// `with_state_store(...)` on the per-turn `AgentLoop` (not just
    /// `with_intercepts(...)`). `with_intercepts` only registers the
    /// `session_search` tool intercept; it does NOT set `AgentLoop.state_store`.
    /// Without `with_state_store`, the post-LLM-call write site in
    /// `agent_loop.rs` (`if let Some(store) = &self.state_store`) silently
    /// skips on every turn that runs through the runtime — `usage_events`
    /// stays empty, `sessions.input_tokens`/`output_tokens`/cost columns
    /// never increment, /usage shows "no data", and the Plan 10 status pills
    /// never render.
    #[test]
    fn inv_36_2_07_runtime_calls_with_state_store_before_with_intercepts() {
        let non_comment: String = SOURCE
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        let with_state_store_pos = non_comment.find("agent.with_state_store(");

        assert!(
            with_state_store_pos.is_some(),
            "Phase 36.2 Plan 07: run_turn MUST call `agent.with_state_store(store)` so \
             the post-LLM-call write site receives the state store. Otherwise \
             usage_events writes silently skip on every turn."
        );

        // Phase 36.2 follow-up: the `with_intercepts(None, Some(store), ...)`
        // call was REMOVED from run_turn because registering session_search
        // as a per-turn tool intercept confused multi-iteration tool flows
        // (chat truncation observed on gateway after enabling state_store).
        // The write site only needs state_store, not the intercept. This
        // assertion locks the removal — if anyone re-adds it, debug carefully.
        let intercept_needle = concat!(".with_intercepts(None, Some(", "store)");
        assert!(
            !non_comment.contains(intercept_needle),
            "Phase 36.2 follow-up: run_turn must NOT call with_intercepts to register \
             session_search per-turn. Tool registration must happen once on AgentRuntime \
             construction — not in run_turn. See agent_runtime.rs comment for context."
        );
    }

    /// WR-05 regression test (36.3.12-REVIEW.md): `run_turn` must evict a turn's
    /// session-scoped `"terminal"` intercept from the shared `ToolRegistry` once
    /// the turn completes — otherwise every distinct session that ever installs
    /// one leaks an entry in `ToolRegistry.intercepts["terminal"].1` for the life
    /// of the process. `unregister_intercepts_for_session` (registry.rs) was
    /// fully implemented and unit-tested but had zero call sites before this fix
    /// — this test exercises the real call site in `run_turn`, not the registry
    /// method directly, so a future edit that drops or misplaces the call is
    /// caught here.
    ///
    /// Uses `AgentRuntime::for_tests()`, whose client points at `localhost:0` —
    /// `run_turn` always returns `Err` (connection refused) on the network call
    /// (per `for_tests()`'s own doc comment). This is intentional, not
    /// incidental: the WR-05 eviction step in `run_turn` runs BEFORE the
    /// `run_result?` early return, so exercising the failure path proves
    /// eviction happens on both the `Ok` and `Err` outcomes, not just success.
    ///
    /// Pre-fix (eviction call site removed from `run_turn`), this test failed
    /// immediately on the first turn (each iteration re-asserts, so the failure
    /// fires as soon as one session's intercept is left behind — verified by
    /// temporarily disabling the eviction block and re-running):
    /// ```text
    /// thread '...' panicked at crates/ironhermes-agent/src/agent_runtime.rs:...:
    /// assertion `left == right` failed: after session wr05-sess-0 (turn 1), \
    /// "terminal" must have 0 session-scoped intercepts left registered in \
    /// ToolRegistry — got 1. run_turn must evict this turn's intercept after \
    /// the turn completes (WR-05); a leak here means every distinct session \
    /// that ever installs a terminal intercept permanently grows the \
    /// registry's intercepts map.
    ///   left: 1
    ///  right: 0
    /// ```
    #[tokio::test]
    async fn wr05_terminal_intercept_does_not_accumulate_across_sessions() {
        let runtime = AgentRuntime::for_tests();

        for i in 0..3 {
            let session_id = format!("wr05-sess-{i}");
            let req = TurnRequest {
                session_id: session_id.clone(),
                messages: vec![ChatMessage::user("hi")],
                terminal_intercept: Some(std::sync::Arc::new(|_args| {
                    Box::pin(async { Ok("noop".to_string()) })
                })),
                ..Default::default()
            };

            let result = runtime.run_turn(req).await;
            assert!(
                result.is_err(),
                "AgentRuntime::for_tests()'s client points at localhost:0 — run_turn \
                 must fail to connect on every turn in this test (by design, per its \
                 own doc comment); a non-error result means this test's assumptions \
                 about for_tests() have changed and it needs to be revisited"
            );

            let count = runtime
                .bundle
                .registry
                .read()
                .await
                .intercept_session_count("terminal");
            assert_eq!(
                count,
                0,
                "after session {session_id} (turn {}), \"terminal\" must have 0 \
                 session-scoped intercepts left registered in ToolRegistry — got \
                 {count}. run_turn must evict this turn's intercept after the turn \
                 completes (WR-05); a leak here means every distinct session that \
                 ever installs a terminal intercept permanently grows the \
                 registry's intercepts map.",
                i + 1,
            );
        }
    }

    /// INV-36.2-CR-09: Phase 36.2 code-review CR-09 regression net.
    /// `AgentRuntime::from_config` MUST call `enable_openrouter_caching` on
    /// the freshly-built `AnyClient` so the streaming send path can route
    /// OpenRouter Claude requests through the `cache_control`-attaching
    /// builder. Pre-fix the Plan 11 OpenRouter Claude wiring was defined
    /// and unit-tested but never invoked from any production code — Claude
    /// via OpenRouter never received cache_control markers, so the cache
    /// hits Plan 11 was designed to deliver never fired.
    #[test]
    fn inv_36_2_cr_09_from_config_enables_openrouter_caching() {
        let non_comment: String = SOURCE
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            non_comment.contains("client.enable_openrouter_caching("),
            "Phase 36.2 CR-09: from_config MUST call `client.enable_openrouter_caching(...)` \
             so the streaming send path routes OpenRouter Claude requests through the \
             cache_control-attaching builder (build_openrouter_chat_request_full)."
        );
    }

    /// Phase 50.4 (D-14, Test 5): widens the CR-09 invariant above to also
    /// cover `reload_config_and_resolver`. `enable_openrouter_caching` stamps
    /// private state on the inner `LlmClient` with no public read-back, so the
    /// call site itself is the only observable proof a reload re-arms it —
    /// this MUST stay a source assertion, not be "upgraded" into a behavioral
    /// one, because there is no field to read the caching posture back from.
    #[test]
    fn inv_50_4_reload_config_and_resolver_reapplies_openrouter_caching() {
        // Scope to PRODUCTION code only (before `mod tests {`) — the plain
        // `.contains()` checks elsewhere in this file are safe to run over
        // the whole SOURCE, but this test does exact counting and byte-range
        // comparison, and the assertion/panic messages in THIS test module
        // (this one included) themselves contain the literal substring
        // "client.enable_openrouter_caching(" as prose, which would inflate
        // the count and confuse `rfind` if not excluded.
        let test_mod_start = SOURCE
            .find("\nmod tests {")
            .expect("test module marker must exist");
        let production_source = &SOURCE[..test_mod_start];

        let non_comment: String = production_source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        let occurrences = non_comment
            .matches("client.enable_openrouter_caching(")
            .count();
        assert!(
            occurrences >= 2,
            "Phase 50.4 D-14: `client.enable_openrouter_caching(` must appear at least twice \
             in the comment-stripped source — once in `from_config`, once in \
             `reload_config_and_resolver` — found {occurrences}."
        );

        // NOTE: deliberately searches for "async fn reload_..." rather than
        // "pub async fn reload_..." — the plan's own acceptance criterion
        // greps this file for the exact substring "pub async fn
        // reload_config_and_resolver" and expects exactly ONE match (the
        // real declaration); using the identical needle here would make
        // this test's own source line a second match.
        let method_start = non_comment
            .find("async fn reload_config_and_resolver(")
            .expect("reload_config_and_resolver must exist");
        // The method's own end: the next 4-space-indented `pub ` item after its
        // start, which is where rustfmt places the next impl-block method.
        let method_end = non_comment[method_start..]
            .find("\n    pub ")
            .map(|offset| method_start + offset)
            .unwrap_or(non_comment.len());
        let second_call_pos = non_comment
            .rfind("client.enable_openrouter_caching(")
            .expect("at least one enable_openrouter_caching call must exist");
        assert!(
            second_call_pos >= method_start && second_call_pos < method_end,
            "Phase 50.4 D-14: the SECOND (last) `enable_openrouter_caching` call must fall \
             inside reload_config_and_resolver's own body ({method_start}..{method_end}), but \
             was found at byte {second_call_pos} — a bare occurrence count of 2 would also be \
             satisfied by two calls inside `from_config` alone, which is exactly the gap this \
             test exists to close."
        );
    }

    // ── Phase 50.4 (D-08/D-09/D-14): reload_config_and_resolver behavioral tests ──

    /// Build a `(Config, ProviderResolver)` pair naming a single custom
    /// provider, for the reload tests below.
    #[cfg(test)]
    fn build_test_provider_config(
        provider_name: &str,
        base_url: &str,
        model: &str,
    ) -> (Arc<Config>, Arc<ProviderResolver>) {
        let mut config = Config::default();
        let provider_cfg = ironhermes_core::config::ProviderConfig {
            base_url: Some(base_url.to_string()),
            // A custom (non-built-in) provider entry's `default_model` comes
            // from `providers.<name>.default_model`, NOT `config.model.default`
            // — that global field only pre-seeds the built-in openrouter entry.
            default_model: Some(model.to_string()),
            ..ironhermes_core::config::ProviderConfig::default()
        };
        config.providers.insert(provider_name.to_string(), provider_cfg);
        config.model.provider = provider_name.to_string();
        config.model.default = model.to_string();
        let config = Arc::new(config);
        let resolver = Arc::new(
            ProviderResolver::build(&config)
                .expect("ProviderResolver::build must succeed with a real base_url"),
        );
        (config, resolver)
    }

    /// Build a minimal, fully-functional `AgentRuntime` around a caller-supplied
    /// config/resolver/client, for the reload tests below. Mirrors the manual
    /// construction `resolved_extras_for_test_turn_returns_provider_extras`
    /// already uses, adapted to the Phase 50.4 handle fields.
    #[cfg(test)]
    fn build_runtime_for_reload_test(
        config: Arc<Config>,
        resolver: Arc<ProviderResolver>,
        client: AnyClient,
    ) -> AgentRuntime {
        let max_iterations = config.agent.max_iterations;
        let budget = crate::budget::BudgetHandle::new(max_iterations);
        let registry = Arc::new(tokio::sync::RwLock::new(ironhermes_tools::ToolRegistry::new()));
        let hook_registry = Arc::new(HookRegistry::new(ironhermes_hooks::HooksConfig::default()));
        let skill_registry = Arc::new(std::sync::RwLock::new(Arc::new(
            SkillRegistry::load_with_paths(&[]),
        )));
        let active_skills = Arc::new(std::sync::Mutex::new(Vec::new()));
        // Unique per-call temp dir — several tests in this module build their own
        // runtime in the same process, and JobStore::open needs a fresh path each
        // time to avoid colliding with a sibling test's still-open store.
        static CRON_DIR_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let cron_dir = std::env::temp_dir().join(format!(
            "ironhermes_test_cron_reload_{}_{}",
            std::process::id(),
            CRON_DIR_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let job_store = Arc::new(std::sync::Mutex::new(
            ironhermes_cron::JobStore::open(cron_dir).expect("temp-dir JobStore must succeed"),
        ));
        let browser_session = Arc::new(TokioMutex::new(None));
        let bundle = AppRuntimeBundle {
            registry,
            hook_registry,
            skill_registry,
            skills_cwd: std::env::temp_dir(),
            skills_credential_dir: std::env::temp_dir(),
            active_skills,
            job_store,
            browser_session,
            mcp_manager: None,
            merged_tools: ironhermes_core::config::ToolsConfig::default(),
            bb_recorder: None,
        };
        let subagent_registry = Arc::new(RwLock::new(SubagentRegistry::new()));

        AgentRuntime {
            config_handle: Arc::new(std::sync::RwLock::new(config)),
            resolver_handle: Arc::new(std::sync::RwLock::new(resolver)),
            client_handle: Arc::new(std::sync::RwLock::new(client)),
            bundle,
            budget,
            memory_manager: None,
            subagent_registry,
            max_iterations,
            cwd: std::path::PathBuf::from("."),
            previous_model: std::sync::Mutex::new(None),
            session_turn_count: std::sync::atomic::AtomicUsize::new(0),
            context_file_paths: Vec::new(),
            bb_recorder: None,
            terminal_tool_arc: None,
            execute_code_tool_arc: None,
            write_file_tool_arc: None,
            patch_tool_arc: None,
        }
    }

    /// Test 1: reloading with a resolver built from a config naming a
    /// DIFFERENT main provider swaps what `resolver()`/`config()` report —
    /// the assertion an `AppState`-only implementation (D-14's central
    /// finding) fails, since it never touches `AgentRuntime` at all.
    #[tokio::test]
    async fn reload_config_and_resolver_swaps_the_provider_the_next_turn_reads() {
        let (config_a, resolver_a) =
            build_test_provider_config("provider_a", "https://provider-a.example.test", "model-a");
        let client_a = AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "https://provider-a.example.test",
            "key-a",
            "model-a",
        ));
        let runtime = build_runtime_for_reload_test(config_a, resolver_a, client_a);

        assert_eq!(runtime.resolver().main_provider(), "provider_a");

        let (config_b, resolver_b) =
            build_test_provider_config("provider_b", "https://provider-b.example.test", "model-b");

        runtime
            .reload_config_and_resolver(config_b, resolver_b)
            .await
            .expect("reload must succeed — provider B has a real base_url");

        assert_eq!(runtime.resolver().main_provider(), "provider_b");
        assert_eq!(runtime.config().model.default, "model-b");
    }

    /// Test 2: reloading rebuilds the CACHED MAIN CLIENT against the new
    /// resolver — the property D-14 says a labels-only (`AppState`-only)
    /// reload fails to move. Asserts the before-values explicitly: a test
    /// that only checked the post-reload state would pass against an
    /// implementation that was already pointing at B.
    #[tokio::test]
    async fn reload_config_and_resolver_rebuilds_the_cached_main_client() {
        let (config_a, resolver_a) =
            build_test_provider_config("provider_a", "https://provider-a.example.test", "model-a");
        let client_a = AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "https://provider-a.example.test",
            "key-a",
            "model-a",
        ));
        let runtime = build_runtime_for_reload_test(config_a, resolver_a, client_a);

        assert_eq!(runtime.client().base_url(), "https://provider-a.example.test");
        assert_eq!(runtime.client().model(), "model-a");

        let (config_b, resolver_b) =
            build_test_provider_config("provider_b", "https://provider-b.example.test", "model-b");

        runtime
            .reload_config_and_resolver(config_b, resolver_b)
            .await
            .expect("reload must succeed — provider B has a real base_url");

        assert_eq!(runtime.client().base_url(), "https://provider-b.example.test");
        assert_eq!(runtime.client().model(), "model-b");
    }

    /// Test 3 (D-09): a reload whose main endpoint cannot produce a client
    /// returns Err AND leaves the resolver, config and cached client all on
    /// their pre-reload values.
    ///
    /// The unbuildable endpoint is a REAL misconfiguration, not a fabricated
    /// one: `ProviderResolver::build` deliberately lets an unrecognized/
    /// misconfigured `model.provider` name through (its own comment: "allow
    /// build to succeed so operators can introspect... [failure caught at]
    /// resolve_for_main() time"), so a `providers.<name>` entry with NO
    /// `base_url` produces a resolver whose main endpoint has an empty
    /// `base_url`. `AnyClient::from_endpoint` previously accepted that
    /// silently (every match arm unconditionally returned `Ok`, so
    /// `build_main_client` — the ONE fallible call in
    /// `reload_config_and_resolver` — was provably unable to fail with any
    /// resolver reachable through the public API); this plan adds the
    /// missing empty-`base_url` validation there (Rule 2), which is what
    /// this test exercises.
    #[tokio::test]
    async fn reload_config_and_resolver_publishes_nothing_when_the_client_cannot_be_built() {
        let (config_a, resolver_a) =
            build_test_provider_config("provider_a", "https://provider-a.example.test", "model-a");
        let client_a = AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "https://provider-a.example.test",
            "key-a",
            "model-a",
        ));
        let runtime = build_runtime_for_reload_test(config_a, resolver_a, client_a);

        // Provider B: a real config entry with NO base_url — resolvable
        // (build succeeds), but not buildable into a client.
        let mut config_b = Config::default();
        config_b.providers.insert(
            "provider_b_broken".to_string(),
            ironhermes_core::config::ProviderConfig::default(),
        );
        config_b.model.provider = "provider_b_broken".to_string();
        config_b.model.default = "model-b".to_string();
        let config_b = Arc::new(config_b);
        let resolver_b = Arc::new(
            ProviderResolver::build(&config_b)
                .expect("ProviderResolver::build succeeds even for a base_url-less provider"),
        );
        assert_eq!(
            resolver_b.resolve_for_main().base_url,
            "",
            "test precondition: provider B's endpoint must have an empty base_url"
        );

        let err = runtime
            .reload_config_and_resolver(config_b, resolver_b)
            .await
            .expect_err("reload must fail when the new resolver's client cannot be built");
        assert!(
            err.to_string().contains("base_url"),
            "error should name the actual cause (empty base_url), got: {err}"
        );

        // D-09: the previously-running resolver, config and client are all
        // still exactly what they were before the failed reload attempt.
        assert_eq!(runtime.resolver().main_provider(), "provider_a");
        assert_eq!(runtime.config().model.default, "model-a");
        assert_eq!(runtime.client().base_url(), "https://provider-a.example.test");
        assert_eq!(runtime.client().model(), "model-a");
    }

    // ── Phase 50.4 (D-14, wave 2): AgentSubagentRunner follows the reload ──

    /// Build a runner sharing the SAME handle objects a `build_runtime_for_reload_test`
    /// runtime holds — the exact shape `from_config` wires via its
    /// shared-handles builder chain. The constructor arguments below are
    /// throwaway values immediately superseded by that builder call,
    /// matching the production comment in `from_config`.
    ///
    /// Imports the runner type under a LOCAL alias rather than the plain
    /// name already in scope via `use super::*;`: `invariants_21_7.rs`
    /// counts occurrences of the runner's constructor call (as literal text)
    /// across this entire file (`include_str!`, which does not strip
    /// `#[cfg(test)]` text) and requires exactly one — the real construction
    /// site inside `from_config`. Spelling that same call out a second time
    /// here, in a test helper that has nothing to do with that invariant,
    /// would break it for a reason unrelated to what it actually guards.
    #[cfg(test)]
    fn build_subagent_runner_sharing(runtime: &AgentRuntime) -> AgentSubagentRunner {
        use crate::subagent_runner::AgentSubagentRunner as RunnerCtor;
        let dummy_client = AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "http://localhost:9999",
            "dummy-key",
            "dummy-model",
        ));
        let dummy_resolver = ProviderResolver::build(&Config::default())
            .expect("default Config should produce a valid resolver");
        RunnerCtor::new(dummy_client, dummy_resolver, None)
            .with_shared_handles(runtime.client_handle.clone(), runtime.resolver_handle.clone())
    }

    /// Test 1: a subagent runner sharing the runtime's handles resolves
    /// provider A before a reload and provider B — both resolver AND
    /// client base_url — after one. A runner holding the pre-Phase-50.4
    /// deep clone (`(*resolver).clone()` into a plain owned field) would
    /// report A both times and fail this test.
    #[tokio::test]
    async fn subagent_runner_sharing_runtime_handles_sees_the_reloaded_provider() {
        let (config_a, resolver_a) =
            build_test_provider_config("provider_a", "https://provider-a.example.test", "model-a");
        let client_a = AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "https://provider-a.example.test",
            "key-a",
            "model-a",
        ));
        let runtime = build_runtime_for_reload_test(config_a, resolver_a, client_a);
        let runner = build_subagent_runner_sharing(&runtime);

        assert_eq!(
            runner.resolver_snapshot_for_test().main_provider(),
            "provider_a",
            "before any reload, a runner sharing the runtime's handles must resolve \
             the same provider the runtime was constructed with"
        );

        let (config_b, resolver_b) =
            build_test_provider_config("provider_b", "https://provider-b.example.test", "model-b");
        runtime
            .reload_config_and_resolver(config_b, resolver_b)
            .await
            .expect("reload must succeed — provider B has a real base_url");

        assert_eq!(
            runner.resolver_snapshot_for_test().main_provider(),
            "provider_b",
            "after a successful reload, the shared-handle runner must resolve the NEW provider"
        );
        assert_eq!(
            runner.client_snapshot_for_test().base_url(),
            "https://provider-b.example.test",
            "after a successful reload, the shared-handle runner's client must be on the \
             NEW endpoint, not a boot-time snapshot"
        );
    }

    /// Test 3 (D-09 across the delegation boundary): a reload that fails at
    /// the client-build step must leave a shared-handle runner on the
    /// pre-reload provider — not merely the main `AgentRuntime` path.
    #[tokio::test]
    async fn subagent_runner_does_not_observe_a_failed_reload() {
        let (config_a, resolver_a) =
            build_test_provider_config("provider_a", "https://provider-a.example.test", "model-a");
        let client_a = AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "https://provider-a.example.test",
            "key-a",
            "model-a",
        ));
        let runtime = build_runtime_for_reload_test(config_a, resolver_a, client_a);
        let runner = build_subagent_runner_sharing(&runtime);

        // Provider B: resolvable but not buildable into a client (no base_url) —
        // the same real misconfiguration shape used by
        // `reload_config_and_resolver_publishes_nothing_when_the_client_cannot_be_built`.
        let mut config_b = Config::default();
        config_b.providers.insert(
            "provider_b_broken".to_string(),
            ironhermes_core::config::ProviderConfig::default(),
        );
        config_b.model.provider = "provider_b_broken".to_string();
        config_b.model.default = "model-b".to_string();
        let config_b = Arc::new(config_b);
        let resolver_b = Arc::new(
            ProviderResolver::build(&config_b)
                .expect("ProviderResolver::build succeeds even for a base_url-less provider"),
        );

        runtime
            .reload_config_and_resolver(config_b, resolver_b)
            .await
            .expect_err("reload must fail when the new resolver's client cannot be built");

        assert_eq!(
            runner.resolver_snapshot_for_test().main_provider(),
            "provider_a",
            "a failed reload must leave a shared-handle runner on the PRE-reload provider"
        );
        assert_eq!(
            runner.client_snapshot_for_test().base_url(),
            "https://provider-a.example.test",
            "a failed reload must leave a shared-handle runner's client on the PRE-reload endpoint"
        );
    }

    /// Test 4: `from_config` must actually wire the shared-handles builder
    /// onto the subagent runner constructor call — a source assertion, not
    /// a behavioral test, because every test above constructs its own runner
    /// directly (via `build_subagent_runner_sharing`) and would still pass
    /// even if `from_config` dropped the builder call entirely.
    ///
    /// Scoped to `from_config`'s own body (not the whole file) — this test
    /// module's own `build_subagent_runner_sharing` helper contains the
    /// identical constructor-then-builder shape, so an unscoped whole-file
    /// search would keep finding a match after the production wiring was
    /// removed, which is exactly the self-matching failure mode this plan's
    /// split-concat guidance exists to avoid.
    #[test]
    fn from_config_gives_the_subagent_runner_the_runtime_handles() {
        let non_comment: String = SOURCE
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let fn_start = non_comment
            .find("pub async fn from_config(")
            .expect("from_config must exist in this module");
        let fn_end = non_comment[fn_start..]
            .find("\n    pub ")
            .map(|offset| fn_start + offset)
            .unwrap_or(non_comment.len());
        let scope = &non_comment[fn_start..fn_end];

        let ctor_needle = "AgentSubagentRunner".to_string() + "::new(";
        let builder_needle = "with_shared".to_string() + "_handles(";
        let ctor_pos = scope
            .find(&ctor_needle)
            .expect("from_config must construct the subagent runner via its constructor");
        let builder_pos = scope.find(&builder_needle).expect(
            "from_config must chain the shared-handles builder onto the subagent runner \
             constructor",
        );
        assert!(
            ctor_pos < builder_pos,
            "the shared-handles builder must be chained AFTER the subagent runner constructor \
             call inside from_config — it replaces the values the constructor wrapped, not \
             the other way around"
        );
    }

    // ── Phase 50.4 (D-14, wave 2): vision/summarization tool handles follow the reload ──

    /// Test 4: `from_config` must pass its OWN resolver handle into the
    /// factory input's `resolver_handle` field — the line that makes Task 2's
    /// wiring load-bearing rather than theoretical. Without it, every unit
    /// test in Task 2 still passes while production silently takes the
    /// derive-a-fresh-handle fallback and neither tool handle ever observes
    /// a reload.
    ///
    /// Scoped to `from_config`'s own body for the same self-matching reason
    /// as `from_config_gives_the_subagent_runner_the_runtime_handles` above.
    #[test]
    fn from_config_passes_its_resolver_handle_to_the_factory() {
        let non_comment: String = SOURCE
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let fn_start = non_comment
            .find("pub async fn from_config(")
            .expect("from_config must exist in this module");
        let fn_end = non_comment[fn_start..]
            .find("\n    pub ")
            .map(|offset| fn_start + offset)
            .unwrap_or(non_comment.len());
        let scope = &non_comment[fn_start..fn_end];

        let field_needle = "resolver_handle".to_string() + ": Some(";
        assert!(
            scope.contains(&field_needle),
            "from_config must set AppRuntimeFactoryInput.resolver_handle to Some(..) from its \
             own resolver handle — a missing or None value degrades to the derive-a-fresh-handle \
             fallback with no compile error and no test failure elsewhere"
        );
    }

    // ── Phase 50.4 (D-14, wave 2), Task 3: cross-crate construction-time capture audit ──
    //
    // See `.planning/phases/50.4-web-ui-memory-edit-and-providers-models-list-load/
    // 50.4-08-SUMMARY.md` for the full enumeration (method, per-class counts, every
    // CAPTURED site named by file/symbol/severity/disposition). This test is the
    // mechanical backstop for the two wiring lines Tasks 1 and 2 depend on — it turns
    // the audit from a point-in-time SUMMARY paragraph into something a future refactor
    // trips over.

    /// Combines the two individual wiring assertions above into the single
    /// audit-backstop test the plan names explicitly. Deliberately redundant
    /// with `from_config_gives_the_subagent_runner_the_runtime_handles` and
    /// `from_config_passes_its_resolver_handle_to_the_factory` — this one
    /// exists as the audit's own named regression net, not a replacement for
    /// either.
    #[test]
    fn from_config_passes_shared_handles_to_every_construction_time_consumer() {
        let non_comment: String = SOURCE
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let fn_start = non_comment
            .find("pub async fn from_config(")
            .expect("from_config must exist in this module");
        let fn_end = non_comment[fn_start..]
            .find("\n    pub ")
            .map(|offset| fn_start + offset)
            .unwrap_or(non_comment.len());
        let scope = &non_comment[fn_start..fn_end];

        let builder_needle = "with_shared".to_string() + "_handles(";
        let field_needle = "resolver_handle".to_string() + ": Some(";
        assert!(
            scope.contains(&builder_needle),
            "from_config must chain the shared-handles builder onto the subagent runner \
             constructor — dropping this line silently returns delegated child agents to \
             serving a boot-time snapshot"
        );
        assert!(
            scope.contains(&field_needle),
            "from_config must set AppRuntimeFactoryInput.resolver_handle to Some(..) — \
             dropping this line silently returns the vision/web-extract tool handles to the \
             factory's derive-a-fresh-handle fallback"
        );
    }

    /// INV-36.2-CR-04: Phase 36.2 code-review CR-04 regression net.
    /// `run_turn` MUST chain the Plan 08 cache-break advisory builders so
    /// the model-swap and context-file-edit triggers can actually fire in
    /// production. Pre-fix these builders were defined and unit-tested but
    /// never called from any production entry point — the warnings were
    /// dead code on every surface.
    #[test]
    fn inv_36_2_cr_04_runtime_wires_cache_break_builders() {
        let non_comment: String = SOURCE
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            non_comment.contains("agent.with_session_has_prior_turns("),
            "Phase 36.2 CR-04: run_turn MUST call agent.with_session_has_prior_turns(...) \
             so trigger 1 (model-swap cache break) can suppress on session zero."
        );
        assert!(
            non_comment.contains("agent.with_previous_model("),
            "Phase 36.2 CR-04: run_turn MUST call agent.with_previous_model(...) \
             so trigger 1 (model-swap cache break) can compare the new model \
             against the prior turn's model name."
        );
        assert!(
            non_comment.contains("agent.with_context_file_paths("),
            "Phase 36.2 CR-04: run_turn MUST call agent.with_context_file_paths(...) \
             so trigger 3 (context-file-edit cache break) can mtime-snapshot \
             SOUL.md / AGENTS.md / CLAUDE.md."
        );

        // Post-turn state update must also be present so the next turn has
        // the prior model name to compare against.
        assert!(
            non_comment.contains("self.previous_model.lock()"),
            "Phase 36.2 CR-04: run_turn MUST store the just-run model name \
             into self.previous_model after agent.run completes."
        );
        assert!(
            non_comment.contains("self.session_turn_count.fetch_add(1"),
            "Phase 36.2 CR-04: run_turn MUST increment self.session_turn_count \
             after agent.run completes so the next turn's `has_prior_turns` flag \
             becomes true."
        );
    }

    /// INV-36.2-CR-02: Phase 36.2 code-review CR-02 regression net.
    /// `run_turn` MUST call `with_provider_name(...)` and
    /// `with_api_key_for_usage_tracking(...)` on the per-turn `AgentLoop`.
    /// Without these, `usage_events.provider` is empty on every production
    /// row, the `/usage --provider X` filter is useless, and the
    /// `RateLimitTracker` keys all sessions into a single shared bucket
    /// (sha256 of empty key) — a cross-tenant data-leak in any multi-tenant
    /// deployment. The test `inv_36_2_07_runtime_calls_with_state_store_before_with_intercepts`
    /// covers the related state_store wiring; this complements it.
    #[test]
    fn inv_36_2_cr_02_runtime_calls_with_provider_name_and_api_key() {
        let non_comment: String = SOURCE
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            non_comment.contains("agent.with_provider_name("),
            "Phase 36.2 CR-02: run_turn MUST call `agent.with_provider_name(...)` so the \
             post-LLM-call write site stamps a non-empty provider column on every \
             usage_events row. Without it, /usage --provider filters are useless."
        );
        assert!(
            non_comment.contains("agent.with_api_key_for_usage_tracking("),
            "Phase 36.2 CR-02: run_turn MUST call `agent.with_api_key_for_usage_tracking(...)` \
             so the SHA-256 hash bucket on usage_events is per-key, not a constant \
             empty-string hash that collapses every session into one bucket."
        );
    }

    // ── Phase 36.15 Plan 04 (PROV-11): extras resolution wiring ──────────

    /// Verify that run_turn calls config_extras::resolve_extras (source guard).
    #[test]
    fn run_turn_calls_resolve_extras() {
        assert!(
            SOURCE.contains("ironhermes_core::config_extras::resolve_extras"),
            "Phase 36.15 (PROV-11): run_turn must call \
             ironhermes_core::config_extras::resolve_extras to resolve per-turn extras."
        );
    }

    /// Verify that run_turn wires extras into AgentLoop via with_resolved_extras (source guard).
    #[test]
    fn run_turn_wires_with_resolved_extras() {
        assert!(
            SOURCE.contains("with_resolved_extras("),
            "Phase 36.15 (PROV-11): run_turn must call agent.with_resolved_extras(...) \
             to pass resolved extras into AgentLoop."
        );
    }

    /// Behavioral test: resolved_extras_for_test_turn returns Some(map) with
    /// num_ctx=4096 when Config has providers.test_provider.extra_request_options
    /// set accordingly. Validates the D-10 per-turn resolution path.
    #[test]
    fn resolved_extras_for_test_turn_returns_provider_extras() {
        use ironhermes_core::{Config, ProviderResolver};
        use std::collections::HashMap;

        // Build a Config with a single provider that has num_ctx=4096 as an extra.
        let mut config = Config::default();
        let mut extras: HashMap<String, serde_json::Value> = HashMap::new();
        extras.insert("num_ctx".to_string(), serde_json::json!(4096u32));

        // Give the provider a base_url so the resolver can build successfully.
        let provider_cfg = ironhermes_core::config::ProviderConfig {
            extra_request_options: extras,
            base_url: Some("http://localhost:11434".to_string()),
            ..ironhermes_core::config::ProviderConfig::default()
        };

        config
            .providers
            .insert("test_provider".to_string(), provider_cfg);

        // Make test_provider the main provider.
        config.model.provider = "test_provider".to_string();
        config.model.default = "llama3.1:8b".to_string();

        let config = std::sync::Arc::new(config);
        let resolver = std::sync::Arc::new(
            ProviderResolver::build(&config)
                .expect("ProviderResolver::build must succeed with test config"),
        );

        // Build a minimal AgentRuntime with the test config + resolver.
        let client = crate::AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "http://localhost:0",
            "test-key",
            "llama3.1:8b",
        ));
        let max_iterations = config.agent.max_iterations;
        let budget = crate::budget::BudgetHandle::new(max_iterations);
        let registry = std::sync::Arc::new(tokio::sync::RwLock::new(
            ironhermes_tools::ToolRegistry::new(),
        ));
        let hook_registry = std::sync::Arc::new(ironhermes_hooks::HookRegistry::new(
            ironhermes_hooks::HooksConfig::default(),
        ));
        let skill_registry = std::sync::Arc::new(std::sync::RwLock::new(std::sync::Arc::new(
            ironhermes_core::SkillRegistry::load_with_paths(&[]),
        )));
        let active_skills = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let cron_dir = std::env::temp_dir().join(format!(
            "ironhermes_test_cron_extras_{}",
            std::process::id()
        ));
        let job_store = std::sync::Arc::new(std::sync::Mutex::new(
            ironhermes_cron::JobStore::open(cron_dir).expect("temp-dir JobStore must succeed"),
        ));
        let browser_session = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let bundle = crate::app_runtime_factory::AppRuntimeBundle {
            registry,
            hook_registry,
            skill_registry,
            skills_cwd: std::env::temp_dir(),
            skills_credential_dir: std::env::temp_dir(),
            active_skills,
            job_store,
            browser_session,
            mcp_manager: None,
            merged_tools: ironhermes_core::config::ToolsConfig::default(),
            bb_recorder: None, // Phase 39.2: disabled in test helpers
        };
        let subagent_registry = std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::subagent_registry::SubagentRegistry::new(),
        ));

        let runtime = AgentRuntime {
            config_handle: std::sync::Arc::new(std::sync::RwLock::new(config)),
            resolver_handle: std::sync::Arc::new(std::sync::RwLock::new(resolver)),
            client_handle: std::sync::Arc::new(std::sync::RwLock::new(client)),
            bundle,
            budget,
            memory_manager: None,
            subagent_registry,
            max_iterations,
            cwd: std::path::PathBuf::from("."),
            previous_model: std::sync::Mutex::new(None),
            session_turn_count: std::sync::atomic::AtomicUsize::new(0),
            context_file_paths: Vec::new(),
            bb_recorder: None, // Phase 39.2: disabled in test helpers
            // Phase 36.3.12 D-08: this test helper builds a minimal registry with
            // no "terminal"/"execute_code" tools registered — no capture needed.
            terminal_tool_arc: None,
            execute_code_tool_arc: None,
            write_file_tool_arc: None,
            patch_tool_arc: None,
        };

        let result = runtime.resolved_extras_for_test_turn();
        let map = result
            .expect("resolved_extras_for_test_turn must return Some when provider has extras");
        assert_eq!(
            map.get("num_ctx"),
            Some(&serde_json::json!(4096u32)),
            "num_ctx=4096 set in provider config must appear in resolved extras"
        );
    }

    // ── Phase 50.4 Plan 07 (D-15 Side B): compaction follows the reload ────
    //
    // D-14's reloadable handles make compaction follow the new model only
    // TRANSITIVELY; the operator ruled that insufficient. These tests pin
    // the window explicitly: the compaction trigger verdict must flip
    // between the pre- and post-reload windows on ONE identical message
    // vector, the captured ContextStats must carry the window it was given,
    // the pressure signal must move with it, and run_turn must be pinned by
    // source assertion to derive the window from the reloadable resolver
    // accessor and feed the SAME binding to both consumers.
    //
    // No test here asserts on the resolved model id or provider name as its
    // primary claim — D-15 states that shape reproduces the D-14 failure
    // mode one level down.

    /// Variant of `build_test_provider_config` that also sets an explicit
    /// `config.model.context_length` override. Per `provider.rs`'s (pre-50.5)
    /// D-06 precedence, `config_context_length` (sourced from this field) won
    /// over model metadata and the default, so two configs differing ONLY
    /// in this field resolve to genuinely different windows — the lever
    /// these tests need without a populated model registry or network call.
    /// This test exercises ONLY the global-pin tier; the per-model-config
    /// tier is exercised separately by
    /// `build_test_provider_config_with_per_model_window` below, and the
    /// model-metadata tier is NOT covered by either helper.
    #[cfg(test)]
    fn build_test_provider_config_with_window(
        provider_name: &str,
        base_url: &str,
        model: &str,
        context_length: usize,
    ) -> (Arc<Config>, Arc<ProviderResolver>) {
        let mut config = Config::default();
        let provider_cfg = ironhermes_core::config::ProviderConfig {
            base_url: Some(base_url.to_string()),
            default_model: Some(model.to_string()),
            ..ironhermes_core::config::ProviderConfig::default()
        };
        config.providers.insert(provider_name.to_string(), provider_cfg);
        config.model.provider = provider_name.to_string();
        config.model.default = model.to_string();
        config.model.context_length = Some(context_length);
        let config = Arc::new(config);
        let resolver = Arc::new(
            ProviderResolver::build(&config)
                .expect("ProviderResolver::build must succeed with a real base_url"),
        );
        (config, resolver)
    }

    /// Phase 50.5 Plan 03 (VALIDATION Wave 0 gap 2): variant of
    /// `build_test_provider_config_with_window` that ALSO inserts a
    /// per-(provider, model) `ProviderModelConfig.context_length` override —
    /// a DELIBERATELY CONFLICTING pair with `config.model.context_length`
    /// (the global pin). A resolver that still honoured the pre-50.5 D-06
    /// order (pin wins) would return `global_pin` here and fail the
    /// compaction-budget assertion in
    /// `per_model_window_drives_the_compaction_budget`; only a resolver that
    /// implements Phase 50.5 D-02's inverted tier order (per-model config
    /// beats the pin) returns `per_model_window`. This helper still does not
    /// exercise the model-metadata tier — only the per-model-config-vs-pin
    /// conflict.
    #[cfg(test)]
    fn build_test_provider_config_with_per_model_window(
        provider_name: &str,
        base_url: &str,
        model: &str,
        per_model_window: usize,
        global_pin: usize,
    ) -> (Arc<Config>, Arc<ProviderResolver>) {
        use ironhermes_core::config_extras::ProviderModelConfig;

        let mut config = Config::default();
        let mut models = std::collections::HashMap::new();
        models.insert(
            model.to_string(),
            ProviderModelConfig {
                context_length: Some(per_model_window),
                ..Default::default()
            },
        );
        let provider_cfg = ironhermes_core::config::ProviderConfig {
            base_url: Some(base_url.to_string()),
            default_model: Some(model.to_string()),
            models,
            ..ironhermes_core::config::ProviderConfig::default()
        };
        config.providers.insert(provider_name.to_string(), provider_cfg);
        config.model.provider = provider_name.to_string();
        config.model.default = model.to_string();
        config.model.context_length = Some(global_pin);
        let config = Arc::new(config);
        let resolver = Arc::new(
            ProviderResolver::build(&config)
                .expect("ProviderResolver::build must succeed with a real base_url"),
        );
        (config, resolver)
    }

    /// Records every `ContextStats` `compress` receives and every
    /// `context_length` `check_pressure` receives, rather than merely
    /// counting calls (`agent_loop.rs`'s existing `RecordingEngine` counts
    /// but discards the stats it's given — insufficient here, since Test 2
    /// needs the captured `context_length` and `protect_last_tokens`).
    #[cfg(test)]
    struct WindowRecordingEngine {
        engine_threshold: f32,
        captured_compress_stats: Arc<std::sync::Mutex<Vec<crate::context_engine::ContextStats>>>,
        captured_pressure_windows: Arc<std::sync::Mutex<Vec<usize>>>,
    }

    #[cfg(test)]
    #[async_trait::async_trait]
    impl crate::context_engine::ContextEngine for WindowRecordingEngine {
        async fn compress(
            &self,
            _messages: &mut Vec<ChatMessage>,
            stats: crate::context_engine::ContextStats,
        ) -> Result<crate::context_engine::CompressionOutcome, crate::context_engine::ContextError>
        {
            self.captured_compress_stats.lock().unwrap().push(stats);
            Ok(crate::context_engine::CompressionOutcome {
                compressed: true,
                ..Default::default()
            })
        }
        fn threshold(&self) -> f32 {
            self.engine_threshold
        }
        fn mode(&self) -> crate::context_engine::CompressionMode {
            crate::context_engine::CompressionMode::Hard
        }
        async fn check_pressure(&self, stats: &crate::context_engine::ContextStats) -> bool {
            self.captured_pressure_windows
                .lock()
                .unwrap()
                .push(stats.context_length);
            false
        }
    }

    /// A single fixed message vector reused by both directions of Test 2 —
    /// one big user message built from a repeating readable phrase, sized to
    /// straddle `engine_threshold * window_a` and `engine_threshold *
    /// window_b` for the small/large windows those tests use. Kept cheap
    /// (tens of KB, not megabytes) since both windows are deliberately
    /// small test values, not the 128k/1M example from the Task 3 checkpoint.
    #[cfg(test)]
    fn straddling_message_vector(char_len: usize) -> Vec<ChatMessage> {
        let text: String = "the quick brown fox jumps over the lazy dog "
            .chars()
            .cycle()
            .take(char_len)
            .collect();
        vec![ChatMessage {
            role: ironhermes_core::Role::User,
            content: Some(ironhermes_core::MessageContent::Text(text)),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            is_recall_context: false,
        }]
    }

    /// Bare `AgentLoop` for the compaction-window tests — mirrors
    /// `agent_wiring.rs`'s own `bare_agent()` test helper.
    #[cfg(test)]
    fn bare_agent_for_window_test() -> AgentLoop {
        let client = AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "http://localhost:0".to_string(),
            "test".to_string(),
            "test-model",
        ));
        AgentLoop::new(
            client,
            Arc::new(RwLock::new(ironhermes_tools::ToolRegistry::new())),
            4,
        )
    }

    /// Test 1: the exact expression `run_turn` evaluates to obtain its
    /// `context_length` binding — NOT the model-id assertion D-15 rejects.
    #[tokio::test]
    async fn reload_moves_the_resolved_compaction_window() {
        let (config_a, resolver_a) = build_test_provider_config_with_window(
            "provider_a",
            "https://provider-a.example.test",
            "model-a",
            2_000,
        );
        let client_a = AnyClient::ChatCompletions(crate::client::LlmClient::new(
            "https://provider-a.example.test",
            "key-a",
            "model-a",
        ));
        let runtime = build_runtime_for_reload_test(config_a, resolver_a, client_a);
        assert_eq!(runtime.resolver().resolve_for_main().context_length(), 2_000);

        let (config_b, resolver_b) = build_test_provider_config_with_window(
            "provider_b",
            "https://provider-b.example.test",
            "model-b",
            200_000,
        );
        runtime
            .reload_config_and_resolver(config_b, resolver_b)
            .await
            .expect("reload must succeed — provider B has a real base_url");
        assert_eq!(runtime.resolver().resolve_for_main().context_length(), 200_000);
    }

    /// Test 2: the differential. Drives the REAL compaction path
    /// (`pre_chat_compress`, extracted for exactly this purpose) twice with
    /// a fresh `AgentLoop` each time, once per window, on ONE identical
    /// message vector. An implementation where the window does not follow
    /// the reload produces the same verdict twice and fails this test.
    #[tokio::test]
    async fn post_reload_window_flips_the_compaction_trigger() {
        let engine_threshold: f32 = 0.5;
        let (_config_a, resolver_a) = build_test_provider_config_with_window(
            "provider_a",
            "https://provider-a.example.test",
            "model-a",
            2_000,
        );
        let (_config_b, resolver_b) = build_test_provider_config_with_window(
            "provider_b",
            "https://provider-b.example.test",
            "model-b",
            200_000,
        );
        let window_a = resolver_a.resolve_for_main().context_length();
        let window_b = resolver_b.resolve_for_main().context_length();
        assert_eq!(window_a, 2_000);
        assert_eq!(window_b, 200_000);

        // Fixed message vector, built ONCE and reused unmodified (bar cloning
        // for each of the two pre_chat_compress calls, which may drain/mutate
        // its argument) for both directions of the differential.
        let messages = straddling_message_vector(40_000);
        let estimated = crate::context_compressor::estimate_messages_tokens(&messages);

        // Explicit precondition: the differential is meaningless if the two
        // windows agree, so a sizing drift must fail loudly here rather than
        // let the test below pass vacuously.
        assert!(
            (estimated as f32) > engine_threshold * window_a as f32,
            "message vector ({estimated} est. tokens) must exceed the pre-reload trigger \
             ({} tokens = {engine_threshold} * {window_a}) or Test 2 cannot distinguish a \
             working fix from a broken one",
            engine_threshold * window_a as f32,
        );
        assert!(
            (estimated as f32) < engine_threshold * window_b as f32,
            "message vector ({estimated} est. tokens) must stay below the post-reload trigger \
             ({} tokens = {engine_threshold} * {window_b}) or Test 2 cannot distinguish a \
             working fix from a broken one",
            engine_threshold * window_b as f32,
        );

        // Pre-reload window: compression must fire.
        let stats_a = Arc::new(std::sync::Mutex::new(Vec::new()));
        let engine_a = Arc::new(WindowRecordingEngine {
            engine_threshold,
            captured_compress_stats: stats_a.clone(),
            captured_pressure_windows: Arc::new(std::sync::Mutex::new(Vec::new())),
        });
        let mut agent_a = bare_agent_for_window_test().with_context_engine(engine_a, window_a);
        let mut messages_a = messages.clone();
        agent_a.pre_chat_compress(&mut messages_a).await;
        let captured_a = stats_a.lock().unwrap().clone();
        assert_eq!(
            captured_a.len(),
            1,
            "compression must fire exactly once against the pre-reload (smaller) window for \
             this message vector — an implementation that does not follow the reload would \
             either fire twice (matching Test 2's post-reload run too) or zero times"
        );
        assert_eq!(
            captured_a[0].context_length, window_a,
            "the captured ContextStats.context_length must equal the window it was given"
        );
        assert_eq!(
            captured_a[0].protect_last_tokens,
            20_000usize.min(window_a / 4),
            "protect_last_tokens must follow the same window (20_000.min(window / 4))"
        );

        // Post-reload window: compression must NOT fire for the SAME vector.
        let stats_b = Arc::new(std::sync::Mutex::new(Vec::new()));
        let engine_b = Arc::new(WindowRecordingEngine {
            engine_threshold,
            captured_compress_stats: stats_b.clone(),
            captured_pressure_windows: Arc::new(std::sync::Mutex::new(Vec::new())),
        });
        let mut agent_b = bare_agent_for_window_test().with_context_engine(engine_b, window_b);
        let mut messages_b = messages.clone();
        agent_b.pre_chat_compress(&mut messages_b).await;
        assert_eq!(
            stats_b.lock().unwrap().len(),
            0,
            "compression must NOT fire against the post-reload (larger) window for the \
             IDENTICAL message vector — a verdict of 1 here means the window did not move \
             with the reload (D-15's central regression)"
        );
    }

    /// Phase 50.5 Plan 03 (VALIDATION Wave 0 gap 2): proves the phase's
    /// central claim — "the window follows the model" — at the compaction
    /// READ (`agent_runtime.rs`'s per-turn
    /// `resolver.resolve_for_main().context_length()` binding, unchanged by
    /// this phase), not only at the `ResolvedEndpoint` unit level. The config
    /// carries a per-model window (1,048,576) and a DELIBERATELY CONFLICTING
    /// global pin (256,000); a resolver that still honoured the pre-50.5
    /// order would drive the compaction budget from the pin and fail this
    /// test.
    #[tokio::test]
    async fn per_model_window_drives_the_compaction_budget() {
        let engine_threshold: f32 = 0.5;
        let (_config, resolver) = build_test_provider_config_with_per_model_window(
            "provider_c",
            "https://provider-c.example.test",
            "model-c",
            1_048_576,
            256_000,
        );
        let window = resolver.resolve_for_main().context_length();
        assert_eq!(
            window, 1_048_576,
            "a per-model config entry (1,048,576) must win over a conflicting global pin \
             (256,000) — Phase 50.5 D-02"
        );

        let messages = straddling_message_vector(40_000);
        let captured_pressure_windows = Arc::new(std::sync::Mutex::new(Vec::new()));
        let engine = Arc::new(WindowRecordingEngine {
            engine_threshold,
            captured_compress_stats: Arc::new(std::sync::Mutex::new(Vec::new())),
            captured_pressure_windows: captured_pressure_windows.clone(),
        });
        let mut agent = bare_agent_for_window_test().with_context_engine(engine, window);
        let mut messages = messages.clone();
        agent.pre_chat_compress(&mut messages).await;

        // Literal expected value — NOT re-derived via `resolve_for_main()` a
        // second time, which would let a broken resolver agree with itself.
        assert_eq!(
            captured_pressure_windows.lock().unwrap().first().copied(),
            Some(1_048_576),
            "the value reaching `check_pressure` (the compaction budget) must be the \
             per-model window, not the conflicting global pin"
        );
    }

    /// Test 3: the third D-15-named observable, the one that does not go
    /// through `ContextStats` at all.
    #[tokio::test]
    async fn post_reload_window_moves_the_pressure_signal() {
        let engine_threshold: f32 = 0.5;
        let (_config_a, resolver_a) = build_test_provider_config_with_window(
            "provider_a",
            "https://provider-a.example.test",
            "model-a",
            2_000,
        );
        let (_config_b, resolver_b) = build_test_provider_config_with_window(
            "provider_b",
            "https://provider-b.example.test",
            "model-b",
            200_000,
        );
        let window_a = resolver_a.resolve_for_main().context_length();
        let window_b = resolver_b.resolve_for_main().context_length();

        // Same estimated_tokens value fed against both windows; PressureTracker
        // is per-session, so two DISTINCT session ids give each call a fresh
        // SessionState — isolating the comparison to the window math alone
        // rather than any cross-call cooldown/above_threshold carryover.
        let estimated_tokens = 900usize; // 900 / 2_000 = 0.45 -> 0.45/0.5 = 90% of threshold (crosses 85%)
        let tracker = crate::pressure_warning::PressureTracker::new();
        let fired_a = tracker
            .check_and_maybe_emit(
                "session-pre-reload",
                engine_threshold,
                estimated_tokens,
                window_a,
                "soft",
                None,
            )
            .await;
        let fired_b = tracker
            .check_and_maybe_emit(
                "session-post-reload",
                engine_threshold,
                estimated_tokens,
                window_b,
                "soft",
                None,
            )
            .await;
        assert_ne!(
            fired_a, fired_b,
            "PressureTracker::check_and_maybe_emit must return a DIFFERENT verdict for the \
             pre- and post-reload windows given identical estimated_tokens ({estimated_tokens}) \
             and engine_threshold ({engine_threshold}) — window_a={window_a} window_b={window_b}"
        );
        assert!(fired_a, "the pre-reload (smaller) window must cross the 85% pressure trigger");
        assert!(!fired_b, "the post-reload (larger) window must NOT cross the 85% pressure trigger");
    }

    /// Test 4: source assertion pinning `run_turn`'s wiring so a future
    /// refactor cannot re-freeze the compaction window at construction.
    /// `SOURCE` (`include_str!("agent_runtime.rs")`) includes this very test
    /// module, so every needle below is built by concatenating two
    /// fragments — the same technique `iron_hermes_ui`'s `mod.rs` test
    /// module uses for its negative assertion — so the needle cannot appear
    /// contiguously in this file's own test source and therefore cannot
    /// satisfy itself.
    #[test]
    fn run_turn_derives_the_compaction_window_from_the_reloadable_resolver() {
        let binding_needle = [
            "let context_length = resolver.resolve_for_ma",
            "in().context_length();",
        ]
        .concat();
        let binding_pos = SOURCE.find(&binding_needle).expect(
            "run_turn's context_length binding must be assigned from \
             resolver.resolve_for_main() — where `resolver` is the reloadable accessor \
             local (`let resolver = self.resolver();`), not a bare self.resolver field read",
        );

        let with_compression_needle = [
            ".with_compression(context_len",
            "gth, config.agent.context_compression)",
        ]
        .concat();
        let with_compression_pos = SOURCE.find(&with_compression_needle).expect(
            ".with_compression( must be called with the context_length binding as its \
             first argument",
        );

        let attach_engine_needle = [
            "req.pressure_tracker,\n            context_len",
            "gth,\n            self.memory_manager.clone(),",
        ]
        .concat();
        let attach_engine_pos = SOURCE.find(&attach_engine_needle).expect(
            "attach_context_engine( must be called with the SAME context_length binding, \
             positioned as the argument right before memory_manager",
        );

        assert!(
            binding_pos < with_compression_pos,
            "the context_length binding must be assigned before .with_compression( reads it"
        );
        assert!(
            binding_pos < attach_engine_pos,
            "the context_length binding must be assigned before attach_context_engine( reads it"
        );

        // Confirm the single-binding premise itself: exactly ONE occurrence
        // of the full binding pattern must exist in this file. Reuses
        // `binding_needle` itself (rather than a shorter marker) for the
        // count — the shorter prefix "let context_length =" appears
        // literally inside `binding_needle`'s own first fragment above, so
        // counting on that prefix would self-match this very test.
        let occurrences = SOURCE.matches(&binding_needle).count();
        assert_eq!(
            occurrences, 1,
            "exactly one occurrence of the context_length binding pattern (see binding_needle \
             above) must exist in this file; found {occurrences} — Test 4's premise is that \
             ONE binding feeds both consumers"
        );
    }
}

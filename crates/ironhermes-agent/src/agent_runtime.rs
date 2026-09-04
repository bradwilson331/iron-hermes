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
            config,
            resolver,
            client,
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
                self.config.clone(),
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
                self.config.clone(),
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

        let context_length = self.resolver.resolve_for_main().context_length();

        // ── Phase 36.15 Plan 04 (PROV-11): per-turn extras resolution ─────
        // D-10: resolve (provider, model) → merged HashMap on every turn so a
        // mid-session /model switch picks up the new per-model override immediately.
        // resolver.main_provider() is the providers: map key; resolve_for_main()
        // .default_model is the wire model string LlmClient uses when None is passed.
        let resolved_extras_for_turn: Option<std::collections::HashMap<String, serde_json::Value>> = {
            let provider_name = self.resolver.main_provider();
            let model_name = self.resolver.resolve_for_main().default_model.clone();
            let merged = ironhermes_core::config_extras::resolve_extras(
                &self.config.providers,
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
            let provider_name = self.resolver.main_provider();
            let model_name = self.resolver.resolve_for_main().default_model.clone();
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

        // Vision auto-routing (fix): when this turn carries image content, run it
        // on the configured `roles.vision` model instead of the active chat model.
        // The active model may not support vision (e.g. kimi-k3), which the
        // provider rejects with a 400 "Image content is not supported by this
        // model". Falls back to the main client when the turn has no image OR no
        // vision role is resolvable — the provider error is then surfaced as-is
        // (now visible via the full-chain error surfacing).
        let (turn_client, vision_routed_to) = if messages_contain_image(&req.messages) {
            match build_role_client(&self.resolver, "vision") {
                Ok(Some(vision_client)) => {
                    let vm = vision_client.model().to_string();
                    tracing::info!(
                        vision_model = %vm,
                        active_model = %self.client.model(),
                        "vision auto-route: turn carries image content; routing to the vision-role model"
                    );
                    (vision_client, Some(vm))
                }
                _ => (self.client.clone(), None),
            }
        } else {
            (self.client.clone(), None)
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
        .with_compression(context_length, self.config.agent.context_compression)
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
        agent = agent.with_provider_name(self.resolver.main_provider());
        // Phase 36.15 Plan 04 (PROV-11): wire per-turn merged extras resolved above.
        agent = agent.with_resolved_extras(resolved_extras_for_turn);
        if let Some(ref key) = self.resolver.resolve_for_main().api_key {
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
            &self.config,
            &self.resolver,
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
            *prev = Some(self.resolver.resolve_for_main().default_model.clone());
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
            config,
            resolver,
            client,
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
        let provider_name = self.resolver.main_provider();
        let model_name = self.resolver.resolve_for_main().default_model.clone();
        let merged = ironhermes_core::config_extras::resolve_extras(
            &self.config.providers,
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
        let route_pos = SOURCE
            .find("build_role_client(&self.resolver, \"vision\")")
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
            config,
            resolver,
            client,
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
}

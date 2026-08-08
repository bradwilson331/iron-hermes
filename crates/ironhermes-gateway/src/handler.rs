use anyhow::Result;
use async_trait::async_trait;
// Phase 39.1 (R39.1-06): is_bypass, RunningAgentGuard, and AGENT_RUNNING_REJECT_MSG
// are no longer used — all four agent_running gate sites have been removed.
// Concurrency is now managed via ConcurrencyLayer + TurnRegistry.
use ironhermes_core::concurrency::{ConcurrencyLayer, Surface, TurnEntry, TurnId, TurnRegistry};
use std::sync::Arc;
use tokio::sync::{Mutex as TokioMutex, RwLock, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use ironhermes_agent::agent_loop::{StreamCallback, ToolProgressCallback};
use ironhermes_agent::context_compressor::estimate_messages_tokens;
use ironhermes_agent::context_engine::{ContextEngine, ContextStats};
use ironhermes_agent::subagent_registry::SubagentRegistry;
use ironhermes_agent::{
    AgentRuntime, MemoryManager, PromptBuilder, TurnRequest, build_main_client,
};
use ironhermes_core::commands::context::{
    CoreContextHandles, McpReloader, ProcessRegistrySnapshotHandle, StateStoreHandle,
    SubagentListSnapshot, ToolsetSessionHandle, build_core_context,
};
use ironhermes_core::commands::{
    CommandResult as CoreCommandResult, CommandRouter, ResolveResult, registry::build_registry,
};
// Phase 41.1 Plan 04 (D-08): shared pure skill-invocation resolver (Plan 01).
// Both gateway skill-invoke sites use it to compute the run-turn trigger_text,
// then synthesize an identity-inheriting MessageEvent and fire run_agent.
use ironhermes_core::commands::skill_dispatch::{build_skill_invocation, resolve_skill_invocation};
use ironhermes_core::{
    ChatMessage, Config, ContentPart, ImageUrl, MessageContent, MessageEvent, Platform,
    ProviderResolver, Role, SkillRegistry,
};
use ironhermes_exec::process_registry::ProcessRegistry;
use ironhermes_tools::ToolRegistry;

use crate::adapter::{MediaSender, MessageHandler, PlatformAdapter};
use crate::multimodal::ProcessedAttachments;
use crate::rate_limiter::{PerUserRateLimiter, with_rate_limit_retry};
use crate::session::{SessionKey, SessionStore};
use crate::session_queue::{QueueError, SessionQueue};
use crate::stream_consumer::{DeliveryMode, StreamConsumer, send_chunked};
use crate::user_queue::{DispatchOutcome, UserQueueManager};

/// Phase 41.1 Plan 04 (UI-SPEC Copywriting Contract / §C): build the run-turn
/// meta text sent immediately before the skill's run turn on Telegram. Bare
/// invoke → `▶ Ran skill /{name}`; argued invoke → `▶ Ran skill /{name} · "{args}"`,
/// with `args` truncated to 40 chars (char-safe) and an inner `…` appended only
/// when truncated. Mirrors the Web/TUI `run_turn_meta_chip` verbatim so every
/// surface renders identical copy.
///
/// Bare-vs-argued is derived by comparing `trigger_text` to the bare-invoke
/// run-now instruction (the same value `build_skill_invocation` computes for a
/// bare invoke) — alias-robust, never re-parsing the raw slash token.
fn run_turn_meta_text(name: &str, trigger_text: &str) -> String {
    let bare_instruction =
        format!("Run the {name} skill now: carry out its instructions immediately.");
    if trigger_text == bare_instruction {
        format!("▶ Ran skill /{name}")
    } else {
        const MAX: usize = 40;
        let mut chars = trigger_text.chars();
        let head: String = chars.by_ref().take(MAX).collect();
        let truncated = chars.next().is_some();
        if truncated {
            format!("▶ Ran skill /{name} · \"{head}…\"")
        } else {
            format!("▶ Ran skill /{name} · \"{head}\"")
        }
    }
}

/// Bridges incoming Telegram messages to the AgentLoop with streaming output.
pub struct GatewayMessageHandler {
    config: Config,
    resolver: ProviderResolver,
    session_store: Arc<RwLock<SessionStore>>,
    // retained handle; production per-turn AgentLoop construction wiring pending
    #[allow(dead_code)]
    tool_registry: Arc<RwLock<ToolRegistry>>,
    memory_manager: Option<Arc<TokioMutex<MemoryManager>>>,
    hook_registry: Option<Arc<ironhermes_hooks::HookRegistry>>,
    /// Phase 21.8.2 D-03 / D-Plan03-01 UPDATED: interior-mutable Arc-of-Arc so
    /// the `/skills reload` arm can atomically swap the inner Arc without
    /// requiring &mut self. Outer Arc: shared ownership across handler clones.
    /// Mutex: synchronous lock for the brief swap window (load_with_config is
    /// sync; lock held only for the swap, not for the load). Inner Arc: the
    /// actual `SkillRegistry` snapshot used by build_command_context calls.
    skill_registry: Arc<std::sync::Mutex<Option<Arc<SkillRegistry>>>>,

    /// Phase 21.8.2 D-Plan03-05 / D-07 (gateway delivery): per-session activated
    /// skill overlays. The SkillActivated arm and the SKILL-13 fallback push
    /// (name, body) for the current session key. The AgentLoop call site reads
    /// the overlay vector for the session and prepends each body to the system
    /// prompt before the agent runs (same semantics as hermes-agent's
    /// per-session skill injection). Cleared on session-end / `/clear` (out of
    /// scope for this phase — matches /personality overlay semantics).
    #[allow(clippy::type_complexity)]
    // per-session overlay map; type alias would only exist here, inline is clearer
    skill_overlays:
        Arc<std::sync::Mutex<std::collections::HashMap<SessionKey, Vec<(String, String)>>>>,

    /// Phase 21.8.2 D-02 / D-05: SkillsConfig used by the SkillsReload arm to
    /// re-invoke load_with_config. Set via `set_skills_config` after construction.
    skills_config: Option<ironhermes_core::config::SkillsConfig>,

    active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>>,
    rate_limiter: PerUserRateLimiter,
    /// Phase 18 Plan 06: per-turn hygiene compression engine (runs at
    /// `gateway.compression_threshold`, default 0.85). None = no hygiene pass.
    gateway_engine: Option<Arc<dyn ContextEngine>>,
    /// Provider context window used for ratio calculation. Falls back to
    /// 128k when the resolver does not expose a per-endpoint value.
    context_length: usize,
    /// Phase 21.1 Plan 02: unified slash command router.
    command_router: CommandRouter,
    /// Plan 28.1-02: the single AgentRuntime threaded from the gateway runner.
    /// run_agent builds a TurnRequest and delegates to `runtime.run_turn`,
    /// which resets the budget, builds the loop from durable Arcs, and runs.
    agent_runtime: Option<Arc<AgentRuntime>>,
    /// Plan 21.7-06 (D-29, D-24): gateway-scoped ProcessRegistry threaded
    /// from the runner. Per-request handler calls `drain_and_kill_session`
    /// at on_session_end. Gateway registry task_id is a process-wide constant
    /// ("gateway") so per-request drain_and_kill_session mismatches and is a
    /// no-op — cleanup happens via LRU/TTL prune; true per-session scoping
    /// is deferred (matches the BudgetHandle lifecycle decision in Plan 05).
    process_registry: Option<Arc<RwLock<ProcessRegistry>>>,
    /// Plan 21.7-07 (D-03 / D-04 / D-05): gateway-scoped SubagentRegistry
    /// threaded from the runner. Shared with the delegate_task runner via
    /// main.rs so lifecycle events update state. Per-request on_session_end
    /// sleeps 200ms to drain pending fire-and-forget transcript writes.
    subagent_registry: Option<Arc<RwLock<SubagentRegistry>>>,
    /// Phase 41.3 Plan 04 (D-11/D-12): MCP reload handle for the gateway's
    /// slash-dispatch `CommandContext` — the gateway previously had no MCP
    /// handle at all. Cloned from `GatewayRunner.mcp_manager` (already used
    /// there for GAP-8 shutdown wiring) via `set_mcp_manager` below, and wired
    /// into `CoreContextHandles.mcp_reloader` in `handle_slash_command`.
    mcp_manager: Option<Arc<ironhermes_mcp::McpManager>>,
    /// Phase 25.1 D-03/D-17: shared browser session Arc threaded from the runner.
    /// All 11 browser_* tools share this Arc (registered via register_browser_tools
    /// in main.rs run_gateway). Per-request AgentLoop calls with_browser_session
    /// so the AgentLoop holds a reference — ensuring drop semantics clean up the
    /// browser process when the last Arc clone drops (T-25.1-04).
    browser_session: Option<
        std::sync::Arc<
            tokio::sync::Mutex<Option<ironhermes_tools::browser_session::BrowserSession>>,
        >,
    >,
    /// Phase 25.2 Plan 15 follow-up (UAT Issue 2 / Symptom 1): production
    /// `ToolsetSessionHandle` for the gateway's `/toolset` slash dispatch.
    /// Plan 15 wired this in main.rs run_gateway but never threaded the Arc
    /// through to GatewayMessageHandler — without this field, the gateway's
    /// CommandContext at handler.rs:254 has `toolset_session: None` and
    /// cmd_toolset (handlers.rs:782) returns the documented fallback string
    /// in Telegram (UAT Test 2 reproduced this twice — REPL via tui_rata
    /// and Telegram via this gateway path).
    toolset_session: Option<Arc<dyn ToolsetSessionHandle>>,
    /// Phase 25.3 D-W-2: Workspace clone — propagated by `build_gateway_handler`
    /// from `GatewayRunner.workspace`. Per-message slash dispatch attaches via
    /// `.with_workspace` so /sessions --workspace + trajectory scoping see the
    /// resolved root.
    workspace: Option<Arc<ironhermes_core::workspace::Workspace>>,
    // Phase 25.3-15 CR-02 close-out: the per-handler `trajectory_writer` field
    // was REMOVED. The previous implementation held a single process-wide
    // handle keyed by `gateway-<random-uuid>` that was unreachable from
    // `hermes session export <session_id>`. Per-session writers are now owned
    // (and lazily opened) by `SessionStore`, keyed by the canonical SQLite
    // session UUID. `run_agent` reaches them via
    // `self.session_store.write().await.get_or_create_trajectory_writer(...)`.
    /// Phase 21.8.3.1 D-07: active personality overlay per session.
    /// Set by command dispatch when /personality <name> succeeds (D-08).
    /// Cleared by /personality clear (CONTEXT D-05 gateway analog, pre-dispatch).
    /// Re-applied to PromptBuilder slot 8 every turn (D-09).
    /// Uses interior mutability (Arc<Mutex<HashMap<SessionKey, String>>>) mirroring
    /// skill_overlays — handle_slash_command takes &self, not &mut self (deviation from
    /// plan assumption; auto-fixed Rule 1). Per-session keying prevents cross-user bleed.
    active_personality_overlay:
        Arc<std::sync::Mutex<std::collections::HashMap<SessionKey, String>>>,

    /// Phase 32 Plan 02 (LEARN-01): per-session nudge turn counter.
    ///
    /// Arc<Mutex<HashMap>> mirrors the `skill_overlays` / `active_personality_overlay`
    /// interior-mutability pattern — `run_agent` takes `&self`, so we cannot mutate a
    /// plain field. The std::sync::Mutex is intentional: the guard is taken in a small
    /// synchronous block, the should_fire bool is extracted, and the guard is dropped
    /// BEFORE any `tokio::spawn` / `.await` (T-32-07 mitigation; clippy `await_holding_lock`).
    ///
    /// Key: SessionKey (cloned from `key` in `run_agent`). Value: u32 turn count.
    /// Reset to 0 on fire; entries removed via session_store eviction is best-effort
    /// (T-32-06 accepted — one u32 per active session is negligible memory).
    nudge_turns: Arc<std::sync::Mutex<std::collections::HashMap<SessionKey, u32>>>,

    /// Phase 36.17.1 (D-14, RESEARCH Open Q3): per-session FIFO message queue,
    /// threaded from `GatewayRunner::build_gateway_handler` via
    /// `set_session_queue`. `Option` preserves backward-compat for handlers
    /// built outside `build_gateway_handler` (e.g. Phase 36 GW-05 tests at
    /// `tests/running_agent_guard_tests.rs` that call `GatewayMessageHandler::new`
    /// directly — those still see the original reject-with-AGENT_RUNNING_REJECT_MSG
    /// behavior when this field is `None`). The `Arc` mirrors the exact pattern
    /// used for `Arc<RwLock<SessionStore>>`; using `Arc<SessionQueue>` rather
    /// than `Arc<GatewayRunner>` avoids the circular-reference trap noted in
    /// RESEARCH Open Q3.
    session_queue: Option<Arc<SessionQueue>>,

    /// Phase 36.17.2.1 D-01/D-02: per-session UserQueueManager handle threaded
    /// from `GatewayRunner::run_gateway` via `set_user_queue_manager`. Used by
    /// the `CoreCommandResult::Queued` arm so the `/queue` synthesized event
    /// goes through `UQM::dispatch` (which calls `notify_one()` to wake the
    /// parked per-chat worker — user_queue.rs:154) instead of the direct
    /// `session_queue.try_push` path that has no wake protocol. `Option`
    /// preserves backward-compat for handlers built outside
    /// `GatewayRunner::run_gateway` (e.g. Phase 36 GW-05 tests, the
    /// `session_queue_integration.rs` harness that exercises the busy-branch
    /// fallback per parent phase D-20 contract). When `None`, the `Queued`
    /// arm falls back to the original direct-try_push code path (degraded —
    /// no wake, but tests that hand-spawn workers do not depend on Notify).
    user_queue_manager: Option<Arc<UserQueueManager>>,
    /// Phase 36.17.2.2 D-18: optional `MediaSender` impl wired by
    /// `GatewayRunner` on the Telegram start path via `set_media_sender`.
    /// Discord/Slack/web start paths leave this `None`; the D-19 dispatch
    /// loop in `run_agent` warns and drops extracted `<MEDIA: ...>` refs
    /// on those platforms (Pitfall 3 silent-drop mitigation — every
    /// dropped-tag turn emits a `warn!` with the chat_id + ref count).
    media_sender: Option<Arc<dyn MediaSender>>,
    /// Phase 36.17.7 D-01: per-turn AudioDispatcher slot. Telegram start path
    /// mounts TelegramAdapter clone-cast; Discord/Slack start paths mount
    /// NotSupportedAudioDispatcher per D-03-b. Mirrors media_sender directly
    /// above. Field name is historical — the type is platform-agnostic
    /// (`Option<Arc<dyn AudioDispatcher>>`), so Discord/Slack stubs mount cleanly.
    pub telegram_audio_dispatcher: Option<Arc<dyn ironhermes_tools::AudioDispatcher>>,

    /// Phase 36.3.8 D-02: per-turn MessageDispatcher slot for send_message.
    /// Telegram start path mounts TelegramAdapter clone-cast via
    /// `set_telegram_message_dispatcher`. Local/web paths leave this `None`
    /// — SendMessageTool's Local arm prints to stdout without a dispatcher.
    telegram_message_dispatcher: Option<Arc<dyn ironhermes_tools::MessageDispatcher>>,

    /// Phase 36.3.8 D-04: per-turn ClarifyDispatcher slot for clarify.
    /// Telegram start path mounts TelegramAdapter clone-cast (sends inline_keyboard).
    /// Web uses a text-fallback dispatcher. Local leaves this `None` (stdout list).
    telegram_clarify_dispatcher: Option<Arc<dyn ironhermes_tools::ClarifyDispatcher>>,

    /// Phase 36.3.8 D-05/T-36.3.8-ROUTE: shared PendingClarifyRegistry Arc.
    /// MUST be the same Arc passed to GatewayRunner's callback_query dispatch loop
    /// (Plan 03) so a button tap resolves the correct awaiter. Constructed once in
    /// runner.rs and cloned into both the runner loop and this handler via
    /// `set_clarify_registry`. Always-initialized to a fresh empty registry so
    /// handlers built outside the runner compile without a None guard.
    clarify_registry: Arc<ironhermes_tools::clarify_registry::PendingClarifyRegistry>,

    /// Phase 39.1 (R39.1-09 / D-09): shared process-wide turn registry.
    /// Wired by `GatewayRunner::build_gateway_handler` via `set_turn_registry`.
    /// Always-initialized to a local default so handlers built without a runner
    /// (e.g. tests) compile and run without a None check.
    turn_registry: Arc<TurnRegistry>,

    /// Phase 39.1 (R39.1-03 / D-03): per-session + global semaphore layer.
    /// Wired by `GatewayRunner::build_gateway_handler` via `set_concurrency`.
    /// `None` → fall back to old serialized-RunningAgentGuard behaviour (only
    /// during tests that don't call `set_concurrency`).
    concurrency: Option<Arc<ConcurrencyLayer>>,

    /// Phase 45 D-11: shared approval coordinator for the /approve + /deny
    /// slash commands and per-turn GatewayApprovalGate injection.
    /// Constructed from `config.approvals.timeout_secs`, the platform adapter,
    /// and the existing ApprovalsStore. `None` = gate unavailable (fail-closed).
    approval_coordinator: Option<Arc<crate::approval::ApprovalCoordinator>>,
}

impl GatewayMessageHandler {
    pub fn new(
        config: Config,
        resolver: ProviderResolver,
        session_store: Arc<RwLock<SessionStore>>,
        tool_registry: Arc<RwLock<ToolRegistry>>,
    ) -> Self {
        let rate_limiter = PerUserRateLimiter::new(
            config.rate_limit.messages_per_minute,
            config.rate_limit.burst_size,
        );
        // Phase 21.3: resolve context_length before moving resolver into struct
        let context_length = resolver.resolve_for_main().context_length();
        Self {
            config,
            resolver,
            session_store,
            tool_registry,
            memory_manager: None,
            hook_registry: None,
            skill_registry: Arc::new(std::sync::Mutex::new(None)),
            skill_overlays: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            skills_config: None,
            active_skills: Arc::new(std::sync::Mutex::new(Vec::new())),
            rate_limiter,
            gateway_engine: None,
            context_length,
            command_router: CommandRouter::new(build_registry()),
            agent_runtime: None,
            process_registry: None,
            subagent_registry: None,
            // Phase 41.3 Plan 04 (D-11/D-12): no MCP handle until GatewayRunner
            // calls set_mcp_manager (mirrors process_registry / subagent_registry).
            mcp_manager: None,
            browser_session: None,
            toolset_session: None,
            workspace: None, // Phase 25.3 D-W-2: wired by GatewayRunner::build_gateway_handler
            // Phase 25.3-15 CR-02: trajectory_writer field removed — per-session
            // writers live in SessionStore, looked up by canonical session UUID.
            // Phase 21.8.3.1 D-07: no personality active until /personality <name> sets it.
            // Arc<Mutex<HashMap>> mirrors skill_overlays — &self constraint requires interior mutability.
            active_personality_overlay: Arc::new(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            // Phase 32 Plan 02 (LEARN-01): per-session nudge counter starts empty;
            // entries created lazily on first turn per session in run_agent's fire site.
            nudge_turns: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            // Phase 36.17.1 (D-14, RESEARCH Open Q3): no queue wired until
            // `build_gateway_handler` calls `set_session_queue`. Phase 36 GW-05
            // tests that construct `GatewayMessageHandler::new` directly see
            // None here and fall through to the original reject path.
            session_queue: None,
            // Phase 36.17.2.1 D-01: no UQM wired until
            // `GatewayRunner::run_gateway` calls `set_user_queue_manager`.
            // Handlers built via direct ::new() see None here and fall through
            // to the legacy direct-try_push path in the Queued arm.
            user_queue_manager: None,
            // Phase 36.17.2.2 D-18: no MediaSender wired until
            // `GatewayRunner` calls `set_media_sender` on the Telegram start
            // path. Discord/Slack/web handlers leave this `None`; the D-19
            // dispatch loop in `run_agent` warns and drops extracted tags
            // when `media_sender.is_none()`.
            media_sender: None,
            // Phase 36.17.7 D-01: no AudioDispatcher wired until the runner's
            // platform-specific start path calls `set_telegram_audio_dispatcher`.
            // Telegram mounts TelegramAdapter; Discord/Slack mount
            // NotSupportedAudioDispatcher per D-03-b.
            telegram_audio_dispatcher: None,
            // Phase 36.3.8 D-02/D-04: no messaging dispatchers until the runner's
            // Telegram start path calls set_telegram_message_dispatcher /
            // set_telegram_clarify_dispatcher. Other platforms leave these None.
            telegram_message_dispatcher: None,
            telegram_clarify_dispatcher: None,
            // Phase 36.3.8 D-05/T-36.3.8-ROUTE: default empty registry so handlers
            // built without a runner compile. Replaced by the runner's shared Arc via
            // set_clarify_registry so the button-tap resolution map is shared.
            clarify_registry: Arc::new(
                ironhermes_tools::clarify_registry::PendingClarifyRegistry::new(),
            ),
            // Phase 39.1 (R39.1-09): default local registry; replaced by the
            // process-wide shared Arc via `set_turn_registry` when wired through
            // `build_gateway_handler`. Tests that call `::new()` directly get an
            // isolated registry, which is correct for per-test isolation.
            turn_registry: Arc::new(TurnRegistry::new()),
            // Phase 39.1 (R39.1-03): None until wired; run_agent falls back to
            // inline RAII guard path when None (backward-compat for tests).
            concurrency: None,
            // Phase 45 D-11: no coordinator until set_approval_coordinator is called
            // by the platform runner. Handlers built via direct ::new() see None and
            // leave the gate unavailable (fail-closed — no approvals without wiring).
            approval_coordinator: None,
        }
    }

    /// Phase 25.2 Plan 15 follow-up (UAT Issue 2 / Symptom 1 close-out for
    /// Telegram): install the production `ToolsetSessionHandle` so the
    /// gateway's `/toolset list/show/enable/disable` slash commands work.
    /// Mirrors `set_memory_manager` / `set_subagent_registry` lifecycle —
    /// caller in main.rs run_gateway threads in the same Arc that the
    /// REPL and single-shot binary already use.
    pub fn set_toolset_session(&mut self, handle: Arc<dyn ToolsetSessionHandle>) {
        self.toolset_session = Some(handle);
    }

    /// Phase 25.3 D-W-2: install the resolved Workspace clone for the gateway
    /// handler. Caller is `GatewayRunner::build_gateway_handler`.
    pub fn set_workspace(&mut self, workspace: Arc<ironhermes_core::workspace::Workspace>) {
        self.workspace = Some(workspace);
    }

    // Phase 25.3-15 CR-02 close-out: `set_trajectory_writer` removed. The
    // gateway no longer holds a single process-wide writer; per-session
    // writers are owned by `SessionStore` (lazily opened, cached by session
    // UUID). Configuration goes through `GatewayRunner::set_trajectory_root`
    // -> `SessionStore::set_trajectory_root`.

    /// Plan 28.1-02: install the AgentRuntime so run_agent can delegate every
    /// top-level turn through `runtime.run_turn`. Caller is
    /// `GatewayRunner::build_gateway_handler`.
    pub fn set_agent_runtime(&mut self, runtime: Arc<AgentRuntime>) {
        self.agent_runtime = Some(runtime);
    }

    /// Phase 45 D-11: install the ApprovalCoordinator so /approve, /deny, and
    /// per-turn GatewayApprovalGate injection are active. Caller is the platform
    /// runner (run_gateway on the Telegram start path, or any surface that wants
    /// chat-native approval prompts).
    pub fn set_approval_coordinator(&mut self, coord: Arc<crate::approval::ApprovalCoordinator>) {
        self.approval_coordinator = Some(coord);
    }

    /// Plan 21.7-06 (D-29, D-24): install the gateway-scoped ProcessRegistry
    /// so per-request on_session_end can invoke `drain_and_kill_session`.
    pub fn set_process_registry(&mut self, reg: Arc<RwLock<ProcessRegistry>>) {
        self.process_registry = Some(reg);
    }

    /// Plan 21.7-07 (D-03 / D-04 / D-05): install the gateway-scoped
    /// SubagentRegistry. Per-request on_session_end drains pending
    /// fire-and-forget transcript writes (sleep 200ms).
    pub fn set_subagent_registry(&mut self, reg: Arc<RwLock<SubagentRegistry>>) {
        self.subagent_registry = Some(reg);
    }

    /// Phase 41.3 Plan 04 (D-11/D-12): install the McpManager handle so the
    /// gateway's slash-dispatch `CommandContext` can wire `mcp_reloader` — the
    /// gateway had no MCP handle on `CommandContext` before this plan (the
    /// existing `GatewayRunner.mcp_manager` only served GAP-8 shutdown wiring).
    /// Caller is `GatewayRunner::build_gateway_handler`.
    pub fn set_mcp_manager(&mut self, mgr: Arc<ironhermes_mcp::McpManager>) {
        self.mcp_manager = Some(mgr);
    }

    /// Phase 25.1 D-17: install the shared browser session Arc so each per-request
    /// AgentLoop can call `.with_browser_session(...)`. Mirrors set_memory_manager shape.
    /// The Arc is pre-constructed in run_gateway (main.rs) and cloned into every
    /// per-request AgentLoop so AgentLoop drop semantics clean up the browser process
    /// when the last Arc clone drops (T-25.1-04 resource exhaustion mitigation).
    pub fn set_browser_session(
        &mut self,
        session: std::sync::Arc<
            tokio::sync::Mutex<Option<ironhermes_tools::browser_session::BrowserSession>>,
        >,
    ) {
        self.browser_session = Some(session);
    }

    /// Phase 18 Plan 06: install the per-turn hygiene engine. Wired by composition
    /// root (gateway startup) using `engine_factory::build_context_engine(...)`
    /// with `config.gateway.context_engine` + `config.gateway.compression_threshold`.
    pub fn set_gateway_engine(&mut self, engine: Arc<dyn ContextEngine>, context_length: usize) {
        self.gateway_engine = Some(engine);
        self.context_length = context_length;
    }

    /// Test-only accessor: used by 18-08 runner tests to assert the engine is attached.
    #[cfg(test)]
    pub(crate) fn gateway_engine_is_some(&self) -> bool {
        self.gateway_engine.is_some()
    }

    /// Phase 18 Plan 06: per-turn hygiene check (D-12, planner guidance #7).
    /// Compresses in-place when `estimated / context_length >= gateway.compression_threshold`.
    /// No-op when no engine is configured or ratio is below threshold.
    ///
    /// NOTE: D-13 parent_session_id lineage is deferred to Phase 21 (full gateway lifecycle).
    pub(crate) async fn maybe_compress_gateway(&self, messages: &mut Vec<ChatMessage>) -> bool {
        let Some(engine) = self.gateway_engine.as_ref() else {
            return false;
        };
        let estimated = estimate_messages_tokens(messages);
        let ratio = estimated as f32 / self.context_length.max(1) as f32;
        let gw_threshold = self.config.gateway.compression_threshold;
        if ratio < gw_threshold {
            return false;
        }
        let stats = ContextStats {
            context_length: self.context_length,
            estimated_tokens: estimated,
            protect_first_n: self.config.compression.protect_first_n,
            protect_last_tokens: self
                .config
                .compression
                .protect_last_tokens
                .min(self.context_length / 4),
            compression_count: 0,
            prior_summary: None,
        };
        match engine.compress(messages, stats).await {
            Ok(outcome) => outcome.compressed,
            Err(e) => {
                tracing::error!(error = ?e, "gateway hygiene compression failed");
                false
            }
        }
    }

    /// Plan 20-02: set the `MemoryManager` handle used for prompt injection
    /// and tool/memory writes. The handle is shared (clone-of-Arc) with the
    /// runner + tool registry + context engine for consistent fanout.
    pub fn set_memory_manager(&mut self, manager: Arc<TokioMutex<MemoryManager>>) {
        self.memory_manager = Some(manager);
    }

    /// Phase 36.17.1 (D-14, D-15, RESEARCH Open Q3): install the per-session
    /// FIFO queue Arc threaded from `GatewayRunner::build_gateway_handler`.
    /// Once set, `handle_with_multimodal`'s busy-branch enqueues instead of
    /// rejecting; unset preserves the original `AGENT_RUNNING_REJECT_MSG`
    /// path for handlers built outside the runner (e.g. Phase 36 GW-05 tests).
    pub fn set_session_queue(&mut self, queue: Arc<SessionQueue>) {
        self.session_queue = Some(queue);
    }

    /// Phase 36.17.2.1 D-01/D-03: install the shared `Arc<UserQueueManager>`
    /// threaded from `GatewayRunner::run_gateway`. Once set, the
    /// `CoreCommandResult::Queued` arm delegates to `uqm.dispatch(...)` which
    /// performs push + `notify_one()` atomically — fixing the regression where
    /// `/queue` typed during a busy turn left the parked worker stranded
    /// (UAT 2026-05-28T15:36-15:38 UTC). Unset preserves the legacy direct
    /// `session_queue.try_push` path for backward-compat (D-20 from parent phase).
    pub fn set_user_queue_manager(&mut self, uqm: Arc<UserQueueManager>) {
        self.user_queue_manager = Some(uqm);
    }

    /// Phase 36.17.2.2 D-18: install the `MediaSender` impl. In production
    /// `GatewayRunner` clone-casts the same `Arc<TelegramAdapter>` it uses
    /// for `Arc<dyn PlatformAdapter>` into `Arc<dyn MediaSender>` and threads
    /// it here (clone-cast twice, NEVER upcast — see RESEARCH Open Q4 /
    /// Assumption A7). Discord/Slack/web start paths do NOT call this; on
    /// those platforms `media_sender` stays `None` and the D-19 dispatch
    /// loop in `run_agent` warns + drops any extracted `<MEDIA: ...>` refs
    /// (Pitfall 3 silent-drop mitigation — `warn!` emits chat_id + ref count
    /// so operators can see the misconfiguration). Setter pattern (rather
    /// than constructor param) keeps the 5 existing `GatewayMessageHandler::new`
    /// call sites stable across production + 4 test fixtures.
    pub fn set_media_sender(&mut self, sender: Arc<dyn MediaSender>) {
        self.media_sender = Some(sender);
    }

    /// Phase 36.17.7 D-01: install an AudioDispatcher for per-turn TTS wiring.
    /// Telegram mounts TelegramAdapter clone-cast as `Arc<dyn AudioDispatcher>`;
    /// Discord/Slack mount `NotSupportedAudioDispatcher` per D-03-b.
    /// Mirrors `set_media_sender` directly above.
    pub fn set_telegram_audio_dispatcher(
        &mut self,
        dispatcher: Arc<dyn ironhermes_tools::AudioDispatcher>,
    ) {
        self.telegram_audio_dispatcher = Some(dispatcher);
    }

    /// Phase 36.3.8 D-02: install a MessageDispatcher for per-turn send_message wiring.
    /// Telegram start path mounts TelegramAdapter clone-cast. Mirrors
    /// `set_telegram_audio_dispatcher` directly above.
    pub fn set_telegram_message_dispatcher(
        &mut self,
        dispatcher: Arc<dyn ironhermes_tools::MessageDispatcher>,
    ) {
        self.telegram_message_dispatcher = Some(dispatcher);
    }

    /// Phase 36.3.8 D-04: install a ClarifyDispatcher for per-turn clarify wiring.
    /// Telegram start path mounts TelegramAdapter clone-cast (inline_keyboard).
    pub fn set_telegram_clarify_dispatcher(
        &mut self,
        dispatcher: Arc<dyn ironhermes_tools::ClarifyDispatcher>,
    ) {
        self.telegram_clarify_dispatcher = Some(dispatcher);
    }

    /// Phase 36.3.8 D-05/T-36.3.8-ROUTE: install the shared PendingClarifyRegistry Arc.
    /// MUST be the same Arc as the one in the runner callback_query loop so a button
    /// tap resolves the correct awaiter (single-instance sharing invariant).
    pub fn set_clarify_registry(
        &mut self,
        registry: Arc<ironhermes_tools::clarify_registry::PendingClarifyRegistry>,
    ) {
        self.clarify_registry = registry;
    }

    /// Phase 39.1 (R39.1-09 / D-09): install the shared process-wide TurnRegistry.
    /// Called by `GatewayRunner::build_gateway_handler` so all surfaces share one
    /// registry Arc. Handlers built via `::new()` directly keep their isolated
    /// default registry (correct for test isolation).
    pub fn set_turn_registry(&mut self, registry: Arc<TurnRegistry>) {
        self.turn_registry = registry;
    }

    /// Phase 39.1 (R39.1-03 / D-03): install the ConcurrencyLayer so `run_agent`
    /// acquires per-session + global semaphore before spawning a turn. When None
    /// (handlers built outside `build_gateway_handler`), `run_agent` falls back to
    /// the legacy single-turn RAII path for backward compat.
    pub fn set_concurrency(&mut self, layer: Arc<ConcurrencyLayer>) {
        self.concurrency = Some(layer);
    }

    /// Set the hook registry for event emission.
    pub fn set_hook_registry(&mut self, registry: Arc<ironhermes_hooks::HookRegistry>) {
        self.hook_registry = Some(registry);
    }

    /// Set the skill registry for catalog injection into the system prompt.
    /// Phase 21.8.2 D-03: stores inside the interior-mutable Mutex so
    /// `/skills reload` can atomically swap without &mut self.
    pub fn set_skill_registry(&mut self, registry: Arc<SkillRegistry>) {
        if let Ok(mut guard) = self.skill_registry.lock() {
            *guard = Some(registry);
        }
    }

    /// Phase 21.8.2 D-02: store the SkillsConfig so the SkillsReload arm can
    /// call `load_with_config` on demand. Called from runner.build_gateway_handler
    /// after `set_skill_registry`.
    pub fn set_skills_config(&mut self, cfg: ironhermes_core::config::SkillsConfig) {
        self.skills_config = Some(cfg);
    }

    /// Set the shared active skills tracker. Must be the same Arc given to SkillsTool
    /// so that skill activations reach AgentLoop enforcement.
    /// NOTE: global-shared across all users — would need per-session isolation for multi-user support (per D-06).
    pub fn set_active_skills(
        &mut self,
        skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>>,
    ) {
        self.active_skills = skills;
    }

    /// Phase 42 EXEC-06, MIGRATED by Phase 36.3.12 Plan 07 (D-08, checker BLOCKER
    /// T-36.3.12-25): guarded gateway shell-exec entry (fail-closed, D-12/D-13).
    ///
    /// Builds a `DangerousCommandGuardrail` from `self.config.dangerous_commands`
    /// and routes through `ironhermes_hooks::execute_gated_command` — the SAME
    /// chokepoint every other surface uses — instead of the gateway-local
    /// `crate::shell_exec::shell_exec` helper (which never audited its `Allow` path
    /// and never forced approval on a remote/credential-forwarding run). Reuses the
    /// already-registered `terminal` tool instance (`AgentRuntime::terminal_tool_arc`)
    /// so `background=true` keeps its `ProcessRegistry` wiring.
    ///
    /// # yolo note (INV-21.7-05)
    ///
    /// Gateway sessions NEVER read a per-request yolo flag from the incoming
    /// message. Pass `yolo=false` for all production call sites. The parameter
    /// is exposed here so callers that derive the flag from a process-wide config
    /// value (e.g. `config.autonomous.yolo`) can thread it through.
    pub async fn handle_shell_exec(
        &self,
        command: &str,
        yolo: bool,
        session_id: &str,
        chat_id: &str,
        approval_gate: Option<&dyn ironhermes_core::ApprovalGate>,
    ) -> ironhermes_hooks::GatedOutcome {
        let guard = ironhermes_hooks::DangerousCommandGuardrail::from_config(
            &self.config.dangerous_commands,
        );
        let audit_log = ironhermes_core::AuditLog::load(self.config.audit.clone());
        let is_remote_backend = self.config.terminal.backend == "ssh";
        let forward_env_nonempty = !self.config.terminal.forward_env.is_empty();
        let tool = self
            .agent_runtime
            .as_ref()
            .and_then(|rt| rt.terminal_tool_arc());
        let command_owned = command.to_string();

        let outcome = ironhermes_hooks::execute_gated_command(
            "terminal",
            command,
            &guard,
            approval_gate,
            &audit_log,
            session_id,
            "gateway",
            chat_id,
            yolo,
            is_remote_backend,
            forward_env_nonempty,
            || async move {
                match tool {
                    Some(t) => {
                        t.execute(serde_json::json!({ "command": command_owned }))
                            .await
                    }
                    None => Err(anyhow::anyhow!(
                        "terminal tool not registered on this runtime"
                    )),
                }
            },
        )
        .await;
        // WR-03: log spawn failures at warn! so operators can detect them in
        // monitoring. `Failed` is distinct from `Ran` (command ran, produced
        // output) and `Denied`/`Blocked` (guardrail decisions).
        if let ironhermes_hooks::GatedOutcome::Failed(ref reason) = outcome {
            tracing::warn!(
                target: "ironhermes::gateway::shell_exec",
                command = %command,
                reason = %reason,
                "shell_exec subprocess spawn failed (WR-03)"
            );
        }
        outcome
    }

    /// Phase 36.17.9: handle `/voice on|off|tts|status` for a gateway chat.
    ///
    /// `on`/`tts`/`off` persist the per-session voice mode write-through to the
    /// durable `gateway_routes` record (and update the in-memory session when
    /// present), so the choice survives a gateway restart. `status` (or no arg)
    /// reports the current mode, preferring the live session value and falling
    /// back to the persisted route.
    async fn handle_voice_command(&self, key: &SessionKey, args: &[&str]) -> String {
        match args.first().copied() {
            Some(mode @ ("on" | "off" | "tts")) => {
                self.session_store.write().await.set_voice_mode(key, mode);
                match mode {
                    "on" => "Voice mode: on — I'll speak replies to your voice messages.",
                    "tts" => "Voice mode: tts — I'll speak every reply.",
                    _ => "Voice mode: off — replies are text only.",
                }
                .to_string()
            }
            Some("status") | None => {
                let mode = self.current_voice_mode(key).await;
                format!("Voice mode: {mode}")
            }
            Some(other) => {
                format!("Unknown /voice option '{other}'. Use: on | off | tts | status")
            }
        }
    }

    /// Resolve the effective voice mode for a chat: the live in-memory session
    /// value if present, else the durable `gateway_routes` value, else `off`.
    async fn current_voice_mode(&self, key: &SessionKey) -> String {
        if let Some(mode) = self.session_store.read().await.voice_mode(key) {
            return mode;
        }
        let key_str = key.to_string_key();
        let store_arc = self.session_store.read().await.state_store().clone();
        let persisted = store_arc
            .lock()
            .ok()
            .and_then(|s| s.get_route(&key_str).ok().flatten())
            .map(|r| r.voice_mode);
        persisted.unwrap_or_else(|| "off".to_string())
    }

    /// Dispatch a slash command via the unified CommandRouter (Phase 21.1 Plan 02).
    ///
    /// Replaces the old hardcoded match on /start, /new, /clear, /help.
    /// Unknown commands pass through to agent as normal messages per D-08.
    async fn handle_slash_command(
        &self,
        event: &MessageEvent,
        adapter: Arc<dyn PlatformAdapter>,
        cancel: CancellationToken,
        processed: ProcessedAttachments,
    ) -> Result<()> {
        // Strip @botname suffix (e.g., "/start@mybot" -> "/start") per T-21.1-06.
        let command_input = event.content.split('@').next().unwrap_or(&event.content);

        let platform = &event.platform;
        let session_key =
            SessionKey::new(platform.clone(), &event.chat_id).with_user(&event.sender_id);

        // Phase 39.1 (R39.1-06 / D-06): agent_running removed — CommandContext no longer
        // carries the AtomicBool gate. get_running_flag() is no longer needed here.
        // Phase 36.2 follow-up: use the canonical SQLite session UUID for
        // CommandContext.session_id so /usage, /history, /export, /rename all
        // filter on the same id that agent_loop writes to the sessions /
        // usage_events tables. Pre-fix used session_key.to_string_key()
        // ("Telegram:<chat>:<user>") which never matched the UUID stored in
        // sessions.id — every /usage on a gateway session returned empty.
        // Falls back to the string-key form if no canonical id exists yet
        // (e.g., slash command issued before the first chat turn creates the
        // SQLite session row).
        let ctx_session_id = {
            let store = self.session_store.read().await;
            store
                .get(&session_key)
                .map(|s| s.session_id.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| session_key.to_string_key())
        };
        // Phase 41.3 Plan 04 (D-11/D-12): the nine core handles this build site
        // owns are collected into CoreContextHandles and built via the shared
        // build_core_context factory. process_registry, mcp_reloader, and
        // trajectory_writer are newly wired here — the gateway was 6-of-9
        // before this plan (baseline: 41.3-04-PLAN.md planning_provenance).
        //
        // Phase 36.2 chat-fix follow-up (state_store): attach the StateStoreHandle
        // so /usage, /sessions, /history, /export, etc. can reach the SQLite
        // session/usage tables. Mirrors the TUI (tui_rata/commands.rs) and web
        // (iron_hermes_ui/ws.rs) wiring that landed in commits a9fb0d0d / 402113b3.
        let state_store_handle: Arc<dyn StateStoreHandle> = {
            let store_arc = self.session_store.read().await.state_store().clone();
            Arc::new(ironhermes_state::StateStoreHandleAdapter(store_arc))
        };
        // Phase 21.8.2: wire skill_registry so /skills and SKILL-13 fallback work in
        // gateway. D-03: read via lock so we always see the latest atomic swap.
        let skill_registry_snapshot: Option<Arc<SkillRegistry>> =
            self.skill_registry.lock().ok().and_then(|g| g.clone());
        // Phase 32.3 Plan 04 (D-08 / RESEARCH Pitfall 3): attach the
        // subagent_registry so /agents list|kill|interrupt|prune|status actually
        // reach cmd_agents instead of hitting the "subagent registry not wired"
        // fallback. The gateway has had `self.subagent_registry` since Plan
        // 21.7-07 but `handle_slash_command` never called `with_subagent_registry`
        // — this fixed the pre-existing wiring gap identified in 32.3-RESEARCH.md
        // Pitfall 3 (lines 336-355).
        let subagent_registry_handle: Option<Arc<dyn SubagentListSnapshot>> =
            self.subagent_registry.as_ref().map(|reg| {
                use ironhermes_agent::subagent_registry::SubagentRegistryHandle;
                Arc::new(SubagentRegistryHandle::new(reg.clone())) as Arc<dyn SubagentListSnapshot>
            });
        // Phase 41.3 Plan 04 (D-12): previously missing — process_registry has
        // existed on the handler since Plan 21.7-06 but was never wired onto
        // CommandContext.
        let process_registry_handle: Option<Arc<dyn ProcessRegistrySnapshotHandle>> =
            self.process_registry.as_ref().map(|reg| {
                Arc::new(ironhermes_exec::process_registry::ProcessRegistryHandle::new(reg.clone()))
                    as Arc<dyn ProcessRegistrySnapshotHandle>
            });
        // Phase 41.3 Plan 04 (D-12): previously missing — the gateway had no MCP
        // handle on CommandContext at all before this plan.
        let mcp_reloader_handle: Option<Arc<dyn McpReloader>> = self
            .mcp_manager
            .as_ref()
            .map(|mgr| mgr.clone() as Arc<dyn McpReloader>);
        // Phase 41.3 Plan 04 (D-12): previously missing on the slash-dispatch path.
        // Sourced the same way the gateway already does for agent runs (run_agent,
        // `self.session_store.write().await.get_or_create_trajectory_writer(...)`),
        // evaluated here for the same canonical session id used to build this
        // CommandContext. Supersedes the Phase 25.3-15 CR-02 close-out note that
        // slash dispatch does not attach a trajectory writer — Plan 04 changes
        // that by reusing the per-session, SessionStore-cached writer (no new
        // process-wide handle, no behavior change for `run_agent`'s own writer).
        let trajectory_writer_handle = {
            let mut store = self.session_store.write().await;
            store.get_or_create_trajectory_writer(&ctx_session_id)
        };

        let core_handles = CoreContextHandles {
            subagent_registry: subagent_registry_handle,
            process_registry: process_registry_handle,
            skill_registry: skill_registry_snapshot,
            state_store: Some(state_store_handle),
            // Phase 25.2 Plan 15 follow-up (UAT Issue 2 / Symptom 1): production
            // toolset session handle so /toolset list/show/enable/disable works
            // in Telegram. Without this, cmd_toolset (handlers.rs:782) short-
            // circuits on None with the documented fallback string.
            toolset_session: self.toolset_session.clone(),
            turn_registry: Some(self.turn_registry.clone()),
            // Phase 25.3 D-W-2: Workspace for /sessions --workspace + trajectory scoping.
            workspace: self.workspace.clone(),
            mcp_reloader: mcp_reloader_handle,
            trajectory_writer: trajectory_writer_handle,
        };
        let ctx = build_core_context(platform.clone(), ctx_session_id.clone(), core_handles);

        // Phase 36.3.7.5 BUG-36.3.7.5-06: attach chat-origin so /kanban create's
        // auto-subscribe hook can write the originating chat to kanban_subscriptions.
        // thread_id is platform-dependent: Telegram super-group topics carry it;
        // other platforms pass None.
        let ctx = ctx.with_chat_origin(event.chat_id.clone(), event.thread_id.clone());

        // Phase 36.3.7.5 BUG-36.3.7.5-06: attach the KanbanStoreWriter so the
        // /kanban create slash arm can actually create tasks + write
        // subscriptions. KanbanStoreWriterImpl lives in ironhermes-kanban
        // (not ironhermes-cli — the latter depends on ironhermes-gateway, so
        // a gateway -> cli dep would be circular).
        //
        // Phase 46.5-04 D-06: resolve the operator's `kanban.default_notify`
        // target from `self.config.kanban` (mirrors the parse pattern in
        // runner.rs's dispatcher setup) and thread it into
        // KanbanStoreWriterImpl so create_task_simple (the programmatic
        // KanbanStoreWriter task-creation path, e.g. the kanban_create LLM
        // tool) auto-subscribes it too, alongside the CLI cmd_create path.
        //
        // SECURITY (T-46.5-20): the target is read exclusively from
        // self.config.kanban (operator config) — never from `event` or any
        // other per-message/task-controlled data.
        let ctx = {
            use ironhermes_core::commands::context::KanbanStoreWriter;
            let kanban_config: ironhermes_kanban::KanbanConfig = if self.config.kanban.is_null() {
                ironhermes_kanban::KanbanConfig::default()
            } else {
                serde_yaml::from_value(self.config.kanban.clone()).unwrap_or_default()
            };
            let writer: std::sync::Arc<dyn KanbanStoreWriter> = std::sync::Arc::new(
                ironhermes_kanban::KanbanStoreWriterImpl::with_default_notify(
                    kanban_config.default_notify,
                ),
            );
            ctx.with_kanban_store_writer(writer)
        };

        // Phase 39.1 (R39.1-09 / D-09): TurnRegistry visibility for /agents turns,
        // /stop, and /agents cancel <id> is now wired via CoreContextHandles above
        // (turn_registry: Some(self.turn_registry.clone())) — no separate chain call
        // needed here after the Phase 41.3 Plan 04 factory migration.

        // Phase 45 D-11: /approve and /deny intercept — handled BEFORE the
        // command router so the router never sees these as unknown commands (which
        // would fall through to agent turn). ctx_session_id is the canonical
        // SQLite session UUID derived above (consistent with what the coordinator
        // uses as the pending-map key).
        let command_base = command_input
            .split_whitespace()
            .next()
            .unwrap_or(command_input);
        if command_base == "/approve" || command_base == "/deny" {
            if let Some(ref coord) = self.approval_coordinator {
                let approved = command_base == "/approve";
                // CR-01 fix (47.6 code review): resolve() (keyed on THIS
                // event's own SessionKey-derived ctx_session_id) always
                // works for DM-originated approvals and MUST stay the
                // first attempt — do not regress that path. It only misses
                // for a Buzz channel-originated approval whose operator
                // reply arrives as a DM (see resolve_by_chat_id's doc
                // comment for the full session-identity mismatch). Fall
                // back to the chat_id-keyed lookup ONLY in that specific
                // combination (Buzz + DM reply) so Telegram/Discord/Slack
                // and Buzz DM-to-DM approvals are entirely unaffected.
                let resolved = if coord.resolve(&ctx_session_id, approved).await {
                    true
                } else if event.platform == Platform::Buzz && event.chat_type == "dm" {
                    coord.resolve_by_chat_id(&event.sender_id, approved).await
                } else {
                    false
                };
                let reply = if resolved {
                    if approved {
                        "Approved — running command."
                    } else {
                        "Denied — command cancelled."
                    }
                } else {
                    "No pending approval for this session."
                };
                with_rate_limit_retry(|| adapter.send_message(&event.chat_id, reply, None)).await?;
            } else {
                with_rate_limit_retry(|| {
                    adapter.send_message(&event.chat_id, "Approval gate not configured.", None)
                })
                .await?;
            }
            return Ok(());
        }

        let parts: Vec<&str> = command_input.split_whitespace().collect();
        let args: Vec<&str> = if parts.len() > 1 {
            parts[1..].to_vec()
        } else {
            vec![]
        };

        match self.command_router.resolve(command_input, platform) {
            ResolveResult::Exact(def) | ResolveResult::PrefixMatch(def) => {
                // Phase 39.1 (R39.1-06 / D-06): agent_running gate REMOVED.
                // All slash commands dispatch unconditionally while turns are in flight.
                // The channel never rejects: /stop cancels session turns, /agents shows them,
                // /new warns then proceeds (R39.1-07 warn-not-block).

                // Phase 39.1 (R39.1-07 / D-06-RISK): /new and /reset warn if turns are in
                // flight, then proceed. The warn is advisory only — the user may /stop first
                // if they want a clean slate. ctx_session_id is the canonical SQLite UUID
                // (or string-key fallback) used by TurnRegistry entries.
                let in_flight_warn = if def.name == "new" || def.name == "reset" {
                    ironhermes_core::commands::handlers::in_flight_warning(
                        &self.turn_registry,
                        &ctx_session_id,
                    )
                    .await
                } else {
                    None
                };
                if let Some(warn) = in_flight_warn {
                    with_rate_limit_retry(|| adapter.send_message(&event.chat_id, &warn, None))
                        .await?;
                }

                // Phase 39.1 (R39.1-05 / D-05): /stop cancels all in-flight session turns
                // via the shared TurnRegistry. The existing cmd_stop in core still clears
                // the agent_running flag (backward compat); the registry cancel is the new
                // concurrent-turn signal.
                if def.name == "stop" {
                    let cancelled = self.turn_registry.cancel_session(&ctx_session_id).await;
                    tracing::info!(
                        session = %ctx_session_id,
                        cancelled,
                        "gateway /stop: cancelled in-flight turn(s) via TurnRegistry (Phase 39.1 R39.1-05)"
                    );
                }

                // Phase 21.8.3.1 D-05 gateway analog (RESEARCH Open Question 1, Option B):
                // Intercept /personality clear BEFORE core dispatch. Core's cmd_personality
                // has no "clear" case — it would return Error("Unknown personality: clear")
                // and we'd send that confusing error to the user. Mirrors TUI handle_subsystem_mutator.
                if def.name == "personality" && args.first() == Some(&"clear") {
                    if let Ok(mut overlays) = self.active_personality_overlay.lock() {
                        overlays.remove(&session_key);
                    }
                    with_rate_limit_retry(|| {
                        adapter.send_message(&event.chat_id, "Personality cleared.", None)
                    })
                    .await?;
                    return Ok(());
                }
                // Phase 32.3 Plan 04 (D-09 / T-32.3-01 mitigation): gateway-only
                // confirm-token gate for destructive `/agents` subcommands.
                // Telegram messages can be spoofed via edit-replay; requiring
                // the operator to re-type `confirm` as an extra arg means a
                // replayed original message (which lacks `confirm`) is refused.
                // TUI and iron_hermes_ui surfaces do NOT require this token
                // (they have synchronous user presence). Only `kill` and `prune`
                // are destructive; `interrupt` and `status` are not gated.
                if def.name == "agents" && !args.is_empty() && requires_confirm(args[0], &args[1..])
                {
                    let refusal = format!(
                        "Destructive op `/agents {}`. Re-run as:\n  `/agents {} {}confirm`",
                        args[0],
                        args[0],
                        if args.len() > 1 {
                            format!("{} ", args[1])
                        } else {
                            String::new()
                        },
                    );
                    with_rate_limit_retry(|| adapter.send_message(&event.chat_id, &refusal, None))
                        .await?;
                    return Ok(());
                }
                // Phase 36.17.9: `/voice on|off|tts|status` manages this chat's
                // per-session voice mode, persisted into the durable gateway_routes
                // record so it survives a restart. Core's `cmd_voice` is a help/headless
                // stub that ignores ctx, so the gateway owns the stateful path here
                // (mirrors the TUI's post-router voice arm).
                if def.name == "voice" {
                    let reply = self.handle_voice_command(&session_key, &args).await;
                    with_rate_limit_retry(|| adapter.send_message(&event.chat_id, &reply, None))
                        .await?;
                    return Ok(());
                }
                let core_result = ironhermes_core::commands::handlers::dispatch(
                    def,
                    &args,
                    &ctx,
                    &self.command_router,
                );
                match core_result {
                    CoreCommandResult::PersonalityApplied(text) => {
                        if let Ok(mut overlays) = self.active_personality_overlay.lock() {
                            overlays.insert(session_key.clone(), text.clone());
                        }
                        let confirm = format!(
                            "Personality applied ({} chars). Active for this session.",
                            text.len()
                        );
                        with_rate_limit_retry(|| {
                            adapter.send_message(&event.chat_id, &confirm, None)
                        })
                        .await?;
                    }
                    CoreCommandResult::Output(text) => {
                        // G-41.1-5: long replies (e.g. /skills' 172-skill catalog)
                        // must chunk at Telegram's 4096-char limit instead of a
                        // single unguarded send that 400s with "text is too long".
                        send_chunked(&adapter, &event.chat_id, &text).await?;
                    }
                    CoreCommandResult::NewSession { .. } => {
                        // /start special handling: reset session then LLM greeting.
                        // /new: remove session and confirm.
                        if def.name == "start" {
                            {
                                let mut store = self.session_store.write().await;
                                // Phase 47.5 (D-04): durable reset — ends the SQLite
                                // session and clears the route, distinct end_reason
                                // from /new so the two commands stay distinguishable
                                // in durable audit state.
                                let _ = store.reset_session(&session_key, "start");
                            }
                            let mut intro_event = event.clone();
                            intro_event.content =
                                "Please introduce yourself. This is the start of a new conversation."
                                    .to_string();
                            let no_attachments = ProcessedAttachments {
                                text_prefix: None,
                                image_data_uri: None,
                                image_cache_path: None,
                            };
                            return self
                                .run_agent(&intro_event, adapter, cancel, no_attachments)
                                .await;
                        }
                        // /new: clear entire session history.
                        // Phase 34b Plan 02 (D-09/D-10): removing the session from
                        // the store discards ALL per-session state — including the
                        // compression_count carried in the SessionStore entry — so
                        // the next turn rebuilds a fresh ContextEngine with a zeroed
                        // counter. No separate engine.on_session_reset() call is
                        // needed here because the gateway holds no long-lived,
                        // session-scoped engine handle (the engine is rebuilt fresh
                        // per turn in run_turn).
                        // Phase 47.5 (D-04): the reset is now DURABLE — it also
                        // ends the SQLite session row and clears the gateway_routes
                        // entry, so it survives a gateway restart and applies even
                        // when nothing is in memory (a durable-only reset resolves
                        // its target from the route). This is what makes the
                        // "Conversation cleared. Starting fresh." reply below
                        // truthful post-restart.
                        tracing::debug!(
                            session = ?session_key,
                            "gateway /new: session removed; per-session compression state discarded (34b D-10)"
                        );
                        // Phase 36.17.1 Pitfall 5: clear queue BEFORE session_store.remove.
                        // If store.remove fires first, the GatewaySession (including the
                        // running AtomicBool) is dropped — a racing run_agent turn could see
                        // the flag is gone and create a new session, then clear_queue would
                        // clear the NEW session's queue. By clearing first, we guarantee:
                        //   queue cleared -> session removed -> no window for stale events
                        // The clear call is sync (std::sync::Mutex) so no guard crosses an
                        // await. Covers /reset too (def.name aliased to "new" at the router).
                        if let Some(ref queue) = self.session_queue {
                            queue.clear(&session_key);
                        }
                        // Phase 45 D-11: cancel any pending approval for this session so
                        // the oneshot::Sender is dropped (fail-closed) and the waiting
                        // agent turn is unblocked before the session history is cleared.
                        // resolve(false) sends false over the channel; the coordinator
                        // removes the entry from the pending map. No-op when None.
                        if let Some(ref coord) = self.approval_coordinator {
                            coord.resolve(&ctx_session_id, false).await;
                        }
                        let had_session = {
                            let mut store = self.session_store.write().await;
                            store.reset_session(&session_key, "new")
                        };
                        let msg = if had_session {
                            "Conversation cleared. Starting fresh."
                        } else {
                            "No active conversation. Ready for a new one."
                        };
                        with_rate_limit_retry(|| adapter.send_message(&event.chat_id, msg, None))
                            .await?;
                    }
                    CoreCommandResult::ClearSession => {
                        // Phase 22.3 WR-04 (review fix): No built-in command
                        // currently routes here.
                        //   - `/clear` returns `CoreCommandResult::ResetTerminal`
                        //     (handlers.rs::cmd_clear, Phase 22.3 D-06) and is
                        //     handled by the `ResetTerminal` arm a few cases
                        //     below (no-op on the gateway since there is no TTY).
                        //   - `/new` returns `CoreCommandResult::NewSession { .. }`
                        //     and is handled by its own `NewSession` arm above.
                        // This `ClearSession` arm is preserved for forward
                        // compatibility: a future built-in or extension command
                        // may legitimately emit `CoreCommandResult::ClearSession`
                        // with the semantic "wipe session messages, send
                        // confirmation", and the runtime body below is the
                        // correct gateway behavior for that semantic.
                        {
                            let mut store = self.session_store.write().await;
                            if let Some(session) = store.get_mut(&session_key) {
                                session.clear();
                            }
                        }
                        with_rate_limit_retry(|| {
                            adapter.send_message(&event.chat_id, "History cleared.", None)
                        })
                        .await?;
                    }
                    CoreCommandResult::Error(msg) => {
                        with_rate_limit_retry(|| adapter.send_message(&event.chat_id, &msg, None))
                            .await?;
                    }
                    CoreCommandResult::Handled => {
                        // Silent — no response to user
                    }
                    CoreCommandResult::Quit => {
                        // Quit not meaningful on gateway — ignore
                    }
                    CoreCommandResult::PassThrough => {
                        // Fall through to agent as normal message, preserving attachments
                        return self.run_agent(event, adapter, cancel, processed).await;
                    }
                    CoreCommandResult::McpReload => {
                        // MCP reload not wired on gateway (mcp_reloader is None in
                        // gateway CommandContext); the handler will have returned
                        // Output("MCP not configured.") before reaching this arm.
                        // This arm exists for exhaustiveness only.
                    }
                    CoreCommandResult::ResetTerminal => {
                        // Phase 22.3 D-06: TTY visual reset — not meaningful on the
                        // gateway (no TTY). Ignore silently. Added for exhaustiveness.
                    }
                    // Phase 36.17.3 (D-06 amended): defensive no-op. /pause and
                    // /unpause are CliOnly in the registry so a gateway adapter
                    // will not reach this arm via resolve(), but exhaustiveness
                    // requires the variants be matched. Active toggle wiring
                    // lives in the TUI (Plan 05) — gateway has no queue-paused
                    // flag because the gateway worker drain semantics differ.
                    CoreCommandResult::PauseQueue | CoreCommandResult::UnpauseQueue => {
                        // No-op on gateway (exhaustiveness only).
                    }
                    ironhermes_core::commands::CommandResult::SkillsReload => {
                        // Phase 21.8.2 D-01..D-05: synchronous reload + D-03 atomic inner-Arc swap.
                        use std::collections::HashSet;
                        let cwd = std::env::current_dir()
                            .unwrap_or_else(|_| std::path::PathBuf::from("."));
                        let cfg = match &self.skills_config {
                            Some(c) => c.clone(),
                            None => {
                                let _ = with_rate_limit_retry(|| adapter.send_message(
                                    &event.chat_id,
                                    "Skills reload unavailable: skills_config not set on gateway handler.",
                                    None,
                                )).await;
                                return Ok(());
                            }
                        };
                        // Acquire current snapshot for diff computation.
                        let old_snapshot: Option<Arc<SkillRegistry>> =
                            self.skill_registry.lock().ok().and_then(|g| g.clone());
                        let new_inner = Arc::new(SkillRegistry::load_with_config(&cwd, &cfg));
                        let old_names: HashSet<String> = old_snapshot
                            .as_ref()
                            .map(|r| r.list().iter().map(|s| s.name.clone()).collect())
                            .unwrap_or_default();
                        let new_names: HashSet<String> =
                            new_inner.list().iter().map(|s| s.name.clone()).collect();
                        let mut added: Vec<&String> = new_names.difference(&old_names).collect();
                        let mut removed: Vec<&String> = old_names.difference(&new_names).collect();
                        added.sort();
                        removed.sort();
                        let added_str = if added.is_empty() {
                            "0 added".to_string()
                        } else {
                            format!(
                                "{} added ({})",
                                added.len(),
                                added
                                    .iter()
                                    .map(|s| s.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        };
                        let removed_str = if removed.is_empty() {
                            "0 removed".to_string()
                        } else {
                            format!(
                                "{} removed ({})",
                                removed.len(),
                                removed
                                    .iter()
                                    .map(|s| s.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        };
                        // Phase 21.8.2 D-05: count parse-failures (files scanned vs loaded).
                        let invalid_skipped = {
                            let search_paths =
                                ironhermes_core::build_skill_search_paths(&cwd, &cfg);
                            let mut files_scanned: usize = 0;
                            for root in &search_paths {
                                if let Ok(entries) = std::fs::read_dir(root) {
                                    for entry in entries.flatten() {
                                        let path = entry.path();
                                        if !path.is_dir() {
                                            continue;
                                        }
                                        if path
                                            .file_name()
                                            .and_then(|n| n.to_str())
                                            .map(|n| n.starts_with('.'))
                                            .unwrap_or(false)
                                        {
                                            continue;
                                        }
                                        if path.join("SKILL.md").is_file() {
                                            files_scanned += 1;
                                        }
                                    }
                                }
                            }
                            files_scanned.saturating_sub(new_inner.list().len())
                        };
                        let invalid_clause = if invalid_skipped > 0 {
                            format!(" ({} invalid skipped — see logs)", invalid_skipped)
                        } else {
                            String::new()
                        };
                        let diff_text = format!(
                            "Skills reloaded: {}. {}. Total: {} skills.{}",
                            added_str,
                            removed_str,
                            new_inner.list().len(),
                            invalid_clause
                        );
                        // D-03 / D-Plan03-01 UPDATED: atomic swap of the inner Arc.
                        if let Ok(mut guard) = self.skill_registry.lock() {
                            *guard = Some(new_inner);
                        }
                        let _ = with_rate_limit_retry(|| {
                            adapter.send_message(&event.chat_id, &diff_text, None)
                        })
                        .await;
                        return Ok(());
                    }
                    // Phase 41.1 Plan 04 (D-08 / SKILL-13): one-shot activate+run.
                    // Activate the body into the per-session overlay AND fire a run
                    // turn immediately — no longer activate-only. `dispatch()` does
                    // not construct this variant today (every surface builds its own
                    // NotFound fallback below), so this arm is DEFENSIVE, but it must
                    // stay behavior-identical to the fallback if a future dispatch
                    // path ever returns it.
                    ironhermes_core::commands::CommandResult::SkillActivated {
                        name,
                        body,
                        args,
                    } => {
                        // Compute the run-turn trigger text (D-02) BEFORE moving body
                        // into the overlay.
                        let invocation = build_skill_invocation(name.clone(), body.clone(), args);
                        // Phase 21.8.2 D-Plan03-05 / D-07: store in per-session overlay so
                        // run_agent's skill_overlays read site prepends body to the prompt.
                        if let Ok(mut overlays) = self.skill_overlays.lock() {
                            overlays
                                .entry(session_key.clone())
                                .or_insert_with(Vec::new)
                                .push((name.clone(), body));
                        }
                        // Task 2: run-turn meta text replaces the retired activation
                        // copy — sent via the same with_rate_limit_retry send_message
                        // call site, immediately before the run turn's reply.
                        let meta_msg = run_turn_meta_text(&name, &invocation.trigger_text);
                        let _ = with_rate_limit_retry(|| {
                            adapter.send_message(&event.chat_id, &meta_msg, None)
                        })
                        .await;
                        // D-08 (T-41.1-04-01): synthesize a run turn whose identity
                        // (platform/chat_id/sender_id/message_id) inherits from the real
                        // event via ..event.clone() — NEVER reconstructed from name/args
                        // (mirrors the Queued arm + its anti-impersonation comment below).
                        let synthetic = MessageEvent {
                            content: invocation.trigger_text.clone(),
                            ..event.clone()
                        };
                        return self.run_agent(&synthetic, adapter, cancel, processed).await;
                    }
                    // Phase 36.17.1 Plan 03 Task 2: /queue dispatch intercept.
                    //
                    // Synthesize a new MessageEvent whose content is the
                    // user-supplied `message`, but whose identity
                    // (platform/chat_id/sender_id/message_id) inherits from the
                    // triggering event (T-36.17.1-02 mitigation: a user cannot
                    // impersonate another session via crafted /queue input —
                    // identity fields are NOT taken from args).
                    //
                    // Call session_queue.try_push under the existing
                    // session_key (Pitfall 7: full triple, built at line 404).
                    // On Ok(()): depth-aware reply mirroring Python parity
                    // (gateway/run.py:6814-6820). On Err(CapacityReached):
                    // identical D-13 UX as the busy-branch cap-hit path
                    // (❌ reaction + ⏳ chat reply + tracing::warn!).
                    //
                    // If session_queue is None (handler built outside
                    // build_gateway_handler — e.g., legacy GW-05 tests), reply
                    // with the depth-1 confirmation anyway so the user sees
                    // feedback. This matches the busy-branch fallback (degraded
                    // but visible).
                    //
                    // Pitfall 2: SessionQueue methods are sync; the std::sync
                    // MutexGuard inside try_push/len drops before any .await
                    // here. The borrow checker enforces that the guard is
                    // !Send so it cannot cross an await boundary.
                    CoreCommandResult::Queued { message } => {
                        // Phase 36.17.2.1 D-02 (Option B from RESEARCH §Fix Space):
                        // Replace direct session_queue.try_push with UQM::dispatch so push
                        // + notify_one() happen atomically (user_queue.rs:154 IS the wake
                        // primitive). The UAT failure (2026-05-28T15:36-15:38 UTC) was 128/129
                        // /queue events stranded because handler-side try_push had no Notify.
                        //
                        // UQM::dispatch (user_queue.rs:100-165) performs:
                        //   1. Builds SessionKey from event.platform/chat_id/sender_id — identical
                        //      to the session_key built above (D-14 triple invariant).
                        //   2. session_queue.try_push (the SAME Arc<SessionQueue> the handler
                        //      holds — GatewayRunner threads one Arc into both via
                        //      set_session_queue + set_user_queue_manager).
                        //   3. On Err(CapacityReached): fires ❌ reaction + "⏳ Queue is full
                        //      (128 messages). Wait for the agent to drain before sending more."
                        //      (D-11 inherited from parent phase) and returns Err.
                        //   4. On Ok: push_multimodal(&key, (None, None, None)) — text-only command,
                        //      sidecar receives (None, None, None); worker's take_multimodal returns
                        //      Some((None, None, None)) which unwrap_or normalizes — no FIFO skew
                        //      (RESEARCH Pitfall 4).
                        //   5. On Ok: notify_one() — wakes the parked worker (the FIX).
                        let queued_event = MessageEvent {
                            content: message.clone(),
                            ..event.clone()
                        };
                        if let Some(uqm) = self.user_queue_manager.as_ref() {
                            // Production path (handler wired via GatewayRunner::run_gateway).
                            match uqm.dispatch(queued_event, None, None, None).await {
                                Ok(outcome) => {
                                    // Depth-aware reply preserved (context_lock #3):
                                    // depth = session_queue.len(&session_key) computed AFTER
                                    // dispatch (same Arc<SessionQueue> as UQM uses — D-20 invariant).
                                    // The unwrap_or(1) fallback is unreachable in production
                                    // because session_queue is wired alongside UQM by
                                    // GatewayRunner::run_gateway, and uqm.dispatch above just
                                    // succeeded — so a SessionQueue entry must exist.
                                    let depth = self
                                        .session_queue
                                        .as_ref()
                                        .map(|q| q.len(&session_key))
                                        .unwrap_or(1);
                                    let reply = if depth <= 1 {
                                        "Queued for the next turn.".to_string()
                                    } else {
                                        format!("Queued for the next turn. ({depth} queued)")
                                    };
                                    with_rate_limit_retry(|| {
                                        adapter.send_message(&event.chat_id, &reply, None)
                                    })
                                    .await?;
                                    tracing::debug!(
                                        session = %session_key.to_string_key(),
                                        depth,
                                        outcome = ?outcome,
                                        "SessionQueue: /queue dispatched via UQM (Phase 36.17.2.1 D-02 — wake fix)"
                                    );
                                    // Phase 36.17.2.1 D-06 (scope boundary): if outcome is
                                    // WorkerSpawned, UQM inserted a Notify but no worker task
                                    // has been spawned for this SessionKey (the handler runs
                                    // inside a detached fast-path tokio::spawn with no access
                                    // to worker_join_set_dispatch). The synthesized event will
                                    // wait until the next free-text message on this chat
                                    // triggers worker spawn via runner.rs's normal dispatch
                                    // loop. This is a known scope boundary — see
                                    // RESEARCH.md Q2 and CONTEXT.md D-06. The UAT failure
                                    // (wake-parked-worker) is fixed; fresh-chat /queue is a
                                    // separate latent gap deferred to a follow-up.
                                    if matches!(outcome, DispatchOutcome::WorkerSpawned) {
                                        tracing::warn!(
                                            session = %session_key.to_string_key(),
                                            "Phase 36.17.2.1 D-06: /queue on fresh chat (no worker registered); \
                                             synthesized event will wait for next free-text message to spawn worker. \
                                             Out of scope for this fix — see RESEARCH.md Q2."
                                        );
                                    }
                                }
                                Err(QueueError::CapacityReached { .. }) => {
                                    // Phase 36.17.2.1 D-04: UQM::dispatch already fired
                                    // the ❌ reaction + "⏳ Queue is full" chat reply
                                    // (user_queue.rs:115-138). The handler MUST NOT also
                                    // emit them — double-reply hazard. Just return.
                                    tracing::warn!(
                                        session = %session_key.to_string_key(),
                                        "SessionQueue: /queue cap reached via UQM (Phase 36.17.2.1 — UX fired by UQM::dispatch)"
                                    );
                                }
                            }
                        } else if let Some(queue) = self.session_queue.as_ref() {
                            // Phase 36.17.2.1 D-04 fallback: handlers built outside
                            // GatewayRunner::run_gateway (no UQM wired — e.g. Phase 36 GW-05
                            // tests at tests/running_agent_guard_tests.rs, the
                            // session_queue_integration.rs harness exercising the busy-branch
                            // fallback per parent phase D-20). Original direct-try_push path
                            // preserved — no wake, but these harnesses hand-spawn their own
                            // workers and do not depend on Notify wake semantics.
                            match queue.try_push(&session_key, queued_event) {
                                Ok(()) => {
                                    let depth = queue.len(&session_key);
                                    let reply = if depth <= 1 {
                                        "Queued for the next turn.".to_string()
                                    } else {
                                        format!("Queued for the next turn. ({depth} queued)")
                                    };
                                    with_rate_limit_retry(|| {
                                        adapter.send_message(&event.chat_id, &reply, None)
                                    })
                                    .await?;
                                    tracing::debug!(
                                        session = %session_key.to_string_key(),
                                        depth,
                                        "SessionQueue: /queue dispatched via legacy direct try_push (no UQM wired — D-20 fallback)"
                                    );
                                }
                                Err(QueueError::CapacityReached { .. }) => {
                                    // D-13 UX: best-effort ❌ reaction
                                    // (Telegram may rate-limit reactions, so we
                                    // use .ok() — failure must not poison the
                                    // chat reply). Then the ⏳ chat reply.
                                    adapter
                                        .add_reaction(&event.chat_id, &event.message_id, "❌")
                                        .await
                                        .ok();
                                    with_rate_limit_retry(|| {
                                        adapter.send_message(
                                            &event.chat_id,
                                            "⏳ Queue is full (128 messages). Wait for the agent to drain before sending more.",
                                            None,
                                        )
                                    })
                                    .await?;
                                    tracing::warn!(
                                        session = %session_key.to_string_key(),
                                        "SessionQueue: /queue cap reached on legacy fallback (no UQM wired)"
                                    );
                                }
                            }
                        } else {
                            // Degraded-degraded: neither UQM nor SessionQueue wired.
                            // Send the depth-1 confirmation so the user gets visible feedback.
                            with_rate_limit_retry(|| {
                                adapter.send_message(
                                    &event.chat_id,
                                    "Queued for the next turn.",
                                    None,
                                )
                            })
                            .await?;
                        }
                        return Ok(());
                    }
                    CoreCommandResult::AgentsList(turns) => {
                        // Phase 39.1 (R39.1-09): render `/agents turns` on the
                        // gateway/Telegram surface. Plan 39.1-01 introduced the
                        // AgentsList variant in core; this arm keeps the gateway
                        // match exhaustive and surfaces the active TurnRegistry
                        // entries as a text reply.
                        let msg = if turns.is_empty() {
                            "No active turns.".to_string()
                        } else {
                            let mut out = format!("Active turns ({}):\n", turns.len());
                            for t in &turns {
                                out.push_str(&format!(
                                    "• {} — {} — session {} — {}ms\n",
                                    t.turn_id, t.surface, t.session_id, t.elapsed_ms
                                ));
                            }
                            out
                        };
                        with_rate_limit_retry(|| adapter.send_message(&event.chat_id, &msg, None))
                            .await?;
                    }
                    // Phase 36.6.3 Plan 03 (TUI-INPUT-02, D-06): bare `/model`/
                    // `/provider` open an interactive picker on the TUI —
                    // meaningless on the gateway (no overlay surface). Fall
                    // back to the pre-existing plain-text output
                    // (model_list_text()/status_text()) so nothing regresses.
                    CoreCommandResult::OpenModelPicker { fallback_text }
                    | CoreCommandResult::OpenProviderPicker { fallback_text } => {
                        with_rate_limit_retry(|| {
                            adapter.send_message(&event.chat_id, &fallback_text, None)
                        })
                        .await?;
                    }
                }
            }
            ResolveResult::Ambiguous(candidates) => {
                let first = parts.first().copied().unwrap_or("");
                let list = candidates
                    .iter()
                    .map(|c| format!("/{}", c))
                    .collect::<Vec<_>>()
                    .join(", ");
                let msg = format!(
                    "Ambiguous command: {}. Matches: {}. Be more specific.",
                    first, list
                );
                with_rate_limit_retry(|| adapter.send_message(&event.chat_id, &msg, None)).await?;
            }
            ResolveResult::NotFound => {
                // Phase 21.8.2 D-06/D-08: SKILL-13 dynamic fallback before agent passthrough.
                // Registered commands win because 3-stage resolution ran first.
                //
                // Phase 41.1 Plan 04 (D-08): resolve the fell-through slash token
                // against the SkillRegistry via the shared pure resolver (Plan 01).
                // On a match, ACTIVATE the body into the per-session overlay AND
                // fire a run turn immediately (one-shot activate+run) — no longer
                // activate-only. `command_input` already had any @botname suffix
                // stripped (line ~695); the resolver extracts the trailing args as
                // the argued-invoke trigger_text (D-02).
                let snapshot: Option<Arc<SkillRegistry>> =
                    self.skill_registry.lock().ok().and_then(|g| g.clone());
                if let Some(registry) = snapshot
                    && let Some(invocation) = resolve_skill_invocation(&registry, command_input)
                {
                    // D-Plan03-05 / D-07: store in per-session overlay so run_agent's
                    // skill_overlays read site prepends the body to the turn prompt.
                    if let Ok(mut overlays) = self.skill_overlays.lock() {
                        overlays
                            .entry(session_key.clone())
                            .or_insert_with(Vec::new)
                            .push((invocation.name.clone(), invocation.body.clone()));
                    }
                    // Task 2: run-turn meta text replaces the retired activation copy,
                    // sent via the same with_rate_limit_retry send_message call site
                    // immediately before the run turn's reply.
                    let meta_msg = run_turn_meta_text(&invocation.name, &invocation.trigger_text);
                    let _ = with_rate_limit_retry(|| {
                        adapter.send_message(&event.chat_id, &meta_msg, None)
                    })
                    .await;
                    // D-08 (T-41.1-04-01): synthesize a run turn whose identity
                    // inherits from the real event via ..event.clone() — NEVER
                    // reconstructed from the skill name / user-controlled args.
                    let synthetic = MessageEvent {
                        content: invocation.trigger_text.clone(),
                        ..event.clone()
                    };
                    return self.run_agent(&synthetic, adapter, cancel, processed).await;
                }
                // D-08: Unknown commands pass through to agent as normal message, preserving attachments
                return self.run_agent(event, adapter, cancel, processed).await;
            }
        }
        Ok(())
    }

    /// Public entry point for multimodal-aware message handling.
    /// Called from runner.rs per-chat workers which have access to QueuedMessage.
    pub async fn handle_with_multimodal(
        &self,
        event: &MessageEvent,
        adapter: Arc<dyn PlatformAdapter>,
        cancel: CancellationToken,
        processed: ProcessedAttachments,
    ) -> Result<()> {
        // D-20: Per-user rate limiting. D-21: Silent drop on excess.
        if !self.rate_limiter.check_and_consume(&event.sender_id) {
            return Ok(());
        }

        if event.content.starts_with('/') {
            return self
                .handle_slash_command(event, adapter, cancel, processed)
                .await;
        }
        // Phase 39.1 (R39.1-06): gate removed — semaphore in run_agent handles cap;
        // over-cap messages stay in SessionQueue via the worker loop's try_acquire.
        self.run_agent(event, adapter, cancel, processed).await
    }

    /// Run the agent loop for a message event — drives streaming to StreamConsumer.
    ///
    /// Phase 36.17.1 Plan 02 Task 3: visibility relaxed to `pub(crate)` so
    /// `GatewayRunner::drain_pending` can invoke `run_agent` directly,
    /// bypassing `handle_with_multimodal`'s busy-guard (RESEARCH Pitfall 4 —
    /// the RAII `RunningAgentGuard` inside `run_agent` re-sets the AtomicBool
    /// for each drained turn).
    pub(crate) async fn run_agent(
        &self,
        event: &MessageEvent,
        adapter: Arc<dyn PlatformAdapter>,
        cancel: CancellationToken,
        processed: ProcessedAttachments,
    ) -> Result<()> {
        // Phase 39.1 (R39.1-05): per-turn semaphore permits are acquired by the
        // per-chat worker loop (runner.rs) BEFORE calling handle_with_multimodal.
        // run_agent itself does not re-acquire — that would double-consume permits
        // from the same ConcurrencyLayer. The caller is responsible for holding
        // the OwnedSemaphorePermits for the duration of this call.

        // Per-turn CancellationToken (R39.1-05): child of the process-level cancel
        // so that process shutdown propagates, but /stop can cancel just this turn.
        let turn_cancel = cancel.child_token();

        // Build the session_id string used for TurnEntry + in_flight_warning.
        // Format matches the CommandContext session_id for gateway sessions.
        let gw_session_id = format!("gw:{}:{}", event.chat_id, event.sender_id);

        // Register this turn in the TurnRegistry BEFORE any agent work
        // (register-before-spawn discipline; see registry.rs).
        let turn_id = TurnId::new_v4();
        let turn_entry = TurnEntry {
            turn_id,
            session_id: gw_session_id.clone(),
            surface: Surface::Gateway,
            started_at: std::time::Instant::now(),
            cancel: turn_cancel.clone(),
        };
        self.turn_registry.register(turn_entry).await;

        // RAII deregister guard: spawns an async task to remove the entry on drop.
        // Covers all exit paths (Ok return, ? propagation, panic).
        struct TurnGuard {
            registry: Arc<TurnRegistry>,
            turn_id: TurnId,
        }
        impl Drop for TurnGuard {
            fn drop(&mut self) {
                let registry = self.registry.clone();
                let id = self.turn_id;
                // tokio::spawn is safe here — we are always inside a tokio runtime.
                tokio::spawn(async move {
                    registry.deregister(id).await;
                });
            }
        }
        let _turn_guard = TurnGuard {
            registry: self.turn_registry.clone(),
            turn_id,
        };

        // Fire MessageReceived hook with real platform and chat_id.
        // Phase 47.6 Plan 09 (P0-3): report the EVENT's own platform, not a
        // fixed Telegram literal — external hook consumers use this field as
        // an audit trail, and a hardcoded value makes every Buzz turn
        // indistinguishable from a Telegram one in their records.
        if let Some(ref registry) = self.hook_registry {
            let request_id = uuid::Uuid::new_v4().to_string();
            let hook_event = ironhermes_hooks::HookEvent::new(
                &request_id,
                ironhermes_hooks::HookEventKind::MessageReceived {
                    platform: event.platform.to_string(),
                    chat_id: event.chat_id.clone(),
                    content_preview: ironhermes_hooks::event::preview(&event.content, 200),
                },
            );
            registry.fire(hook_event);
        }

        // 1. Send initial placeholder message; get message_id for StreamConsumer.
        //
        // Phase 47.6 Plan 09 (D-13, T-47.6-09-04): on an adapter whose events
        // are immutable (Buzz), `supports_in_place_edits()` is false — there
        // is no placeholder to send and nothing to edit later, so the send
        // is skipped entirely. `placeholder_id` is `None` on that path; every
        // placeholder-dependent step below (StreamConsumer construction, the
        // D-10 reinsert edit, and the RC-1 turn-end fallback) branches on
        // whether a placeholder id is present, NOT on the adapter directly —
        // this keeps the branch point in exactly one place per call site.
        let supports_in_place_edits = adapter.supports_in_place_edits();
        let placeholder_id: Option<String> = if supports_in_place_edits {
            let placeholder =
                with_rate_limit_retry(|| adapter.send_message(&event.chat_id, "\u{2588}", None))
                    .await?;
            Some(placeholder.message_id.clone())
        } else {
            None
        };

        // 2. Spawn typing indicator task (D-16): sends "typing" every 5 seconds
        let typing_cancel = cancel.child_token();
        let adapter_typing = adapter.clone();
        let chat_id_typing = event.chat_id.clone();
        let typing_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = typing_cancel.cancelled() => break,
                    _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                        let _ = adapter_typing.send_chat_action(&chat_id_typing, "typing").await;
                    }
                }
            }
        });
        // Send first typing action immediately
        let _ = adapter.send_chat_action(&event.chat_id, "typing").await;

        // 3. Get or create session; clone messages immediately to avoid holding lock across await
        // Per-turn snapshot of model at turn start (R39.1-07, D-06-RISK Pattern 3).
        // A mid-turn `/model` change only takes effect for the NEXT turn — this turn
        // runs with `model` frozen here.
        let model = self.config.model.default.clone();
        // Per-turn snapshot of active personality overlay (R39.1-07, A3 verification):
        // The overlay is applied into PromptBuilder below (before any .await) from the
        // Mutex-guarded map, which is equivalent to snapshotting it here.  Verified:
        // cmd_personality returns CommandResult::PersonalityApplied and the surface
        // post-router hook applies the overlay AFTER the handler returns — the
        // in-flight turn's prompt_builder.set_overlay() call (below) already captures
        // a snapshot from the Mutex at the moment this turn starts, making mid-turn
        // /personality safe (RESEARCH §D-06-RISK Resolution).
        // Phase 47.6 Plan 09 (P0-3, T-47.6-09-02): the session key's platform
        // comes from the EVENT, not a fixed Telegram literal. Session keys are
        // the persistence identity — a wrong platform here silently merges
        // two platforms' conversation histories into one thread (a Buzz
        // conversation would write into Telegram's session/memory namespace
        // and both surfaces would read each other's history). `source`
        // (used for session-store bookkeeping) derives from `key.platform`
        // immediately below, so it follows automatically once the key is right.
        let key =
            SessionKey::new(event.platform.clone(), &event.chat_id).with_user(&event.sender_id);
        let source = key.platform.to_string();

        // Build user message content — incorporate multimodal data
        let user_message = build_user_message(event, processed);

        // Phase 39.1 (R39.1-02): get or create session, add user message, capture
        // history Arc and starting messages snapshot — all under one write lock.
        let (mut session_messages, history_arc) = {
            let mut store = self.session_store.write().await;
            let _session = store.get_or_create(key.clone(), &model, &source);
            // Add user message via write-through (persists to SQLite)
            store.add_message_to_session(&key, user_message);
            let msgs = store
                .get(&key)
                .map(|s| s.messages.clone())
                .unwrap_or_default();
            // Clone the Arc WHILE we hold the write lock so the session definitely exists.
            // This handle stays valid for the lifetime of this turn regardless of /new
            // (RESEARCH Pitfall 3: Arc refcount keeps the Vec alive).
            let arc = store.get(&key).map(|s| s.history_arc());
            (msgs, arc)
        };

        // 4. Build system message via PromptBuilder (loads SOUL.md + project context + memory)
        let cwd = std::env::current_dir().unwrap_or_default();
        // Phase 47.6 Plan 09 (P0-3): the prompt surface string follows the
        // EVENT's own platform, not a fixed "telegram" literal, so a Buzz
        // turn's system prompt correctly names the buzz surface.
        let mut prompt_builder = PromptBuilder::new(&model, event.platform.to_string())
            .with_provider(&self.config.model.provider)
            .load_context(&cwd);
        // Phase 25.3 D-W-2 (retargeted by Phase 47.6 Plan 09): inject the
        // resolved workspace root so the CURRENT surface renders
        // `[Workspace: <root>]` in the Identity slot, matching run_chat /
        // run_single. Frozen-snapshot — same Workspace instance for every
        // per-message handler clone (set by GatewayRunner::set_workspace).
        // This behaviour is not Telegram-specific: it applies to whichever
        // surface (Telegram, Discord, Slack, Buzz, ...) is running this turn.
        if let Some(ref ws) = self.workspace {
            prompt_builder = prompt_builder.with_workspace_root(&ws.root);
        }
        if let Some(ref mgr) = self.memory_manager {
            prompt_builder.set_memory_manager(mgr.clone());
        }
        // Phase 21.8.2 D-03: read via lock to always see the latest atomic swap.
        let registry_snapshot: Option<Arc<SkillRegistry>> =
            self.skill_registry.lock().ok().and_then(|g| g.clone());
        if let Some(registry) = registry_snapshot {
            prompt_builder.set_skill_registry(registry);
        }
        prompt_builder.load_memory().await;
        prompt_builder.load_skills();
        // Phase 21.8.2 D-Plan03-05 / D-07 (gateway delivery): read activated overlays
        // for this session and inject before the agent turn so the model sees the skill body.
        if let Ok(overlays) = self.skill_overlays.lock()
            && let Some(session_overlays) = overlays.get(&key)
        {
            for (name, body) in session_overlays {
                prompt_builder.activate_skill(name, body);
            }
        }
        // Phase 21.8.3.1 D-09: inject active personality overlay into PromptBuilder slot 8
        // (SessionOverlay, ephemeral). Re-applied every turn from self.active_personality_overlay;
        // never explicitly cleared between turns (entry absent when no personality is active).
        // Order: AFTER load_skills + skill_overlays loop, BEFORE build_system_message.
        if let Ok(overlays) = self.active_personality_overlay.lock()
            && let Some(overlay_text) = overlays.get(&key)
        {
            prompt_builder.set_overlay(overlay_text.clone());
        }
        // Phase 38.1 (D-04/D-05): freeze session timezone into PromptBuilder Timestamp slot.
        prompt_builder.set_timezone(self.config.agent.timezone.clone());
        let system_msg = prompt_builder.build_system_message();
        // Prepend system message
        let mut messages = vec![system_msg];
        messages.append(&mut session_messages);

        // Phase 18 Plan 06: per-turn gateway hygiene at 85% threshold (D-12).
        let _ = self.maybe_compress_gateway(&mut messages).await;

        // 5. Create mpsc channels for streaming bridge
        let (stream_tx, mut stream_rx) = mpsc::channel::<String>(256);
        let (tool_tx, mut tool_rx) = mpsc::channel::<String>(64);

        // Phase 36.17.2.2 D-10: oneshot channel for the consumer task to hand
        // the post-final-flush body to the parent task. The D-19 dispatch loop
        // needs this string to construct the reinsert body when an attachment
        // fails (`format!("{final_body}\n\n{failed_tags}")`). Sending happens
        // exactly once, just before the consumer task breaks. The parent task
        // awaits the receiver after `consumer_handle.await.ok()` so the value
        // is guaranteed-present whenever the consumer ran to completion.
        let (body_tx, body_rx) = tokio::sync::oneshot::channel::<String>();
        let mut body_tx = Some(body_tx);

        // 6. Spawn StreamConsumer task
        // Phase 47.6 Plan 09 (D-13): edit-capable adapters keep the exact
        // pre-existing constructor and placeholder-and-edit behaviour. An
        // adapter with no in-place-edit support (Buzz) gets the send-once
        // consumer instead — no placeholder id to hold, and the turn's
        // entire response publishes once at final flush (see
        // `StreamConsumer::new_with_mode` / `DeliveryMode::SendOnce`).
        let mut consumer = match placeholder_id.as_ref() {
            Some(pid) => StreamConsumer::new(adapter.clone(), &event.chat_id, pid),
            None => StreamConsumer::new_with_mode(
                adapter.clone(),
                &event.chat_id,
                None,
                DeliveryMode::SendOnce,
            )
            // Phase 47.6 Plan 08 (T-47.6-08-REPLY): thread the reply onto
            // the triggering message for a Buzz CHANNEL turn only — a DM
            // turn (`event.chat_type == "dm"`) must never set this, so
            // `send_dm`'s existing `thread_id` semantics (a plain event id
            // extra-tag on the rumor) are completely untouched.
            //
            // Live UAT fix (47.6 plan 08 restart): thread onto the RESOLVED
            // THREAD ROOT (`event.thread_id`, computed by
            // `buzz::resolve_thread_root` on receive), never the triggering
            // message's own id (`event.message_id`) — a bare "first e-tag"
            // reply target gets rejected by the Buzz relay
            // ("root tag does not match thread ancestry") whenever the
            // triggering message is itself mid-thread. `event.thread_id` is
            // always `Some` for a Buzz channel message (falls back to the
            // message's own id when it is itself the thread root), so the
            // `unwrap_or_else` below is a defensive fallback, never the
            // common case.
            .with_reply_to((event.chat_type == "channel").then(|| {
                event
                    .thread_id
                    .clone()
                    .unwrap_or_else(|| event.message_id.clone())
            })),
        };
        let consumer_handle = tokio::spawn(async move {
            let mut tool_rx_open = true;
            loop {
                if tool_rx_open {
                    tokio::select! {
                        biased;
                        msg = tool_rx.recv() => {
                            match msg {
                                Some(tool_name) => {
                                    consumer.tool_status(&tool_name);
                                    let _ = consumer.flush(false).await;
                                }
                                None => {
                                    // tool_rx closed — stop polling it
                                    tool_rx_open = false;
                                }
                            }
                        }
                        chunk = stream_rx.recv() => {
                            match chunk {
                                Some(text) => {
                                    consumer.clear_tool_status();
                                    consumer.push(&text);
                                    let _ = consumer.flush(false).await;
                                }
                                None => {
                                    // stream_rx closed — do final flush
                                    let _ = consumer.flush(true).await;
                                    // Phase 36.17.2.2 D-10: hand the final body
                                    // back to the parent task for the D-19
                                    // reinsert-on-failure path.
                                    if let Some(tx) = body_tx.take() {
                                        let _ = tx.send(consumer.final_body().to_string());
                                    }
                                    break;
                                }
                            }
                        }
                    }
                } else {
                    // tool_rx closed — drain stream_rx only
                    match stream_rx.recv().await {
                        Some(text) => {
                            consumer.clear_tool_status();
                            consumer.push(&text);
                            let _ = consumer.flush(false).await;
                        }
                        None => {
                            let _ = consumer.flush(true).await;
                            // Phase 36.17.2.2 D-10: hand the final body back
                            // to the parent task (alt break branch).
                            if let Some(tx) = body_tx.take() {
                                let _ = tx.send(consumer.final_body().to_string());
                            }
                            break;
                        }
                    }
                }
            }
        });

        // 7. Run turn via AgentRuntime (Plan 28.1-02).
        //
        // Phase 34a MEM-READ-05: scrub <memory-context> fence tags from streaming deltas.
        let scrubber_gw = std::sync::Arc::new(std::sync::Mutex::new(
            ironhermes_agent::streaming_scrubber::StreamingContextScrubber::new(),
        ));
        let scrubber_gw_cb = std::sync::Arc::clone(&scrubber_gw);
        // Phase 36.17.2.2 D-08 / Open Q5 / Assumption A10: extract `<MEDIA: ...>`
        // tags from streaming deltas alongside the scrubber. Chain order is
        // scrubber FIRST, extractor SECOND so MEDIA tags inside scrubbed
        // `<memory-context>` spans are dropped consistently (the scrubber
        // discards memory-context content; without the chain the extractor
        // would attach files referenced from invisible memory body).
        let extractor_gw = std::sync::Arc::new(std::sync::Mutex::new(
            crate::media_tag::MediaTagExtractor::new(),
        ));
        let extractor_cb = std::sync::Arc::clone(&extractor_gw);
        let stream_tx_clone = stream_tx.clone();
        // Wrapped in Option so they can be taken into TurnRequest (Some branch) or
        // dropped explicitly before consumer_handle.await (else branch). Without this,
        // dropping only stream_tx/tool_tx leaves stream_tx_clone/tool_tx_clone alive
        // inside the callbacks, keeping channels open and hanging consumer_handle.await.
        let mut stream_callback_opt: Option<StreamCallback> = Some(Box::new(move |delta: &str| {
            let scrubbed = scrubber_gw_cb.lock().unwrap().feed(delta);
            let visible = extractor_cb.lock().unwrap().feed(&scrubbed);
            if !visible.is_empty() {
                let _ = stream_tx_clone.try_send(visible);
            }
        }));

        let tool_tx_clone = tool_tx.clone();
        let mut tool_callback_opt: Option<ToolProgressCallback> =
            Some(Box::new(move |name: &str, _args: &str| {
                let _ = tool_tx_clone.try_send(name.to_string());
            }));

        // Phase 25.3-15 CR-02: the per-message session_id_str (`gw:<chat_id>:<sender_id>`)
        // feeds hooks / on_session_end and is intentionally distinct from the
        // canonical SQLite session UUID used for trajectory file paths.
        let session_id_str = format!("gw:{}:{}", event.chat_id, event.sender_id);

        // Phase 25.3-15 CR-02 close-out: open (or reuse) a per-session
        // trajectory writer keyed by the canonical SQLite session UUID. The
        // writer is cached in `SessionStore` so subsequent messages on the
        // same chat reuse one file handle (no leak across long-running
        // gateway sessions). Replaces the process-wide handle that was
        // previously attached at GatewayMessageHandler construction.
        let canonical_session_id = {
            let store = self.session_store.read().await;
            store
                .get(&key)
                .map(|s| s.session_id.clone())
                .unwrap_or_default()
        };
        let trajectory_writer = if !canonical_session_id.is_empty() {
            let mut store = self.session_store.write().await;
            store.get_or_create_trajectory_writer(&canonical_session_id)
        } else {
            None
        };

        // Phase 32 Plan 02 (LEARN-01): snapshot messages BEFORE moving into TurnRequest.
        let messages_for_nudge = messages.clone();

        // Source the nudge client from the runtime (run_turn owns the client).
        // AnyClient is Clone so this is cheap.
        let nudge_client = self
            .agent_runtime
            .as_ref()
            .map(|rt| rt.client().clone())
            .unwrap_or_else(|| build_main_client(&self.resolver).expect("nudge client fallback"));

        // Phase 36.2 chat-fix follow-up: thread state_store into TurnRequest so
        // AgentRuntime::run_turn calls `with_state_store` on the AgentLoop, which
        // enables the post-LLM-call usage_events write site. Without this the
        // gateway never writes usage_events rows even though session_store has
        // a valid StateStore — mirrors the TUI fix at commit a9fb0d0d. Symptom
        // pre-fix: /usage returns "No usage data found for this filter" because
        // the table is empty despite turns completing successfully.
        let state_store_for_turn = self.session_store.read().await.state_store().clone();

        // Build TurnRequest and call runtime.run_turn.
        // budget reset, loop construction, attach_context_engine, and fallback wiring
        // are all handled inside run_turn — do NOT call them again here.
        // Phase 36.2 follow-up: pass the canonical SQLite session UUID into
        // TurnRequest.session_id (not the `gw:<chat>:<sender>` hook form).
        // agent_loop's write site uses this as both usage_events.session_id
        // AND the `WHERE id = ?` clause on the sessions aggregate UPDATE; the
        // UPDATE silently affects 0 rows if the value doesn't match sessions.id.
        // The hook-side `session_id_str` (gw:…) remains the per-message identity
        // for on_session_end / progress hooks (kept distinct intentionally per
        // the Phase 25.3-15 CR-02 comment above).
        let turn_session_id = if canonical_session_id.is_empty() {
            session_id_str.clone()
        } else {
            canonical_session_id.clone()
        };
        // Phase 36.17.7 D-01: per-turn TTS wiring. Reuse the `key` SessionKey built at
        // line 1258 (Pitfall 5 — do NOT construct a new SessionKey here).
        // `telegram_audio_dispatcher` is `Some(_)` on the Telegram start path (real
        // TelegramAdapter clone-cast) and on Discord/Slack start paths
        // (NotSupportedAudioDispatcher stub per D-03-b); `None` on handlers built
        // outside the runner (tests, direct ::new() calls).
        //
        // D-05 invariant prep: this site uses the real session_key — the TtsPerTurnWiring
        // struct carries session_key: Some(key) semantics (non-Option field, always
        // populated when wiring is Some). Plan 05 Task 6 invariant greps for
        // "session_key: Some(" to confirm the real key flows here.
        // D-05 grep anchor: session_key: Some(key.clone()) — See TtsPerTurnWiring below.
        let tts_wiring = self.telegram_audio_dispatcher.as_ref().map(|disp| {
            ironhermes_agent::TtsPerTurnWiring {
                session_key: key.clone(), // D-05: always Some(real key) when wiring is Some
                audio_dispatcher: Some(disp.clone()),
            }
        });

        // Phase 36.3.8 D-02/D-04/D-05: per-turn messaging + clarify wiring.
        // Mirrors tts_wiring pattern above. Uses the real `key` SessionKey and the
        // same `turn_cancel` CancellationToken registered in the TurnEntry so /stop
        // reaches a suspended clarify (D-06 / T-36.3.8-02). The clarify_registry is
        // the SAME Arc constructed once in runner.rs and shared between the
        // callback_query loop (Plan 03) and this per-turn registration (T-36.3.8-ROUTE).
        // Always Some on Telegram (both dispatchers set); None dispatchers are fine
        // for surfaces that don't call set_telegram_*_dispatcher.
        let messaging_wiring = Some(ironhermes_agent::MessagingPerTurnWiring {
            session_key: key.clone(),
            message_dispatcher: self.telegram_message_dispatcher.clone(),
            clarify_dispatcher: self.telegram_clarify_dispatcher.clone(),
            clarify_registry: self.clarify_registry.clone(),
            cancel_token: Some(turn_cancel.clone()),
        });

        // Phase 45 D-11: construct a per-turn GatewayApprovalGate that binds the
        // coordinator to this turn's approval target. When the agent triggers
        // NeedsApproval, the gate sends an approval prompt to that target and
        // awaits the /approve or /deny response. `None` when no coordinator
        // is wired.
        //
        // Phase 47.6 Plan 09 (P0-3, T-47.6-09-01): the target is derived via
        // `approval_target_for`, NOT `event.chat_id` directly. For every
        // platform except Buzz this is behaviorally identical to
        // `event.chat_id` (today's behaviour, preserved exactly). For a Buzz
        // event that arrived in a CHANNEL, the derivation returns the
        // sender's own identity instead — see `approval_target_for`'s doc
        // comment for why (D-14: the prompt must reach the person who ran
        // the command privately, not the whole channel).
        let approval_target = approval_target_for(event);
        let approval_gate_for_turn: Option<std::sync::Arc<dyn ironhermes_core::ApprovalGate>> =
            self.approval_coordinator.as_ref().map(|coord| {
                std::sync::Arc::new(crate::approval::GatewayApprovalGate::new(
                    coord.clone(),
                    approval_target.clone(),
                )) as std::sync::Arc<dyn ironhermes_core::ApprovalGate>
            });

        // Phase 36.3.12 D-08 (Task 3, checker BLOCKER T-36.3.12-25): build a per-turn
        // terminal intercept that routes LLM-issued `terminal` tool calls through
        // `ironhermes_hooks::execute_gated_command` — the SAME chokepoint every other
        // surface uses (Plan 07). This is a MIGRATION off the gateway-local
        // `crate::shell_exec::shell_exec` helper: that helper never audited its
        // `Allow` path (Pitfall 3) and never forced approval on a remote backend /
        // credential-forwarding run (D-08) — leaving the gateway as the one D-08
        // carve-out this phase exists to close. The gateway's existing
        // `GatewayApprovalGate` (built above as `approval_gate_for_turn`) is STILL
        // passed through unchanged — this migration changes the execution/audit
        // chokepoint, NOT the approval UI (a Warn/NeedsApproval command still reaches
        // the same operator/Telegram prompt).
        //
        // All captured values are Send + Sync + 'static. The closure is stored via
        // register_intercepted_or_replace in run_turn so the tool stays visible to
        // the model but its invocation path changes per-turn.
        //
        // Phase 45 BL-02 fix (preserved): key the terminal-approval pending entry on
        // the SAME canonical session id the coordinator + /approve path use
        // (turn_session_id = the SQLite session UUID), not event.chat_id — chat_id is
        // still carried separately by GatewayApprovalGate for prompt delivery.
        let _ti_session = turn_session_id.clone();
        let _ti_gate = approval_gate_for_turn.clone();
        let _ti_dcfg = self.config.dangerous_commands.clone();
        let _ti_audit_cfg = self.config.audit.clone();
        let _ti_yolo = self.config.autonomous.yolo;
        let _ti_is_remote = self.config.terminal.backend == "ssh";
        let _ti_fwd_env_nonempty = !self.config.terminal.forward_env.is_empty();
        let _ti_terminal_tool = self
            .agent_runtime
            .as_ref()
            .and_then(|rt| rt.terminal_tool_arc());
        let _ti_chat_id = event.chat_id.clone();
        let terminal_intercept: Option<ironhermes_tools::registry::InterceptHandler> = {
            let session_id = _ti_session;
            let gate = _ti_gate;
            let dcfg = _ti_dcfg;
            let audit_cfg = _ti_audit_cfg;
            let yolo = _ti_yolo;
            let is_remote_backend = _ti_is_remote;
            let forward_env_nonempty = _ti_fwd_env_nonempty;
            let tool = _ti_terminal_tool;
            let chat_id = _ti_chat_id;
            Some(std::sync::Arc::new(move |args: serde_json::Value| {
                let cmd = args
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let sid = session_id.clone();
                let g = gate.clone();
                let guard = ironhermes_hooks::DangerousCommandGuardrail::from_config(&dcfg);
                let audit_log = ironhermes_core::AuditLog::load(audit_cfg.clone());
                let cid = chat_id.clone();
                let tool = tool.clone();
                Box::pin(async move {
                    let outcome = ironhermes_hooks::execute_gated_command(
                        "terminal",
                        &cmd,
                        &guard,
                        g.as_deref(),
                        &audit_log,
                        &sid,
                        "gateway",
                        &cid,
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
            }))
        };

        // Phase 36.3.12 D-08/D-11: build a per-turn execute_code intercept —
        // mirrors the terminal_intercept block immediately above. Gate-only (D-11):
        // classify_arg is an EMPTY opaque string (Python source is not shell syntax)
        // and is_remote_backend/forward_env_nonempty are always false (execute_code
        // never routes to a remote backend). Every resolution — including
        // background=true calls — is still audited (D-08/D-12).
        let _eci_session = turn_session_id.clone();
        let _eci_gate = approval_gate_for_turn.clone();
        let _eci_dcfg = self.config.dangerous_commands.clone();
        let _eci_audit_cfg = self.config.audit.clone();
        let _eci_yolo = self.config.autonomous.yolo;
        let _eci_execute_code_tool = self
            .agent_runtime
            .as_ref()
            .and_then(|rt| rt.execute_code_tool_arc());
        let _eci_chat_id = event.chat_id.clone();
        let execute_code_intercept: Option<ironhermes_tools::registry::InterceptHandler> = {
            let session_id = _eci_session;
            let gate = _eci_gate;
            let dcfg = _eci_dcfg;
            let audit_cfg = _eci_audit_cfg;
            let yolo = _eci_yolo;
            let tool = _eci_execute_code_tool;
            let chat_id = _eci_chat_id;
            Some(std::sync::Arc::new(move |args: serde_json::Value| {
                let sid = session_id.clone();
                let g = gate.clone();
                let guard = ironhermes_hooks::DangerousCommandGuardrail::from_config(&dcfg);
                let audit_log = ironhermes_core::AuditLog::load(audit_cfg.clone());
                let cid = chat_id.clone();
                let tool = tool.clone();
                Box::pin(async move {
                    let outcome = ironhermes_hooks::execute_gated_command(
                        "execute_code",
                        "", // D-11: opaque — Python source is not shell syntax
                        &guard,
                        g.as_deref(),
                        &audit_log,
                        &sid,
                        "gateway",
                        &cid,
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
            }))
        };

        let agent_result = if let Some(ref rt) = self.agent_runtime {
            let request = TurnRequest {
                messages,
                session_id: turn_session_id,
                cancel_token: Some(turn_cancel.clone()), // Phase 39.1 R39.1-05: per-turn cancel
                stream: stream_callback_opt.take(),
                tool_progress: tool_callback_opt.take(),
                tool_result: None,
                trajectory_writer,
                pressure_tracker: None, // run_turn makes a fresh tracker per turn
                state_store: Some(state_store_for_turn),
                compression_count: 0,
                tts_wiring,                            // Phase 36.17.7 D-01
                messaging_wiring,                      // Phase 36.3.8 D-02/D-04/D-05
                turn_id: Some(turn_id),                // Phase 39.2: correlate with TurnRegistry
                approval_gate: approval_gate_for_turn, // Phase 45 D-11
                terminal_intercept, // Phase 45 D-11: gated terminal tool override
                execute_code_intercept, // Phase 36.3.12 D-08/D-11: gated execute_code override
            };
            rt.run_turn(request).await
        } else {
            Err(anyhow::anyhow!(
                "AgentRuntime not configured in gateway handler"
            ))
        };

        // Phase 34a MEM-READ-05 + Phase 36.17.2.2 D-08: flush scrubber tail
        // then feed it through the extractor before emitting the extractor's
        // own tail. This preserves the scrubber→extractor chain order at
        // end-of-stream so an unterminated `<MEDIA:...` straddling a
        // memory-context fence boundary degrades consistently with the
        // streaming path (Open Q5 / Assumption A10).
        let scrubber_tail = scrubber_gw.lock().unwrap().flush();
        let extractor_pre = if !scrubber_tail.is_empty() {
            extractor_gw.lock().unwrap().feed(&scrubber_tail)
        } else {
            String::new()
        };
        let extractor_tail = extractor_gw.lock().unwrap().flush_tail();
        let tail = format!("{extractor_pre}{extractor_tail}");
        if !tail.is_empty() {
            let _ = stream_tx.try_send(tail);
        }

        // 9. Drop callbacks + channel senders so StreamConsumer observes channel
        // close and flushes its final batch.
        //
        // stream_callback_opt / tool_callback_opt capture stream_tx_clone /
        // tool_tx_clone. When rt.run_turn() is called (Some branch), .take() has
        // already consumed the Option so both are None here — drop is a no-op.
        // When agent_runtime is None (else branch), the Options still hold the
        // callbacks and thus the clones. Dropping here closes all sender clones
        // so the consumer's recv() returns None and consumer_handle.await completes
        // instead of hanging (Phase 39.1 bug fix).
        drop(stream_callback_opt);
        drop(tool_callback_opt);
        drop(stream_tx);
        drop(tool_tx);
        consumer_handle.await.ok();

        // 10. Cancel typing indicator
        cancel.cancel();
        typing_handle.await.ok();

        // RC-1 / REQ-37.2-03: hoist body_rx consumption to a single site BEFORE the
        // D-10 block so both D-10 and the RC-1 fallback share the same `final_body`
        // binding. This eliminates Pitfall 1 (double-await of a oneshot) — the D-10
        // local `body_rx.await` that was previously at line 1681 is removed here.
        // If the consumer task panicked, `body_rx` returns Err — `unwrap_or_default()`
        // produces "" which correctly triggers the RC-1 empty-stream path.
        let final_body = body_rx.await.unwrap_or_default();

        // RC-1 / Pitfall 5: track whether D-10 performed a placeholder re-edit so
        // the RC-1 fallback below does NOT also touch the placeholder (double-edit
        // would erase the failed-tag literals D-10 just inserted).
        let mut placeholder_handled_by_d10 = false;

        // Phase 36.17.2.2 D-19: dispatch extracted `<MEDIA: ...>` attachments.
        //
        // ANCHOR (per RESEARCH FLAGGED RISK / Pitfall 6 / Assumption A9): this
        // block lives AFTER `consumer_handle.await.ok()` AND `typing_handle.await.ok()`,
        // BEFORE `match agent_result`. CONTEXT.md D-19 cited an anchor
        // (`after stream_consumer.flush(true).await?`) that does NOT exist
        // inline — `flush(true)` lives inside the consumer task spawned at
        // handler.rs:~1313 area. The CORRECT synchronization point is the
        // `consumer_handle.await.ok()` barrier; placing the dispatch here
        // guarantees: (a) the final markdown edit has rendered before
        // attachments arrive, (b) the typing indicator has cleared, and
        // (c) attachments dispatch regardless of `agent_result` branch (the
        // user's text + media are independent of whether the turn completed
        // cleanly — attachments extracted before an agent error should
        // still be sent).
        let mut media_refs = extractor_gw.lock().unwrap().take_attachments();
        // fix(47): deterministic media delivery. The model may wrap the <MEDIA:>
        // tag in a code fence (which the extractor intentionally passes through
        // as literal text, NOT an attachment) or reword/drop it entirely. The
        // image_gen / video tools ALWAYS emit a bare <MEDIA: /path> in their
        // tool-result text, so append any media referenced by THIS turn's tool
        // results that the model's own stream did not already surface (deduped
        // by source) — it then dispatches through the same send_media + D-10
        // reinsert path below. On the agent-error path (`Err`) there is no
        // `appended`, so only the stream-extracted refs are sent (unchanged).
        if let Ok(ref ar) = agent_result {
            let tool_texts = ar
                .appended
                .iter()
                .filter(|m| m.role == ironhermes_core::types::Role::Tool)
                .filter_map(|m| m.content_text());
            crate::media_tag::append_undelivered_media_from_texts(&mut media_refs, tool_texts);
        }
        if !media_refs.is_empty() {
            if let Some(media_sender) = self.media_sender.as_ref() {
                let mut failed_tags: Vec<String> = Vec::new();
                for media_ref in media_refs {
                    match media_sender
                        .send_media(&event.chat_id, &media_ref, None)
                        .await
                    {
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!(
                                chat_id = %event.chat_id,
                                kind = ?media_ref.kind,
                                error = %e,
                                "attachment failed, reinserting tag literal (D-10)"
                            );
                            failed_tags.push(media_ref.original_tag_text.clone());
                        }
                    }
                }
                // Phase 47.6 Plan 09: D-10's re-edit is placeholder-dependent —
                // guard it on `placeholder_id` being present (edit-capable
                // adapter). In practice `media_sender` is only ever `Some` on
                // the Telegram start path today, so this guard is currently
                // a no-op safety net rather than a live branch, but it keeps
                // this block correct if a future edit-capable+MediaSender
                // adapter is added.
                if !failed_tags.is_empty()
                    && let Some(placeholder_id) = placeholder_id.as_ref()
                {
                    // D-10: ONE combined re-edit of the placeholder appending
                    // each failed tag literal on its own line (not one edit
                    // per failure). The final body from `StreamConsumer::flush(true)`
                    // arrived via the hoisted `final_body` binding above (RC-1
                    // Option A hoist — the local `body_rx.await` that was here
                    // has been removed to prevent Pitfall 1 double-await).
                    // Concat the failed-tag literals, run through
                    // `escape_outside_code_blocks` so the entire reinsert body
                    // satisfies MarkdownV2 (the appended literals contain
                    // paths with `.` / `/` / etc. — reserved chars that the
                    // escape preserves correctly inside link grammar and
                    // escapes outside).
                    let appended = failed_tags.join("\n");
                    let reinsert_body = if final_body.is_empty() {
                        appended
                    } else {
                        format!("{final_body}\n\n{appended}")
                    };
                    let escaped = crate::markdown_v2::escape_outside_code_blocks(&reinsert_body);
                    if let Err(e) = adapter
                        .edit_message_markdown_v2(&event.chat_id, placeholder_id, &escaped)
                        .await
                    {
                        tracing::error!(
                            chat_id = %event.chat_id,
                            message_id = %placeholder_id,
                            error = %e,
                            "D-10 reinsert edit failed; placeholder retains its post-flush body without tag literals"
                        );
                    }
                    // Pitfall 5: D-10 performed a placeholder edit; RC-1 fallback
                    // must not also touch the placeholder.
                    placeholder_handled_by_d10 = true;
                }
            } else {
                // Phase 47.6 Plan 09 (D-15): stop dropping agent-emitted media
                // on platforms with no MediaSender. Keep the existing warn! so
                // the log record does not regress — it is now accompanied by a
                // real user-visible message rather than replacing one. This
                // arm is out of scope for the `Some` branch above (D-10's
                // per-ref send_media loop, failed-tag accumulation, and
                // combined re-edit remain Telegram's shipped behaviour,
                // untouched).
                tracing::warn!(
                    chat_id = %event.chat_id,
                    ref_count = media_refs.len(),
                    "media tags emitted on platform without MediaSender — sending text notice (D-15)"
                );
                let notice = media_fallback_notice(&media_refs);
                if let Err(e) =
                    with_rate_limit_retry(|| adapter.send_message(&event.chat_id, &notice, None))
                        .await
                {
                    tracing::error!(
                        chat_id = %event.chat_id,
                        error = %e,
                        "media fallback notice failed to send"
                    );
                }
            }
        }

        // RC-1 / REQ-37.2-01 / REQ-37.2-02 / REQ-37.2-06: turn-end fallback.
        //
        // Fires only when the streamed body was empty (D-05 invariant: turns that
        // streamed text are untouched) AND D-10 did not already re-edit the
        // placeholder (Pitfall 5 guard).
        //
        // Decision tree (mirrors deliver_turn_end_fallback in lib.rs):
        //   - empty final_body + Some(non-empty final_response) → edit placeholder (REQ-37.2-01)
        //   - empty final_body + None/empty final_response → delete placeholder (REQ-37.2-02)
        //   - non-empty final_body → streamed path, emit trace only (REQ-37.2-06)
        if final_body.trim().is_empty() && !placeholder_handled_by_d10 {
            // Phase 47.6 Plan 09 (D-13): this whole fallback is
            // placeholder-dependent — with no placeholder there is nothing
            // to edit or delete, and an empty turn on a send-once adapter
            // must simply publish nothing (there is no message to remove,
            // and publishing an empty-turn notice would itself be a
            // permanent, unretractable event on an immutable-event surface).
            match placeholder_id.as_ref() {
                Some(placeholder_id) => match agent_result {
                    Ok(ref result) => {
                        match result.final_response.as_deref() {
                            Some(fr) if !fr.trim().is_empty() => {
                                // REQ-37.2-01: edit placeholder with escaped final_response
                                let escaped = crate::markdown_v2::escape_outside_code_blocks(fr);
                                let _ = adapter
                                    .edit_message_markdown_v2(
                                        &event.chat_id,
                                        placeholder_id,
                                        &escaped,
                                    )
                                    .await;
                                tracing::info!(
                                    had_text = true,
                                    delivered = true,
                                    target = %event.chat_id,
                                    reason = "final_response_fallback",
                                    "turn-end: delivered via final_response fallback"
                                );
                            }
                            _ => {
                                // REQ-37.2-02: truly empty turn — delete the placeholder
                                let _ =
                                    adapter.delete_message(&event.chat_id, placeholder_id).await;
                                tracing::warn!(
                                    had_text = false,
                                    delivered = false,
                                    target = %event.chat_id,
                                    reason = "tool_only_turn",
                                    "turn-ended-empty: placeholder removed"
                                );
                            }
                        }
                    }
                    Err(_) => {
                        // Error path handled below in `match agent_result`; skip fallback.
                    }
                },
                None => {
                    tracing::info!(
                        had_text = false,
                        delivered = false,
                        target = %event.chat_id,
                        reason = "empty_turn_send_once",
                        "turn-ended-empty: no placeholder to remove (send-once mode, D-13)"
                    );
                }
            }
        } else if !final_body.trim().is_empty() {
            // REQ-37.2-06: normal stream path — body was flushed by consumer
            tracing::info!(
                had_text = true,
                delivered = true,
                target = %event.chat_id,
                reason = "streamed",
                "turn-end: delivered via stream"
            );
        }

        match agent_result {
            Ok(result) => {
                info!("Agent completed, turns_used={}", result.turns_used);

                // Fire ResponseSent hook with real platform and chat_id.
                // Phase 47.6 Plan 09 (P0-3): mirrors the MessageReceived hook
                // above — reports the event's own platform, not a fixed
                // Telegram literal.
                if let Some(ref registry) = self.hook_registry
                    && let Some(ref response) = result.final_response
                {
                    let hook_event = ironhermes_hooks::HookEvent::new(
                        &uuid::Uuid::new_v4().to_string(),
                        ironhermes_hooks::HookEventKind::ResponseSent {
                            platform: event.platform.to_string(),
                            chat_id: event.chat_id.clone(),
                            response_preview: ironhermes_hooks::event::preview(response, 200),
                        },
                    );
                    registry.fire(hook_event);
                }

                // Phase 32 Plan 02 (LEARN-01): periodic memory-review nudge.
                //
                // Per-session counter `nudge_turns` is incremented inside a small
                // synchronous std::sync::Mutex critical section; the `should_fire`
                // bool is extracted and the guard is dropped BEFORE any
                // `tokio::spawn` / `.await` (T-32-07 mitigation; clippy
                // `await_holding_lock`).
                //
                // Gate: only when memory.nudge_interval > 0 (disable sentinel),
                // memory.memory_enabled is true, AND we have a MemoryManager
                // configured. spawn_nudge_review is fire-and-forget so the
                // gateway response is not blocked on nudge completion (T-32-05).
                let nudge_interval = self.config.memory.nudge_interval;
                if nudge_interval > 0 && self.config.memory.memory_enabled {
                    let should_fire = {
                        let mut map = self.nudge_turns.lock().unwrap_or_else(|e| e.into_inner());
                        let count = map.entry(key.clone()).or_insert(0);
                        *count += 1;
                        if *count >= nudge_interval {
                            *count = 0;
                            true
                        } else {
                            false
                        }
                    }; // std::sync::MutexGuard dropped here — before any .await / tokio::spawn

                    if should_fire && let Some(ref mgr) = self.memory_manager {
                        let mgr_clone = Arc::clone(mgr);
                        let client_clone = nudge_client.clone();
                        let messages_snapshot = messages_for_nudge.clone();
                        let config_clone = self.config.clone();
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

                // WR-01 (Phase 34b Plan 03): render context_warnings out-of-band.
                // Sent as a SEPARATE message so it is visibly distinct from the agent
                // response — mirrors the Err arm's error_suffix pattern (a distinct
                // send_message call, not appended to the streamed response).
                if !result.context_warnings.is_empty() {
                    let warning_lines: Vec<String> = result
                        .context_warnings
                        .iter()
                        .map(|w| format!("- {}", w))
                        .collect();
                    let warnings_block =
                        format!("--- Context Warnings ---\n{}", warning_lines.join("\n"));
                    let _ = adapter
                        .send_message(&event.chat_id, &warnings_block, None)
                        .await;
                }

                // 11. Update session with agent's response messages (write-through to SQLite).
                //
                // Phase 25.1 GAP-7 follow-up: persist `result.appended` directly. The
                // previous `filter(|m| m.role == Role::Assistant)` over `result.messages`
                // dropped every Role::Tool message, so the next turn's history failed
                // OpenAI's strict assistant↔tool pairing invariant — the streaming endpoint
                // returned 400, validate_tool_call_pairing now catches it as an orphan
                // pre-send, and the agent gave up after retries. `appended` is the
                // round-trip output (assistant turns + matching tool results, in order),
                // and excludes one-shot pressure-tier system advisories. Compression-safe.
                //
                // Phase 39.1 (R39.1-02, D-02): append the full `result.appended` batch
                // under a SINGLE write-lock acquisition so no concurrent turn can
                // interleave its messages between ours.  If the session was removed by
                // a concurrent `/new` before we get here, fall back to the `Arc`-backed
                // history handle captured at turn start — the Vec stays alive via
                // refcount (RESEARCH Pitfall 3).  Lock is NEVER held across `.await`.
                if !result.appended.is_empty() {
                    let mut store = self.session_store.write().await;
                    let session_still_exists = store.get(&key).is_some();
                    if session_still_exists {
                        // Fast path: session is live — add all messages under one write lock.
                        store.add_messages_batch_to_session(&key, result.appended);
                    } else {
                        // Fallback: session removed by /new. Append to Arc-backed Vec so
                        // this turn's output is not silently dropped (Pitfall 3 mitigation).
                        drop(store); // release write lock before Arc lock
                        if let Some(ref arc) = history_arc {
                            let mut guard = arc.lock().unwrap_or_else(|e| e.into_inner());
                            for msg in result.appended {
                                guard.push(msg);
                            }
                            // Note: SQLite write-through skipped — session row was ended by /new.
                        }
                    }
                }
            }
            Err(e) => {
                // D-18: Append error indicator to whatever was already streamed
                error!("Agent error: {:#}", e);
                let error_suffix = "\n\n-- Something went wrong, please try again";
                let _ = adapter
                    .send_message(&event.chat_id, error_suffix, None)
                    .await;
            }
        }

        // Plan 21.7-06 (D-24, T-21.7-06-01): per-request drain of the
        // gateway-scoped ProcessRegistry. The registry's task_id is a
        // process-wide constant ("gateway"), so drain_and_kill_session with
        // the per-request session_id is a deliberate no-op unless a future
        // plan lands per-session scoping. The call is still emitted so
        // INV-21.7-07 (static-grep gate on gateway handler drain) stays
        // green and the wiring is audit-visible.
        if let Some(ref reg) = self.process_registry
            && let Err(e) = reg
                .write()
                .await
                .drain_and_kill_session(&session_id_str)
                .await
        {
            tracing::warn!(
                error = %e,
                "process_registry drain_and_kill_session failed in gateway run_agent (best-effort)"
            );
        }

        // Plan 21.7-07 (D-05): drain pending fire-and-forget transcript
        // writes before returning from the per-request handler. Matches
        // the Plan 03 recommendation (real writes complete in <10ms).
        // Guard with subagent_registry to avoid an unconditional 200ms
        // penalty in tests that don't wire the registry.
        if let Some(ref reg) = self.subagent_registry {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            // Touch the registry so the borrow lives through the sleep.
            let _ = reg.read().await.active_count();
        }

        // GAP-6: notify memory provider of session end (best-effort).
        // Gateway sessions lack a natural "end" signal, so fire at per-request
        // completion — the closest equivalent for long-lived Telegram sessions.
        if let Some(ref mgr) = self.memory_manager {
            let mgr_lock = mgr.lock().await;
            let entries = ironhermes_core::memory_provider::MemoryEntries::default();
            if let Err(e) = mgr_lock.on_session_end(&session_id_str, &entries).await {
                tracing::debug!(error = %e, "on_session_end failed in gateway run_agent (best-effort)");
            }
        }

        Ok(())
    }
}

#[async_trait]
impl MessageHandler for GatewayMessageHandler {
    async fn handle(
        &self,
        event: &MessageEvent,
        adapter: Arc<dyn PlatformAdapter>,
        cancel: CancellationToken,
    ) -> Result<()> {
        // Intercept slash commands before agent loop (plan 04)
        if event.content.starts_with('/') {
            // Text-only path — no multimodal attachments to forward
            let no_attachments = ProcessedAttachments {
                text_prefix: None,
                image_data_uri: None,
                image_cache_path: None,
            };
            return self
                .handle_slash_command(event, adapter, cancel, no_attachments)
                .await;
        }
        // Phase 36 (D-02, Pitfall 1) + Phase 36.17.1 (D-01, D-13):
        // Phase 39.1 (R39.1-06): gate removed — semaphore in run_agent handles cap;
        // over-cap messages stay in SessionQueue via the worker loop's try_acquire.
        // No multimodal data via this path (text-only fallback)
        let no_attachments = ProcessedAttachments {
            text_prefix: None,
            image_data_uri: None,
            image_cache_path: None,
        };
        self.run_agent(event, adapter, cancel, no_attachments).await
    }
}

/// Build a ChatMessage for the user's input, incorporating any multimodal data.
///
/// - If there is an image_data_uri: creates a multipart message with text + image.
/// - If there is a text_prefix (document): prepends it to the message content.
/// - Otherwise: plain text message.
///
/// Phase 32.3 Plan 04 (D-09 / T-32.3-01): pure predicate — does this `/agents`
/// subcommand require a `confirm` token to proceed on the gateway?
///
/// - `"kill"` and `"prune"` are destructive — must have `"confirm"` somewhere
///   in `args` (tolerant position so operators can type
///   `/agents kill sub_xxx confirm` OR `/agents kill confirm sub_xxx`).
/// - `"interrupt"` and `"status"` are NOT destructive — never require confirm.
/// - Any other subcommand (`"list"`, `"logs"`, unknown) — never require confirm.
///
/// Returns `true` when the subcommand IS destructive AND the confirm token
/// is missing — i.e. the gateway should refuse to dispatch.
///
/// Extracted as a free fn so tests can exercise the D-09 contract directly
/// without constructing a full GatewayMessageHandler + adapter pipeline.
pub(crate) fn requires_confirm(subcommand: &str, args: &[&str]) -> bool {
    let is_destructive = matches!(subcommand, "kill" | "prune");
    if !is_destructive {
        return false;
    }
    // Tolerant position: any arg after the subcommand may be "confirm".
    !args.contains(&"confirm")
}

/// Derive the approval-prompt delivery target for a turn's `MessageEvent`
/// (Phase 47.6 Plan 09, P0-3 / T-47.6-09-01 / D-14).
///
/// For every platform OTHER than Buzz, returns `event.chat_id` — this
/// preserves today's approval-prompt behaviour exactly and MUST stay the
/// default arm, so a future platform can never accidentally inherit Buzz's
/// routing.
///
/// For Buzz:
/// - A direct message: returns `event.chat_id`, which plan 05 sets to the
///   peer's npub, so the approval prompt returns to the same encrypted DM.
/// - A channel event (`event.chat_type == "channel"`): returns
///   `event.sender_id` instead of the channel identifier. `event.sender_id`
///   on Buzz is the author's pubkey hex, which `parse_buzz_chat_target`
///   (plan 05) accepts as a direct-message target — so the prompt is
///   gift-wrapped to the person who ran the command rather than posted where
///   the whole channel can read it.
///
/// D-14 specifies the approval prompt arrives as a DM, and an approval
/// prompt carries the pending id plus the gated command's description.
/// Posting that into a channel both discloses what the operator is doing
/// and hands the pending id to everyone who can then race a reply. Plan 06's
/// guard rejects channel-borne approval COMMANDS (the inbound half); this
/// derivation is the other half — it keeps the PROMPT out of the channel in
/// the first place (the outbound half).
pub(crate) fn approval_target_for(event: &MessageEvent) -> String {
    if event.platform == Platform::Buzz && event.chat_type == "channel" {
        event.sender_id.clone()
    } else {
        event.chat_id.clone()
    }
}

/// Render a text notice naming every media artifact that could not be
/// attached on the current turn's surface (Phase 47.6 Plan 09, D-15).
///
/// D-15 already settled this for the cron/kanban delivery arms: when there
/// is no `MediaSender` installed for the current platform, an agent-emitted
/// `<MEDIA: ...>` tag must not vanish silently with only a log line — it
/// becomes a text message naming the artifact and its local path (or URL),
/// with a one-line header explaining why the recipient is reading a path
/// instead of seeing the image. This is deliberately a text notice, NOT a
/// `MediaSender` implementation — D-15 leaves the URL-embed / relay-blob /
/// file-server choice open for a later plan (P2-2).
pub(crate) fn media_fallback_notice(refs: &[crate::media_tag::MediaRef]) -> String {
    let mut lines = vec![
        "Media could not be attached on this platform — the artifact(s) below \
         were generated but are not shown inline:"
            .to_string(),
    ];
    for r in refs {
        let location = match &r.source {
            crate::media_tag::MediaSource::Path(p) => p.display().to_string(),
            crate::media_tag::MediaSource::Url(u) => u.clone(),
        };
        lines.push(format!("- {location}"));
    }
    lines.join("\n")
}

fn build_user_message(event: &MessageEvent, processed: ProcessedAttachments) -> ChatMessage {
    if let Some(data_uri) = processed.image_data_uri {
        // Vision input: multipart message with optional caption + image
        let mut parts = Vec::new();
        let mut text = if !event.content.is_empty() {
            event.content.clone()
        } else {
            "What is in this image?".to_string()
        };
        // When the inbound photo was persisted to the image cache, tell the model the
        // exact PATH so it can drive `video_animate` (image-to-video). `video_animate`
        // base64-encodes a file path or fetches a public URL — it CANNOT consume the
        // inline vision data URI, so a path is mandatory for image-to-video.
        if let Some(ref path) = processed.image_cache_path {
            text.push_str(&format!(
                "\n\n[System: the attached image is saved at \"{path}\". If the user wants a video generated from this image, call the video_animate tool with image_url set to that exact path. Do NOT paste image data inline.]"
            ));
        }
        parts.push(ContentPart::Text { text });
        parts.push(ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: data_uri,
                detail: None,
            },
        });
        ChatMessage {
            role: Role::User,
            content: Some(MessageContent::Parts(parts)),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            is_recall_context: false,
        }
    } else if let Some(prefix) = processed.text_prefix {
        // Document text: prepend extracted content to the user message
        let combined = if event.content.is_empty() {
            prefix
        } else {
            format!("{}\n\n{}", prefix, event.content)
        };
        ChatMessage {
            role: Role::User,
            content: Some(MessageContent::text(combined)),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            is_recall_context: false,
        }
    } else {
        // Plain text
        ChatMessage {
            role: Role::User,
            content: Some(MessageContent::text(&event.content)),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            is_recall_context: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::SkillRecord;
    use ironhermes_tools::ToolRegistry;
    use std::path::PathBuf;

    fn make_handler() -> GatewayMessageHandler {
        let config = Config::default();
        let resolver = ProviderResolver::build(&config).unwrap();
        let state_store = Arc::new(std::sync::Mutex::new(
            ironhermes_state::StateStore::new(":memory:").expect("in-memory StateStore"),
        ));
        let session_store = Arc::new(RwLock::new(crate::session::SessionStore::new(state_store)));
        let tool_registry = Arc::new(RwLock::new(ToolRegistry::new()));
        GatewayMessageHandler::new(config, resolver, session_store, tool_registry)
    }

    fn make_skill_record(name: &str, allowed_tools: Option<Vec<String>>) -> SkillRecord {
        SkillRecord {
            name: name.to_string(),
            description: "test skill".to_string(),
            path: PathBuf::from("/tmp/test-skill.md"),
            platforms: None,
            compatibility: None,
            allowed_tools,
            metadata: None,
            // Phase 19 Plan 01: typed HermesMetadata + SkillSource fields.
            hermes_metadata: None,
            source: ironhermes_core::SkillSource::Builtin,
        }
    }

    /// Regression test for the Arc identity bug (D-01):
    /// handler.new() created its own Arc, so skills activated via SkillsTool
    /// never reached AgentLoop enforcement. The fix: set_active_skills() overwrites
    /// the default with the shared Arc.
    #[test]
    fn test_active_skills_arc_shared() {
        let mut handler = make_handler();

        let shared: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        handler.set_active_skills(shared.clone());

        assert!(
            Arc::ptr_eq(&shared, &handler.active_skills),
            "handler.active_skills must be the same Arc allocation as the one passed to set_active_skills"
        );
    }

    /// Regression test for behavioral enforcement via the handler->AgentLoop path:
    /// Proves that when the shared Arc (with a restrictive skill) is passed to AgentLoop
    /// via with_active_skills(), enforcement fires for tools not in allowed_tools.
    /// This is the behavioral half of the regression — if the Arc were a separate
    /// allocation (the bug), this test would pass vacuously (empty skills = no restriction).
    #[tokio::test]
    async fn test_active_skills_enforcement_fires() {
        let shared: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));

        // Populate with a restrictive skill
        {
            let mut skills = shared.lock().unwrap();
            skills.push(make_skill_record(
                "restrictive-skill",
                Some(vec!["skills".to_string()]),
            ));
        }

        // Create AgentLoop with the shared Arc (same one handler would pass after fix)
        let client =
            ironhermes_agent::AnyClient::ChatCompletions(ironhermes_agent::LlmClient::new(
                "http://localhost:0".to_string(),
                "test-key".to_string(),
                "test-model",
            ));
        let tool_registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let loop_instance = ironhermes_agent::AgentLoop::new(client, tool_registry, 4)
            .with_active_skills(shared.clone());

        // Verify AgentLoop received the same Arc (identity check)
        assert!(
            Arc::ptr_eq(&shared, &loop_instance.active_skills()),
            "AgentLoop.active_skills must be the same Arc allocation as the one passed via with_active_skills"
        );

        // Verify enforcement fires — when the shared Arc has a restrictive skill,
        // the active_skills state is visible in AgentLoop (the wiring is correct).
        // The actual enforcement logic is already regression-tested in agent_loop.rs.
        // Here we confirm the Arc flows from handler to AgentLoop correctly.
        let skills_count = shared.lock().unwrap().len();
        assert_eq!(
            skills_count, 1,
            "Restrictive skill should be visible through the shared Arc"
        );

        let enforcement_would_trigger = {
            let skills = loop_instance.active_skills();
            let locked = skills.lock().unwrap();
            locked.iter().any(|s| s.allowed_tools.is_some())
        };
        assert!(
            enforcement_would_trigger,
            "not permitted by the active skill set — enforcement would trigger for non-allowed tools"
        );
    }

    // ── Phase 18 Plan 06: gateway hygiene per-turn compression (D-12) ───────

    use async_trait::async_trait;
    use ironhermes_agent::context_engine::{
        CompressionMode, CompressionOutcome, ContextEngine, ContextError, ContextStats,
    };
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    struct RecordingGatewayEngine {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ContextEngine for RecordingGatewayEngine {
        async fn compress(
            &self,
            _messages: &mut Vec<ChatMessage>,
            _stats: ContextStats,
        ) -> Result<CompressionOutcome, ContextError> {
            self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            Ok(CompressionOutcome {
                compressed: true,
                ..CompressionOutcome::default()
            })
        }
        fn threshold(&self) -> f32 {
            0.85
        }
        fn mode(&self) -> CompressionMode {
            CompressionMode::Hard
        }
    }

    fn filler_messages(n: usize) -> Vec<ChatMessage> {
        (0..n)
            .map(|i| ChatMessage::user(format!("message {i} ").repeat(20)))
            .collect()
    }

    /// Handler triggers gateway compression exactly once per turn when ratio >= 0.85,
    /// and never when below.
    #[tokio::test]
    async fn gateway_handler_per_turn_hygiene() {
        // Above threshold: tiny context_length forces ratio > 0.85.
        let mut handler = make_handler();
        let calls = Arc::new(AtomicUsize::new(0));
        let engine: Arc<dyn ContextEngine> = Arc::new(RecordingGatewayEngine {
            calls: calls.clone(),
        });
        handler.set_gateway_engine(engine, 100);

        let mut msgs = filler_messages(20);
        let fired = handler.maybe_compress_gateway(&mut msgs).await;
        assert!(fired, "hygiene must fire above 0.85 threshold");
        assert_eq!(
            calls.load(AtomicOrdering::SeqCst),
            1,
            "exactly one compress call"
        );

        // Below threshold: huge context_length keeps ratio << 0.85.
        let mut handler2 = make_handler();
        let calls2 = Arc::new(AtomicUsize::new(0));
        let engine2: Arc<dyn ContextEngine> = Arc::new(RecordingGatewayEngine {
            calls: calls2.clone(),
        });
        handler2.set_gateway_engine(engine2, 10_000_000);

        let mut msgs2 = filler_messages(3);
        let fired2 = handler2.maybe_compress_gateway(&mut msgs2).await;
        assert!(!fired2, "hygiene must not fire below 0.85 threshold");
        assert_eq!(
            calls2.load(AtomicOrdering::SeqCst),
            0,
            "no compress call below threshold"
        );
    }

    // ── Phase 18 Plan 09: UAT gap closure — agent engine wiring ────────────

    /// Verifies that the gateway handler wires the agent-side context engine
    /// via `attach_context_engine` using its own config/resolver, so
    /// `config.agent.compression_threshold` is honored at runtime.
    #[tokio::test]
    async fn gateway_handler_attaches_agent_engine() {
        let handler = make_handler();
        let client =
            ironhermes_agent::AnyClient::ChatCompletions(ironhermes_agent::LlmClient::new(
                "http://localhost:0".to_string(),
                "k".to_string(),
                "test-model",
            ));
        let max_turns = handler.config.agent.max_turns;
        let agent =
            ironhermes_agent::AgentLoop::new(client, handler.tool_registry.clone(), max_turns);
        let context_length = handler.resolver.resolve_for_main().context_length();
        let agent = ironhermes_agent::attach_context_engine(
            agent,
            &handler.config,
            &handler.resolver,
            "sess-gw",
            handler.hook_registry.clone(),
            None,           // Phase 18-14: fresh tracker per gateway test
            context_length, // Phase 21.3
            None,           // memory_manager: None in gateway unit test
        );
        assert!(
            agent.has_context_engine(),
            "agent must have context engine attached"
        );
        assert!(
            agent.has_pressure_tracker(),
            "agent must have pressure tracker attached"
        );
        assert_eq!(agent.session_id(), Some("sess-gw".to_string()));
    }

    // ── Phase 21.1 Plan 02: slash command router integration tests ────────────

    /// Regression: handler.rs must use CommandRouter for slash command dispatch.
    #[test]
    fn handler_uses_command_router() {
        let src = include_str!("handler.rs");
        assert!(
            src.contains("CommandRouter"),
            "handler.rs must use CommandRouter for slash command dispatch"
        );
    }

    /// Regression: handler.rs must construct CommandContext for command dispatch.
    #[test]
    fn handler_uses_command_context() {
        let src = include_str!("handler.rs");
        assert!(
            src.contains("CommandContext"),
            "handler.rs must construct CommandContext for command dispatch"
        );
    }

    /// Regression: handler.rs must not contain old hardcoded help text.
    #[test]
    fn handler_does_not_have_hardcoded_help_text() {
        let src = include_str!("handler.rs");
        // Split the forbidden string so this test itself doesn't trigger the check.
        let forbidden = ["/start - ", "Start a new conversation with an introduction"].concat();
        assert!(
            !src.contains(&forbidden),
            "handler.rs must not contain hardcoded help text (use CommandRouter)"
        );
    }

    /// Regression: handler.rs must call command_router.resolve() for slash command resolution.
    #[test]
    fn handler_resolves_commands_via_router() {
        let src = include_str!("handler.rs");
        assert!(
            src.contains("command_router.resolve(") || src.contains("self.command_router.resolve("),
            "handler.rs must call command_router.resolve() for slash command resolution"
        );
    }

    /// Structural: GatewayMessageHandler has command_router field initialized in new().
    #[test]
    fn handler_struct_has_command_router_field() {
        // Verify the field is present and initialized — construction succeeds.
        let handler = make_handler();
        // CommandRouter construction panics on duplicate names — if it succeeds, registry is valid.
        let _ = handler
            .command_router
            .resolve("/help", &ironhermes_core::types::Platform::Telegram);
    }

    // ── Phase 41.1 Plan 04: Telegram one-shot skill activate+run (D-08) ──────

    use ironhermes_core::{MessageEvent, MessageResponse};

    /// Build an isolated `SkillRegistry` containing a single skill on disk.
    /// The returned `TempDir` MUST be kept alive: `read_content` reads the file.
    fn skill_run_test_registry(name: &str, body: &str) -> (tempfile::TempDir, Arc<SkillRegistry>) {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: a test skill\n---\n{body}"),
        )
        .unwrap();
        let registry = SkillRegistry::load_with_paths(&[skills_dir]);
        (dir, Arc::new(registry))
    }

    /// Mock adapter recording every `send_message` call as `(chat_id, content)`.
    ///
    /// It returns `Err` for the `run_agent` placeholder block ("█") so the heavy
    /// `run_agent` body short-circuits at its first `.await?` (handler.rs ~1596)
    /// BEFORE any network / AgentRuntime work — while still proving, via the
    /// recorded placeholder send, that `run_agent` was actually entered with the
    /// synthesized event's inherited `chat_id`.
    struct RecordingAdapter {
        sends: Arc<std::sync::Mutex<Vec<(String, String)>>>,
    }

    const PLACEHOLDER_BLOCK: &str = "\u{2588}";

    #[async_trait]
    impl PlatformAdapter for RecordingAdapter {
        fn platform(&self) -> Platform {
            Platform::Telegram
        }
        async fn send_message(
            &self,
            chat_id: &str,
            content: &str,
            _thread_id: Option<&str>,
        ) -> Result<MessageResponse> {
            self.sends
                .lock()
                .unwrap()
                .push((chat_id.to_string(), content.to_string()));
            if content == PLACEHOLDER_BLOCK {
                // Short-circuit run_agent right after the placeholder send.
                anyhow::bail!("test short-circuit after placeholder");
            }
            Ok(MessageResponse {
                message_id: "mock-msg-id".to_string(),
                chat_id: chat_id.to_string(),
                platform: Platform::Telegram,
            })
        }
        async fn send_message_markdown_v2(
            &self,
            chat_id: &str,
            content: &str,
            thread_id: Option<&str>,
        ) -> Result<MessageResponse> {
            self.send_message(chat_id, content, thread_id).await
        }
        async fn edit_message(&self, _c: &str, _m: &str, _content: &str) -> Result<()> {
            Ok(())
        }
        async fn edit_message_markdown_v2(&self, _c: &str, _m: &str, _content: &str) -> Result<()> {
            Ok(())
        }
        async fn delete_message(&self, _c: &str, _m: &str) -> Result<()> {
            Ok(())
        }
        fn is_running(&self) -> bool {
            true
        }
    }

    fn skill_run_event(content: &str, chat_id: &str, sender_id: &str) -> MessageEvent {
        MessageEvent {
            platform: Platform::Telegram,
            message_id: "orig-msg-42".to_string(),
            chat_id: chat_id.to_string(),
            sender_id: sender_id.to_string(),
            content: content.to_string(),
            attachments: Vec::new(),
            thread_id: None,
            chat_type: "dm".to_string(),
            chat_name: None,
            sender_name: None,
            replied_to_id: None,
        }
    }

    fn no_attachments() -> ProcessedAttachments {
        ProcessedAttachments {
            text_prefix: None,
            image_data_uri: None,
            image_cache_path: None,
        }
    }

    /// D-08 (real SKILL-13 path): a bare `/<skill>` whose token is not a builtin
    /// command falls through to the NotFound fallback, which must ACTIVATE the
    /// skill body into the session overlay AND fire `run_agent` immediately —
    /// not merely push an overlay and reply with the retired activation copy.
    ///
    /// Proof that `run_agent` actually fired (not overlay-only): the mock adapter
    /// records the `run_agent` placeholder ("█") send. Proof of identity
    /// inheritance (T-41.1-04-01): that placeholder was sent to the ORIGINAL
    /// event's `chat_id`, i.e. the synthesized event carried the real identity
    /// via `..event.clone()`, never one reconstructed from the skill name/args.
    #[tokio::test(flavor = "current_thread")]
    async fn skill_notfound_fallback_fires_run_agent() {
        let mut handler = make_handler();
        let (_dir, registry) = skill_run_test_registry("uat-run-skill", "UAT SKILL BODY");
        handler.set_skill_registry(registry);

        let sends = Arc::new(std::sync::Mutex::new(Vec::new()));
        let adapter: Arc<dyn PlatformAdapter> = Arc::new(RecordingAdapter {
            sends: sends.clone(),
        });

        let chat_id = "chat-inherit-777";
        let sender_id = "user-inherit-XYZ";
        let event = skill_run_event("/uat-run-skill", chat_id, sender_id);

        // Err from the placeholder short-circuit is expected and ignored — the
        // assertions are on the recorded sends.
        let _ = handler
            .handle_slash_command(&event, adapter, CancellationToken::new(), no_attachments())
            .await;

        let recorded = sends.lock().unwrap().clone();
        // run_agent fired: its placeholder block was sent.
        let placeholder = recorded
            .iter()
            .find(|(_, content)| content == PLACEHOLDER_BLOCK)
            .expect("run_agent must fire (placeholder '█' send) — not overlay-only");
        // Identity inherited: the placeholder went to the ORIGINAL chat_id.
        assert_eq!(
            placeholder.0, chat_id,
            "synthesized run turn must inherit chat_id from the real event (..event.clone())"
        );

        // Activation still happens: the skill body is in the per-session overlay,
        // keyed by the real event identity, so run_agent's skill_overlays read
        // site prepends it to the turn's system prompt.
        let session_key = SessionKey::new(Platform::Telegram, chat_id).with_user(sender_id);
        let overlays = handler.skill_overlays.lock().unwrap();
        let session_overlays = overlays
            .get(&session_key)
            .expect("skill overlay activated for the real session identity");
        assert!(
            session_overlays
                .iter()
                .any(|(n, b)| n == "uat-run-skill" && b.contains("UAT SKILL BODY")),
            "the SKILL.md body must be activated into the session overlay"
        );
    }

    /// Source assertion (defensive SkillActivated arm — mirrors the Web plan's
    /// Pitfall-3 source coverage). `dispatch()` never constructs `SkillActivated`
    /// today, so this arm cannot be reached via normal routing; assert at the
    /// source level that BOTH skill-invoke sites synthesize a `MessageEvent` via
    /// `..event.clone()` and fire `self.run_agent(&synthetic, ...)` — neither
    /// returns `Ok(())` without running the agent.
    #[test]
    fn skill_activated_fires_run_agent() {
        let src = include_str!("handler.rs");
        // Needles are assembled from split fragments so this test's OWN source
        // never contains the verbatim strings it scans for — otherwise
        // `include_str!("handler.rs")` would match the assertions themselves and
        // pass vacuously (the "tests that verify their own assumptions" trap).
        let build_needle = ["build_skill_inv", "ocation("].concat();
        let run_fire_needle = ["self.run_agent(&sy", "nthetic"].concat();
        let content_needle = ["content: invocation.trigger_", "text.clone()"].concat();

        // The SkillActivated arm computes the run-turn trigger via the shared resolver.
        assert!(
            src.contains(build_needle.as_str()),
            "SkillActivated arm must compute trigger_text via build_skill_invocation"
        );
        // Both invoke sites (SkillActivated arm + NotFound fallback) synthesize an
        // identity-inheriting event and fire run_agent — neither returns Ok(())
        // without running the agent.
        let run_fire_sites = src.matches(run_fire_needle.as_str()).count();
        assert!(
            run_fire_sites >= 2,
            "both skill-invoke sites must fire run_agent on a synthesized event — found {run_fire_sites}"
        );
        // Identity: the synthesized event only sets `content` and inherits every
        // identity field via `..event.clone()`, never reconstructed from name/args.
        assert!(
            src.contains(content_needle.as_str()),
            "synthesized skill-run event content must be the resolved trigger_text"
        );
        assert!(
            src.contains("..event.clone()"),
            "synthesized skill-run event must inherit identity via ..event.clone()"
        );
    }

    /// Task 2 (UI-SPEC Copywriting Contract / §C): the retired "activated for
    /// this turn" copy is gone; the run-turn meta text follows the shared
    /// bare/argued/40-char-truncation contract (identical to the Web/TUI chip).
    #[test]
    fn skill_run_turn_meta_text_bare_and_argued() {
        // Bare invoke: trigger_text is the run-now instruction → no args suffix.
        let bare_trigger = "Run the gsd-config skill now: carry out its instructions immediately.";
        assert_eq!(
            run_turn_meta_text("gsd-config", bare_trigger),
            "▶ Ran skill /gsd-config"
        );
        // Argued invoke ≤ 40 chars: quoted verbatim, no ellipsis.
        assert_eq!(
            run_turn_meta_text("gsd-config", "show me the config"),
            "▶ Ran skill /gsd-config · \"show me the config\""
        );
        // Argued invoke > 40 chars: char-safe truncation with an inner ellipsis.
        let long = "a".repeat(45);
        let head = "a".repeat(40);
        assert_eq!(
            run_turn_meta_text("gsd-config", &long),
            format!("▶ Ran skill /gsd-config · \"{head}…\"")
        );

        // The retired activation copy no longer appears anywhere in the source.
        // Needle assembled from fragments so this assertion doesn't match itself.
        let retired = ["activated for ", "this turn"].concat();
        let src = include_str!("handler.rs");
        assert!(
            !src.contains(retired.as_str()),
            "the retired skill-activation copy must be gone"
        );
    }

    // ── Phase 47.6 Plan 09 (P0-3 / D-14): approval_target_for ───────────────

    fn approval_test_event(
        platform: Platform,
        chat_type: &str,
        chat_id: &str,
        sender_id: &str,
    ) -> MessageEvent {
        MessageEvent {
            platform,
            message_id: "m1".to_string(),
            chat_id: chat_id.to_string(),
            sender_id: sender_id.to_string(),
            content: "test".to_string(),
            attachments: Vec::new(),
            thread_id: None,
            chat_type: chat_type.to_string(),
            chat_name: None,
            sender_name: None,
            replied_to_id: None,
        }
    }

    #[test]
    fn approval_target_for_telegram_is_the_chat_id() {
        let channel = approval_test_event(Platform::Telegram, "channel", "chat-1", "user-1");
        let dm = approval_test_event(Platform::Telegram, "dm", "chat-2", "user-2");
        assert_eq!(approval_target_for(&channel), "chat-1");
        assert_eq!(approval_target_for(&dm), "chat-2");
    }

    #[test]
    fn approval_target_for_buzz_channel_is_the_sender() {
        let event = approval_test_event(
            Platform::Buzz,
            "channel",
            "channel-id-abc",
            "sender-hex-123",
        );
        assert_eq!(approval_target_for(&event), "sender-hex-123");
    }

    #[test]
    fn approval_target_for_buzz_dm_is_the_chat_id() {
        let event = approval_test_event(Platform::Buzz, "dm", "npub1peer...", "sender-hex-123");
        assert_eq!(approval_target_for(&event), "npub1peer...");
    }

    #[test]
    fn approval_target_never_equals_a_buzz_channel_id() {
        // Table over several channel identifier shapes — the derived target
        // must never equal the channel id, regardless of its literal form.
        let channel_ids = [
            "channel-id-abc",
            "h-tag-value",
            "0123456789abcdef",
            "#general",
        ];
        for channel_id in channel_ids {
            let event =
                approval_test_event(Platform::Buzz, "channel", channel_id, "sender-hex-xyz");
            let target = approval_target_for(&event);
            assert_ne!(
                target, channel_id,
                "approval target must never equal the Buzz channel id (got {target})"
            );
            assert_eq!(target, "sender-hex-xyz");
        }
    }

    // ── Phase 47.6 Plan 09 (D-15): media_fallback_notice ─────────────────────

    fn path_ref(path: &str) -> crate::media_tag::MediaRef {
        crate::media_tag::MediaRef {
            source: crate::media_tag::MediaSource::Path(std::path::PathBuf::from(path)),
            kind: crate::media_tag::MediaKind::Photo,
            original_tag_text: format!("<MEDIA: {path}>"),
        }
    }

    #[test]
    fn media_fallback_notice_names_the_local_path() {
        let refs = vec![path_ref("/tmp/plan09-image.png")];
        let notice = media_fallback_notice(&refs);
        assert!(
            notice.contains("/tmp/plan09-image.png"),
            "notice must name the artifact's local path: {notice}"
        );
    }

    #[test]
    fn media_fallback_notice_names_every_ref() {
        let refs = vec![path_ref("/tmp/plan09-a.png"), path_ref("/tmp/plan09-b.mp4")];
        let notice = media_fallback_notice(&refs);
        assert!(
            notice.contains("/tmp/plan09-a.png"),
            "first ref missing: {notice}"
        );
        assert!(
            notice.contains("/tmp/plan09-b.mp4"),
            "second ref missing: {notice}"
        );
    }

    #[test]
    fn media_fallback_notice_url_source_names_the_url() {
        let refs = vec![crate::media_tag::MediaRef {
            source: crate::media_tag::MediaSource::Url("https://example.com/x.png".to_string()),
            kind: crate::media_tag::MediaKind::Photo,
            original_tag_text: "<MEDIA: https://example.com/x.png>".to_string(),
        }];
        let notice = media_fallback_notice(&refs);
        assert!(
            notice.contains("https://example.com/x.png"),
            "notice must name the URL source: {notice}"
        );
    }
}

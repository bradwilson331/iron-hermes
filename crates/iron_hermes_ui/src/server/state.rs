//! Shared server state for the Dioxus UI backend.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use ironhermes_agent::agent_loop::{StreamCallback, ToolProgressCallback, ToolResultCallback};
use ironhermes_agent::{
    AgentRuntime, AgentRuntimeInput, MemoryManager, PressureTracker, PromptBuilder, TurnRequest,
};
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

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub command_router: Arc<CommandRouter>,
    pub state_store: Arc<std::sync::Mutex<StateStore>>,
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
    /// Phase 32.3 Plan 04 (D-08): subagent registry Arc — same handle threaded
    /// into the AgentRuntime via `subagent_registry`. Held on
    /// AppState so the four `/api/agents/*` endpoints can read it for status
    /// / prune (which need the full registry, not just the ShrikeService).
    pub subagent_registry: Arc<RwLock<ironhermes_agent::subagent_registry::SubagentRegistry>>,
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
    pub subagent_callback_slot:
        Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<crate::protocol::ChatStreamEvent>>>>,
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
        let resolver = ProviderResolver::build(&config)?;
        let command_router = Arc::new(CommandRouter::new(build_command_registry()));
        let state_store = Arc::new(std::sync::Mutex::new(
            StateStore::open_default().context("failed to open state.db for web UI")?,
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
        let progress_callback: ironhermes_tools::delegate_task::SubagentProgressCallback =
            Arc::new(move |_index: usize,
                           _event: ironhermes_tools::delegate_task::SubagentProgress| {
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
            });

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

        Ok(Self {
            config: Arc::new(config),
            command_router,
            state_store,
            resolver: Arc::new(resolver),
            runtime: Arc::new(runtime),
            memory_manager,
            nudge_turns: Arc::new(std::sync::Mutex::new(HashMap::new())),
            // Phase 32.3 Plan 04: subagent_registry + shrike — same Arcs the
            // delegate-task runner uses, so the four `/api/agents/*` endpoints
            // operate on the live registry.
            subagent_registry,
            shrike: Some(shrike),
            // Phase 26.7.1 Plan 02 (D-06 / Path A): callback slot — ws.rs
            // installs the per-turn sender before run_web_turn, RAII guard clears
            // it on return/panic.
            subagent_callback_slot,
        })
    }

    pub fn ensure_web_session(&self, session_id: &str) -> Result<()> {
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

    pub async fn run_web_turn(
        &self,
        session_id: &str,
        user_input: &str,
        stream_callback: StreamCallback,
        tool_progress_callback: Option<ToolProgressCallback>,
        tool_result_callback: Option<ToolResultCallback>,
    ) -> Result<ironhermes_agent::AgentResult> {
        let messages = self.build_messages_for_turn(session_id, user_input).await?;
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
        };
        let result = self.runtime.run_turn(request).await?;

        let mut store = self.state_store.lock().unwrap();
        for msg in &result.appended {
            let _ = store.add_message(session_id, msg);
        }
        // Drop the state_store guard explicitly so we don't hold two locks
        // when we acquire the nudge_turns Mutex below.
        drop(store);

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
    ) -> Result<Vec<ChatMessage>> {
        self.ensure_web_session(session_id)?;

        let mut prompt_builder = PromptBuilder::new(&self.config.model.default, "web")
            .with_provider(&self.config.model.provider)
            .load_context(&std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        // Phase 28.1 Plan 03: source skill_registry and merged_tools from the runtime.
        prompt_builder.set_skill_registry(self.runtime.skill_registry().clone());
        if let Some(ref manager) = self.memory_manager {
            prompt_builder.set_memory_manager(manager.clone());
        }
        prompt_builder.set_user_profile_enabled(self.config.memory.user_profile_enabled);
        // Phase 27.1.1-gap-02: populate active_toolsets so the system-prompt skills
        // catalog text reflects the same enabled set as the API tool schemas.
        prompt_builder.set_active_toolsets(
            self.runtime.merged_tools().enabled_toolset_names(),
        );
        prompt_builder.load_memory().await;
        prompt_builder.load_skills();
        let system_msg = prompt_builder.build_system_message();

        let (history_rows, user_msg) = {
            let mut store = self.state_store.lock().unwrap();
            let rows = store
                .get_messages(session_id)
                .context("failed to load persisted web messages")?;
            let user_msg = ChatMessage::user(user_input);
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
    let stale_secs = body.get("stale_secs").and_then(|v| v.as_u64()).unwrap_or(120);
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
    pub fn api_agents_kill(&self, body: serde_json::Value) -> serde_json::Value {
        api_agents_kill(self.shrike.as_deref(), body)
    }

    /// Phase 32.3 Plan 04: thin AppState wrapper over `api_agents_interrupt`.
    pub fn api_agents_interrupt(&self, body: serde_json::Value) -> serde_json::Value {
        api_agents_interrupt(self.shrike.as_deref(), body)
    }

    /// Phase 32.3 Plan 04: thin AppState wrapper over `api_agents_prune`.
    pub fn api_agents_prune(&self, body: serde_json::Value) -> serde_json::Value {
        api_agents_prune(self.shrike.as_deref(), body)
    }

    /// Phase 32.3 Plan 04: thin AppState wrapper over `api_agents_status`.
    pub fn api_agents_status(&self, id: &str) -> Option<serde_json::Value> {
        api_agents_status(self.shrike.as_deref(), id)
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
        let weak: std::sync::Weak<tokio::sync::RwLock<SubagentRegistry>> =
            std::sync::Weak::new();
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
        assert!(root["uptime_secs"].is_number(), "uptime_secs should be a number");
        assert!(root["started_at_unix_ms"].is_null(), "started_at_unix_ms should be null");

        // root.children has 1 mid node
        let root_children = root["children"].as_array().expect("root.children should be array");
        assert_eq!(root_children.len(), 1, "root should have 1 child (mid)");

        let mid = &root_children[0];
        assert_eq!(mid["id"], "sub_mid11111");
        assert_eq!(mid["task_summary"], "task for sub_mid11111");

        // mid.children has 1 leaf node
        let mid_children = mid["children"].as_array().expect("mid.children should be array");
        assert_eq!(mid_children.len(), 1, "mid should have 1 child (leaf)");

        let leaf = &mid_children[0];
        assert_eq!(leaf["id"], "sub_leaf2222");
        assert_eq!(leaf["task_summary"], "task for sub_leaf2222");
        let leaf_children = leaf["children"].as_array().expect("leaf.children should be array");
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

        assert!(resp["pruned"].is_array(), "pruned must be an array: {}", resp);
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


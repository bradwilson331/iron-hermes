//! Shared server state for the Dioxus UI backend.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use ironhermes_agent::agent_loop::{StreamCallback, ToolProgressCallback, ToolResultCallback};
use ironhermes_agent::budget::BudgetHandle;
use ironhermes_agent::{
    attach_context_engine, build_app_runtime_bundle, build_main_client, wire_fallback_if_configured,
    AgentLoop, AgentSubagentRunner, AppRuntimeBundle, AppRuntimeFactoryInput,
    DelegateTaskWiring, MemoryManager, PressureTracker, PromptBuilder,
};
use ironhermes_core::commands::registry::build_registry as build_command_registry;
use ironhermes_core::commands::CommandRouter;
use ironhermes_core::config::Config;
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
    pub runtime_bundle: Arc<AppRuntimeBundle>,
    pub memory_manager: Option<Arc<tokio::sync::Mutex<MemoryManager>>>,
    /// Per-session nudge turn counter. Arc<Mutex<HashMap>> mirrors the gateway
    /// nudge_turns pattern from Plan 32-02. Interior mutability required: run_web_turn
    /// takes &self, but the counter must mutate across calls for the same session.
    /// Session key is the same String used by run_web_turn (e.g.
    /// "agent:main:web:dm:{uuid}" produced by api.rs create_session).
    /// (Phase 32 LEARN-01 — web UI nudge wiring)
    pub nudge_turns: Arc<std::sync::Mutex<HashMap<String, u32>>>,
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

        let budget = BudgetHandle::new(config.agent.max_iterations);
        let subagent_registry = Arc::new(RwLock::new(
            ironhermes_agent::subagent_registry::SubagentRegistry::new(),
        ));
        let subagent_runner = Arc::new(
            AgentSubagentRunner::new(
                build_main_client(&resolver)?,
                resolver.clone(),
                Some(budget),
            )
            .with_subagent_registry(subagent_registry)
            .with_transcript_scope(
                ironhermes_core::constants::get_hermes_home(),
                "web-ui".to_string(),
            ),
        );

        let runtime_bundle = build_app_runtime_bundle(AppRuntimeFactoryInput {
            config: Arc::new(config.clone()),
            resolver: Arc::new(resolver.clone()),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            process_registry: process_registry.clone(),
            memory_manager: memory_manager
                .clone()
                .map(|m| m as ironhermes_tools::memory_tool::SharedMemoryManager),
            delegate_task: Some(DelegateTaskWiring {
                runner: subagent_runner,
                semaphore: Arc::new(tokio::sync::Semaphore::new(config.delegation.max_concurrent_children)),
                config: config.delegation.clone(),
                cancel_token: None,
                progress_callback: None,
            }),
            hooks_config: HooksConfig::load().unwrap_or_default(),
            emit_mcp_startup_logs: false,
        })
        .await
        .context("building shared app runtime bundle for web UI")?;

        Ok(Self {
            config: Arc::new(config),
            command_router,
            state_store,
            resolver: Arc::new(resolver),
            runtime_bundle: Arc::new(runtime_bundle),
            memory_manager,
            nudge_turns: Arc::new(std::sync::Mutex::new(HashMap::new())),
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
        let mut agent = self.build_agent_loop(stream_callback, tool_progress_callback)?;
        if let Some(cb) = tool_result_callback {
            agent = agent.with_tool_result(cb);
        }
        let result = agent.run(messages).await?;

        let mut store = self.state_store.lock().unwrap();
        for msg in &result.appended {
            let _ = store.add_message(session_id, msg);
        }
        // Drop the state_store guard explicitly so we don't hold two locks
        // when we acquire the nudge_turns Mutex below.
        drop(store);

        // Phase 32 LEARN-01: periodic memory nudge (turn-based, post-response).
        // Fires AFTER agent.run() succeeded and AFTER result.appended is persisted.
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
                    let client_clone = build_main_client(&self.resolver)?;
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

    pub fn build_agent_loop(
        &self,
        stream_callback: StreamCallback,
        tool_progress_callback: Option<ToolProgressCallback>,
    ) -> Result<AgentLoop> {
        let client = build_main_client(&self.resolver)?;
        let max_turns = self.config.agent.max_turns;
        let context_length = self.resolver.resolve_for_main().context_length();

        let mut agent = AgentLoop::new(client, self.runtime_bundle.registry.clone(), max_turns)
            .with_streaming(stream_callback)
            .with_hook_registry(self.runtime_bundle.hook_registry.clone())
            .with_compression(context_length, self.config.agent.context_compression)
            .with_intercepts(None, Some(self.state_store.clone()), None, None, None)
            .with_browser_session(self.runtime_bundle.browser_session.clone());

        if let Some(cb) = tool_progress_callback {
            agent = agent.with_tool_progress(cb);
        }

        // Wire fallback via shared helper (adds warn! on misconfiguration — PROV-07 / phase 27.1.4.1)
        agent = wire_fallback_if_configured(agent, &self.resolver);

        Ok(attach_context_engine(
            agent,
            &self.config,
            &self.resolver,
            "web-ui",
            Some(self.runtime_bundle.hook_registry.clone()),
            Some(Arc::new(PressureTracker::new())),
            context_length,
            self.memory_manager.clone(),
        ))
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
        prompt_builder.set_skill_registry(self.runtime_bundle.skill_registry.clone());
        if let Some(ref manager) = self.memory_manager {
            prompt_builder.set_memory_manager(manager.clone());
        }
        prompt_builder.set_user_profile_enabled(self.config.memory.user_profile_enabled);
        // Phase 27.1.1-gap-02: populate active_toolsets so the system-prompt skills
        // catalog text reflects the same enabled set as the API tool schemas.
        prompt_builder.set_active_toolsets(
            self.runtime_bundle.merged_tools.enabled_toolset_names(),
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
    /// and is not re-exposed through `AppRuntimeBundle`.
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

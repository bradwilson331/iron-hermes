//! Shared server state for the Dioxus UI backend.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use ironhermes_agent::agent_loop::{StreamCallback, ToolProgressCallback, ToolResultCallback};
use ironhermes_agent::budget::BudgetHandle;
use ironhermes_agent::{
    attach_context_engine, build_app_runtime_bundle, build_client as build_provider_client,
    build_main_client, AgentLoop, AgentSubagentRunner, AppRuntimeBundle, AppRuntimeFactoryInput,
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
                semaphore: Arc::new(tokio::sync::Semaphore::new(config.subagent.max_subagents)),
                config: config.subagent.clone(),
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
        let mut agent = self.build_agent_loop(stream_callback, tool_progress_callback)?;
        if let Some(cb) = tool_result_callback {
            agent = agent.with_tool_result(cb);
        }
        let result = agent.run(messages).await?;

        let mut store = self.state_store.lock().unwrap();
        for msg in &result.appended {
            let _ = store.add_message(session_id, msg);
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

        let main_endpoint = self.resolver.resolve_for_main();
        if let Some(fallback_name) = main_endpoint.fallback_providers.first() {
            if let Ok(fallback_client) =
                build_provider_client(&self.resolver, fallback_name, &main_endpoint.default_model)
            {
                agent = agent.with_fallback(fallback_client);
            }
        }

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

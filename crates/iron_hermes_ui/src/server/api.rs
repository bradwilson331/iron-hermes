//! Server functions for the Dioxus UI.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SessionInfo {
    pub id: String,
    pub title: Option<String>,
    pub created_at: String,
    pub message_count: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SlashCommandInfo {
    pub name: String,
    pub description: String,
    pub category: String,
    pub aliases: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ConfigSummary {
    pub model: String,
    pub provider: String,
    pub context_length: u32,
    pub memory_enabled: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
}

#[get("/api/commands")]
pub async fn list_slash_commands() -> Result<Vec<SlashCommandInfo>> {
    let state = crate::server::state::global_app_state();
    let commands = state
        .command_router
        .commands
        .iter()
        .map(|def| SlashCommandInfo {
            name: def.name.to_string(),
            description: def.description.to_string(),
            category: format!("{:?}", def.category),
            aliases: def.aliases.iter().map(|a| a.to_string()).collect(),
        })
        .collect();
    Ok(commands)
}

#[get("/api/sessions")]
pub async fn list_sessions() -> Result<Vec<SessionInfo>> {
    let state = crate::server::state::global_app_state();
    let sessions = state
        .state_store
        .lock()
        .unwrap()
        .list_sessions(
            Some(&ironhermes_core::types::Platform::Web.to_string()),
            100,
        )
        .map_err(|e| ServerFnError::new(format!("StateStore list sessions failed: {e}")))?;

    let out = sessions
        .into_iter()
        .map(|session| SessionInfo {
            id: session.id,
            title: session.title,
            created_at: format!("{}", session.started_at as i64),
            message_count: session.message_count.max(0) as u32,
        })
        .collect();
    Ok(out)
}

#[get("/api/config")]
pub async fn get_config_summary() -> Result<ConfigSummary> {
    let state = crate::server::state::global_app_state();
    let cfg = state.config.clone();
    let context_length = state.resolver.resolve_for_main().context_length() as u32;
    Ok(ConfigSummary {
        model: cfg.model.default.clone(),
        provider: cfg.model.provider.clone(),
        context_length,
        memory_enabled: cfg.memory.memory_enabled,
    })
}

#[get("/api/tools")]
pub async fn list_tools() -> Result<Vec<ToolInfo>> {
    let state = crate::server::state::global_app_state();
    let definitions = state
        .runtime_bundle
        .registry
        .read()
        .await
        .get_definitions(None);
    let out = definitions
        .into_iter()
        .map(|def| ToolInfo {
            name: def.function.name,
            description: def.function.description,
        })
        .collect();
    Ok(out)
}

#[post("/api/sessions/create")]
pub async fn create_session() -> Result<String> {
    let state = crate::server::state::global_app_state();
    let session_uuid = uuid::Uuid::new_v4().to_string();
    let session_key = format!("agent:main:web:dm:{session_uuid}");

    state
        .ensure_web_session(&session_key)
        .map_err(|e| ServerFnError::new(format!("Session creation failed: {e}")))?;

    Ok(session_key)
}

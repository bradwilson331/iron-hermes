//! Server functions for the Dioxus UI.
//!
//! Each `#[get]` / `#[post]` function compiles to an API endpoint on the server
//! and an HTTP call on the client. Per CONTEXT D-01: server functions handle
//! commands, config queries, session listing, and one-shot ops.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Minimal session info sent to the client.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SessionInfo {
    pub id: String,
    pub title: Option<String>,
    pub created_at: String,
    pub message_count: u32,
}

/// Slash command definition sent to the client.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SlashCommandInfo {
    pub name: String,
    pub description: String,
    pub category: String,
    pub aliases: Vec<String>,
}

/// Config summary sent to the client.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ConfigSummary {
    pub model: String,
    pub provider: String,
    pub context_length: u32,
}

/// Tool schema info sent to the client.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
}

/// List all slash commands from CommandRouter (for command palette).
#[get("/api/commands")]
pub async fn list_slash_commands() -> Result<Vec<SlashCommandInfo>> {
    let registry = ironhermes_core::commands::registry::build_registry();
    let commands: Vec<SlashCommandInfo> = registry
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

/// List sessions from StateStore (for title bar tabs).
#[get("/api/sessions")]
pub async fn list_sessions() -> Result<Vec<SessionInfo>> {
    // TODO: Wire to real StateStore — requires AppState from server context.
    // For now return empty; Plan 03 will wire this with proper state injection.
    Ok(vec![])
}

/// Get current config summary (for status bar).
#[get("/api/config")]
pub async fn get_config_summary() -> Result<ConfigSummary> {
    let config = ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
    let model = config.model.default.clone();
    let provider = config.model.provider.clone();
    let context_length = config.model.context_length.unwrap_or(128_000) as u32;
    Ok(ConfigSummary {
        model,
        provider,
        context_length,
    })
}

/// List available tools (for command palette/agent panel).
#[get("/api/tools")]
pub async fn list_tools() -> Result<Vec<ToolInfo>> {
    // TODO: Wire to real ToolRegistry — requires AppState.
    // Plan 03 will wire this.
    Ok(vec![])
}

/// Create a new web UI session with Platform::Web session key.
///
/// Session key format: `agent:main:web:dm:{session_uuid}`
/// per the pattern established by CLI and gateway paths.
#[post("/api/sessions/create")]
pub async fn create_session() -> Result<String> {
    let session_uuid = uuid::Uuid::new_v4().to_string();
    let session_key = format!("agent:main:web:dm:{session_uuid}");

    // Create session in StateStore with Platform::Web source.
    let config = ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
    let home = ironhermes_core::constants::get_hermes_home();
    let db_path = home.join("state.db");
    let mut state_store = ironhermes_state::StateStore::new(&db_path)
        .map_err(|e| ServerFnError::new(format!("StateStore init failed: {e}")))?;

    let platform = ironhermes_core::types::Platform::Web;
    state_store
        .create_session(
            &session_key,
            &platform.to_string(),
            Some(&config.model.default),
            None, // system_prompt — built per-turn
            None, // parent_session_id
            None, // workspace_root — web sessions have no cwd context
        )
        .map_err(|e| ServerFnError::new(format!("Session creation failed: {e}")))?;

    Ok(session_key)
}

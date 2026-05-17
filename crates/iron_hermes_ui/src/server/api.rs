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

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MemoryEntry {
    /// "agent" for MEMORY.md, "user" for USER.md.
    pub store: String,
    /// Raw text block from MemoryEntries Vec<String> — one row per block.
    pub body: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct MemoryInfo {
    pub entries: Vec<MemoryEntry>,
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
        // GAP-26.2.1-09 / D-26.2.1-13-A (USER-APPROVED 2026-05-14):
        // Pass None instead of Some(Platform::Web) so the SESSIONS wedge sources
        // the full cross-platform on-disk session catalog (~/.ironhermes/sessions/).
        // See .planning/phases/26.2.1-new-web-ui-with-wheel-menu/26.2.1-UAT.md
        // §"Round-2 D-02 Decision" for the user authorization quote.
        .list_sessions(
            None,
            100,
        )
        .map_err(|e| ServerFnError::new(format!("StateStore list sessions failed: {e}")))?;

    let out = sessions
        .into_iter()
        // GAP-26.2.1-09-R3 / D-26.2.1-14-B (USER CHOSE OPTION A, 2026-05-14):
        // Drop non-loadable sessions. The D-26.2.1-13-A surgical lift broadened
        // the listing to ~74 entries from ~/.ironhermes/sessions/, but some
        // are foreign-format directories (only trajectories.jsonl, no SQLite
        // messages row → message_count == 0) that the chat-load path cannot
        // populate. Filtering message_count > 0 yields ONLY sessions the
        // SESSIONS wedge can actually open. See 26.2.1-UAT.md round-3 notes.
        .filter(|s| s.message_count > 0)
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

/// Phase 26.7 Plan 02 (D-07, D-08, R-3): Read-only snapshot of MemoryManager
/// content for the Memory screen. Returns one MemoryEntry per raw text block in
/// each MemoryTarget store. Em-dash placeholder for timestamps (no per-block
/// timestamp exists in the underlying `Vec<String>` storage).
#[get("/api/memory")]
pub async fn get_memory() -> Result<MemoryInfo> {
    let state = crate::server::state::global_app_state();
    let Some(ref mgr) = state.memory_manager else {
        // Memory disabled at runtime — render empty panels, not an error.
        return Ok(MemoryInfo::default());
    };
    // tokio Mutex .lock().await.to_memory_entries().await chained; guard
    // drops at the `;`. Pattern mirrors list_tools (api.rs:107-112).
    let entries = mgr.lock().await.to_memory_entries().await;

    let mut out: Vec<MemoryEntry> = Vec::new();
    for (target, items) in entries.entries.iter() {
        let store = match target {
            ironhermes_core::memory_store::MemoryTarget::Memory => "agent",
            ironhermes_core::memory_store::MemoryTarget::User   => "user",
        };
        for body in items.iter() {
            out.push(MemoryEntry {
                store: store.to_string(),
                body: body.clone(),
            });
        }
    }
    Ok(MemoryInfo { entries: out })
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

// =============================================================================
// Phase 32.3 Plan 04: /api/agents/{kill,interrupt,prune,status} REST endpoints
// =============================================================================
//
// Four Dioxus server functions wrapping the free fns in `state.rs`. They
// follow the established `#[get(...)]` / `#[post(...)]` pattern in this file
// (list_slash_commands, list_sessions, etc.) — iron_hermes_ui's REST surface
// is Dioxus server functions, not a separate Axum routes.rs (main.rs has only
// one Axum mount point: `serve_dioxus_application`).
//
// **No confirm token on the web surface** (Phase 32.3 D-09 — only the
// Telegram gateway enforces `confirm` because of spoof-replay risk; TUI and
// web both have synchronous user presence).

/// Phase 32.3 Plan 04 (D-08): `POST /api/agents/kill` body `{"id": "sub_xxx"}`.
/// Returns the JSON shape documented in `state::api_agents_kill`.
#[post("/api/agents/kill")]
pub async fn api_agents_kill(id: String) -> Result<serde_json::Value> {
    let state = crate::server::state::global_app_state();
    Ok(state.api_agents_kill(serde_json::json!({ "id": id })))
}

/// Phase 32.3 Plan 04 (D-08): `POST /api/agents/interrupt` body `{"id": "sub_xxx"}`.
#[post("/api/agents/interrupt")]
pub async fn api_agents_interrupt(id: String) -> Result<serde_json::Value> {
    let state = crate::server::state::global_app_state();
    Ok(state.api_agents_interrupt(serde_json::json!({ "id": id })))
}

/// Phase 32.3 Plan 04 (D-08): `POST /api/agents/prune` body `{"stale_secs": 120}`
/// (optional; defaults to 120 to match `SubagentConfig::stale_warn_seconds`).
#[post("/api/agents/prune")]
pub async fn api_agents_prune(stale_secs: Option<u64>) -> Result<serde_json::Value> {
    let state = crate::server::state::global_app_state();
    let body = match stale_secs {
        Some(s) => serde_json::json!({ "stale_secs": s }),
        None => serde_json::json!({}),
    };
    Ok(state.api_agents_prune(body))
}

/// Phase 32.3 Plan 04 (D-08): `GET /api/agents/status?id=sub_xxx`.
/// Returns the `SubagentStatusInfo` JSON (Serialize derived in Plan 03), or
/// a `not_found` error when the id is unknown.
#[get("/api/agents/status")]
pub async fn api_agents_status(id: String) -> Result<serde_json::Value> {
    let state = crate::server::state::global_app_state();
    Ok(state
        .api_agents_status(&id)
        .ok_or_else(|| ServerFnError::new(format!("subagent {} not found", id)))?)
}

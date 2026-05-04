//! Shared server state for the Dioxus UI backend.
//!
//! `AppState` wraps the IronHermes subsystems needed by server functions
//! and WebSocket handlers. Constructed once at startup in main.rs and
//! injected into Axum via Extension.

use std::sync::Arc;
use tokio::sync::RwLock;
use ironhermes_core::config::Config;
use ironhermes_core::commands::CommandRouter;
use ironhermes_state::StateStore;
use ironhermes_tools::registry::ToolRegistry;

/// Shared state for server functions. Cloned into each request handler.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub command_router: Arc<CommandRouter>,
    pub state_store: Arc<RwLock<StateStore>>,
    pub tool_registry: Arc<RwLock<ToolRegistry>>,
}

impl AppState {
    /// Bootstrap AppState from the standard IronHermes config paths.
    /// Called once at server startup.
    pub async fn init() -> anyhow::Result<Self> {
        use ironhermes_core::constants::get_hermes_home;

        let config = Config::load()?;
        let command_router = ironhermes_core::commands::registry::build_registry();
        let command_router = CommandRouter::new(command_router);

        let home = get_hermes_home();
        let db_path = home.join("state.db");
        let state_store = StateStore::new(&db_path)?;
        let tool_registry = ToolRegistry::new();

        Ok(Self {
            config: Arc::new(config),
            command_router: Arc::new(command_router),
            state_store: Arc::new(RwLock::new(state_store)),
            tool_registry: Arc::new(RwLock::new(tool_registry)),
        })
    }
}

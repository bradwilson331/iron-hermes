//! Shared server state for the Dioxus UI backend.
//!
//! `AppState` wraps the IronHermes subsystems needed by server functions
//! and WebSocket handlers. Constructed once at startup in main.rs and
//! injected into Axum via Extension.
//!
//! Phase 25.5 Plan 04: Added `ProviderResolver` and `build_agent_loop()`
//! method for real AgentLoop dispatch from WebSocket handler.

use std::sync::Arc;
use tokio::sync::RwLock;
use ironhermes_core::config::Config;
use ironhermes_core::commands::CommandRouter;
use ironhermes_core::ProviderResolver;
use ironhermes_state::StateStore;
use ironhermes_tools::registry::ToolRegistry;

/// Shared state for server functions. Cloned into each request handler.
///
/// `StateStore` wraps `rusqlite::Connection` which is `!Sync`, so we use
/// `std::sync::Mutex` (not `tokio::sync::RwLock`) — same pattern as
/// `ironhermes-cli/src/main.rs` line 2163.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub command_router: Arc<CommandRouter>,
    pub state_store: Arc<std::sync::Mutex<StateStore>>,
    pub tool_registry: Arc<RwLock<ToolRegistry>>,
    pub resolver: Arc<ProviderResolver>,
}

impl AppState {
    /// Bootstrap AppState from the standard IronHermes config paths.
    /// Called once at server startup.
    pub async fn init() -> anyhow::Result<Self> {
        use ironhermes_core::constants::get_hermes_home;

        let config = Config::load()?;
        let resolver = ProviderResolver::build(&config)?;
        let command_router = ironhermes_core::commands::registry::build_registry();
        let command_router = CommandRouter::new(command_router);

        let home = get_hermes_home();
        let db_path = home.join("state.db");
        let state_store = StateStore::new(&db_path)?;
        let tool_registry = ToolRegistry::new();

        Ok(Self {
            config: Arc::new(config),
            command_router: Arc::new(command_router),
            state_store: Arc::new(std::sync::Mutex::new(state_store)),
            tool_registry: Arc::new(RwLock::new(tool_registry)),
            resolver: Arc::new(resolver),
        })
    }

    /// Build a new AgentLoop with streaming callback for a single agent turn.
    ///
    /// Follows the same wiring pattern as `run_single` in `ironhermes-cli/src/main.rs`
    /// but simplified for the web UI: single-turn, streaming via callback, no
    /// TUI/REPL concerns.
    pub fn build_agent_loop(
        &self,
        stream_callback: ironhermes_agent::agent_loop::StreamCallback,
        tool_progress_callback: Option<ironhermes_agent::agent_loop::ToolProgressCallback>,
    ) -> anyhow::Result<ironhermes_agent::AgentLoop> {
        let client = ironhermes_agent::build_main_client(&self.resolver)?;
        let max_turns = self.config.agent.max_turns;
        let context_length = self.resolver.resolve_for_main().context_length() as usize;

        let mut agent = ironhermes_agent::AgentLoop::new(
            client,
            self.tool_registry.clone(),
            max_turns,
        )
        .with_streaming(stream_callback)
        .with_compression(context_length, self.config.agent.context_compression);

        if let Some(cb) = tool_progress_callback {
            agent = agent.with_tool_progress(cb);
        }

        Ok(agent)
    }
}

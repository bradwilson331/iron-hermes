use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::skills::SkillRegistry;
use crate::types::Platform;

// =============================================================================
// McpReloader trait — D-15 (trait-object form, avoids circular dep)
// =============================================================================

/// Result of an MCP reload operation (D-12).
///
/// The `failed` field carries `(server_name, error_message)` tuples sourced from
/// `ServerTaskResult.failure_reason` (Plan 03 data contract). Used by the REPL
/// loop to format partial failure messages per UI-SPEC.
pub struct McpReloadResult {
    pub connected: Vec<String>,
    /// `(server_name, sanitized_error)` — populated from `ServerTaskResult.failure_reason`.
    pub failed: Vec<(String, String)>,
    pub tool_count: usize,
}

/// Trait for reloading MCP server connections (D-15, trait-object form).
///
/// Defined in `ironhermes-core` to avoid a circular dependency with
/// `ironhermes-mcp`. `McpManager` implements this in `ironhermes-mcp`.
/// Follows the `MemoryManagerHandle` pattern from Phase 20.
#[async_trait::async_trait]
pub trait McpReloader: Send + Sync {
    /// Reload all MCP connections: disconnect, re-read config, reconnect, re-discover.
    /// Returns `McpReloadResult` with connected/failed server lists (D-12).
    async fn reload(&self) -> McpReloadResult;
    /// Return names of currently connected servers.
    fn connected_server_names(&self) -> Vec<String>;
    /// Count of registered MCP tools.
    async fn registered_tool_count(&self) -> usize;
}

// =============================================================================
// CommandContext
// =============================================================================

/// Context passed to every command handler.
///
/// Keeps ironhermes-core as a leaf crate by only including deps
/// that live in core itself. CLI and gateway extend context at
/// their integration layer before calling dispatch().
pub struct CommandContext {
    // Required — always available
    pub platform: Platform,
    pub session_id: String,
    pub agent_running: Arc<AtomicBool>,

    // Optional — platform-dependent or not always wired
    pub skill_registry: Option<Arc<SkillRegistry>>,
    /// MCP reload capability (D-15, trait object to avoid circular dep with ironhermes-mcp).
    pub mcp_reloader: Option<Arc<dyn McpReloader>>,
}

impl CommandContext {
    /// Create a minimal context with all optional fields set to None.
    pub fn new(
        platform: Platform,
        session_id: String,
        agent_running: Arc<AtomicBool>,
    ) -> Self {
        Self {
            platform,
            session_id,
            agent_running,
            skill_registry: None,
            mcp_reloader: None,
        }
    }

    /// Builder: attach a skill registry.
    pub fn with_skill_registry(mut self, registry: Arc<SkillRegistry>) -> Self {
        self.skill_registry = Some(registry);
        self
    }

    /// Builder: attach an MCP reloader (D-15).
    pub fn with_mcp_reloader(mut self, reloader: Arc<dyn McpReloader>) -> Self {
        self.mcp_reloader = Some(reloader);
        self
    }
}

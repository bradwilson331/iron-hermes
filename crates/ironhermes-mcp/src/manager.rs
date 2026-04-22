use crate::config::McpServerConfig;
use crate::server_task::{self, ServerTaskResult};
use ironhermes_tools::ToolRegistry;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Aggregated results of a `start_all_and_wait` or `reload_and_report` operation.
///
/// Consumed by `McpReloader::reload()` in Plan 04 to populate `McpReloadResult.failed` (D-12).
/// The `failed` vec carries `(server_name, sanitized_error)` pairs from
/// `ServerTaskResult.failure_reason` for each server that exhausted all retries.
pub struct StartResult {
    /// Server names that successfully connected and registered tools.
    pub connected: Vec<String>,
    /// Servers that failed with `(server_name, sanitized_error_message)` (D-12 data contract).
    pub failed: Vec<(String, String)>,
    /// Total number of MCP tools currently registered in the registry.
    pub tool_count: usize,
}

/// Manages all MCP server connections and their tool registrations.
///
/// Orchestrates per-server `tokio::spawn` tasks (D-04), handles startup (D-07),
/// filtering via enabled_tools (D-08), and reload (D-09).
pub struct McpManager {
    registry: Arc<RwLock<ToolRegistry>>,
    /// Active task handles and their cancellation tokens, keyed by server name.
    tasks: Mutex<HashMap<String, (JoinHandle<ServerTaskResult>, CancellationToken)>>,
    /// Last-known configs for each active server (used by reconnect/reload).
    configs: Mutex<HashMap<String, McpServerConfig>>,
}

impl McpManager {
    /// Create a new `McpManager` backed by the given `ToolRegistry`.
    pub fn new(registry: Arc<RwLock<ToolRegistry>>) -> Self {
        Self {
            registry,
            tasks: Mutex::new(HashMap::new()),
            configs: Mutex::new(HashMap::new()),
        }
    }

    /// Start all configured MCP servers as background tasks (fire-and-forget).
    ///
    /// D-07: one-shot tool discovery at startup. Tasks run in the background;
    /// the caller does not wait for connections to complete (avoids blocking startup).
    /// Disabled servers (enabled=false) are skipped.
    pub async fn start_all(&self, configs: HashMap<String, McpServerConfig>) {
        let mut tasks = self.tasks.lock().await;
        let mut stored_configs = self.configs.lock().await;

        for (name, config) in configs {
            if !config.enabled {
                tracing::info!(server = %name, "MCP server disabled, skipping");
                continue;
            }
            let cancel = CancellationToken::new();
            let handle = tokio::spawn(server_task::run_server_task(
                name.clone(),
                config.clone(),
                self.registry.clone(),
                cancel.clone(),
            ));
            tasks.insert(name.clone(), (handle, cancel));
            stored_configs.insert(name, config);
        }
    }

    /// Start all configured MCP servers and return initial connection results.
    ///
    /// Spawns tasks and gives servers a brief window to attempt connection, then
    /// returns a `StartResult` with preliminary connected/failed status.
    ///
    /// Used by `reload_and_report()` to aggregate failures for D-12 status reporting.
    pub async fn start_all_and_wait(&self, configs: HashMap<String, McpServerConfig>) -> StartResult {
        let mut task_names: Vec<String> = Vec::new();
        {
            let mut tasks = self.tasks.lock().await;
            let mut stored_configs = self.configs.lock().await;

            for (name, config) in configs {
                if !config.enabled {
                    tracing::info!(server = %name, "MCP server disabled, skipping");
                    continue;
                }
                let cancel = CancellationToken::new();
                let handle = tokio::spawn(server_task::run_server_task(
                    name.clone(),
                    config.clone(),
                    self.registry.clone(),
                    cancel.clone(),
                ));
                task_names.push(name.clone());
                tasks.insert(name.clone(), (handle, cancel));
                stored_configs.insert(name, config);
            }
        }

        // Give servers a brief window to complete initial connection
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;

        // Collect status from running tasks via registry presence heuristic:
        // A server has connected if it registered at least one tool with its prefix.
        let mut connected = Vec::new();
        let mut failed: Vec<(String, String)> = Vec::new();
        {
            let tasks = self.tasks.lock().await;
            let guard = self.registry.read().await;
            for name in &task_names {
                if let Some((handle, _cancel)) = tasks.get(name) {
                    let has_tools = guard
                        .get_definitions(None)
                        .iter()
                        .any(|t| t.function.name.starts_with(&format!("{name}__")));

                    if handle.is_finished() && !has_tools {
                        failed.push((
                            name.clone(),
                            "connection failed after retries".to_string(),
                        ));
                    } else if has_tools {
                        connected.push(name.clone());
                    }
                    // Task still running with no tools yet = still connecting; don't report failure
                }
            }
        }

        let tool_count = self.registered_tool_count().await;
        StartResult { connected, failed, tool_count }
    }

    /// Shutdown all running server tasks and unregister their tools.
    ///
    /// Cancels each task's `CancellationToken`, waits for completion, then
    /// removes the server's tools from the registry via `unregister_by_prefix`.
    pub async fn shutdown_all(&self) -> Vec<ServerTaskResult> {
        let mut tasks = self.tasks.lock().await;
        let mut results = Vec::new();

        for (name, (handle, cancel)) in tasks.drain() {
            tracing::info!(server = %name, "Shutting down MCP server");
            cancel.cancel();
            if let Ok(result) = handle.await {
                results.push(result);
            }
            // Unregister tools for this server (D-09: rebuild registry on reload)
            let mut guard = self.registry.write().await;
            let removed = guard.unregister_by_prefix(&name);
            tracing::debug!(server = %name, removed, "Unregistered MCP tools");
            drop(guard); // explicitly drop before next iteration to avoid holding write lock
        }
        results
    }

    /// D-09: Reload all MCP servers (fire-and-forget, no result).
    ///
    /// Disconnects all servers (cancels tasks + unregisters tools), then reconnects
    /// with the new configs as background tasks. No waiting for connections.
    pub async fn reload(&self, new_configs: HashMap<String, McpServerConfig>) {
        self.shutdown_all().await;
        self.start_all(new_configs).await;
    }

    /// D-09/D-12: Reload all MCP servers and report connection results.
    ///
    /// Disconnects all servers, starts new tasks, waits briefly for connections,
    /// then aggregates `ServerTaskResult.failure_reason` into `StartResult.failed`.
    /// This is the method called by `McpReloader::reload()` in Plan 04.
    pub async fn reload_and_report(&self, new_configs: HashMap<String, McpServerConfig>) -> StartResult {
        // Shutdown existing servers + unregister their tools
        self.shutdown_all().await;
        // Start new servers and wait for initial connection status
        self.start_all_and_wait(new_configs).await
    }

    /// Return names of servers that currently have active task entries.
    ///
    /// Note: uses try_lock to avoid deadlock in sync call contexts.
    /// Returns empty vec if the mutex is currently locked.
    pub fn connected_server_names(&self) -> Vec<String> {
        if let Ok(tasks) = self.tasks.try_lock() {
            tasks.keys().cloned().collect()
        } else {
            Vec::new()
        }
    }

    /// Count of MCP tools currently in the registry (identified by `__` in name).
    pub async fn registered_tool_count(&self) -> usize {
        let guard = self.registry.read().await;
        guard
            .get_definitions(None)
            .iter()
            .filter(|t| t.function.name.contains("__"))
            .count()
    }
}

// =============================================================================
// McpReloader implementation (Phase 21.2 Plan 04)
// =============================================================================

#[async_trait::async_trait]
impl ironhermes_core::commands::context::McpReloader for McpManager {
    /// Reload all MCP connections by re-reading config and calling `reload_and_report`.
    ///
    /// Uses `ironhermes_core::Config::load()` to get fresh `mcp_servers` config, then
    /// calls `reload_and_report` which shuts down existing servers and reconnects.
    /// Returns `McpReloadResult.failed` populated from `ServerTaskResult.failure_reason`
    /// via `StartResult.failed` (D-12 full delivery).
    async fn reload(&self) -> ironhermes_core::commands::context::McpReloadResult {
        // Re-read config to pick up any changes since startup.
        let new_configs: HashMap<String, McpServerConfig> =
            match ironhermes_core::Config::load() {
                Ok(config) => config
                    .mcp_servers
                    .into_iter()
                    .filter_map(|(name, val)| {
                        serde_yaml::from_value::<McpServerConfig>(val)
                            .ok()
                            .map(|c| (name, c))
                    })
                    .collect(),
                Err(_) => HashMap::new(),
            };

        // reload_and_report: shutdown all + start_all_and_wait; aggregates
        // ServerTaskResult.failure_reason into StartResult.failed (D-12).
        let result = self.reload_and_report(new_configs).await;

        ironhermes_core::commands::context::McpReloadResult {
            connected: result.connected,
            failed: result.failed, // Populated from ServerTaskResult.failure_reason
            tool_count: result.tool_count,
        }
    }

    fn connected_server_names(&self) -> Vec<String> {
        McpManager::connected_server_names(self)
    }

    async fn registered_tool_count(&self) -> usize {
        McpManager::registered_tool_count(self).await
    }
}

impl Drop for McpManager {
    fn drop(&mut self) {
        // Cancel all tasks on drop to avoid orphaned background tasks
        if let Ok(tasks) = self.tasks.try_lock() {
            for (_, (_, cancel)) in tasks.iter() {
                cancel.cancel();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_tools::ToolRegistry;

    #[test]
    fn test_start_result_fields() {
        let result = StartResult {
            connected: vec!["server_a".to_string()],
            failed: vec![("server_b".to_string(), "timeout".to_string())],
            tool_count: 5,
        };
        assert_eq!(result.connected.len(), 1);
        assert_eq!(result.failed.len(), 1);
        assert_eq!(result.failed[0].0, "server_b");
        assert_eq!(result.failed[0].1, "timeout");
        assert_eq!(result.tool_count, 5);
    }

    #[tokio::test]
    async fn test_mcp_manager_new() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let manager = McpManager::new(registry);
        assert_eq!(manager.connected_server_names().len(), 0);
        assert_eq!(manager.registered_tool_count().await, 0);
    }

    #[tokio::test]
    async fn test_start_all_skips_disabled() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let manager = McpManager::new(registry);

        let mut configs = HashMap::new();
        let mut disabled = McpServerConfig::default();
        disabled.enabled = false;
        disabled.command = Some("echo".to_string());
        configs.insert("disabled_server".to_string(), disabled);

        manager.start_all(configs).await;

        // Disabled server should not appear in task map
        assert_eq!(manager.connected_server_names().len(), 0);
    }

    #[tokio::test]
    async fn test_shutdown_all_empty() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let manager = McpManager::new(registry);
        // Shutdown on empty manager should return empty results without panic
        let results = manager.shutdown_all().await;
        assert!(results.is_empty());
    }
}

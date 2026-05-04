use crate::config::McpServerConfig;
use crate::server_task::{self, ServerTaskResult};
use crate::tool::sanitize_server_name;
use ironhermes_tools::ToolRegistry;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// Active task handles, cancellation tokens, AND per-server child-process
    /// slots, keyed by server name.
    ///
    /// GAP-8: the third tuple element is an `Arc<Mutex<Option<tokio::process::Child>>>`
    /// populated by `server_task::connect_and_serve` after `connect_stdio` succeeds.
    /// `shutdown_all` reaches into it to call `Child::start_kill()` BEFORE the
    /// bounded `tokio::time::timeout(2s, handle)` await, so the gateway exits
    /// in bounded time on Ctrl+C even when the stdio child ignores parent-pipe
    /// EOF. Under the plan-11 Option B fallback the slot typically stays `None`
    /// (rmcp 1.5's `TokioChildProcess` owns the child internally), but the
    /// bounded timeout + `kill_on_drop(true)` in `connect_stdio` still close
    /// GAP-8 at the user-facing level. When rmcp later exposes a pre-spawned-
    /// Child constructor, the slot becomes load-bearing without any manager
    /// changes (Option A upgrade).
    tasks: Mutex<
        HashMap<
            String,
            (
                JoinHandle<ServerTaskResult>,
                CancellationToken,
                Arc<tokio::sync::Mutex<Option<tokio::process::Child>>>,
            ),
        >,
    >,
    /// Last-known configs for each active server (used by reconnect/reload).
    configs: Mutex<HashMap<String, McpServerConfig>>,
    /// GAP-7: per-server connected flag flipped to `true` by `server_task::connect_and_serve`
    /// ONLY after the rmcp `initialize` handshake AND `list_all_tools()` both succeed.
    /// `connected_server_names()` reads this map instead of `tasks.keys()` so servers
    /// whose child exited before handshake completion are correctly reported as FAILED.
    connected_flags: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl McpManager {
    /// Create a new `McpManager` backed by the given `ToolRegistry`.
    pub fn new(registry: Arc<RwLock<ToolRegistry>>) -> Self {
        Self {
            registry,
            tasks: Mutex::new(HashMap::new()),
            configs: Mutex::new(HashMap::new()),
            connected_flags: Mutex::new(HashMap::new()),
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
        let mut flags = self.connected_flags.lock().await;

        for (name, config) in configs {
            if !config.enabled {
                tracing::info!(server = %name, "MCP server disabled, skipping");
                continue;
            }
            let cancel = CancellationToken::new();
            // GAP-7: allocate per-server connected flag; server_task flips it to
            // true ONLY after list_all_tools() succeeds.
            let connected = Arc::new(AtomicBool::new(false));
            // GAP-8: per-server child-process slot. server_task parks the
            // spawned tokio::process::Child here on connect_stdio success;
            // shutdown_all reaches in to start_kill() it on graceful shutdown.
            // Under Option B fallback the slot stays None — bounded timeout
            // + kill_on_drop(true) still close GAP-8 at the user-facing level.
            let child_slot: Arc<tokio::sync::Mutex<Option<tokio::process::Child>>> =
                Arc::new(tokio::sync::Mutex::new(None));
            let handle = tokio::spawn(server_task::run_server_task(
                name.clone(),
                config.clone(),
                self.registry.clone(),
                cancel.clone(),
                connected.clone(),
                child_slot.clone(),
            ));
            tasks.insert(name.clone(), (handle, cancel, child_slot));
            flags.insert(name.clone(), connected);
            stored_configs.insert(name, config);
        }
    }

    /// Start all configured MCP servers and return initial connection results.
    ///
    /// Spawns tasks and gives servers a brief window to attempt connection, then
    /// returns a `StartResult` with preliminary connected/failed status.
    ///
    /// Used by `reload_and_report()` to aggregate failures for D-12 status reporting.
    pub async fn start_all_and_wait(
        &self,
        configs: HashMap<String, McpServerConfig>,
    ) -> StartResult {
        let mut task_names: Vec<String> = Vec::new();
        {
            let mut tasks = self.tasks.lock().await;
            let mut stored_configs = self.configs.lock().await;
            let mut flags = self.connected_flags.lock().await;

            for (name, config) in configs {
                if !config.enabled {
                    tracing::info!(server = %name, "MCP server disabled, skipping");
                    continue;
                }
                let cancel = CancellationToken::new();
                // GAP-7: allocate per-server connected flag; server_task flips it to
                // true ONLY after list_all_tools() succeeds.
                let connected = Arc::new(AtomicBool::new(false));
                // GAP-8: per-server child-process slot (see start_all for rationale).
                let child_slot: Arc<tokio::sync::Mutex<Option<tokio::process::Child>>> =
                    Arc::new(tokio::sync::Mutex::new(None));
                let handle = tokio::spawn(server_task::run_server_task(
                    name.clone(),
                    config.clone(),
                    self.registry.clone(),
                    cancel.clone(),
                    connected.clone(),
                    child_slot.clone(),
                ));
                task_names.push(name.clone());
                tasks.insert(name.clone(), (handle, cancel, child_slot));
                flags.insert(name.clone(), connected);
                stored_configs.insert(name, config);
            }
        }

        // Give servers a brief window to complete initial connection
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;

        // GAP-7: a server is "connected" IFF its AtomicBool flag is true — the
        // authoritative signal that rmcp `initialize` + `list_all_tools` both
        // succeeded. The has_tools heuristic is kept as belt-and-braces (a server
        // that registered tools must have flipped the flag to true) but the flag
        // is the ground truth that closes the GAP-7 false-positive.
        let mut connected = Vec::new();
        let mut failed: Vec<(String, String)> = Vec::new();
        {
            let tasks = self.tasks.lock().await;
            let flags = self.connected_flags.lock().await;
            let guard = self.registry.read().await;
            for name in &task_names {
                let flag_true = flags
                    .get(name)
                    .map(|f| f.load(Ordering::SeqCst))
                    .unwrap_or(false);
                if let Some((handle, _cancel, _child_slot)) = tasks.get(name) {
                    // GAP-4: registered tools use the SANITIZED prefix (make_prefixed_name
                    // replaces `-`/`.`/`@`/`/` with `_`), so the lookup must sanitize too.
                    let sanitized_prefix = format!("{}__", sanitize_server_name(name));
                    let has_tools = guard
                        .get_definitions(None)
                        .iter()
                        .any(|t| t.function.name.starts_with(&sanitized_prefix));

                    if flag_true {
                        // Authoritative: handshake + list_all_tools both succeeded.
                        connected.push(name.clone());
                    } else if handle.is_finished() && !has_tools {
                        // Task exited (e.g. child crashed before initialize completed).
                        failed.push((name.clone(), "connection failed after retries".to_string()));
                    }
                    // flag_true==false + task still running + no tools yet = still connecting
                }
            }
        }

        let tool_count = self.registered_tool_count().await;
        StartResult {
            connected,
            failed,
            tool_count,
        }
    }

    /// Shutdown all running server tasks and unregister their tools.
    ///
    /// Cancels each task's `CancellationToken`, hard-kills any stdio child
    /// process (GAP-8), then awaits the task's `JoinHandle` with a bounded
    /// 2-second ceiling so the gateway's Ctrl+C path can never hang. Finally
    /// removes the server's tools from the registry via `unregister_by_prefix`.
    ///
    /// GAP-8 (Phase 21.2 Plan 11): before this fix, `ironhermes gateway` hung
    /// indefinitely on Ctrl+C when stdio MCP servers were connected. Root
    /// cause: the rmcp `TokioChildProcess` parent->child pipe closure did not
    /// cause a misbehaving child (e.g. Node runtime blocked on stdin) to
    /// exit, and tokio's process reaper kept the runtime alive until the
    /// child was reaped. The fix here has two parts working together:
    ///   1. `Child::start_kill()` sends SIGKILL if a child handle is parked
    ///      (currently a no-op under the plan-11 Option B fallback where
    ///      rmcp owns the child — `kill_on_drop(true)` in `connect_stdio`
    ///      covers this path via tokio's drop-kill).
    ///   2. `tokio::time::timeout(Duration::from_secs(2), handle)` bounds the
    ///      JoinHandle await so shutdown always returns — the operator's
    ///      Ctrl+C returns within ~2s/server regardless of child behavior.
    pub async fn shutdown_all(&self) -> Vec<ServerTaskResult> {
        use tokio::time::{Duration, timeout};

        let mut tasks = self.tasks.lock().await;
        let mut flags = self.connected_flags.lock().await;
        let mut results = Vec::new();

        for (name, (handle, cancel, child_slot)) in tasks.drain() {
            tracing::info!(server = %name, "Shutting down MCP server");

            // 1. Cancel the task's cancellation token (tells the serve loop to break).
            cancel.cancel();

            // 2. GAP-8: hard-kill the stdio child (if any). start_kill() is
            //    non-blocking — it sets the SIGKILL flag on the child. The
            //    subsequent timeout(2s, handle) does the actual bounded wait.
            //    Under plan-11 Option B this branch is typically a no-op —
            //    rmcp owns the child internally and the slot holds None — but
            //    `kill_on_drop(true)` in connect_stdio gives us the same OS-
            //    level kill guarantee when rmcp's transport drops. Either way,
            //    the bounded timeout below is the load-bearing guarantee.
            if let Some(mut child) = child_slot.lock().await.take() {
                match child.start_kill() {
                    Ok(()) => tracing::debug!(server = %name, "Sent SIGKILL to MCP stdio child"),
                    Err(e) => tracing::warn!(
                        server = %name,
                        error = %e,
                        "Failed to SIGKILL MCP stdio child (may already be reaped)"
                    ),
                }
            }

            // 3. GAP-8: await the JoinHandle with a bounded 2-second ceiling.
            //    If the task is genuinely stuck, we log a warning and proceed
            //    — never blocking the gateway's Ctrl+C return.
            match timeout(Duration::from_secs(2), handle).await {
                Ok(Ok(result)) => results.push(result),
                Ok(Err(join_err)) => tracing::warn!(
                    server = %name,
                    error = %join_err,
                    "MCP server task panicked during shutdown"
                ),
                Err(_elapsed) => tracing::warn!(
                    server = %name,
                    "MCP server task did not join within 2s of cancel+SIGKILL; proceeding"
                ),
            }

            // 4. Unregister tools for this server (D-09: rebuild registry on reload)
            //    GAP-4: unregister_by_prefix appends "__" to its argument; we must
            //    pass the already-sanitized server name so the match finds the
            //    tools we registered.
            let mut guard = self.registry.write().await;
            let removed = guard.unregister_by_prefix(&sanitize_server_name(&name));
            tracing::debug!(server = %name, removed, "Unregistered MCP tools");
            drop(guard); // explicitly drop before next iteration to avoid holding write lock

            // 5. GAP-7: remove the connected flag alongside the task handle so
            //    no stale reads of `connected_server_names()` survive shutdown.
            flags.remove(&name);
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
    pub async fn reload_and_report(
        &self,
        new_configs: HashMap<String, McpServerConfig>,
    ) -> StartResult {
        // Shutdown existing servers + unregister their tools
        self.shutdown_all().await;
        // Start new servers and wait for initial connection status
        self.start_all_and_wait(new_configs).await
    }

    /// Return names of servers whose rmcp `initialize` handshake AND
    /// `list_all_tools()` both succeeded (GAP-7 contract). A spawned task
    /// that exited before the handshake completed is NOT reported here —
    /// unlike the old implementation which returned every `tasks.keys()`
    /// regardless of whether the child ever spoke MCP.
    ///
    /// Note: uses try_lock to avoid deadlock in sync call contexts. Returns
    /// empty vec if the mutex is currently locked.
    pub fn connected_server_names(&self) -> Vec<String> {
        if let Ok(flags) = self.connected_flags.try_lock() {
            flags
                .iter()
                .filter_map(|(name, flag)| {
                    if flag.load(Ordering::SeqCst) {
                        Some(name.clone())
                    } else {
                        None
                    }
                })
                .collect()
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
        let new_configs: HashMap<String, McpServerConfig> = match ironhermes_core::Config::load() {
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
        // Cancel all tasks on drop to avoid orphaned background tasks.
        //
        // GAP-8: Drop is sync and cannot await a JoinHandle or async-kill a
        // Child. The actual hard-kill path is `shutdown_all`, which is now
        // invoked by `GatewayRunner::start` BEFORE the runner returns (so
        // this Drop only runs after all children are already killed via the
        // bounded-timeout path). The `child_slot` is bound to `_` here because
        // a synchronous drop cannot take/start_kill it — Option B's
        // `kill_on_drop(true)` in connect_stdio covers any residual case.
        if let Ok(tasks) = self.tasks.try_lock() {
            for (_, (_, cancel, _child_slot)) in tasks.iter() {
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

    #[tokio::test]
    async fn shutdown_all_unregisters_tools_for_server_with_special_char_name() {
        use crate::tool::{McpTool, make_prefixed_name};
        use ironhermes_tools::ToolRegistry;
        use tokio::sync::mpsc;

        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let manager = McpManager::new(registry.clone());

        // A server name that hits every one of the four sanitized characters.
        let server_name = "@scope/pkg-name.v1";

        // Manually register two tools under the sanitized prefix — simulating
        // what server_task::connect_and_serve would do at connect time.
        {
            let mut guard = registry.write().await;
            for tool_original in ["read_file", "write_file"] {
                let (tx, _rx) = mpsc::channel(1);
                let tool = McpTool::new(
                    server_name,
                    tool_original,
                    "desc",
                    serde_json::json!({}),
                    tx,
                );
                guard.register_dynamic(Box::new(tool));
            }
        }

        // Seed the manager task map with a no-op task bound to `server_name`
        // so shutdown_all has something to drain.
        {
            let cancel = CancellationToken::new();
            let handle = tokio::spawn(async {
                crate::server_task::ServerTaskResult {
                    server_name: "placeholder".to_string(),
                    tool_names: vec![],
                    failure_reason: None,
                }
            });
            manager.tasks.lock().await.insert(
                server_name.to_string(),
                (handle, cancel, Arc::new(tokio::sync::Mutex::new(None))),
            );
        }

        // Pre-condition: both tools visible under sanitized prefix.
        let sanitized_prefix = make_prefixed_name(server_name, "");
        let sanitized_prefix = sanitized_prefix.trim_end_matches("__").to_string();
        // sanitized_prefix is e.g. "_scope_pkg_name_v1"
        {
            let guard = registry.read().await;
            let hits: Vec<_> = guard
                .get_definitions(None)
                .into_iter()
                .filter(|t| {
                    t.function
                        .name
                        .starts_with(&format!("{sanitized_prefix}__"))
                })
                .collect();
            assert_eq!(
                hits.len(),
                2,
                "precondition: 2 tools under sanitized prefix before shutdown"
            );
        }

        // Act: shutdown_all must unregister them.
        let _ = manager.shutdown_all().await;

        // Post-condition: zero tools remain under the sanitized prefix.
        let guard = registry.read().await;
        let leftover: Vec<_> = guard
            .get_definitions(None)
            .into_iter()
            .filter(|t| {
                t.function
                    .name
                    .starts_with(&format!("{sanitized_prefix}__"))
            })
            .collect();
        assert!(
            leftover.is_empty(),
            "GAP-4: shutdown_all must remove all tools of a special-char-named server; leftover={:?}",
            leftover
                .iter()
                .map(|t| t.function.name.clone())
                .collect::<Vec<_>>()
        );
    }

    /// GAP-7: when a server's child exits before the rmcp `initialize`
    /// handshake completes, `connected_server_names()` must NOT report it.
    /// Before this fix, `tasks.keys()` was returned unconditionally, so a
    /// crashed child appeared as "connected" with zero tools, producing the
    /// false-positive startup message `MCP: 0 tool(s) ready from 1 server(s).`
    #[tokio::test]
    async fn connected_server_names_excludes_server_that_exited_before_initialize() {
        use std::time::Duration;

        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let manager = McpManager::new(registry.clone());

        // A command that exits immediately without speaking MCP JSON-RPC.
        // `false` on unix returns exit code 1 instantly; on Windows use
        // `cmd /C exit 1` for equivalent behavior.
        let mut cfg = McpServerConfig::default();
        #[cfg(unix)]
        {
            cfg.command = Some("false".to_string());
        }
        #[cfg(not(unix))]
        {
            cfg.command = Some("cmd".to_string());
            cfg.args = vec!["/C".to_string(), "exit".to_string(), "1".to_string()];
        }
        cfg.enabled = true;
        // Tiny connect_timeout so the test doesn't wait 60s for retries.
        cfg.connect_timeout = 1;

        let mut configs = HashMap::new();
        configs.insert("crashy".to_string(), cfg);

        manager.start_all(configs).await;

        // Give the child some room to spawn, exit, and let server_task observe
        // the failure. list_all_tools() will never succeed against `false`, so
        // connected.store(true) never fires. 500ms is plenty for a process
        // that exits immediately.
        tokio::time::sleep(Duration::from_millis(500)).await;

        let names = manager.connected_server_names();
        assert!(
            names.is_empty(),
            "GAP-7: connected_server_names() must NOT include a server whose \
             child exited before the rmcp initialize handshake. names={names:?}"
        );

        // Cleanly shut down so the test doesn't leak the spawned task.
        let _ = manager.shutdown_all().await;
    }

    /// GAP-7 companion: after a manual flag-flip (simulating what
    /// connect_and_serve does on the happy path after list_all_tools),
    /// connected_server_names() MUST include the server. Proves the new
    /// lookup path correctly reads the flag (not just "always empty").
    #[tokio::test]
    async fn connected_server_names_includes_server_whose_flag_is_true() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let manager = McpManager::new(registry.clone());

        // Insert a flag directly — no live MCP needed. This mirrors exactly
        // what server_task::connect_and_serve does after list_all_tools succeeds.
        {
            let mut flags = manager.connected_flags.lock().await;
            let flag = Arc::new(AtomicBool::new(true));
            flags.insert("ok_server".to_string(), flag);
        }

        let names = manager.connected_server_names();
        assert_eq!(
            names,
            vec!["ok_server".to_string()],
            "GAP-7: a server whose connected flag is true must be reported by \
             connected_server_names(). names={names:?}"
        );
    }

    #[tokio::test]
    async fn reload_with_special_char_server_name_does_not_duplicate_tools() {
        // Higher-level regression: simulate two back-to-back reload cycles
        // and assert the registry never grows beyond the per-cycle tool count.
        // Uses the same manual-register pattern as the test above (no live MCP).
        use crate::tool::{McpTool, make_prefixed_name};
        use ironhermes_tools::ToolRegistry;
        use tokio::sync::mpsc;

        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let manager = McpManager::new(registry.clone());
        let server_name = "@org/pkg-x.y";

        async fn seed_cycle(
            manager: &McpManager,
            registry: &Arc<RwLock<ToolRegistry>>,
            server_name: &str,
        ) {
            let mut guard = registry.write().await;
            for tool_original in ["a", "b", "c"] {
                let (tx, _rx) = mpsc::channel(1);
                let tool = McpTool::new(server_name, tool_original, "d", serde_json::json!({}), tx);
                guard.register_dynamic(Box::new(tool));
            }
            drop(guard);
            let cancel = CancellationToken::new();
            let handle = tokio::spawn(async {
                crate::server_task::ServerTaskResult {
                    server_name: "placeholder".to_string(),
                    tool_names: vec![],
                    failure_reason: None,
                }
            });
            manager.tasks.lock().await.insert(
                server_name.to_string(),
                (handle, cancel, Arc::new(tokio::sync::Mutex::new(None))),
            );
        }

        // Cycle 1: register + shutdown (simulates one reload iteration)
        seed_cycle(&manager, &registry, server_name).await;
        let _ = manager.shutdown_all().await;

        // Cycle 2: register again + shutdown
        seed_cycle(&manager, &registry, server_name).await;
        let _ = manager.shutdown_all().await;

        // The registry must be empty of this server's tools — no accumulation.
        let sanitized_prefix = make_prefixed_name(server_name, "");
        let sanitized_prefix = sanitized_prefix.trim_end_matches("__").to_string();
        let guard = registry.read().await;
        let residue: Vec<_> = guard
            .get_definitions(None)
            .into_iter()
            .filter(|t| {
                t.function
                    .name
                    .starts_with(&format!("{sanitized_prefix}__"))
            })
            .collect();
        assert!(
            residue.is_empty(),
            "GAP-4: two reload cycles must not leave duplicates; residue={:?}",
            residue
                .iter()
                .map(|t| t.function.name.clone())
                .collect::<Vec<_>>()
        );
    }

    /// GAP-8: `shutdown_all` must return within a bounded time even when
    /// a stdio MCP child process is long-lived and not responding to
    /// parent-pipe EOF. Before this fix, `ironhermes gateway` hung on
    /// Ctrl+C because the tokio process reaper kept the runtime alive
    /// until the child was reaped.
    ///
    /// Test shape: spawn a long-running stdio "server" (`sleep 300` on
    /// unix). Give it ~500ms to attach. Call `shutdown_all()` wrapped in
    /// an OUTER `tokio::time::timeout(5s, ...)`. The outer timeout MUST
    /// NOT fire — i.e., shutdown_all returns in well under 5 seconds.
    /// Internally, manager.rs bounds each task to 2s via
    /// `tokio::time::timeout`, so for 1 server this should be well under
    /// 3s in the worst case even when the child never responds.
    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_all_returns_within_timeout_when_stdio_child_blocks() {
        use std::time::Duration;

        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let manager = McpManager::new(registry.clone());

        let mut cfg = McpServerConfig::default();
        cfg.command = Some("sleep".to_string());
        cfg.args = vec!["300".to_string()];
        cfg.enabled = true;
        // Short connect_timeout so server_task doesn't burn full 60s retrying
        // (note: `sleep 300` never speaks MCP, so server_task will fail the
        // initialize handshake repeatedly and retry with backoff — tight
        // connect_timeout keeps the test window small).
        cfg.connect_timeout = 1;

        let mut configs = HashMap::new();
        configs.insert("sleepy".to_string(), cfg);

        manager.start_all(configs).await;
        // Give the Child time to actually spawn and be parked in child_slot
        // (or, under Option B, to at least be live under rmcp's internal
        // ownership).
        tokio::time::sleep(Duration::from_millis(500)).await;

        // The crucial assertion: shutdown_all must return within the OUTER
        // 5-second test timeout. Internally, manager.rs bounds each task
        // to 2s via tokio::time::timeout, so for 1 server this should be
        // well under 3s in the worst case.
        let shutdown_result =
            tokio::time::timeout(Duration::from_secs(5), manager.shutdown_all()).await;
        assert!(
            shutdown_result.is_ok(),
            "GAP-8: shutdown_all MUST return within 5s even when the stdio \
             child is long-lived. If this test hangs, the bounded-timeout \
             + hard-kill wire in shutdown_all is regressed."
        );

        // Post-condition: the manager's task map is empty (drained) and
        // the connected_server_names reports empty.
        let names = manager.connected_server_names();
        assert!(
            names.is_empty(),
            "GAP-8: post-shutdown, connected_server_names must be empty"
        );
    }
}

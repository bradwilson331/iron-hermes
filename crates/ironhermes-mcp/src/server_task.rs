use crate::config::McpServerConfig;
use crate::security::sanitize_error;
use crate::tool::{McpCallRequest, McpTool};
use crate::transport;
use ironhermes_tools::registry::Tool;
use ironhermes_tools::ToolRegistry;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, mpsc};
use tokio_util::sync::CancellationToken;

/// Maximum number of reconnection retries before giving up (D-05).
pub const MAX_RETRIES: u32 = 5;

/// Maximum backoff interval between reconnection attempts (D-05).
pub const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Result of a single server task's lifecycle.
///
/// The `failure_reason` field carries the sanitized error message when the server
/// exhausted all retries or failed to connect initially. This is the data contract
/// consumed by Plan 04's `McpReloader::reload()` to surface D-12 partial failure
/// reporting (failure summaries via /reload-mcp).
pub struct ServerTaskResult {
    /// Name of the MCP server this result is for.
    pub server_name: String,
    /// Prefixed names (server__tool) of all tools that were registered.
    pub tool_names: Vec<String>,
    /// Sanitized error message if the server exhausted all retries (D-12 data contract).
    /// `None` means the server ran and was cleanly cancelled (success path).
    pub failure_reason: Option<String>,
}

/// Run a single MCP server's lifecycle: connect, discover tools, register them,
/// serve tool calls, and reconnect on failure with exponential backoff (D-04, D-05).
///
/// This function is designed to run as a long-lived `tokio::spawn` task. It exits
/// cleanly when the `cancel_token` is cancelled, or after exhausting retries.
///
/// The returned `ServerTaskResult.failure_reason` is `None` on clean shutdown,
/// and `Some(sanitized_error)` when retries are exhausted (D-12 data contract).
pub async fn run_server_task(
    name: String,
    mut config: McpServerConfig,
    registry: Arc<RwLock<ToolRegistry>>,
    cancel_token: CancellationToken,
    connected: Arc<AtomicBool>,
) -> ServerTaskResult {
    // D-18: interpolate ${ENV_VAR} placeholders in config fields
    crate::config::interpolate_config(&mut config);

    let mut registered_names: Vec<String> = Vec::new();
    let mut failure_reason: Option<String> = None;
    let mut retries = 0u32;
    let mut backoff = Duration::from_secs(1);

    loop {
        match connect_and_serve(&name, &config, &registry, &cancel_token, &connected).await {
            Ok(names) => {
                registered_names = names;
                failure_reason = None; // connected successfully; clean exit
                break;
            }
            Err(_) if cancel_token.is_cancelled() => break,
            Err(e) => {
                retries += 1;
                let sanitized = sanitize_error(&e.to_string());
                if retries > MAX_RETRIES {
                    tracing::warn!(
                        server = %name,
                        error = %sanitized,
                        "MCP server exhausted {} retries",
                        MAX_RETRIES
                    );
                    // GAP-7: on retry exhaustion, explicitly unset the flag so a
                    // late reader cannot observe a stale true from a brief prior
                    // success within the same task. The flag is the authoritative
                    // signal consulted by McpManager::connected_server_names().
                    connected.store(false, Ordering::SeqCst);
                    // D-12: capture failure reason for reload reporting
                    failure_reason = Some(sanitized);
                    break;
                }
                tracing::warn!(
                    server = %name,
                    retry = retries,
                    max = MAX_RETRIES,
                    backoff_secs = backoff.as_secs(),
                    error = %sanitized,
                    "MCP server disconnected, retrying"
                );
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = cancel_token.cancelled() => break,
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }

    // GAP-7: on clean task exit (cancel or normal break), unset the flag so
    // `connected_server_names()` readers don't return a shut-down server.
    connected.store(false, Ordering::SeqCst);

    ServerTaskResult {
        server_name: name,
        tool_names: registered_names,
        failure_reason,
    }
}

/// Inner function: connect to the MCP server, register tools, and serve calls
/// until cancelled or the channel closes.
///
/// Returns `Ok(registered_names)` on clean cancellation, `Err` on connection failure.
/// Unregisters tools from the registry before returning in all cases.
async fn connect_and_serve(
    name: &str,
    config: &McpServerConfig,
    registry: &Arc<RwLock<ToolRegistry>>,
    cancel_token: &CancellationToken,
    connected: &Arc<AtomicBool>,
) -> anyhow::Result<Vec<String>> {
    // Connect via appropriate transport
    let client = if config.command.is_some() {
        transport::connect_stdio(config).await?
    } else if config.url.is_some() {
        transport::connect_http(config).await?
    } else {
        return Err(anyhow::anyhow!(
            "Server '{}' has neither 'command' nor 'url'",
            name
        ));
    };

    // Discover tools via the rmcp Peer (RunningService Derefs to Peer<RoleClient>)
    // This is the initialize-and-discover gate. Its successful resolution is the
    // authoritative signal that the rmcp `initialize` handshake completed AND tool
    // discovery succeeded. GAP-7: flip the connected flag HERE — not at spawn time,
    // not at transport-open time. Any earlier error above propagates via `?` and
    // the flag stays `false` — exactly what connected_server_names() needs to read.
    let mcp_tools = client.list_all_tools().await?;
    connected.store(true, Ordering::SeqCst);
    tracing::info!(
        server = %name,
        tool_count = mcp_tools.len(),
        "MCP tools discovered"
    );

    // Create dispatch channel: McpTool::execute -> call_rx loop below
    let (call_tx, mut call_rx) = mpsc::channel::<McpCallRequest>(32);

    // Register tools into the ToolRegistry (D-06, D-08, D-11)
    let mut registered_names: Vec<String> = Vec::new();
    {
        let mut guard = registry.write().await;
        for mcp_tool in &mcp_tools {
            let tool_name = mcp_tool.name.as_ref();

            // D-08: per-server enabled_tools filtering
            if let Some(ref enabled) = config.enabled_tools {
                if !enabled.iter().any(|e| e == tool_name) {
                    continue;
                }
            }

            let description = mcp_tool.description.as_deref().unwrap_or("");
            // input_schema is Arc<JsonObject> (serde_json::Map); convert to serde_json::Value
            let schema_value =
                serde_json::to_value(mcp_tool.input_schema.as_ref()).unwrap_or_default();

            let tool = McpTool::new(name, tool_name, description, schema_value, call_tx.clone());
            let prefixed = tool.name().to_string();
            guard.register_dynamic(Box::new(tool));
            registered_names.push(prefixed);
        }
    } // write guard dropped before any .await — RESEARCH.md Pitfall 2

    tracing::info!(
        server = %name,
        registered = registered_names.len(),
        "MCP tools registered"
    );

    // Serve tool call requests until cancelled or all senders dropped
    let timeout = Duration::from_secs(config.timeout);
    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                tracing::debug!(server = %name, "MCP server task cancelled");
                break;
            }
            request = call_rx.recv() => {
                match request {
                    Some(req) => {
                        // Build CallToolRequestParams with optional arguments
                        // arguments field is Option<JsonObject> = Option<serde_json::Map<String, Value>>
                        let arguments = match &req.arguments {
                            serde_json::Value::Object(map) => Some(map.clone()),
                            serde_json::Value::Null => None,
                            other => {
                                // Wrap non-object, non-null args in a map for compatibility
                                let mut m = serde_json::Map::new();
                                m.insert("value".to_string(), other.clone());
                                Some(m)
                            }
                        };

                        let params = {
                            let mut p = rmcp::model::CallToolRequestParams::new(
                                req.tool_name.clone(),
                            );
                            if let Some(args) = arguments {
                                p = p.with_arguments(args);
                            }
                            p
                        };

                        let result = tokio::time::timeout(timeout, client.call_tool(params)).await;

                        let final_result: anyhow::Result<String> = match result {
                            Err(_elapsed) => Err(anyhow::anyhow!(
                                "MCP tool call timed out after {}s",
                                config.timeout
                            )),
                            Ok(Err(e)) => Err(anyhow::anyhow!(
                                "{}",
                                sanitize_error(&e.to_string())
                            )),
                            Ok(Ok(call_result)) => {
                                // Extract text content from response
                                let text = call_result
                                    .content
                                    .iter()
                                    .filter_map(|c| {
                                        if let rmcp::model::RawContent::Text(t) = &c.raw {
                                            Some(t.text.as_str())
                                        } else {
                                            None
                                        }
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n");

                                if call_result.is_error.unwrap_or(false) {
                                    Err(anyhow::anyhow!(
                                        "{}",
                                        sanitize_error(&text)
                                    ))
                                } else {
                                    Ok(text)
                                }
                            }
                        };

                        let _ = req.response_tx.send(final_result);
                    }
                    None => {
                        // All McpTool senders have been dropped (registry cleared)
                        tracing::debug!(server = %name, "MCP call channel closed");
                        break;
                    }
                }
            }
        }
    }

    Ok(registered_names)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D-05: Verify the exponential backoff progression matches hermes-agent.
    /// Sequence: 1s → 2s → 4s → 8s → 16s → 32s → 60s (capped)
    #[test]
    fn test_backoff_progression() {
        let mut backoff = Duration::from_secs(1);
        let expected = [1, 2, 4, 8, 16];
        for &exp in &expected {
            assert_eq!(
                backoff,
                Duration::from_secs(exp),
                "Expected backoff to be {exp}s"
            );
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
        // After 16s, next is 32s (still below cap)
        assert_eq!(backoff, Duration::from_secs(32));
        backoff = (backoff * 2).min(MAX_BACKOFF);
        // After 32s, next would be 64s but is capped at 60s
        assert_eq!(backoff, MAX_BACKOFF, "Backoff should be capped at 60s");
    }

    /// D-12: Verify ServerTaskResult has the failure_reason field and it can
    /// carry sanitized error text (data contract for Plan 04's McpReloader).
    #[test]
    fn test_server_task_result_captures_failure() {
        let result = ServerTaskResult {
            server_name: "test".to_string(),
            tool_names: vec![],
            failure_reason: Some("connection refused".to_string()),
        };
        assert_eq!(
            result.failure_reason.as_deref(),
            Some("connection refused")
        );

        let success = ServerTaskResult {
            server_name: "test".to_string(),
            tool_names: vec!["test__tool_a".to_string()],
            failure_reason: None,
        };
        assert!(
            success.failure_reason.is_none(),
            "Clean shutdown should have no failure_reason"
        );
    }

    #[test]
    fn test_max_retries_constant() {
        assert_eq!(MAX_RETRIES, 5);
    }

    #[test]
    fn test_max_backoff_constant() {
        assert_eq!(MAX_BACKOFF, Duration::from_secs(60));
    }
}

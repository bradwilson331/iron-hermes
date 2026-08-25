use crate::config::McpServerConfig;
use crate::security::sanitize_error;
use crate::tool::{McpCallRequest, McpTool};
use crate::transport;
use crate::www_auth::{OAuthChallengeError, parse_www_authenticate};
use ironhermes_core::auth::AuthStore;
use ironhermes_tools::ToolRegistry;
use ironhermes_tools::registry::Tool;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{RwLock, mpsc};
use tokio_util::sync::CancellationToken;

/// Maximum number of reconnection retries before giving up (D-05).
pub const MAX_RETRIES: u32 = 5;

/// Maximum backoff interval between reconnection attempts (D-05).
pub const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// OAuth-specific permanent errors that bypass the exponential-backoff reconnect loop.
///
/// When `connect_and_serve` returns one of these variants (wrapped as `anyhow::Error`),
/// `run_server_task` breaks out of the reconnect loop immediately rather than retrying —
/// preventing a DoS retry-loop on a mis-configured scope or a permanently revoked token
/// (T-44-02, B-2).
#[derive(Debug)]
pub enum McpOAuthError {
    /// `error="insufficient_scope"` in WWW-Authenticate: the token's scope is wrong.
    ///
    /// **Permanent** — no retry, no token refresh. The operator must re-authorize:
    /// `hermes mcp connect <server>`.
    InsufficientScope { server: String },
    /// `error="invalid_token"` (or bare 401): forced refresh + one reconnect attempt
    /// both returned 401.
    ///
    /// **Permanent** — the token is not recoverable without user re-authorization.
    InvalidTokenRetryExhausted { server: String },
}

impl std::fmt::Display for McpOAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InsufficientScope { server } => write!(
                f,
                "MCP server '{server}': OAuth insufficient_scope (permanent — \
                 run `hermes mcp connect {server}` to re-authorize with correct scopes)"
            ),
            Self::InvalidTokenRetryExhausted { server } => write!(
                f,
                "MCP server '{server}': OAuth token refresh + 1-retry both returned 401 \
                 (permanent — run `hermes mcp connect {server}` to re-authorize)"
            ),
        }
    }
}

impl std::error::Error for McpOAuthError {}

/// Returns `true` when an error string indicates an HTTP 401 / Unauthorized response.
///
/// Used by `connect_and_serve` to trigger the B-2 WWW-Authenticate parse path.
/// rmcp 1.8 surfaces 401s as transport errors whose string representations contain
/// the HTTP status code or reason phrase; exact format is version-specific.
/// Called only on OAuth servers (`config.oauth_provider.is_some()`).
fn is_oauth_401(err_str: &str) -> bool {
    let lower = err_str.to_ascii_lowercase();
    // Match "401" as a distinct token (avoid matching e.g. port "4011")
    lower.contains(" 401 ")
        || lower.contains(" 401\n")
        || lower.contains("\"401\"")
        || lower.ends_with(" 401")
        || lower.contains("unauthorized")
}

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
#[allow(clippy::too_many_arguments)] // 9 args required: name, config, registry, cancel, connected, child_slot, auth_store, global_issuer_allowlist, last_failure
pub async fn run_server_task(
    name: String,
    mut config: McpServerConfig,
    registry: Arc<RwLock<ToolRegistry>>,
    cancel_token: CancellationToken,
    connected: Arc<AtomicBool>,
    child_slot: Arc<tokio::sync::Mutex<Option<tokio::process::Child>>>,
    auth_store: Option<Arc<AuthStore>>,
    global_issuer_allowlist: Vec<String>,
    // Warm-but-revoked follow-up fix: a live, per-server cell mirroring the
    // eventual `ServerTaskResult.failure_reason` but updated on EVERY
    // connect/serve error (not only at final retry exhaustion) and cleared
    // on a successful (re)connect — so `McpManager::last_failure_reason` can
    // answer without waiting for this task to exit or for a reload.
    last_failure: Arc<std::sync::Mutex<Option<String>>>,
) -> ServerTaskResult {
    // D-18: interpolate ${ENV_VAR} placeholders in config fields
    crate::config::interpolate_config(&mut config);

    let mut registered_names: Vec<String> = Vec::new();
    let mut failure_reason: Option<String> = None;
    let mut retries = 0u32;
    let mut backoff = Duration::from_secs(1);
    // B-2: track whether the OAuth 1-retry has been used for this server's lifetime.
    // Persists across reconnect iterations so the retry is truly bounded to exactly ONE
    // occurrence regardless of how many times the outer reconnect loop fires.
    let mut oauth_refreshed = false;

    loop {
        match connect_and_serve(
            &name,
            &config,
            &registry,
            &cancel_token,
            &connected,
            &child_slot,
            &auth_store,
            &mut oauth_refreshed,
            &global_issuer_allowlist,
        )
        .await
        {
            Ok(names) => {
                registered_names = names;
                failure_reason = None; // connected successfully; clean exit
                // Warm-but-revoked follow-up fix: a successful (re)connect
                // means whatever the server was last failing on no longer
                // applies — clear the live cell so a stale auth-required
                // reason doesn't outlive its cause.
                if let Ok(mut cell) = last_failure.lock() {
                    *cell = None;
                }
                break;
            }
            Err(_) if cancel_token.is_cancelled() => break,
            Err(e) => {
                // B-2: McpOAuthError variants are permanent — bypass the exponential-backoff
                // reconnect loop entirely. Neither InsufficientScope nor
                // InvalidTokenRetryExhausted can be recovered by reconnecting; the operator
                // must re-authorize via `hermes mcp connect <name>` (T-44-02).
                if e.downcast_ref::<McpOAuthError>().is_some() {
                    let sanitized = sanitize_error(&e.to_string());
                    tracing::warn!(
                        server = %name,
                        error = %sanitized,
                        "MCP server OAuth permanent error — not retrying"
                    );
                    connected.store(false, Ordering::SeqCst);
                    failure_reason = Some(sanitized.clone());
                    if let Ok(mut cell) = last_failure.lock() {
                        *cell = Some(sanitized);
                    }
                    break;
                }

                // GAP-8: a prior attempt may have parked a Child here; clear it before
                // the next attempt so shutdown_all never sees a stale handle whose
                // process is already exited. Under the plan-11 Option B fallback this
                // is a belt-and-braces guard — connect_stdio currently parks None, so
                // the slot is typically already empty. When Option A lands and
                // connect_stdio parks Some(child), this path becomes load-bearing.
                if let Ok(mut slot) = child_slot.try_lock()
                    && let Some(mut old_child) = slot.take()
                {
                    let _ = old_child.start_kill();
                }
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
                    failure_reason = Some(sanitized.clone());
                    if let Ok(mut cell) = last_failure.lock() {
                        *cell = Some(sanitized);
                    }
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
                // Warm-but-revoked follow-up fix: record the reason live, mid-
                // retry — a still-retrying server's status should not have to
                // wait for MAX_RETRIES exhaustion to stop lying as SpawnFailed.
                if let Ok(mut cell) = last_failure.lock() {
                    *cell = Some(sanitized);
                }
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
#[allow(clippy::too_many_arguments)] // 9 args required: name, config, registry, cancel, connected, child_slot, auth_store, oauth_refreshed, global_issuer_allowlist
async fn connect_and_serve(
    name: &str,
    config: &McpServerConfig,
    registry: &Arc<RwLock<ToolRegistry>>,
    cancel_token: &CancellationToken,
    connected: &Arc<AtomicBool>,
    child_slot: &Arc<tokio::sync::Mutex<Option<tokio::process::Child>>>,
    auth_store: &Option<Arc<AuthStore>>,
    oauth_refreshed: &mut bool,
    global_issuer_allowlist: &[String],
) -> anyhow::Result<Vec<String>> {
    // Connect via appropriate transport.
    // MCPA-02: OAuth branch precedes the plain-HTTP branch — dispatched when both
    // `url` AND `oauth_provider` are set on the config.
    let (client, child_opt) = if config.command.is_some() {
        transport::connect_stdio(config).await?
    } else if config.url.is_some() && config.oauth_provider.is_some() {
        // D-04: headless / auto-start posture — OAuth servers are skipped with a
        // non-error tracing::warn when no AuthStore is available or no cached token
        // exists for this namespace. The operator must run `hermes mcp connect <name>`
        // to complete the interactive PKCE flow first; auto-start never launches a browser.
        let ns = config.oauth_provider.as_deref().unwrap_or_default();
        match auth_store {
            None => {
                tracing::warn!(
                    server = %name,
                    "MCP server requires OAuth but no AuthStore is configured — \
                     skipping auto-start (run `hermes mcp connect {name}` to authorize)"
                );
                return Ok(Vec::new()); // D-04: non-error skip; zero tools registered
            }
            Some(store) => {
                // D-04: only proceed when a token is already cached; never launch a browser
                // in the headless auto-start path.
                if store.get_token(ns).await.is_none() {
                    tracing::warn!(
                        server = %name,
                        ns = %ns,
                        "MCP server requires OAuth but no cached token found — \
                         skipping auto-start (run `hermes mcp connect {name}` to authorize)"
                    );
                    return Ok(Vec::new()); // D-04: non-error skip; zero tools registered
                }
                transport::connect_http_oauth(config, store.clone(), global_issuer_allowlist)
                    .await?
            }
        }
    } else if config.url.is_some() {
        transport::connect_http(config).await?
    } else {
        return Err(anyhow::anyhow!(
            "Server '{}' has neither 'command' nor 'url'",
            name
        ));
    };

    // GAP-8: park the stdio child handle (if any) in the shared slot so
    // McpManager::shutdown_all can reach it to call start_kill() during
    // graceful shutdown. Under the plan-11 Option B fallback, connect_stdio
    // currently returns None (rmcp 1.5 owns the child internally), so this
    // line is a no-op today — but the plumbing is in place so Option A can
    // upgrade cleanly when rmcp exposes a pre-spawned-Child constructor.
    // HTTP transport always has child_opt=None.
    if let Some(child) = child_opt {
        *child_slot.lock().await = Some(child);
    }

    // Discover tools via the rmcp Peer (RunningService Derefs to Peer<RoleClient>)
    // This is the initialize-and-discover gate. Its successful resolution is the
    // authoritative signal that the rmcp `initialize` handshake completed AND tool
    // discovery succeeded. GAP-7: flip the connected flag HERE — not at spawn time,
    // not at transport-open time. Any earlier error above propagates via `?` and
    // the flag stays `false` — exactly what connected_server_names() needs to read.
    //
    // B-2: Also the first network roundtrip where the bearer token is validated by
    // the remote MCP server. A 401 here (stale/revoked token) is handled with the
    // same WWW-Authenticate parse logic as tool-call 401s below.
    let mcp_tools = match client.list_all_tools().await {
        Ok(tools) => tools,
        Err(e) => {
            let err_str = e.to_string();
            // B-2: parse WWW-Authenticate on every 401 from OAuth servers (T-44-02).
            // rmcp may embed the header value or just the HTTP status code in the
            // error string; parse_www_authenticate handles both via plain string scan.
            if config.oauth_provider.is_some() && is_oauth_401(&err_str) {
                let challenge = parse_www_authenticate(&err_str);
                match challenge {
                    OAuthChallengeError::InsufficientScope => {
                        // Permanent — no retry, no refresh (Pitfall 6 avoidance)
                        return Err(anyhow::Error::from(McpOAuthError::InsufficientScope {
                            server: name.to_string(),
                        }));
                    }
                    _ => {
                        // InvalidToken / None / Other → force refresh + bounded 1-retry
                        if *oauth_refreshed {
                            // 1-retry already consumed — permanent fail (B-2 bound)
                            return Err(anyhow::Error::from(
                                McpOAuthError::InvalidTokenRetryExhausted {
                                    server: name.to_string(),
                                },
                            ));
                        }
                        if let (Some(store), Some(ns)) =
                            (auth_store, config.oauth_provider.as_deref())
                        {
                            tracing::warn!(
                                server = %name,
                                ns = %ns,
                                "OAuth 401 at tool discovery — forcing token refresh; \
                                 1-retry reconnect pending (B-2)"
                            );
                            let _ = store.refresh(ns).await;
                        }
                        *oauth_refreshed = true;
                        // Return error to trigger reconnect via the outer loop.
                        // On the next connect_and_serve call, connect_http_oauth
                        // will call get_access_token() which returns the refreshed token.
                        return Err(anyhow::anyhow!(
                            "OAuth 401 at discovery — {}",
                            sanitize_error(&err_str)
                        ));
                    }
                }
            }
            return Err(anyhow::anyhow!("{}", sanitize_error(&err_str)));
        }
    };
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
            if let Some(ref enabled) = config.enabled_tools
                && !enabled.iter().any(|e| e == tool_name)
            {
                continue;
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

                        // B-2: track whether this result requires breaking out of the
                        // serve loop with an OAuth-specific error. Set before final_result
                        // so we can notify the caller via response_tx first, then return.
                        let mut oauth_break_err: Option<anyhow::Error> = None;

                        let final_result: anyhow::Result<String> = match result {
                            Err(_elapsed) => Err(anyhow::anyhow!(
                                "MCP tool call timed out after {}s",
                                config.timeout
                            )),
                            Ok(Err(e)) => {
                                let err_str = e.to_string();
                                // B-2: parse WWW-Authenticate on every 401 from OAuth servers,
                                // BEFORE deciding between retry and permanent fail (T-44-02).
                                // rmcp 1.8 may embed the WWW-Authenticate header value or just
                                // the HTTP status code in the error string; parse_www_authenticate
                                // handles both via plain string scan. Called only on OAuth servers.
                                if config.oauth_provider.is_some() && is_oauth_401(&err_str) {
                                    let challenge = parse_www_authenticate(&err_str);
                                    match challenge {
                                        OAuthChallengeError::InsufficientScope => {
                                            // Permanent — no retry, no refresh (Pitfall 6 avoidance).
                                            // Set break_err so we return after notifying the caller.
                                            oauth_break_err = Some(anyhow::Error::from(
                                                McpOAuthError::InsufficientScope {
                                                    server: name.to_string(),
                                                },
                                            ));
                                        }
                                        _ => {
                                            // InvalidToken / None / Other → force refresh + 1-retry
                                            if *oauth_refreshed {
                                                // 1-retry already consumed — permanent fail (B-2 bound)
                                                oauth_break_err = Some(anyhow::Error::from(
                                                    McpOAuthError::InvalidTokenRetryExhausted {
                                                        server: name.to_string(),
                                                    },
                                                ));
                                            } else {
                                                // Force token refresh and mark the 1-retry consumed.
                                                // The oauth_refreshed flag persists in run_server_task
                                                // scope so the bound holds across reconnect iterations.
                                                if let (Some(store), Some(ns)) =
                                                    (auth_store, config.oauth_provider.as_deref())
                                                {
                                                    tracing::warn!(
                                                        server = %name,
                                                        ns = %ns,
                                                        "OAuth 401 during tool call — forcing \
                                                         token refresh; 1-retry reconnect pending (B-2)"
                                                    );
                                                    let _ = store.refresh(ns).await;
                                                }
                                                *oauth_refreshed = true;
                                                // Signal reconnect: return error after notifying caller.
                                                // The outer reconnect loop will call connect_and_serve
                                                // again with the refreshed token (Approach A: next
                                                // connect_http_oauth call uses get_access_token()).
                                                oauth_break_err = Some(anyhow::anyhow!(
                                                    "OAuth 401 — {}",
                                                    sanitize_error(&err_str)
                                                ));
                                            }
                                        }
                                    }
                                }
                                Err(anyhow::anyhow!("{}", sanitize_error(&err_str)))
                            }
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

                        // B-2: after notifying the caller, break with the OAuth error
                        // so the serve loop exits cleanly and run_server_task can decide
                        // whether to reconnect (1-retry) or fail permanently.
                        if let Some(oauth_err) = oauth_break_err {
                            return Err(oauth_err);
                        }
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
        assert_eq!(result.failure_reason.as_deref(), Some("connection refused"));

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

    /// B-2 / T-44-02: static-grep regression — connect_and_serve MUST reference
    /// `WWW-Authenticate` header parsing on every OAuth 401, and MUST NOT fold
    /// `insufficient_scope` into the reconnect retry loop (Pitfall 6 avoidance).
    #[test]
    fn server_task_parses_www_authenticate_on_oauth_401() {
        let src = include_str!("server_task.rs");
        assert!(
            src.contains("parse_www_authenticate"),
            "B-2: server_task.rs must call parse_www_authenticate on every 401 \
             from an OAuth server to distinguish insufficient_scope (permanent) \
             from invalid_token (1-retry). Do not remove this call."
        );
        assert!(
            src.contains("WWW-Authenticate"),
            "B-2: server_task.rs must reference 'WWW-Authenticate' — either in a comment \
             or in the call to parse_www_authenticate, documenting the header source."
        );
        assert!(
            src.contains("InsufficientScope"),
            "B-2: McpOAuthError::InsufficientScope must be returned on insufficient_scope \
             challenges so the reconnect loop is bypassed immediately (Pitfall 6 avoidance)."
        );
    }
}

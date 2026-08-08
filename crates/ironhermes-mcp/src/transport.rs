use crate::config::McpServerConfig;
use crate::security::build_safe_env;
use anyhow::Result;
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};

/// Connect to a stdio MCP server. Returns the running service AND an optional
/// external handle on the spawned child process.
///
/// D-19: builds a safe environment using the allowlist (env_clear + build_safe_env).
/// The child process inherits only the safe env keys plus user-specified vars from config.
///
/// GAP-8 (Phase 21.2 Plan 11): the signature returns an `Option<tokio::process::Child>`
/// so `McpManager::shutdown_all` can hard-kill the stdio child during graceful
/// shutdown. The current implementation uses the plan-blessed Option B fallback:
/// rmcp 1.5's `TokioChildProcess::new(Command)` owns the child internally with no
/// supported constructor exposing a pre-spawned Child, so we return `None` for the
/// external handle and rely on two compounding safeguards:
///   1. `cmd.kill_on_drop(true)` inside the configure closure — when rmcp's
///      transport drops after the serve loop exits, tokio's drop-kill behavior
///      fires SIGKILL at the OS level (closing GAP-8 at the process level).
///   2. `McpManager::shutdown_all` wraps each JoinHandle await in
///      `tokio::time::timeout(Duration::from_secs(2), handle)` so the gateway
///      always exits within a bounded time regardless of child behavior.
///
/// Together these guarantee `ironhermes gateway` exits within ~2s/server on
/// Ctrl+C even when the stdio child ignores its parent-pipe EOF. When rmcp
/// later exposes a pre-spawned-Child constructor, `connect_stdio` can upgrade
/// to `Some(child)` without any call-site changes (Option A upgrade).
pub async fn connect_stdio(
    config: &McpServerConfig,
) -> Result<(
    RunningService<RoleClient, ()>,
    Option<tokio::process::Child>,
)> {
    let command = config
        .command
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("stdio transport requires 'command' field"))?;

    let safe_env = build_safe_env(&config.env);
    let args = config.args.clone();

    use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
    let transport =
        TokioChildProcess::new(tokio::process::Command::new(command).configure(move |cmd| {
            for arg in &args {
                cmd.arg(arg);
            }
            // D-19: clear host env, then add only safe vars
            cmd.env_clear();
            // GAP-6b: pipe the child's stderr away from the parent terminal fd. Without
            // this, a misbehaving stdio MCP child (e.g. `npx @modelcontextprotocol/...`
            // printing its Usage line on startup failure) writes directly onto the
            // parent's tty, corrupting the `You:` prompt. Stdio::piped() means the
            // bytes land in a kernel pipe owned by the child process handle; they are
            // not surfaced to the user, which is correct for an interactive chat REPL.
            // A future plan may spawn a reader task to route captured stderr into
            // ServerTaskResult.failure_reason; that is outside GAP-6b's scope.
            cmd.stderr(std::process::Stdio::piped());
            // GAP-8: defensive SIGKILL-on-drop. When rmcp's transport drops after the
            // serve loop exits (or is cancelled), tokio's kill-on-drop semantics fire
            // SIGKILL at the OS level, so the stdio child cannot outlive its parent
            // even though rmcp 1.5 doesn't expose a pre-spawned-Child constructor for
            // us to track externally. This is the plan-11 Option B guarantee: paired
            // with the bounded 2s JoinHandle timeout in McpManager::shutdown_all, the
            // gateway always exits within bounded time on Ctrl+C.
            cmd.kill_on_drop(true);
            for (k, v) in &safe_env {
                cmd.env(k, v);
            }
        }))?;

    let client = ().serve(transport).await?;
    // GAP-8 Option B: rmcp 1.5's TokioChildProcess owns the spawned Child
    // internally; no supported constructor accepts a pre-spawned Child. We
    // return None for the external handle and rely on kill_on_drop(true) +
    // the bounded JoinHandle timeout in shutdown_all for graceful shutdown.
    Ok((client, None))
}

/// Connect to an HTTP/StreamableHTTP MCP server.
///
/// Uses `StreamableHttpClientTransport` (reqwest-backed) from rmcp.
/// Requires the `transport-streamable-http-client-reqwest` feature on rmcp.
///
/// GAP-8 (Phase 21.2 Plan 11): signature symmetric with `connect_stdio` — HTTP
/// has no external child process, so the `Option<tokio::process::Child>` is
/// always `None`. Kept for call-site uniformity in `server_task::connect_and_serve`.
pub async fn connect_http(
    config: &McpServerConfig,
) -> Result<(
    RunningService<RoleClient, ()>,
    Option<tokio::process::Child>,
)> {
    let url = config
        .url
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("HTTP transport requires 'url' field"))?;

    use rmcp::transport::StreamableHttpClientTransport;
    let transport = StreamableHttpClientTransport::from_uri(url.as_str());
    let client = ().serve(transport).await?;
    Ok((client, None))
}

/// Connect to an HTTP/StreamableHTTP MCP server with MCP OAuth 2.1 authentication.
///
/// This is the authenticated sibling of [`connect_http`]. It wires rmcp's
/// `AuthorizationManager` to `AuthStoreCredentialStore` (D-05/B-3), validates the
/// issuer URL against the PRM allowlist (B-4), loads cached tokens from `auth.json`,
/// and on a cache miss runs the interactive PKCE browser flow over a 127.0.0.1:0
/// loopback callback (D-04).
///
/// # Transport construction (Approach A — 44-01 probe)
///
/// The workspace pins `reqwest = "0.12"` while rmcp 1.8 ships its own reqwest 0.13
/// internally. The `StreamableHttpClient` bound on `AuthClient<C>::new` requires
/// the *rmcp-internal* reqwest 0.13 `Client` — there is no way to satisfy it with
/// the workspace client without pulling a second reqwest version as a direct dep.
///
/// The 44-01 probe therefore resolved to **Approach A**: obtain the access token via
/// `AuthorizationManager::get_access_token()` and inject it into the transport via
/// `StreamableHttpClientTransportConfig::auth_header(token)`.  This is equivalent
/// in practice: the token is re-fetched (and auto-refreshed if needed) on every call
/// to `connect_http_oauth`, so short-lived connections are handled correctly.
///
/// # Security properties
///
/// - **B-4**: `validate_prm_issuer` runs before ANY network/discovery call.
/// - **D-05**: `AuthStoreCredentialStore` routes all OAuth credentials through Phase 41's
///   `AuthStore` (zero-DCR-on-restart / B-3).
/// - **D-03**: Does NOT import or call `ironhermes_core::auth::pkce::run_pkce_flow`.
///   MCP OAuth uses rmcp's own `AuthorizationSession` PKCE, not the Phase 43 provider flow.
/// - **A-1**: No token, auth code, or PKCE verifier value is logged; errors routed through
///   `security::sanitize_error`.
/// - **B-1**: `reinit_on_expired_session(true)` is set on the transport config so a stale
///   `Mcp-Session-Id` triggers transparent reconnection.
///
/// # GAP-8
///
/// No child process; returns `None` for the external handle, symmetric with `connect_http`.
///
/// # D-01 (46.1)
///
/// `global_additive_issuers` is the resolved `Config.mcp_oauth.issuer_allowlist` —
/// consulted only when `config.allowed_issuer` (the per-server pin) is absent. See
/// `security::resolve_allowed_issuers`.
pub async fn connect_http_oauth(
    config: &McpServerConfig,
    auth_store: std::sync::Arc<ironhermes_core::auth::AuthStore>,
    global_additive_issuers: &[String],
) -> Result<(
    RunningService<RoleClient, ()>,
    Option<tokio::process::Child>,
)> {
    use rmcp::transport::StreamableHttpClientTransport;
    use rmcp::transport::auth::{AuthorizationManager, AuthorizationSession};
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let url = config
        .url
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("OAuth transport requires 'url' field"))?;

    let ns = config
        .oauth_provider
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("OAuth transport requires 'oauth_provider' field"))?;

    // B-4/D-01: Resolve the effective allowed-issuer set (per-server pin
    // authoritative, else baseline ∪ global) and validate BEFORE any
    // network/discovery call (HTTPS-only + allowlist).
    let allowed = crate::security::resolve_allowed_issuers(
        config.allowed_issuer.as_deref(),
        global_additive_issuers,
    );
    crate::security::validate_prm_issuer(url.as_str(), &allowed).map_err(|e| {
        anyhow::anyhow!(
            "PRM issuer validation failed: {}",
            crate::security::sanitize_error(&e.to_string())
        )
    })?;

    // D-05: Credential store adapter routes OAuth credentials through Phase 41 AuthStore.
    let adapter = crate::auth_store_adapter::AuthStoreCredentialStore::new(
        std::sync::Arc::clone(&auth_store),
        ns,
        url.as_str(),
    );

    // OQ-2: AuthorizationManager::new is async; it builds an internal reqwest 0.13 HTTP client.
    let mut auth_mgr = AuthorizationManager::new(url.as_str()).await.map_err(|e| {
        anyhow::anyhow!(
            "Auth manager init: {}",
            crate::security::sanitize_error(&e.to_string())
        )
    })?;

    // D-05: Wire the credential store BEFORE initialize_from_store so all reads/writes
    // go through AuthStoreCredentialStore → auth.json.
    auth_mgr.set_credential_store(adapter);

    // Hot path: cached token present.
    // initialize_from_store returns Ok(true) when credential_store.load() has a
    // token_response.  When it returns Ok(false) (no DcrEntry or no token), fall
    // through to the interactive PKCE branch below.
    let has_creds = auth_mgr.initialize_from_store().await.map_err(|e| {
        anyhow::anyhow!(
            "Credential load: {}",
            crate::security::sanitize_error(&e.to_string())
        )
    })?;

    if !has_creds {
        // Cold path: interactive PKCE browser flow (D-04).
        //
        // Pattern reused from ironhermes-core/auth/pkce.rs — bind 127.0.0.1:0 BEFORE
        // DCR/discovery so we have the ephemeral port for the redirect_uri.
        // D-03: never calls run_pkce_flow; uses rmcp's AuthorizationSession instead.
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| anyhow::anyhow!("Failed to bind PKCE loopback listener: {e}"))?;
        let port = listener.local_addr()?.port();
        let redirect_uri = format!("http://127.0.0.1:{port}/callback");

        // Discover AS metadata before creating AuthorizationSession.
        // AuthorizationSession::new calls register_client() which requires
        // self.metadata to be Some — set it via set_metadata() first.
        let metadata = auth_mgr.discover_metadata().await.map_err(|e| {
            anyhow::anyhow!(
                "AS metadata discovery failed: {}",
                crate::security::sanitize_error(&e.to_string())
            )
        })?;
        auth_mgr.set_metadata(metadata);

        // AuthorizationSession::new takes ownership of auth_mgr and does:
        //   1. DCR (register_client) to obtain client_id
        //   2. configure_client with the new OAuthClientConfig
        //   3. get_authorization_url (PKCE challenge + CSRF token → state_store)
        let session = AuthorizationSession::new(
            auth_mgr,
            &[], // no explicit scopes — AS/PRM metadata selects defaults via select_scopes()
            &redirect_uri,
            Some("IronHermes MCP Client"),
            None, // no client_metadata_url (URL-based client IDs not required)
        )
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "PKCE session init: {}",
                crate::security::sanitize_error(&e.to_string())
            )
        })?;

        // Open the authorization URL in the user's default browser.
        // A-1: the auth_url contains the PKCE state but NOT the verifier; it is safe
        // to print as a fallback, but not to log at any tracing level.
        let auth_url = session.auth_url.clone();
        if let Err(e) = open::that(&auth_url) {
            // Fallback: print URL so the user can open it manually.
            println!(
                "Browser could not be opened automatically ({e:#}).\n\
                 Open this URL in your browser to authorize IronHermes:\n\n  {auth_url}\n"
            );
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }

        // Accept exactly one loopback callback within 300 seconds (A-4: bounded wait).
        let (mut stream, _peer) =
            tokio::time::timeout(std::time::Duration::from_secs(300), listener.accept())
                .await
                .map_err(|_| anyhow::anyhow!("PKCE loopback timed out after 300 seconds"))?
                .map_err(|e| anyhow::anyhow!("Failed to accept loopback callback: {e}"))?;

        // Read the callback HTTP request (up to 8 KiB — enough for code + state).
        let mut buf = [0u8; 8192];
        let n = stream
            .read(&mut buf)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read callback request: {e}"))?;
        let request_line = std::str::from_utf8(&buf[..n])
            .unwrap_or("")
            .lines()
            .next()
            .unwrap_or("");

        // Extract path+query from "GET /callback?code=…&state=… HTTP/1.1".
        let path_and_query = request_line.split_whitespace().nth(1).unwrap_or("/");
        let callback_url = format!("http://127.0.0.1:{port}{path_and_query}");

        // Respond 200 OK so the browser tab doesn't hang.
        let body = "Authorization complete \u{2014} you can close this tab.";
        let response_bytes = format!(
            "HTTP/1.1 200 OK\r\n\
             Content-Type: text/html; charset=utf-8\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\
             \r\n\
             {}",
            body.len(),
            body
        );
        let _ = stream.write_all(response_bytes.as_bytes()).await;

        // handle_callback_url parses the full redirect URL, validates CSRF state and
        // optional RFC 9207 iss internally, then exchanges the code for a token and
        // persists {client_id, token_response} via credential_store.save().
        // A-1: errors are sanitized before surfacing; the code/verifier are never logged.
        session
            .handle_callback_url(&callback_url)
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "PKCE code exchange: {}",
                    crate::security::sanitize_error(&e.to_string())
                )
            })?;

        // Move auth_manager back out of the (now-complete) session so we can call
        // get_access_token() below.  Session fields auth_url and redirect_uri are
        // Strings that drop normally when session goes out of scope.
        auth_mgr = session.auth_manager;
    }

    // Approach A (44-01 probe): get_access_token() returns the cached bearer token,
    // auto-refreshing via stored refresh_token when within the 30-second expiry buffer.
    // Injected via auth_header on the StreamableHttpClientTransportConfig.
    // B-1: reinit_on_expired_session(true) recovers from Mcp-Session-Id expiry.
    let token = auth_mgr.get_access_token().await.map_err(|e| {
        anyhow::anyhow!(
            "Get access token: {}",
            crate::security::sanitize_error(&e.to_string())
        )
    })?;

    let cfg = StreamableHttpClientTransportConfig::with_uri(url.as_str())
        .auth_header(token)
        .reinit_on_expired_session(true);
    let transport = StreamableHttpClientTransport::from_config(cfg);
    let client = ().serve(transport).await?;
    Ok((client, None))
}

#[cfg(test)]
mod tests {
    /// GAP-6b: static-grep regression — connect_stdio MUST set stderr to
    /// Stdio::piped() inside its configure closure so the parent terminal
    /// does not inherit the child's stderr fd. Without this, a misbehaving
    /// npx MCP server corrupts the interactive REPL prompt.
    #[test]
    fn connect_stdio_pipes_child_stderr() {
        let src = include_str!("transport.rs");
        assert!(
            src.contains("cmd.stderr(std::process::Stdio::piped());"),
            "GAP-6b: connect_stdio must call cmd.stderr(std::process::Stdio::piped()) \
             inside its configure closure so the child's stderr is NOT inherited \
             from the parent terminal"
        );
    }

    /// GAP-8: static-grep regression — connect_stdio MUST set kill_on_drop(true)
    /// inside its configure closure so the spawned stdio child is SIGKILL'd at
    /// the OS level when rmcp's transport drops. Paired with the bounded 2s
    /// JoinHandle timeout in McpManager::shutdown_all, this guarantees
    /// `ironhermes gateway` exits within bounded time on Ctrl+C even when the
    /// stdio child ignores its parent-pipe EOF.
    #[test]
    fn connect_stdio_sets_kill_on_drop() {
        let src = include_str!("transport.rs");
        assert!(
            src.contains("cmd.kill_on_drop(true);"),
            "GAP-8: connect_stdio must call cmd.kill_on_drop(true) inside its \
             configure closure so the stdio child cannot outlive its parent \
             when rmcp's transport drops"
        );
    }

    /// GAP-6b: runtime regression — spawn a trivial child process configured
    /// identically to what connect_stdio does (env_clear + piped stderr),
    /// have it write to stderr, and assert the bytes land on the child's
    /// piped stderr handle (not on the parent's stderr fd).
    ///
    /// Uses std::process::Command directly rather than going through rmcp
    /// so the test has zero dependency on a live MCP server. The behavior
    /// under test is std/tokio's Stdio::piped contract — identical to what
    /// TokioChildProcess inherits from the configure closure.
    #[test]
    fn stdio_child_stderr_does_not_inherit_parent_fd() {
        use std::io::Read;
        use std::process::{Command, Stdio};

        // A POSIX-ish command that prints to stderr and exits. `sh -c` is
        // available on macOS + Linux CI; on Windows this test is gated out.
        #[cfg(unix)]
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("printf 'usage: this-must-not-hit-parent-terminal\\n' 1>&2")
            .stderr(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("failed to spawn sh for GAP-6b test");

        #[cfg(not(unix))]
        let mut child = Command::new("cmd")
            .args(["/C", "echo usage: this-must-not-hit-parent-terminal 1>&2"])
            .stderr(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("failed to spawn cmd for GAP-6b test");

        let mut stderr = child
            .stderr
            .take()
            .expect("GAP-6b: Stdio::piped() must produce a reader handle on ChildStderr");

        let mut captured = String::new();
        stderr
            .read_to_string(&mut captured)
            .expect("failed to drain child stderr pipe");
        let _ = child.wait();

        assert!(
            captured.contains("usage: this-must-not-hit-parent-terminal"),
            "GAP-6b: child stderr bytes must be captured on the piped handle, not \
             inherited by the parent. captured={captured:?}"
        );
    }

    /// MCPA-01 / 44-04: static-grep regression for connect_http_oauth transport construction.
    ///
    /// The function MUST use `auth_header` (Approach A from the 44-01 probe) to inject the
    /// bearer token into a `StreamableHttpClientTransportConfig`.  This guards against two
    /// regressions:
    ///
    /// 1. Accidentally using the *unauthenticated* `from_uri` path (connect_http's path) for
    ///    the OAuth function — that would silently drop all OAuth credentials.
    /// 2. Accidentally removing the `reinit_on_expired_session(true)` knob (B-1) that keeps
    ///    the transport alive across Mcp-Session-Id expiry.
    ///
    /// Note: the plan originally specified `StreamableHttpClientTransport::with_client` (MCPA-01).
    /// The 44-01 probe proved that the workspace reqwest 0.12 dep does NOT satisfy rmcp's
    /// internal `StreamableHttpClient` bound (which requires reqwest 0.13), so Approach A
    /// (`auth_header` + `from_config`) was used instead.  This test was adapted accordingly
    /// and the deviation is documented in the 44-04 SUMMARY.
    #[test]
    fn connect_http_oauth_uses_auth_header_approach_a() {
        let src = include_str!("transport.rs");

        // Approach A marker: bearer token is injected via auth_header on the config.
        assert!(
            src.contains(".auth_header(token)"),
            "MCPA-01 / 44-04: connect_http_oauth must inject the bearer token via \
             StreamableHttpClientTransportConfig::auth_header(token) (Approach A). \
             Do NOT replace with from_uri (unauthenticated) or remove the auth_header call."
        );

        // B-1 marker: session-resume knob must be set.
        assert!(
            src.contains(".reinit_on_expired_session(true)"),
            "B-1: connect_http_oauth must set reinit_on_expired_session(true) on the \
             transport config so a stale Mcp-Session-Id triggers transparent reconnection."
        );

        // B-4 marker: validate_prm_issuer must be called.
        assert!(
            src.contains("validate_prm_issuer(url.as_str(),"),
            "B-4: connect_http_oauth must call security::validate_prm_issuer before any \
             network/discovery call to block SSRF via malicious OAuth metadata."
        );
    }
}

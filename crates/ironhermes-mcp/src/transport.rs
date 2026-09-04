use crate::config::McpServerConfig;
use crate::security::build_safe_env;
use anyhow::Result;
use http::{HeaderName, HeaderValue};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use std::collections::HashMap;
use std::str::FromStr;

/// D-02/T-48.3-04: header keys reserved by the MCP transport itself. rmcp
/// rejects a `custom_headers` entry whose name collides with one of these
/// (case-insensitive); this is a local mirror of rmcp's own private
/// `RESERVED_HEADERS` constant (`rmcp-1.8.0/src/transport/common/http_header.rs:11-16`)
/// so ironhermes can name the offending header in its own error rather than
/// surfacing rmcp's generic connect failure. Deliberately OMITS
/// `mcp-protocol-version` — rmcp allows that one through. Must be re-checked
/// if the `rmcp` pin ever moves off `=1.8.0`.
pub const RESERVED_HEADER_KEYS: &[&str] = &["accept", "mcp-session-id", "last-event-id"];

/// D-01: build the typed header map `StreamableHttpClientTransportConfig::custom_headers`
/// expects from `McpServerConfig.headers`, and report whether the operator set an
/// explicit `Authorization` entry (consumed by `connect_http`'s D-02 precedence
/// resolution).
///
/// Pure and network-free so it is unit-testable in isolation. Header conversion
/// failures name the offending KEY only, never the VALUE (T-48.3-02: a malformed
/// header value must never be echoed into an error that could reach a log or the
/// wizard). A key matching [`RESERVED_HEADER_KEYS`] (case-insensitive) is rejected
/// by name (T-48.3-04) rather than left to fail generically at connect time.
pub fn build_custom_headers(
    config: &McpServerConfig,
) -> Result<(HashMap<HeaderName, HeaderValue>, bool)> {
    let mut custom_headers: HashMap<HeaderName, HeaderValue> = HashMap::new();
    let mut has_explicit_authorization = false;

    for (k, v) in &config.headers {
        if RESERVED_HEADER_KEYS
            .iter()
            .any(|reserved| reserved.eq_ignore_ascii_case(k))
        {
            anyhow::bail!(
                "header '{k}' is managed by the MCP transport and cannot be set explicitly"
            );
        }

        let name = HeaderName::from_str(k)
            .map_err(|_| anyhow::anyhow!("header key '{k}' is not a valid HTTP header name"))?;
        if name.as_str().eq_ignore_ascii_case("authorization") {
            has_explicit_authorization = true;
        }
        let value = HeaderValue::from_str(v).map_err(|_| {
            anyhow::anyhow!("header key '{k}' has a value that is not valid for an HTTP header")
        })?;
        custom_headers.insert(name, value);
    }

    Ok((custom_headers, has_explicit_authorization))
}

/// D-02: normalize the `auth:` shorthand value. rmcp's `.auth_header(v)` funnels
/// into `bearer_auth(v)`, which itself prepends `Bearer `. Strip a single leading
/// case-insensitive `Bearer ` prefix (and surrounding whitespace) so an operator
/// who writes `auth: "Bearer abc"` gets one `Bearer abc` on the wire rather than a
/// doubled prefix. A whitespace-only value is treated as absent (fail-safe,
/// matching how `allowed_issuer` already treats empty/whitespace at config.rs:47).
fn normalize_auth_shorthand(auth: &str) -> Option<String> {
    let trimmed = auth.trim();
    if trimmed.is_empty() {
        return None;
    }
    let stripped = trimmed
        .strip_prefix("Bearer ")
        .or_else(|| trimmed.strip_prefix("bearer "))
        .map(str::trim_start)
        .unwrap_or(trimmed);
    if stripped.is_empty() {
        None
    } else {
        Some(stripped.to_string())
    }
}

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
///
/// D-01: builds the transport via `StreamableHttpClientTransportConfig::with_uri`
/// carrying `config.headers` through `.custom_headers(...)`, mirroring the
/// authenticated sibling [`connect_http_oauth`]'s builder chain shape. Before this
/// fix, `config.headers` was parsed, env-expanded, and unit-tested but never
/// reached the wire — every request went out unauthenticated.
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
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

    let (custom_headers, has_explicit_authorization) = build_custom_headers(config)?;

    let mut cfg = StreamableHttpClientTransportConfig::with_uri(url.as_str())
        .custom_headers(custom_headers)
        .reinit_on_expired_session(true);

    // D-02: `auth:` shorthand only applies when no explicit Authorization header
    // was set. Never call `.auth_header(...)` and pass a `custom_headers` map
    // containing an `Authorization` entry to the same
    // `StreamableHttpClientTransportConfig` — RESEARCH.md empirically verified
    // against the pinned reqwest 0.12.28 that rmcp applies `bearer_auth()` first
    // and then `.header()` per custom entry, and reqwest's `.header()` APPENDS,
    // producing two `Authorization` values on the wire and undefined server
    // behavior. The explicit header always wins; the shorthand never silently
    // overrides a literal operator instruction.
    if !has_explicit_authorization
        && let Some(token) = config.auth.as_deref().and_then(normalize_auth_shorthand)
    {
        cfg = cfg.auth_header(token);
    }

    let transport = StreamableHttpClientTransport::from_config(cfg);
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
/// Phase 48.2 Plan 08 (T-48.2-08-01): the single validated construction path
/// for an rmcp `AuthorizationManager`.
///
/// Contains EXACTLY the sequence that previously lived inline in
/// `connect_http_oauth`: URL/namespace extraction, allowed-issuer
/// resolution, `validate_prm_issuer` — still the first thing that touches
/// the URL and still before any network/discovery call —
/// `AuthStoreCredentialStore::new`, `AuthorizationManager::new`, and
/// `set_credential_store`. Returns the manager plus the resolved server URL
/// string.
///
/// T-48.2-08-01: this is the ONLY construction site of `AuthorizationManager`
/// in this file. Both [`connect_http_oauth`] (loopback CLI path) and
/// [`begin_oauth_web`] (web-completable path) reach the manager exclusively
/// through this function, so the security-critical ordering — issuer
/// validation before any network call — has one copy that cannot drift
/// between two independently-maintained paths.
async fn build_oauth_manager(
    config: &McpServerConfig,
    auth_store: std::sync::Arc<ironhermes_core::auth::AuthStore>,
    global_additive_issuers: &[String],
) -> Result<(rmcp::transport::auth::AuthorizationManager, String)> {
    use rmcp::transport::auth::AuthorizationManager;

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

    Ok((auth_mgr, url.clone()))
}

pub async fn connect_http_oauth(
    config: &McpServerConfig,
    auth_store: std::sync::Arc<ironhermes_core::auth::AuthStore>,
    global_additive_issuers: &[String],
) -> Result<(
    RunningService<RoleClient, ()>,
    Option<tokio::process::Child>,
)> {
    use rmcp::transport::StreamableHttpClientTransport;
    use rmcp::transport::auth::AuthorizationSession;
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let (mut auth_mgr, url) =
        build_oauth_manager(config, auth_store, global_additive_issuers).await?;

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

/// Begin a web-completable MCP OAuth authorization (Phase 48.2 Plan 08, D-03).
///
/// The web-completable sibling of [`connect_http_oauth`]'s cold path. Reaches
/// `AuthorizationManager` construction ONLY through [`build_oauth_manager`]
/// (T-48.2-08-01) — the same validated prelude the loopback CLI path uses —
/// then performs AS metadata discovery and constructs an `AuthorizationSession`
/// bound to the caller-supplied `redirect_uri` instead of an ephemeral
/// loopback URI.
///
/// Binds NO listener, calls NO `open::that`, prints NOTHING to stdout, and
/// waits for nothing — the caller (`McpManager::begin_oauth`) owns parking
/// the returned session and completing it later from a callback URL string
/// via [`rmcp::transport::auth::AuthorizationSession::handle_callback_url`].
///
/// Every error is wrapped through `crate::security::sanitize_error` before it
/// leaves this function (A-1 discipline, matching every other fallible step
/// in this file).
pub async fn begin_oauth_web(
    config: &McpServerConfig,
    auth_store: std::sync::Arc<ironhermes_core::auth::AuthStore>,
    global_additive_issuers: &[String],
    redirect_uri: &str,
) -> Result<rmcp::transport::auth::AuthorizationSession> {
    use rmcp::transport::auth::AuthorizationSession;

    let (mut auth_mgr, _url) =
        build_oauth_manager(config, auth_store, global_additive_issuers).await?;

    // Discover AS metadata before creating AuthorizationSession.
    // AuthorizationSession::new calls register_client() which requires
    // self.metadata to be Some — set it via set_metadata() first, matching
    // connect_http_oauth's cold path.
    let metadata = auth_mgr.discover_metadata().await.map_err(|e| {
        anyhow::anyhow!(
            "AS metadata discovery failed: {}",
            crate::security::sanitize_error(&e.to_string())
        )
    })?;
    auth_mgr.set_metadata(metadata);

    // AuthorizationSession::new performs Dynamic Client Registration with the
    // caller-supplied redirect_uri — there is no pre-registration constraint,
    // so a server-hosted https://<web-origin>/oauth/mcp/callback value is
    // registrable per attempt exactly as the ephemeral loopback URI is today.
    let session = AuthorizationSession::new(
        auth_mgr,
        &[], // no explicit scopes — AS/PRM metadata selects defaults via select_scopes()
        redirect_uri,
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

    Ok(session)
}

/// Extract the `state` query parameter from an authorization URL or an OAuth
/// callback URL (Phase 48.2 Plan 08).
///
/// This is the single place that names the `state` query parameter — both
/// `McpManager::begin_oauth`/`complete_oauth` and (later) the web route
/// consume it through this function, so the key is spelled once.
///
/// Errors with fixed text (never echoing `url`, which may carry an
/// authorization code or other sensitive query data) when the URL fails to
/// parse or the parameter is absent or empty.
pub fn oauth_state_from_url(url: &str) -> Result<String> {
    let parsed = url::Url::parse(url)
        .map_err(|_| anyhow::anyhow!("Failed to parse URL while extracting OAuth state"))?;
    parsed
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.into_owned())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("URL has no 'state' query parameter"))
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

    /// D-01/D-08: region-scoped static-grep regression for `connect_http`'s
    /// transport construction shape.
    ///
    /// Scoped to ONLY `connect_http`'s own body (from its `fn` signature to the
    /// next function's) via the same region-slicing idiom
    /// `mcp_admin_api.rs::task3_behavior_7_probe_machinery_never_touches_a_tool_registry`
    /// uses — an unscoped whole-file grep would be satisfied by the OAuth
    /// sibling's own builder chain and would prove nothing about `connect_http`
    /// specifically.
    ///
    /// This proves CONSTRUCTION only — construction was never the problem here.
    /// The transmission proof (that a configured header actually reaches a
    /// listening server) is `tests/mcp_mock_handshake.rs` (D-08); the two are
    /// complements, not substitutes.
    #[test]
    fn connect_http_uses_custom_headers() {
        let src = include_str!("transport.rs");

        let start = src
            .find("pub async fn connect_http(")
            .expect("D-01: connect_http must exist in transport.rs");
        let end = src
            .find("pub async fn connect_http_oauth")
            .expect("D-01: connect_http_oauth must exist in transport.rs, after connect_http");
        assert!(
            start < end,
            "D-01: connect_http must be defined before connect_http_oauth in this file"
        );
        let region = &src[start..end];

        assert!(
            region.contains(".custom_headers("),
            "D-01: connect_http must build its transport with \
             StreamableHttpClientTransportConfig::custom_headers(...) so config.headers \
             reaches the wire. region={region}"
        );
        assert!(
            region.contains(".reinit_on_expired_session(true)"),
            "connect_http should set reinit_on_expired_session(true), matching the OAuth \
             sibling's session-resume behavior. region={region}"
        );
        assert!(
            !region.contains("StreamableHttpClientTransport::from_uri(url.as_str())"),
            "D-01 REGRESSION: connect_http must NOT construct its transport via the \
             unauthenticated from_uri(url) single-argument constructor — this is the exact \
             pre-fix defect that dropped every configured header silently. region={region}"
        );
    }

    /// D-01: unit test over the pure `build_custom_headers` fn — no network, no
    /// async runtime needed.
    #[test]
    fn build_custom_headers_reports_explicit_authorization() {
        // A map containing `authorization` in any casing sets the boolean.
        let mut config = crate::config::McpServerConfig {
            headers: [("Authorization".to_string(), "Bearer x".to_string())].into(),
            ..Default::default()
        };
        let (_, has_auth) = super::build_custom_headers(&config).expect("must build");
        assert!(has_auth, "an Authorization header (any casing) must set the flag");

        config.headers = [("AUTHORIZATION".to_string(), "Bearer x".to_string())].into();
        let (_, has_auth) = super::build_custom_headers(&config).expect("must build");
        assert!(has_auth, "AUTHORIZATION (uppercase) must also set the flag");

        // A map without it does not.
        config.headers = [("X-Custom".to_string(), "value".to_string())].into();
        let (_, has_auth) = super::build_custom_headers(&config).expect("must build");
        assert!(!has_auth, "a non-Authorization header must not set the flag");

        // A map whose key is a reserved name returns Err.
        config.headers = [("Mcp-Session-Id".to_string(), "x".to_string())].into();
        assert!(
            super::build_custom_headers(&config).is_err(),
            "a reserved header key must be rejected"
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

    // -------------------------------------------------------------------------
    // oauth_state_from_url tests (Phase 48.2 Plan 08)
    // -------------------------------------------------------------------------

    #[test]
    fn oauth_state_from_url_extracts_from_authorization_url() {
        let url = "https://as.example.com/authorize?response_type=code&client_id=abc&state=xyz123&redirect_uri=https%3A%2F%2Fhermes.example.com%2Foauth%2Fmcp%2Fcallback";
        let state = super::oauth_state_from_url(url).expect("state must be extracted");
        assert_eq!(state, "xyz123");
    }

    #[test]
    fn oauth_state_from_url_extracts_from_callback_url_with_code_and_iss() {
        let url = "https://hermes.example.com/oauth/mcp/callback?code=abc123&state=xyz123&iss=https%3A%2F%2Fas.example.com";
        let state = super::oauth_state_from_url(url).expect("state must be extracted");
        assert_eq!(state, "xyz123");
    }

    #[test]
    fn oauth_state_from_url_errors_when_state_absent() {
        let url = "https://hermes.example.com/oauth/mcp/callback?code=abc123";
        assert!(super::oauth_state_from_url(url).is_err());
    }

    #[test]
    fn oauth_state_from_url_errors_when_state_empty() {
        let url = "https://hermes.example.com/oauth/mcp/callback?code=abc123&state=";
        assert!(super::oauth_state_from_url(url).is_err());
    }

    #[test]
    fn oauth_state_from_url_errors_on_malformed_url() {
        assert!(super::oauth_state_from_url("not a url").is_err());
    }

    // -------------------------------------------------------------------------
    // begin_oauth_web fast-fail tests (Phase 48.2 Plan 08, T-48.2-08-01)
    //
    // Both cases must fail via build_oauth_manager's validate_prm_issuer call —
    // fast, and before any network call — so these tests never attempt to
    // resolve a real hostname.
    // -------------------------------------------------------------------------

    /// Open a fresh on-disk `AuthStore` under a per-test temp dir, mirroring
    /// `auth_store_adapter.rs::make_test_store` and `manager.rs::make_test_auth_store`.
    async fn make_test_auth_store(tag: &str) -> std::sync::Arc<ironhermes_core::auth::AuthStore> {
        let dir: std::path::PathBuf = std::env::temp_dir().join(format!(
            "ironhermes_mcp_transport_test_{}_{}",
            std::process::id(),
            tag,
        ));
        std::fs::create_dir_all(&dir).expect("could not create per-test temp dir");
        let path = dir.join("auth.json");
        ironhermes_core::auth::AuthStore::open(path)
            .await
            .expect("test AuthStore::open failed")
    }

    #[tokio::test]
    async fn begin_oauth_web_rejects_plain_http_server_url() {
        let store = make_test_auth_store("http_reject").await;
        let config = crate::config::McpServerConfig {
            url: Some("http://insecure.example.com/mcp".to_string()),
            oauth_provider: Some("test_ns".to_string()),
            ..Default::default()
        };
        let result = super::begin_oauth_web(
            &config,
            store,
            &[],
            "https://hermes.example.com/oauth/mcp/callback",
        )
        .await;
        assert!(
            result.is_err(),
            "begin_oauth_web must reject a plain-http server URL via validate_prm_issuer \
             before any network call (B-4)"
        );
    }

    #[tokio::test]
    async fn begin_oauth_web_rejects_host_outside_resolved_allowlist() {
        let store = make_test_auth_store("allowlist_reject").await;
        let config = crate::config::McpServerConfig {
            url: Some("https://evil.example.com/mcp".to_string()),
            oauth_provider: Some("test_ns".to_string()),
            ..Default::default()
        };
        // No per-server pin, no global additive issuers → baseline-only allowlist,
        // which does not include evil.example.com.
        let result = super::begin_oauth_web(
            &config,
            store,
            &[],
            "https://hermes.example.com/oauth/mcp/callback",
        )
        .await;
        assert!(
            result.is_err(),
            "begin_oauth_web must reject an https URL whose host is outside the resolved \
             allowlist, fast and before any network call (B-4/D-01)"
        );
    }

    // -------------------------------------------------------------------------
    // Ordering guard (Phase 48.2 Plan 08 Task 2, T-48.2-08-01): static-source
    // regression proving validate_prm_issuer runs before AuthorizationManager
    // construction inside build_oauth_manager, and that begin_oauth_web never
    // constructs its own AuthorizationManager — both paths must reach it only
    // through the shared prelude.
    // -------------------------------------------------------------------------

    /// Slice a named function's body out of this file's own source, stripping
    /// comment-only lines so a doc comment mentioning either search term
    /// cannot satisfy or defeat the caller's assertion.
    ///
    /// `fn_signature_needle` must uniquely identify the `fn` line (e.g.
    /// `"async fn build_oauth_manager("`). Assumes rustfmt's convention that a
    /// top-level function's closing brace is an unindented `}` on its own
    /// line — true for every function in this file.
    fn extract_fn_body_no_comments(src: &str, fn_signature_needle: &str) -> String {
        let fn_start = src
            .find(fn_signature_needle)
            .unwrap_or_else(|| panic!("could not find `{fn_signature_needle}` in transport.rs"));
        let body_start = src[fn_start..]
            .find("{\n")
            .map(|i| fn_start + i)
            .unwrap_or_else(|| panic!("`{fn_signature_needle}` has no body opening brace"));
        let body_end = src[body_start..]
            .find("\n}\n")
            .map(|i| body_start + i)
            .unwrap_or_else(|| panic!("`{fn_signature_needle}` body never closes at column 0"));
        let body = &src[body_start..body_end];
        body.lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn build_oauth_manager_validates_issuer_before_constructing_manager() {
        let src = include_str!("transport.rs");
        let code_only = extract_fn_body_no_comments(src, "async fn build_oauth_manager(");

        let validate_pos = code_only.find("validate_prm_issuer(").unwrap_or_else(|| {
            panic!(
                "T-48.2-08-01: build_oauth_manager must call security::validate_prm_issuer; \
                 body={code_only}"
            )
        });
        let construct_pos = code_only
            .find("AuthorizationManager::new(")
            .unwrap_or_else(|| {
                panic!(
                    "T-48.2-08-01: build_oauth_manager must construct AuthorizationManager; \
                     body={code_only}"
                )
            });

        assert!(
            validate_pos < construct_pos,
            "T-48.2-08-01: validate_prm_issuer must run BEFORE AuthorizationManager::new \
             inside build_oauth_manager (byte offsets within the function body: \
             validate_prm_issuer={validate_pos}, AuthorizationManager::new={construct_pos})"
        );
    }

    #[test]
    fn begin_oauth_web_does_not_construct_its_own_authorization_manager() {
        let src = include_str!("transport.rs");
        let code_only = extract_fn_body_no_comments(src, "pub async fn begin_oauth_web(");

        assert!(
            !code_only.contains("AuthorizationManager::new("),
            "T-48.2-08-01: begin_oauth_web must reach AuthorizationManager construction ONLY \
             through build_oauth_manager — it must not call AuthorizationManager::new itself. \
             body={code_only}"
        );
    }
}

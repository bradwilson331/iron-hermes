//! PKCE browser/loopback OAuth 2.1 flow for IronHermes.
//!
//! Implements the authorization-code flow with S256 PKCE challenge (OAUTH-03, F-01):
//! - Generates an S256 challenge; never writes the verifier to disk (A-3).
//! - Binds a loopback listener on `127.0.0.1:0` (OS-assigned port) (A-4).
//! - Asserts `code_challenge_method=S256` in the authorization URL before opening
//!   the browser.
//! - Opens the system browser via the injected `open_fn`; prints the URL on failure
//!   and keeps the listener alive (D-04).
//! - Accepts exactly one loopback callback with a 300-second timeout (A-4).
//! - Validates `state` (CSRF guard, T-43-03) and `iss` when present (RFC 9207).
//! - Exchanges the authorization code, persists the `TokenEntry` via `AuthStore`
//!   (A-6: `expires_at = now + expires_in`).
//! - Logs `ns` + error only — never a token or verifier value (A-1).

use super::AuthStore;
use super::store::TokenEntry;

/// Parameters for the PKCE browser/loopback flow.
///
/// Typically constructed from `AuthProviderConfig` (Plan 02).
/// All four fields are required; the flow validates them before starting.
#[derive(Debug, Clone)]
pub struct PkceFlowParams {
    /// Authorization endpoint URL (e.g. `https://accounts.example.com/oauth/authorize`).
    pub authorization_url: String,
    /// Token endpoint URL (e.g. `https://accounts.example.com/oauth/token`).
    pub token_url: String,
    /// Registered client ID (public, no secret — PKCE is for public clients).
    pub client_id: String,
    /// Requested OAuth scopes.
    pub scopes: Vec<String>,
}

/// Run the PKCE loopback flow, persisting the resulting token into `store` under `ns`.
///
/// `open_fn` is called with the fully constructed authorization URL (including the PKCE
/// S256 challenge and the loopback `redirect_uri`). In production, pass
/// `|url| open::that(url).map_err(Into::into)` to launch the system browser. In tests,
/// inject a closure that captures the URL and drives the loopback callback so the flow
/// can complete without a real browser (D-03).
///
/// # Security invariants
///
/// - S256 PKCE only — the auth URL is asserted to contain `code_challenge_method=S256`
///   before `open_fn` is called. The flow bails if the marker is absent (A-3).
/// - The PKCE verifier lives only in a local variable and is consumed by
///   `exchange_code`; it is never written to `auth.json` or any file (A-3, T-43-04).
/// - `state` is validated against the generated CSRF token (T-43-03).
/// - `iss`, if present in the callback, is validated against the issuer derived from
///   `authorization_url` (RFC 9207, T-43-03). When absent the flow proceeds.
/// - The loopback listener is bound to `127.0.0.1:0` and accepts exactly one connection
///   within 300 seconds (A-4, T-43-06).
/// - Failures log `ns` + error only — never a token value or the verifier (A-1).
///
/// # Errors
///
/// Returns `Err` on listener bind failure, timeout, state/iss mismatch, token exchange
/// failure, or token persistence failure.
#[cfg(not(target_arch = "wasm32"))]
pub async fn run_pkce_flow(
    store: &AuthStore,
    ns: &str,
    params: PkceFlowParams,
    open_fn: &dyn Fn(&str) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    use std::time::{Duration, SystemTime};

    use anyhow::Context as _;
    use oauth2::{
        AuthUrl, AuthorizationCode, ClientId, CsrfToken, PkceCodeChallenge, RedirectUrl, Scope,
        TokenResponse, TokenUrl, basic::BasicClient,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    // ── Bind loopback listener (A-4: port 0, OS assigns ephemeral port) ─────
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("failed to bind PKCE loopback listener on 127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    // ── A-3: Generate S256 PKCE challenge — verifier stays in memory only ──
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

    // ── Build OAuth2 client ──────────────────────────────────────────────────
    let oauth_client = BasicClient::new(ClientId::new(params.client_id.clone()))
        .set_auth_uri(
            AuthUrl::new(params.authorization_url.clone()).context("invalid authorization_url")?,
        )
        .set_token_uri(TokenUrl::new(params.token_url.clone()).context("invalid token_url")?)
        .set_redirect_uri(RedirectUrl::new(redirect_uri.clone()).context("invalid redirect_uri")?);

    // ── Build authorization URL (PKCE + CSRF state + scopes) ────────────────
    let mut auth_req = oauth_client
        .authorize_url(CsrfToken::new_random)
        .set_pkce_challenge(pkce_challenge);
    for scope in &params.scopes {
        auth_req = auth_req.add_scope(Scope::new(scope.clone()));
    }
    let (auth_url, csrf_token) = auth_req.url();
    let auth_url_str = auth_url.as_str();

    // ── A-3 gate: assert S256 in URL before opening browser ─────────────────
    if !auth_url_str.contains("code_challenge_method=S256") {
        anyhow::bail!(
            "authorization URL does not contain code_challenge_method=S256 — \
             aborting to prevent PKCE downgrade (A-3, ns={ns})"
        );
    }

    // ── D-04: open browser; print URL on failure and keep listener alive ─────
    if let Err(e) = open_fn(auth_url_str) {
        println!(
            "Browser could not be opened ({e:#}). \
             Open this URL manually to continue authorization:\n{auth_url_str}"
        );
        // Rule 2: flush stdout immediately so the URL is visible before the 300-second
        // wait begins. Without an explicit flush, block-buffered stdout (the default when
        // piped) would hold the URL in userspace until the buffer fills or the process
        // exits — making the URL invisible to the operator during the wait window.
        {
            use std::io::Write as _;
            let _ = std::io::stdout().flush();
        }
    }

    // ── A-4: accept exactly one callback with a 300-second timeout ───────────
    let (mut stream, _peer) = tokio::time::timeout(Duration::from_secs(300), listener.accept())
        .await
        .context("PKCE loopback listener timed out after 300 seconds — no callback received")?
        .context("failed to accept loopback callback connection")?;

    // ── Read the HTTP request (up to 8 KiB) ─────────────────────────────────
    let mut buf = [0u8; 8192];
    let n = stream
        .read(&mut buf)
        .await
        .context("failed to read loopback callback request")?;
    let request_str = String::from_utf8_lossy(&buf[..n]);

    // Parse path+query from: "GET /callback?code=...&state=... HTTP/1.1"
    let path_and_query = request_str
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");

    // Use the `url` crate to parse query parameters (handles percent-encoding)
    let fake_base = format!("http://localhost{path_and_query}");
    let parsed_cb = url::Url::parse(&fake_base).context("failed to parse callback query string")?;

    let mut code: Option<String> = None;
    let mut returned_state: Option<String> = None;
    let mut returned_iss: Option<String> = None;
    for (key, value) in parsed_cb.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => returned_state = Some(value.into_owned()),
            "iss" => returned_iss = Some(value.into_owned()),
            _ => {}
        }
    }

    // ── Send a minimal 200 OK response before validation ────────────────────
    // (The browser needs a response; we keep the page copy generic.)
    let body = "Authorization complete \u{2014} you can close this tab.";
    let response_hdr = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n",
        body.len()
    );
    let _ = stream.write_all(response_hdr.as_bytes()).await;
    let _ = stream.write_all(body.as_bytes()).await;

    // ── T-43-03: Validate CSRF state ─────────────────────────────────────────
    let returned_state =
        returned_state.ok_or_else(|| anyhow::anyhow!("no state parameter in PKCE callback"))?;
    if returned_state != *csrf_token.secret() {
        anyhow::bail!("PKCE callback state mismatch — possible CSRF attack (ns={ns})");
    }

    // ── RFC 9207 / T-43-03: Validate iss when present (best-effort) ──────────
    if let Some(iss) = returned_iss {
        let auth_url_parsed = url::Url::parse(&params.authorization_url)
            .context("failed to parse authorization_url for iss validation")?;
        let expected_issuer = auth_url_parsed.origin().unicode_serialization();
        if iss != expected_issuer {
            anyhow::bail!(
                "PKCE callback iss mismatch: got '{iss}', \
                 expected '{expected_issuer}' (RFC 9207, ns={ns})"
            );
        }
    }

    // ── Extract authorization code ───────────────────────────────────────────
    let code = code.ok_or_else(|| anyhow::anyhow!("no code parameter in PKCE callback"))?;

    // ── Build reqwest HTTP client (STACK.md §1: no redirect following) ───────
    let http_client = reqwest::ClientBuilder::new()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("failed to build reqwest client for token exchange")?;

    // ── A-3: Exchange code — pkce_verifier consumed here, never to disk ──────
    let token_response = oauth_client
        .exchange_code(AuthorizationCode::new(code))
        .set_pkce_verifier(pkce_verifier)
        .request_async(&http_client)
        .await
        .map_err(|e| anyhow::anyhow!("token exchange failed (ns={ns}): {e}"))?;

    // ── A-6: Compute expires_at = now + expires_in ────────────────────────────
    let expires_at = token_response.expires_in().map(|d| SystemTime::now() + d);
    let access_token = token_response.access_token().secret().to_string();
    let refresh_token = token_response
        .refresh_token()
        .map(|t| t.secret().to_string());
    let scopes: Vec<String> = token_response
        .scopes()
        .map(|ss| ss.iter().map(|s| s.to_string()).collect())
        .unwrap_or_else(|| params.scopes.clone());

    // ── T-43-04: Persist token — verifier is consumed above, never appears here
    store
        .put_token(
            ns,
            TokenEntry {
                access_token,
                refresh_token,
                expires_at,
                scopes,
            },
        )
        .await
        .with_context(|| format!("failed to persist PKCE token for ns={ns}"))?;

    Ok(())
}

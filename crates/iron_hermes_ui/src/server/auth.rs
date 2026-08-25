//! Operator authentication for iron_hermes_ui (ADR-001 / AUTH-DESIGN.md).
//!
//! Ported from `.planning/phases/47.3-login-page/auth.rs` (Phase 47.3 Plan 01,
//! D-06/D-07). Design invariants (see AUTH-DESIGN §2/§3):
//! * Deny-by-default: the middleware allowlists exactly the static shell and
//!   the two public auth endpoints, plus the login page's own theme-scoped
//!   assets. EVERYTHING else needs a session cookie. New routes added in
//!   later phases are protected automatically.
//! * `/auth/*` are raw axum routes (server-fn JSON-codec pitfall — same
//!   precedent as `/artifacts/{id}`).
//! * Opaque server-side tokens: logout and process restart are true
//!   revocations. No JWT, no signing keys (ADR-001 Option D rejected).
//! * The token never exists in JS-readable space: `HttpOnly; SameSite=Strict`
//!   cookie only. Never localStorage (code-review finding #3).
//! * Auth is opt-in: no configured hash => `enabled() == false` => middleware
//!   passes everything (today's loopback-only posture stands). main.rs must
//!   refuse a public bind when disabled (fail-closed upgrade of the old warn).
//! * The pre-auth asset allowlist is derived from `login_page::public_asset_paths`
//!   — an `asset!()`-const-backed set, never a `starts_with("/assets/")` /
//!   `starts_with("/wasm/")` prefix rule (RESEARCH.md Pitfall 2 / Pattern 3).

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use argon2::password_hash::PasswordHash;
use argon2::{Argon2, PasswordVerifier};
use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use parking_lot::RwLock;
use rand::RngCore as _;
use serde::Deserialize;

/// Cookie name for the operator session.
pub const SESSION_COOKIE: &str = "ih_session";

/// Server-side cap on submitted password length (DoS guard — the /auth/*
/// routes additionally carry their own DefaultBodyLimit(4096), see main.rs).
const MAX_PASSWORD_LEN: usize = 1024;

/// Fixed-window login rate limit: 5 attempts per 60s per peer IP.
const LOGIN_WINDOW: Duration = Duration::from_secs(60);
const LOGIN_MAX_ATTEMPTS: u32 = 5;

/// Uniform failure delay — no timing oracle between "bad password" and
/// "rate limited but not yet tripped" paths.
const FAILURE_SLEEP: Duration = Duration::from_millis(500);

// ---------------------------------------------------------------------------
// Config -> AuthState
// ---------------------------------------------------------------------------

/// Resolved auth configuration. Built once at startup by [`auth_config_from`]
/// from `web_ui.auth.*` (config.yaml) > `IRONHERMES_WEB_PASSWORD_HASH` env >
/// vault `SecretStore` fallback — mirrors `ProviderResolver::apply_vault_fallback`'s
/// layering, including the hard error when vault is enabled-but-sealed (never
/// silently run authless when auth was configured).
pub struct AuthConfig {
    /// argon2id PHC string (e.g. `$argon2id$v=19$m=19456,t=2,p=1$...`).
    /// `None` => auth disabled (loopback-only posture must be enforced by the
    /// bind gate in main.rs).
    pub password_hash: Option<String>,
    /// D-01/D-02: the configured login treatment slug.
    pub login_theme: String,
    /// Adds `Secure` to the cookie. Default false (plain-LAN HTTP would
    /// otherwise never send it back). Set true behind TLS.
    pub cookie_secure: bool,
    /// Absolute session lifetime. Default 7 days.
    pub session_ttl: Duration,
    /// Sliding idle timeout. Default 24 h.
    pub idle_timeout: Duration,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            password_hash: None,
            login_theme: "basic".to_string(),
            cookie_secure: false,
            session_ttl: Duration::from_secs(7 * 24 * 3600),
            idle_timeout: Duration::from_secs(24 * 3600),
        }
    }
}

/// Resolve [`AuthConfig`] from the layered `password_hash` sources: `config.yaml`
/// (`web_ui.auth.password_hash`) > `IRONHERMES_WEB_PASSWORD_HASH` env >
/// vault `SecretStore` key `web_ui/auth/password_hash`.
///
/// Mirrors `ProviderResolver::apply_vault_fallback` (`ironhermes-core/src/provider.rs`)
/// exactly: a sealed/corrupt vault propagates a hard error via `?`; only a
/// healthy vault returning `Ok(None)` counts as "not configured" (Pitfall 4).
/// Never wrap the vault call in `.ok()` or `.unwrap_or_default()`.
pub async fn auth_config_from(config: &ironhermes_core::config::Config) -> anyhow::Result<AuthConfig> {
    use secrecy::ExposeSecret as _;

    let web_ui_auth = &config.web_ui.auth;
    let password_hash = if let Some(h) = &web_ui_auth.password_hash {
        Some(h.clone())
    } else if let Ok(h) = std::env::var("IRONHERMES_WEB_PASSWORD_HASH") {
        Some(h)
    } else if config.vault.enabled {
        let store = ironhermes_vault::open_store(&ironhermes_core::resolve_vault_config(config))?;
        // `?` propagates a sealed/corrupt-vault Err loudly (Pitfall 4); Ok(None)
        // means a healthy vault with the key genuinely absent — "not configured".
        store
            .get_secret("web_ui/auth/password_hash")
            .await?
            .map(|s| s.expose_secret().to_string())
    } else {
        None
    };

    Ok(AuthConfig {
        password_hash,
        login_theme: web_ui_auth.login_theme.clone(),
        cookie_secure: web_ui_auth.cookie_secure,
        session_ttl: Duration::from_secs(web_ui_auth.session_ttl_hours.saturating_mul(3600)),
        idle_timeout: Duration::from_secs(web_ui_auth.idle_timeout_hours.saturating_mul(3600)),
    })
}

struct SessionEntry {
    created: Instant,
    last_seen: Instant,
}

/// Shared auth state — one per process, injected into the middleware and
/// the /auth/* handlers via `from_fn_with_state` / `State`, and into
/// `root_handler` via `Extension`.
pub struct AuthState {
    config: AuthConfig,
    /// Derived at construction from `login_page::public_asset_paths` — the
    /// pre-auth asset surface. Stored so the middleware does a hash lookup,
    /// never a rebuild (RESEARCH.md Pattern 2).
    login_assets: HashSet<String>,
    sessions: RwLock<HashMap<String, SessionEntry>>,
    login_attempts: RwLock<HashMap<IpAddr, (Instant, u32)>>,
}

impl AuthState {
    /// Validates the PHC string eagerly. Returns Err on a malformed hash so
    /// startup fails loudly instead of locking the operator out at login.
    pub fn new(config: AuthConfig) -> Result<Arc<Self>, String> {
        if let Some(h) = &config.password_hash {
            PasswordHash::new(h)
                .map_err(|e| format!("web_ui.auth.password_hash is not a valid PHC string: {e}"))?;
        }
        let login_assets: HashSet<String> =
            crate::server::login_page::public_asset_paths(&config.login_theme)
                .into_iter()
                .collect();
        Ok(Arc::new(Self {
            config,
            login_assets,
            sessions: RwLock::new(HashMap::new()),
            login_attempts: RwLock::new(HashMap::new()),
        }))
    }

    pub fn enabled(&self) -> bool {
        self.config.password_hash.is_some()
    }

    /// The configured login treatment slug (D-02) — read by `root_handler`
    /// to select which theme's HTML to render.
    pub fn login_theme(&self) -> &str {
        &self.config.login_theme
    }

    fn verify_password(&self, candidate: &str) -> bool {
        let Some(hash_str) = &self.config.password_hash else {
            return false;
        };
        // Parse can't fail (validated in new()), but stay defensive.
        let Ok(parsed) = PasswordHash::new(hash_str) else {
            return false;
        };
        Argon2::default()
            .verify_password(candidate.as_bytes(), &parsed)
            .is_ok()
    }

    fn mint_session(&self) -> String {
        let mut raw = [0u8; 32];
        // OsRng: cryptographic randomness; never a seeded/thread RNG here.
        rand::rngs::OsRng.fill_bytes(&mut raw);
        let token = URL_SAFE_NO_PAD.encode(raw);
        let now = Instant::now();
        self.sessions.write().insert(
            token.clone(),
            SessionEntry {
                created: now,
                last_seen: now,
            },
        );
        token
    }

    /// Validates + slides the session. Refresh of `last_seen` is throttled
    /// (>=60 s between writes) so hot request paths take the read lock only.
    fn validate_session(&self, token: &str) -> bool {
        enum Verdict {
            Missing,
            Expired,
            FreshOk,
            RefreshOk,
        }
        let now = Instant::now();
        let verdict = {
            let sessions = self.sessions.read();
            match sessions.get(token) {
                None => Verdict::Missing,
                Some(e)
                    if now.duration_since(e.created) > self.config.session_ttl
                        || now.duration_since(e.last_seen) > self.config.idle_timeout =>
                {
                    Verdict::Expired
                }
                Some(e) if now.duration_since(e.last_seen) < Duration::from_secs(60) => {
                    Verdict::FreshOk // fresh enough; skip the write lock
                }
                Some(_) => Verdict::RefreshOk,
            }
        }; // read guard drops here — write paths below never overlap it
        match verdict {
            Verdict::Missing => false,
            Verdict::FreshOk => true,
            Verdict::Expired => {
                self.sessions.write().remove(token);
                false
            }
            Verdict::RefreshOk => {
                if let Some(e) = self.sessions.write().get_mut(token) {
                    e.last_seen = now;
                }
                true
            }
        }
    }

    fn remove_session(&self, token: &str) {
        self.sessions.write().remove(token);
    }

    /// Fixed-window limiter. Returns false when the caller is over budget.
    fn check_rate_limit(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut attempts = self.login_attempts.write();
        // Lazy prune keeps the map bounded without a background task.
        attempts.retain(|_, (start, _)| now.duration_since(*start) < LOGIN_WINDOW);
        let entry = attempts.entry(ip).or_insert((now, 0));
        if now.duration_since(entry.0) >= LOGIN_WINDOW {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= LOGIN_MAX_ATTEMPTS
    }
}

// ---------------------------------------------------------------------------
// Middleware — the deny-by-default boundary
// ---------------------------------------------------------------------------

/// Public surface. Everything NOT matched here requires a session.
///
/// `GET /` is public here because `root_handler` itself enforces the auth
/// split (Pattern 1) — the middleware doesn't need to (and must not try to)
/// distinguish authed/unauthed for that one path. `login_assets` is the
/// theme-scoped, `asset!()`-derived allowlist built once at `AuthState`
/// construction (RESEARCH.md Pattern 2/3) — this function MUST NOT fall back
/// to a `starts_with("/assets/")` / `starts_with("/wasm/")` prefix rule; that
/// is the exact regression Pitfall 2 documents (it would make the wasm
/// bundle, `matrix-woman.glb`, `three.module.js` and every app stylesheet
/// public). `/auth/logout` is intentionally NOT here: logging out an
/// operator should require being that operator.
///
/// Phase 48.2 Plan 09 (D-03/T-48.2-09-01): `GET /oauth/mcp/callback` is
/// public too, and for a structurally different reason than the rest of
/// this allowlist — not because it is pre-auth UI chrome, but because the
/// session cookie is `SameSite=Strict` (`session_cookie`, above) and is
/// therefore never sent on the authorization server's cross-site top-level
/// redirect back to this origin. Requiring a session here would make the
/// flow this route exists to serve impossible, not just inconvenient. Its
/// capability is the unguessable, single-use OAuth `state` instead of a
/// session (see `mcp_oauth_callback_route.rs`'s module doc for the full
/// argument). This is one exact-path `GET` arm, not a prefix rule — `POST`
/// on the same path is deliberately absent here and therefore still
/// requires a session (asserted by `deny_by_default_allowlist`, below).
fn is_public(method: &Method, path: &str, login_assets: &HashSet<String>) -> bool {
    match (method, path) {
        (&Method::GET, "/") => true,
        (&Method::GET, "/favicon.ico") => true,
        (&Method::POST, "/auth/login") => true,
        (&Method::GET, "/auth/session") => true,
        (&Method::GET, p) if p == crate::server::mcp_admin_api::MCP_OAUTH_CALLBACK_PATH => true,
        (&Method::GET, p) if login_assets.contains(p) => true,
        _ => false,
    }
}

/// D-13 defense-in-depth: reject requests to `/api/*` whose `Sec-Fetch-Dest`
/// header names an embedded browsing-context destination (`iframe` or
/// `frame`). This is a third containment layer on top of the already-doubly-
/// contained artifact-iframe risk (`ARTIFACT_CSP`'s `connect-src 'none'` +
/// the viewer's `sandbox` never granting `allow-same-origin`) — it still
/// holds if either of those regresses later.
///
/// A missing or unparseable header is PERMISSIVE (returns `false`, i.e. not
/// rejected): treating absence as an iframe would break every non-browser
/// caller and older browser that never sends this header at all. Scoped to
/// `/api/` only — `/artifacts/{id}` is deliberately framed by the artifact
/// viewer and must keep working regardless of `Sec-Fetch-Dest`.
fn is_iframe_originated_api_request(path: &str, headers: &HeaderMap) -> bool {
    if !path.starts_with("/api/") {
        return false;
    }
    headers
        .get("sec-fetch-dest")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == "iframe" || v == "frame")
}

fn session_token_from_headers(req: &Request<Body>) -> Option<String> {
    let cookies = req.headers().get(header::COOKIE)?.to_str().ok()?;
    cookies.split(';').find_map(|kv| {
        let (k, v) = kv.trim().split_once('=')?;
        (k == SESSION_COOKIE).then(|| v.to_string())
    })
}

/// Thin public wrapper over token-extraction + `validate_session` so
/// `login_page::root_handler` can check session validity without
/// duplicating cookie parsing.
pub fn session_is_valid(auth: &AuthState, req: &Request<Body>) -> bool {
    session_token_from_headers(req).is_some_and(|t| auth.validate_session(&t))
}

/// Register LAST (after every route registration) in main.rs — axum layers
/// wrap only previously-added routes:
/// `.layer(axum::middleware::from_fn_with_state(auth_state.clone(), require_auth))`
pub async fn require_auth(
    State(auth): State<Arc<AuthState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if !auth.enabled() || is_public(req.method(), req.uri().path(), &auth.login_assets) {
        return next.run(req).await;
    }
    match session_token_from_headers(&req) {
        Some(token) if auth.validate_session(&token) => {
            // D-13: Sec-Fetch-Dest defense-in-depth, scoped to /api/* only.
            // Checked here (after the session is proven valid, before
            // next.run) so a rejected request never reaches a server fn.
            if is_iframe_originated_api_request(req.uri().path(), req.headers()) {
                tracing::warn!(
                    target: "iron_hermes_ui::auth",
                    path = %req.uri().path(),
                    "rejected iframe-originated /api/* request (Sec-Fetch-Dest)"
                );
                return StatusCode::FORBIDDEN.into_response();
            }
            next.run(req).await
        }
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}

// ---------------------------------------------------------------------------
// Handlers — raw axum routes (server-fn codec pitfall)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct LoginRequest {
    password: String,
}

fn session_cookie(auth: &AuthState, token: &str, max_age: i64) -> HeaderValue {
    let secure = if auth.config.cookie_secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure}"
    ))
    .expect("cookie header is ASCII")
}

/// POST /auth/login — body `{"password": "..."}`. 204 + Set-Cookie | 401 | 429.
/// Requires `into_make_service_with_connect_info::<SocketAddr>()` (main.rs).
pub async fn login(
    State(auth): State<Arc<AuthState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<LoginRequest>,
) -> Response {
    if !auth.enabled() {
        // Auth disabled => nothing to log into; treat as success-shaped no-op.
        return StatusCode::NO_CONTENT.into_response();
    }
    if !auth.check_rate_limit(peer.ip()) {
        tracing::warn!(target: "iron_hermes_ui::auth", ip = %peer.ip(), "login rate limit tripped");
        tokio::time::sleep(FAILURE_SLEEP).await;
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({"error": "too many attempts"})),
        )
            .into_response();
    }
    if body.password.len() > MAX_PASSWORD_LEN || !auth.verify_password(&body.password) {
        // Uniform copy + uniform delay: no oracle. NEVER log the password.
        tracing::warn!(target: "iron_hermes_ui::auth", ip = %peer.ip(), "login failed");
        tokio::time::sleep(FAILURE_SLEEP).await;
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "invalid credentials"})),
        )
            .into_response();
    }
    let token = auth.mint_session();
    tracing::info!(target: "iron_hermes_ui::auth", ip = %peer.ip(), "login ok");
    let max_age = auth.config.session_ttl.as_secs() as i64;
    (
        [(header::SET_COOKIE, session_cookie(&auth, &token, max_age))],
        StatusCode::NO_CONTENT,
    )
        .into_response()
}

/// GET /auth/session — 204 when the caller holds a valid session (or auth is
/// disabled, flagged with `x-ih-auth: disabled`); 401 otherwise. Public by
/// design: it leaks only "a login page exists here".
pub async fn session_probe(State(auth): State<Arc<AuthState>>, req: Request<Body>) -> Response {
    if !auth.enabled() {
        return (
            [("x-ih-auth", HeaderValue::from_static("disabled"))],
            StatusCode::NO_CONTENT,
        )
            .into_response();
    }
    match session_token_from_headers(&req) {
        Some(t) if auth.validate_session(&t) => StatusCode::NO_CONTENT.into_response(),
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}

/// POST /auth/logout — inside the boundary (middleware already validated).
pub async fn logout(State(auth): State<Arc<AuthState>>, req: Request<Body>) -> Response {
    if let Some(t) = session_token_from_headers(&req) {
        auth.remove_session(&t);
    }
    // Expire the cookie client-side too.
    (
        [(header::SET_COOKIE, session_cookie(&auth, "", 0))],
        StatusCode::NO_CONTENT,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use tower::ServiceExt as _;

    fn state_with_password(pw: &str) -> Arc<AuthState> {
        use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
        let salt = SaltString::generate(&mut OsRng);
        let hash = Argon2::default()
            .hash_password(pw.as_bytes(), &salt)
            .unwrap()
            .to_string();
        AuthState::new(AuthConfig {
            password_hash: Some(hash),
            ..Default::default()
        })
        .unwrap()
    }

    #[test]
    fn disabled_when_no_hash() {
        let s = AuthState::new(AuthConfig::default()).unwrap();
        assert!(!s.enabled());
    }

    #[test]
    fn malformed_hash_is_a_boot_error() {
        let cfg = AuthConfig {
            password_hash: Some("not-a-phc-string".into()),
            ..Default::default()
        };
        assert!(AuthState::new(cfg).is_err());
    }

    #[test]
    fn verify_and_session_round_trip() {
        let s = state_with_password("hunter2");
        assert!(s.verify_password("hunter2"));
        assert!(!s.verify_password("hunter3"));
        let tok = s.mint_session();
        assert!(s.validate_session(&tok));
        s.remove_session(&tok);
        assert!(!s.validate_session(&tok), "logout must be a true revocation");
    }

    /// The load-bearing D-07 property: data surfaces are NOT public, and the
    /// login page's own path + the two public auth endpoints ARE. Built from
    /// the real, theme-scoped `public_asset_paths("basic")` — never a
    /// synthetic allowlist — so this test would fail if a future edit ever
    /// widened `is_public` back to a prefix rule.
    #[test]
    fn deny_by_default_allowlist() {
        let assets: HashSet<String> =
            crate::server::login_page::public_asset_paths("basic")
                .into_iter()
                .collect();
        for (m, p) in [
            (Method::POST, "/api/anything"),
            (Method::GET, "/ws"),
            (Method::GET, "/artifacts/abc"),
            (Method::GET, "/chat-attachments/s1/a1"),
            (Method::POST, "/auth/logout"),
            (Method::GET, "/some/future/route"),
            // Phase 48.2 Plan 09 (T-48.2-09-01): the hole is method-scoped —
            // POST on the callback path must still require a session, or a
            // later edit could silently widen it into a write-capable public
            // endpoint.
            (Method::POST, crate::server::mcp_admin_api::MCP_OAUTH_CALLBACK_PATH),
        ] {
            assert!(!is_public(&m, p, &assets), "{m} {p} must require a session");
        }
        for (m, p) in [
            (Method::GET, "/"),
            (Method::POST, "/auth/login"),
            (Method::GET, "/auth/session"),
            (Method::GET, crate::server::mcp_admin_api::MCP_OAUTH_CALLBACK_PATH),
        ] {
            assert!(is_public(&m, p, &assets), "{m} {p} must stay public");
        }
        // At least one real, theme-derived login asset must be public too —
        // proves the HashSet::contains branch actually fires, not just the
        // four hardcoded matches above.
        let a_login_asset = assets
            .iter()
            .next()
            .expect("basic theme must reference at least one asset")
            .clone();
        assert!(is_public(&Method::GET, &a_login_asset, &assets));
    }

    #[test]
    fn rate_limit_trips_at_six() {
        let s = state_with_password("x");
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        for _ in 0..LOGIN_MAX_ATTEMPTS {
            assert!(s.check_rate_limit(ip));
        }
        assert!(!s.check_rate_limit(ip));
    }

    // -------------------------------------------------------------------
    // Phase 47.3 Plan 01 Task 2 (D-07 / RESEARCH.md Pitfall 2, Correction 7):
    // prove the wasm bundle and heavy/non-login assets are NOT public, and
    // that the allowlist is theme-scoped (D-02).
    // -------------------------------------------------------------------

    /// Turns RESEARCH.md's Correction-7 warning into an enforced, tested
    /// invariant: the wasm bundle, a heavy 3D model, a vendored library, and
    /// the app's own stylesheets must all stay behind the auth boundary,
    /// even though they live under the same `/assets/` and `/wasm/` roots
    /// login's own assets live under. A hash-suffixed spelling of one of
    /// them is also asserted non-public, so a future dx content-hash change
    /// cannot accidentally satisfy the allowlist.
    #[test]
    fn asset_allowlist_excludes_bundle_and_heavy_assets() {
        let assets: HashSet<String> =
            crate::server::login_page::public_asset_paths("basic")
                .into_iter()
                .collect();
        for (m, p) in [
            (Method::GET, "/wasm/iron_hermes_ui_bg.wasm"),
            (Method::GET, "/wasm/iron_hermes_ui.js"),
            (Method::GET, "/assets/matrix-woman.glb"),
            (Method::GET, "/assets/three.module.js"),
            (Method::GET, "/assets/design-tokens.css"),
            (Method::GET, "/assets/warp-ih.css"),
            (Method::GET, "/assets/components.css"),
            // Hash-suffixed spelling: a future dx rebuild must not
            // accidentally make this satisfy the allowlist either.
            (Method::GET, "/wasm/iron_hermes_ui_bg-dxh1a2b3c4d.wasm"),
        ] {
            assert!(
                !is_public(&m, p, &assets),
                "{m} {p} must NOT be public — it is not part of the login page's own asset set"
            );
        }
        for p in &assets {
            assert!(
                is_public(&Method::GET, p, &assets),
                "every entry public_asset_paths yields must itself be public: {p}"
            );
        }
    }

    /// D-02: "the server emits only the selected theme" expressed as a
    /// pre-auth-surface property — `basic` must not leak the matrix-rain
    /// script or the globe image, and each theme that needs one must have it.
    #[test]
    fn public_asset_paths_are_theme_scoped() {
        let basic: HashSet<String> =
            crate::server::login_page::public_asset_paths("basic").into_iter().collect();
        let matrix_rain: HashSet<String> =
            crate::server::login_page::public_asset_paths("matrix-rain")
                .into_iter()
                .collect();
        let orbit_veil: HashSet<String> =
            crate::server::login_page::public_asset_paths("orbit-veil")
                .into_iter()
                .collect();

        assert!(
            !basic.iter().any(|p| p.contains("login-rain")),
            "basic must not reference the matrix-rain script"
        );
        assert!(
            !basic.iter().any(|p| p.contains("earth-night")),
            "basic must not reference the globe image"
        );
        assert!(
            matrix_rain.iter().any(|p| p.contains("login-rain")),
            "matrix-rain theme must reference the rain script"
        );
        assert!(
            orbit_veil.iter().any(|p| p.contains("earth-night")),
            "orbit-veil theme must reference the globe image"
        );
    }

    /// An unrecognized theme slug must fall back to `basic` — same allowlist,
    /// same rendered marker — rather than erroring.
    #[test]
    fn unknown_theme_falls_back_to_basic() {
        let basic = crate::server::login_page::public_asset_paths("basic");
        let unknown = crate::server::login_page::public_asset_paths("not-a-real-theme");
        assert_eq!(basic, unknown, "unknown theme must yield the same allowlist as basic");

        let basic_html = crate::server::login_page::login_html("basic");
        let unknown_html = crate::server::login_page::login_html("not-a-real-theme");
        assert_eq!(
            basic_html, unknown_html,
            "unknown theme must render byte-for-byte identical HTML to basic"
        );
    }

    // -------------------------------------------------------------------
    // Phase 47.3 Plan 02 Task 2 (D-13): Sec-Fetch-Dest defense-in-depth on
    // /api/*, plus the credential-endpoint contract (401 vs 429, uniform
    // copy, true logout revocation).
    // -------------------------------------------------------------------

    /// Table test for the predicate itself: covers all four `Sec-Fetch-Dest`
    /// rows from the plan's behavior block, including an explicit empty
    /// `HeaderMap` (no header at all) expecting `false` — absence must never
    /// be treated as an iframe.
    #[test]
    fn is_iframe_originated_predicate_table() {
        fn headers_with(value: Option<&str>) -> HeaderMap {
            let mut h = HeaderMap::new();
            if let Some(v) = value {
                h.insert("sec-fetch-dest", HeaderValue::from_str(v).unwrap());
            }
            h
        }

        // /api/* with an iframe/frame Sec-Fetch-Dest -> rejected.
        assert!(is_iframe_originated_api_request(
            "/api/anything",
            &headers_with(Some("iframe"))
        ));
        assert!(is_iframe_originated_api_request(
            "/api/anything",
            &headers_with(Some("frame"))
        ));
        // /api/* with the normal fetch() value -> allowed.
        assert!(!is_iframe_originated_api_request(
            "/api/anything",
            &headers_with(Some("empty"))
        ));
        // /api/* with NO Sec-Fetch-Dest header at all (empty HeaderMap) ->
        // allowed. Absence is not treated as an iframe.
        assert!(!is_iframe_originated_api_request("/api/anything", &headers_with(None)));
        assert!(
            !is_iframe_originated_api_request("/api/anything", &HeaderMap::new()),
            "an explicit empty HeaderMap must never be treated as iframe-originated"
        );
        // Scoped to /api/ only: /artifacts/{id} with an iframe-shaped header
        // must NOT be rejected by this predicate — the artifact viewer
        // legitimately frames that route.
        assert!(!is_iframe_originated_api_request(
            "/artifacts/abc",
            &headers_with(Some("iframe"))
        ));
    }

    /// Live-wiring test (not just the predicate in isolation — see project
    /// precedent on tests that verify their own assumptions): drives
    /// `require_auth` through a real tiny router via `tower::ServiceExt::oneshot`,
    /// proving the middleware itself rejects an iframe-originated `/api/*`
    /// request with a valid session, passes through the same route when the
    /// header is absent or `empty`, and leaves `/artifacts/{id}` unaffected.
    #[tokio::test]
    async fn sec_fetch_dest_rejects_iframe_originated_api_request() {
        async fn dummy_handler() -> StatusCode {
            StatusCode::OK
        }

        let auth = state_with_password("hunter2");
        let token = auth.mint_session();

        let router = axum::Router::new()
            .route("/api/anything", axum::routing::get(dummy_handler))
            .route("/artifacts/{id}", axum::routing::get(dummy_handler))
            .layer(axum::middleware::from_fn_with_state(auth.clone(), require_auth));

        let cookie = format!("{SESSION_COOKIE}={token}");

        // iframe-originated /api/* with a valid session -> 403, never
        // reaching the handler.
        let req = Request::builder()
            .method("GET")
            .uri("/api/anything")
            .header(header::COOKIE, &cookie)
            .header("sec-fetch-dest", "iframe")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        // Normal fetch() value -> allowed through.
        let req = Request::builder()
            .method("GET")
            .uri("/api/anything")
            .header(header::COOKIE, &cookie)
            .header("sec-fetch-dest", "empty")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // No Sec-Fetch-Dest header at all -> allowed through (absence is not
        // an iframe).
        let req = Request::builder()
            .method("GET")
            .uri("/api/anything")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // /artifacts/{id} with an iframe-shaped header and a valid session
        // -> allowed; the check is scoped to /api/* only.
        let req = Request::builder()
            .method("GET")
            .uri("/artifacts/abc")
            .header(header::COOKIE, &cookie)
            .header("sec-fetch-dest", "iframe")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// AUTH-DESIGN §3.3: 401 responses carry uniform copy regardless of
    /// failure reason. Drives the REAL `login()` handler (not a hand-built
    /// stub) across empty, 1-char, wrong, and 2KB passwords and asserts a
    /// single identical body string — the 2KB case in particular must not
    /// 500 (it trips the `MAX_PASSWORD_LEN` short-circuit before
    /// `verify_password` ever runs).
    #[tokio::test]
    async fn login_wrong_password_returns_uniform_401() {
        let auth = state_with_password("hunter2");
        let peer = |port: u16| SocketAddr::from(([127, 0, 0, 1], port));

        let cases: Vec<(String, u16)> = vec![
            (String::new(), 45001),
            ("a".to_string(), 45002),
            ("definitely-wrong".to_string(), 45003),
            ("x".repeat(2048), 45004),
        ];

        let mut bodies = Vec::new();
        for (pw, port) in cases {
            let resp = login(
                State(auth.clone()),
                ConnectInfo(peer(port)),
                Json(LoginRequest { password: pw }),
            )
            .await;
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "every failure case must be a uniform 401, never a 500"
            );
            let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            bodies.push(body);
        }
        for pair in bodies.windows(2) {
            assert_eq!(
                pair[0], pair[1],
                "401 body must be byte-identical across all failure reasons"
            );
        }
    }

    /// D-18: the sixth login attempt from one peer IP inside the 60s window
    /// returns 429 with a body distinct from the 401 body — drives the REAL
    /// `login()` handler six times from one `ConnectInfo` peer.
    #[tokio::test]
    async fn login_sixth_attempt_returns_429() {
        let auth = state_with_password("hunter2");
        let peer = SocketAddr::from(([127, 0, 0, 1], 45100));

        let mut last_401_body = None;
        let mut body_429 = None;
        for i in 0..6 {
            let resp = login(
                State(auth.clone()),
                ConnectInfo(peer),
                Json(LoginRequest {
                    password: "wrong".to_string(),
                }),
            )
            .await;
            if i < 5 {
                assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "attempt {i} must be 401");
                last_401_body = Some(to_bytes(resp.into_body(), usize::MAX).await.unwrap());
            } else {
                assert_eq!(
                    resp.status(),
                    StatusCode::TOO_MANY_REQUESTS,
                    "the sixth attempt must be 429"
                );
                body_429 = Some(to_bytes(resp.into_body(), usize::MAX).await.unwrap());
            }
        }
        assert_ne!(
            last_401_body.unwrap(),
            body_429.unwrap(),
            "429 body must be distinct from the 401 body"
        );
    }

    /// A token that validated before `logout` must not validate after —
    /// drives the REAL `logout` handler (not `remove_session` directly).
    #[tokio::test]
    async fn logout_revokes_session() {
        let auth = state_with_password("hunter2");
        let token = auth.mint_session();
        assert!(auth.validate_session(&token), "session must be valid before logout");

        let req = Request::builder()
            .method("POST")
            .uri("/auth/logout")
            .header(header::COOKIE, format!("{SESSION_COOKIE}={token}"))
            .body(Body::empty())
            .unwrap();
        let resp = logout(State(auth.clone()), req).await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(
            !auth.validate_session(&token),
            "logout must be a true revocation, not a client-side-only cookie clear"
        );
    }
}

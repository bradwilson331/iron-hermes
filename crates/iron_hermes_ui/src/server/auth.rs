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
use std::path::{Path, PathBuf};
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

/// Free budget before the growing backoff engages: this many consecutive
/// failed login attempts are met with a uniform 401, no `Retry-After`.
const LOGIN_MAX_ATTEMPTS: u32 = 5;

// D-07 / read_first: `ironhermes-gateway::rate_limiter::PerUserRateLimiter`
// was evaluated as the primitive for this curve and deliberately NOT
// reused, nor mirrored. That limiter is a continuous token bucket for
// smoothing inbound MESSAGE throughput (refill-per-second, D-20/D-21) — a
// different domain from a discrete, monotonically growing punitive delay
// keyed on CREDENTIAL-guess failures that must also survive a process
// restart (Task 2's persistence). Mirroring its shape would not reduce
// this file's own logic, and a dependency edge from `iron_hermes_ui` to
// `ironhermes-gateway` for one struct is not justified by that saving.
// This is a deliberately separate, second limiter — not undocumented
// drift of the kind this project's history warns against.

/// D-07 weakness 1 (growing, capped backoff — the operator's actual
/// request): the base delay for the first attempt past
/// `LOGIN_MAX_ATTEMPTS`. 2 seconds is imperceptible to a human who
/// mistyped their own password, but seeds a curve that reaches
/// `LOGIN_BACKOFF_CAP` in nine failures (doubling each time).
const LOGIN_BACKOFF_BASE: Duration = Duration::from_secs(2);

/// D-07 weakness 1: the delay never exceeds this, however many attempts
/// are made. 15 minutes makes an automated online guessing campaign
/// against the single operator password economically dead, while also
/// bounding — per T-49.1-04-05's accepted trade — how long a hammering
/// attacker can extend the (eventually global, Task 2) lockout: at most
/// this long, and cleared entirely by the next successful login.
const LOGIN_BACKOFF_CAP: Duration = Duration::from_secs(900);

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

/// Attempt bookkeeping — a monotonically growing `count` plus the
/// `Instant` of the most recent failure, never a fixed per-window boolean.
/// Two independent uses (D-07 weakness 2): [`AuthState::login_attempts`]
/// holds exactly ONE of these — the single GLOBAL entry that drives the
/// 401/429 verdict — while [`AuthState::login_attempt_sources`] holds one
/// PER SOURCE IP, for diagnostics only, never consulted for the verdict.
/// `last_failure` drives only the decay logic in
/// `AuthState::check_rate_limit`; the backoff curve itself is a pure
/// function of `count` (see `login_backoff_delay`), never of real elapsed
/// time.
struct LoginAttemptEntry {
    count: u32,
    last_failure: Instant,
}

/// D-07 weakness 3: the on-disk shape of the persisted global attempt
/// state. Wall-clock (`u64` seconds since `UNIX_EPOCH`), never `Instant`
/// — `Instant`'s epoch is arbitrary per process and cannot be
/// reconstructed after the very restart this state exists to survive.
/// Deliberately just these two fields: never anything derived from the
/// presented password (acceptance criterion: the serialised form has
/// exactly these two keys, asserted in
/// `tests::login_attempt_state_file_is_mode_0600`).
#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedLoginAttempts {
    count: u32,
    last_failure_unix_secs: u64,
}

/// D-07 weakness 3: where the persisted global attempt state lives — under
/// the resolved IronHermes home (`ironhermes_core::get_hermes_home()`, env
/// var `IRONHERMES_HOME`; the older single-word `HERMES_HOME` form was
/// removed from this codebase and must never be reintroduced), never next
/// to the binary. Resolved once, at `AuthState::new` time, and cached in
/// `AuthState::login_attempts_path` — never re-resolved per request, so a
/// test-only env var override only needs to be in effect during
/// construction, not for the returned `AuthState`'s whole lifetime.
fn login_attempts_path() -> PathBuf {
    ironhermes_core::get_hermes_home().join("login_attempts.json")
}

/// D-07 weakness 3: loads the persisted global attempt state, failing
/// CLOSED on anything but a clean "file does not exist yet" (first-ever
/// use — no attack surface, genuinely zero). A corrupt or unreadable file
/// is treated as already at-or-past the threshold, never as zero:
/// resetting to zero on a parse failure would hand an attacker a bypass
/// (corrupt the file, get a fresh budget) — T-49.1-04-07.
fn load_persisted_login_attempts(path: &Path) -> LoginAttemptEntry {
    let now = Instant::now();
    match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => LoginAttemptEntry { count: 0, last_failure: now },
        Err(e) => {
            tracing::warn!(
                target: "iron_hermes_ui::auth",
                path = %path.display(),
                error = %e,
                "login attempt state file unreadable — failing closed (treating as at the threshold)"
            );
            LoginAttemptEntry {
                count: LOGIN_MAX_ATTEMPTS + 1,
                last_failure: now,
            }
        }
        Ok(contents) => match serde_json::from_str::<PersistedLoginAttempts>(&contents) {
            Ok(p) => LoginAttemptEntry {
                count: p.count,
                last_failure: now,
            },
            Err(e) => {
                tracing::warn!(
                    target: "iron_hermes_ui::auth",
                    path = %path.display(),
                    error = %e,
                    "login attempt state file corrupt — failing closed (treating as at the threshold)"
                );
                LoginAttemptEntry {
                    count: LOGIN_MAX_ATTEMPTS + 1,
                    last_failure: now,
                }
            }
        },
    }
}

/// D-07 weakness 3: writes the global attempt state on every change (not
/// on a timer — the write is tiny and infrequent by definition). Contains
/// exactly a count and a wall-clock timestamp, nothing derived from the
/// presented password. Best-effort: a write failure is logged, never
/// panics the request path.
fn persist_login_attempts(path: &Path, entry: &LoginAttemptEntry) {
    let last_failure_unix_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let persisted = PersistedLoginAttempts {
        count: entry.count,
        last_failure_unix_secs,
    };
    let Ok(json) = serde_json::to_string(&persisted) else {
        tracing::warn!(target: "iron_hermes_ui::auth", "failed to serialize login attempt state");
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!(
                target: "iron_hermes_ui::auth",
                path = %parent.display(),
                error = %e,
                "failed to create login attempt state directory"
            );
            return;
        }
    }
    if let Err(e) = std::fs::write(path, json) {
        tracing::warn!(
            target: "iron_hermes_ui::auth",
            path = %path.display(),
            error = %e,
            "failed to persist login attempt state"
        );
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            tracing::warn!(
                target: "iron_hermes_ui::auth",
                path = %path.display(),
                error = %e,
                "failed to set login attempt state file permissions"
            );
        }
    }
}

/// Verdict from [`AuthState::check_rate_limit`]. `Limited` carries the
/// caller-facing delay, already clamped to `LOGIN_BACKOFF_CAP`.
/// `verify_password` is never invoked on this path — even a correct
/// password is rejected while the caller is over budget, matching the
/// pre-existing threshold-boundary contract.
enum RateLimitVerdict {
    /// Under the free budget — proceed to password verification.
    Allowed,
    /// Over budget — reject with this `Retry-After` delay.
    Limited(Duration),
}

/// D-07 weakness 1: exponential curve, base `LOGIN_BACKOFF_BASE`, doubling
/// once per failure past `LOGIN_MAX_ATTEMPTS`, clamped to
/// `LOGIN_BACKOFF_CAP`. `count` is the just-incremented persistent failure
/// count (always `> LOGIN_MAX_ATTEMPTS` when this is called), so the very
/// first over-threshold failure (`count == LOGIN_MAX_ATTEMPTS + 1`) yields
/// exponent 0, i.e. exactly `LOGIN_BACKOFF_BASE`.
fn login_backoff_delay(count: u32) -> Duration {
    debug_assert!(count > LOGIN_MAX_ATTEMPTS);
    let exponent = count - LOGIN_MAX_ATTEMPTS - 1;
    let multiplier = 1u64.checked_shl(exponent).unwrap_or(u64::MAX);
    let secs = LOGIN_BACKOFF_BASE.as_secs().saturating_mul(multiplier);
    Duration::from_secs(secs).min(LOGIN_BACKOFF_CAP)
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
    /// D-07 weakness 2: the 401/429 VERDICT is driven by this single
    /// global counter, never by source IP — IronHermes has exactly one
    /// operator password, so per-IP keying is the multi-tenant shape
    /// applied to a single-tenant system, and lets an attacker rotating
    /// source addresses reset their own budget for free. Persisted to
    /// `login_attempts_path` on every change (D-07 weakness 3).
    login_attempts: RwLock<LoginAttemptEntry>,
    /// Diagnostics ONLY — which source addresses have attempted, and when.
    /// NEVER consulted for the verdict, and never persisted: only the
    /// punitive global count itself needs to survive a restart.
    login_attempt_sources: RwLock<HashMap<IpAddr, LoginAttemptEntry>>,
    /// `login_attempts_path()` resolved once, here, and cached — see that
    /// function's doc for why re-resolving per request is unnecessary and
    /// why tests can rely on a construction-scoped env var override.
    login_attempts_path: PathBuf,
}

impl AuthState {
    /// Validates the PHC string eagerly. Returns Err on a malformed hash so
    /// startup fails loudly instead of locking the operator out at login.
    ///
    /// D-07 weakness 3: also rehydrates the persisted global attempt state
    /// from `login_attempts_path()` — this is the "construction" main.rs
    /// calls this from, so no separate rehydration step is needed there.
    pub fn new(config: AuthConfig) -> Result<Arc<Self>, String> {
        if let Some(h) = &config.password_hash {
            PasswordHash::new(h)
                .map_err(|e| format!("web_ui.auth.password_hash is not a valid PHC string: {e}"))?;
        }
        let login_assets: HashSet<String> =
            crate::server::login_page::public_asset_paths(&config.login_theme)
                .into_iter()
                .collect();
        let login_attempts_path = login_attempts_path();
        let login_attempts = load_persisted_login_attempts(&login_attempts_path);
        Ok(Arc::new(Self {
            config,
            login_assets,
            sessions: RwLock::new(HashMap::new()),
            login_attempts: RwLock::new(login_attempts),
            login_attempt_sources: RwLock::new(HashMap::new()),
            login_attempts_path,
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

    /// Growing, capped backoff limiter (D-07 weaknesses 1-3). Every call
    /// advances the persistent GLOBAL failure count by one — including
    /// calls that are themselves rejected, since a rejection is itself the
    /// signal that the caller is still hammering. `ip` is recorded ONLY in
    /// the diagnostic side-map below; the verdict never consults it
    /// (weakness 2). The curve is a pure function of the global count,
    /// never of real elapsed time: `login_backoff_delay` clamps it to
    /// `LOGIN_BACKOFF_CAP`, which is what actually bounds how far a
    /// hammering caller can push the delay — not a real-time "have you
    /// waited long enough" gate, which would either have to block the
    /// never-`.await`-a-sleep-on-this-path request handler (D-07's
    /// explicit prohibition) or need a background task to enforce.
    fn check_rate_limit(&self, ip: IpAddr) -> RateLimitVerdict {
        let now = Instant::now();
        let verdict = {
            let mut attempts = self.login_attempts.write();
            // Full decay: a whole LOGIN_BACKOFF_CAP of silence resets the
            // count to zero — same spirit as the old per-key retain
            // sweep, now applied to the single global entry.
            if now.duration_since(attempts.last_failure) >= LOGIN_BACKOFF_CAP {
                attempts.count = 0;
            }
            attempts.count += 1;
            attempts.last_failure = now;
            if attempts.count <= LOGIN_MAX_ATTEMPTS {
                RateLimitVerdict::Allowed
            } else {
                RateLimitVerdict::Limited(login_backoff_delay(attempts.count))
            }
        };
        // D-07 weakness 3: persist the new global state on every change,
        // not on a timer — the write is tiny and infrequent by definition.
        persist_login_attempts(&self.login_attempts_path, &self.login_attempts.read());

        // Diagnostics ONLY (D-07 weakness 2, Test 2): recorded for every
        // call regardless of verdict, but never read back to decide 401
        // vs 429 — no operational signal is lost by moving the verdict to
        // a global counter.
        let mut sources = self.login_attempt_sources.write();
        let source = sources.entry(ip).or_insert(LoginAttemptEntry { count: 0, last_failure: now });
        source.count += 1;
        source.last_failure = now;

        verdict
    }

    /// D-07 weakness 1, Test 5 / weakness 3: a successful login clears the
    /// GLOBAL attempt state entirely, and persists the clear — the next
    /// failure starts back at 401, not 429. Also drops `ip`'s diagnostic
    /// record, since it just proved itself legitimate; other sources'
    /// diagnostic history is left alone.
    fn clear_login_attempts(&self, ip: IpAddr) {
        {
            let mut attempts = self.login_attempts.write();
            attempts.count = 0;
            attempts.last_failure = Instant::now();
        }
        persist_login_attempts(&self.login_attempts_path, &self.login_attempts.read());
        self.login_attempt_sources.write().remove(&ip);
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
            // D-14 websocket-security (T-49.1-08-05): CSWSH defense-in-depth
            // on the WS upgrade endpoints (path list + predicate owned by
            // kanban_ws.rs — see that module's doc for why the check lives
            // here rather than inside ws_kanban's own body). Checked here,
            // after the session is proven valid, so a rejected upgrade
            // never reaches ws_kanban/ws_chat and never completes the WS
            // handshake. Scoped to the two known WS paths only — every
            // other route is unaffected.
            if let Some(host) = req.headers().get(header::HOST).and_then(|v| v.to_str().ok()) {
                let expected =
                    crate::server::kanban_ws::expected_ws_origin(host, auth.config.cookie_secure);
                let origin = req.headers().get(header::ORIGIN).and_then(|v| v.to_str().ok());
                if crate::server::kanban_ws::is_cross_origin_ws_upgrade(
                    req.uri().path(),
                    origin,
                    &expected,
                ) {
                    tracing::warn!(
                        target: "iron_hermes_ui::auth",
                        path = %req.uri().path(),
                        origin = origin.unwrap_or("<none>"),
                        expected = %expected,
                        "rejected cross-origin WebSocket upgrade (CSWSH defense-in-depth)"
                    );
                    return StatusCode::FORBIDDEN.into_response();
                }
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
    if let RateLimitVerdict::Limited(retry_after) = auth.check_rate_limit(peer.ip()) {
        // D-07: the delay is ADVERTISED via Retry-After, never enforced by
        // holding this response open — that would turn the defence into a
        // connection-exhaustion vector (T-49.1-04-04). FAILURE_SLEEP below
        // is the pre-existing uniform anti-timing-oracle delay (500ms),
        // unrelated to and far shorter than the computed backoff.
        tracing::warn!(
            target: "iron_hermes_ui::auth",
            ip = %peer.ip(),
            retry_after_secs = retry_after.as_secs(),
            "login rate limit tripped"
        );
        tokio::time::sleep(FAILURE_SLEEP).await;
        let retry_after_header = HeaderValue::from_str(&retry_after.as_secs().to_string())
            .expect("a whole-second integer formats as valid ASCII");
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, retry_after_header)],
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
    // D-07 weakness 1, Test 5: a successful login is a true reset, not
    // just a bypass of this check — the next failure must start at 401.
    auth.clear_login_attempts(peer.ip());
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

    /// RAII guard that sets an env var and restores the previous value on
    /// drop. Copied verbatim from `ironhermes-kanban/src/paths.rs:449-475`
    /// (same pattern `profile_api.rs`'s own test module copies),
    /// including its safety comment.
    struct ScopedEnv {
        key: String,
        prev: Option<String>,
    }

    impl ScopedEnv {
        fn set(key: &str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: single-threaded test context; no concurrent env access.
            unsafe { std::env::set_var(key, value) };
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: single-threaded test context; no concurrent env access.
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(&self.key, v) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }

    /// D-07 weakness 3: `AuthState::new` resolves and CACHES the persisted
    /// login-attempt state path at construction time (never re-resolved
    /// per request) — so `IRONHERMES_HOME` only needs to be correct for
    /// the duration of this one call, not for the returned `AuthState`'s
    /// whole lifetime; the `ScopedEnv` guard is dropped (restoring the
    /// real value) before this function returns. `home` must stay valid
    /// for as long as the caller uses the returned `AuthState`, since the
    /// cached path points into it — callers that need explicit control
    /// over `home` (persistence round-trip tests) use this directly;
    /// `state_with_password` below is the common case with a fresh,
    /// intentionally-leaked tempdir per call.
    fn state_with_password_at(pw: &str, home: &std::path::Path) -> Arc<AuthState> {
        use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
        let salt = SaltString::generate(&mut OsRng);
        let hash = Argon2::default()
            .hash_password(pw.as_bytes(), &salt)
            .unwrap()
            .to_string();
        let _env_guard = ScopedEnv::set("IRONHERMES_HOME", home.to_str().expect("tempdir path is valid UTF-8"));
        AuthState::new(AuthConfig {
            password_hash: Some(hash),
            ..Default::default()
        })
        .unwrap()
    }

    /// Every test in this module must NEVER touch the real operator
    /// `~/.ironhermes` (D-06) — `.keep()` deliberately disables the
    /// tempdir's auto-delete-on-drop, since the path stays cached inside
    /// the returned `AuthState` for the rest of whichever test called
    /// this helper (see `state_with_password_at`'s doc for why that's
    /// sound even though the env var itself is only set momentarily).
    fn state_with_password(pw: &str) -> Arc<AuthState> {
        let home = tempfile::tempdir().expect("tempdir").keep();
        state_with_password_at(pw, &home)
    }

    #[test]
    fn disabled_when_no_hash() {
        // Auth is disabled here (no password hash), so check_rate_limit /
        // persist_login_attempts are never reached — but AuthState::new
        // itself still resolves login_attempts_path() and attempts a
        // (read-only) rehydration, so this still gets the same isolated
        // home as every other test in this module.
        let home = tempfile::tempdir().expect("tempdir").keep();
        let _env_guard = ScopedEnv::set("IRONHERMES_HOME", home.to_str().expect("tempdir path is valid UTF-8"));
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
            assert!(matches!(s.check_rate_limit(ip), RateLimitVerdict::Allowed));
        }
        assert!(matches!(s.check_rate_limit(ip), RateLimitVerdict::Limited(_)));
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

    // -------------------------------------------------------------------
    // Phase 49.1 Plan 04 Task 1 (D-07 weakness 1): growing, capped backoff
    // advertised via Retry-After. Extends the login_sixth_attempt_returns_429
    // family above rather than replacing it — that test already covers
    // "attempts 1-5 are 401, the 6th is 429" (behavior Test 1).
    // -------------------------------------------------------------------

    /// Test 2: attempts 6, 7 and 8 each return 429 with a `Retry-After`
    /// header, and the values strictly increase — proves the curve grows
    /// with each failure past the threshold, not a flat re-trip of a fixed
    /// window.
    #[tokio::test]
    async fn login_retry_after_strictly_increases_past_threshold() {
        let auth = state_with_password("hunter2");
        let peer = SocketAddr::from(([127, 0, 0, 1], 45200));

        let mut retry_afters = Vec::new();
        for i in 0..8 {
            let resp = login(
                State(auth.clone()),
                ConnectInfo(peer),
                Json(LoginRequest {
                    password: "wrong".to_string(),
                }),
            )
            .await;
            if i >= 5 {
                assert_eq!(
                    resp.status(),
                    StatusCode::TOO_MANY_REQUESTS,
                    "attempt {} must be 429",
                    i + 1
                );
                let value: u64 = resp
                    .headers()
                    .get(header::RETRY_AFTER)
                    .expect("429 response must carry Retry-After")
                    .to_str()
                    .expect("Retry-After must be ASCII")
                    .parse()
                    .expect("Retry-After must be a whole-second integer, not a string compare");
                retry_afters.push(value);
            }
        }
        assert_eq!(retry_afters.len(), 3, "attempts 6, 7, 8 must all be 429");
        assert!(
            retry_afters[1] > retry_afters[0],
            "retry_after(7) must exceed retry_after(6): {retry_afters:?}"
        );
        assert!(
            retry_afters[2] > retry_afters[1],
            "retry_after(8) must exceed retry_after(7): {retry_afters:?}"
        );
    }

    /// Test 3: the `Retry-After` value never exceeds the declared cap,
    /// however many attempts are made — drives 20 attempts and checks the
    /// last value equals `LOGIN_BACKOFF_CAP`.
    #[tokio::test]
    async fn login_retry_after_caps_at_declared_max() {
        let auth = state_with_password("hunter2");
        let peer = SocketAddr::from(([127, 0, 0, 1], 45201));

        let mut last_retry_after = None;
        for _ in 0..20 {
            let resp = login(
                State(auth.clone()),
                ConnectInfo(peer),
                Json(LoginRequest {
                    password: "wrong".to_string(),
                }),
            )
            .await;
            if resp.status() == StatusCode::TOO_MANY_REQUESTS {
                let value: u64 = resp
                    .headers()
                    .get(header::RETRY_AFTER)
                    .expect("429 response must carry Retry-After")
                    .to_str()
                    .expect("Retry-After must be ASCII")
                    .parse()
                    .expect("Retry-After must be a whole-second integer");
                last_retry_after = Some(value);
            }
        }
        assert_eq!(
            last_retry_after,
            Some(LOGIN_BACKOFF_CAP.as_secs()),
            "after 20 attempts the delay must have reached the declared cap"
        );
    }

    /// Test 4 (47.3 D-18 re-asserted): the 429 body stays distinct from the
    /// 401 body, and neither ever echoes the presented password.
    #[tokio::test]
    async fn login_429_body_distinct_and_never_echoes_password() {
        let auth = state_with_password("hunter2");
        let peer = SocketAddr::from(([127, 0, 0, 1], 45202));
        let secret_guess = "definitely-not-hunter2-xyz";

        let mut body_401 = None;
        let mut body_429 = None;
        for i in 0..7 {
            let resp = login(
                State(auth.clone()),
                ConnectInfo(peer),
                Json(LoginRequest {
                    password: secret_guess.to_string(),
                }),
            )
            .await;
            let status = resp.status();
            let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            let body_str = String::from_utf8_lossy(&body);
            assert!(
                !body_str.contains(secret_guess),
                "response body must never echo the presented password (attempt {i})"
            );
            if status == StatusCode::UNAUTHORIZED {
                body_401 = Some(body);
            } else if status == StatusCode::TOO_MANY_REQUESTS {
                body_429 = Some(body);
            }
        }
        assert_ne!(
            body_401.expect("must have seen at least one 401"),
            body_429.expect("must have seen at least one 429"),
            "429 body must remain distinct from the 401 body"
        );
    }

    /// Test 5: a successful login clears the attempt state, so the next
    /// failed attempt starts back at 401, not 429. Drives 4 failures (all
    /// still under the free budget, so this diverges from the pre-fix
    /// binary only via the reset — without a reset the 6th call below
    /// would tip the persistent counter from 5 to 6 and return 429).
    #[tokio::test]
    async fn login_success_resets_attempt_state() {
        let auth = state_with_password("hunter2");
        let peer = SocketAddr::from(([127, 0, 0, 1], 45203));

        for i in 0..4 {
            let resp = login(
                State(auth.clone()),
                ConnectInfo(peer),
                Json(LoginRequest {
                    password: "wrong".to_string(),
                }),
            )
            .await;
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "attempt {i} must be 401");
        }

        let resp = login(
            State(auth.clone()),
            ConnectInfo(peer),
            Json(LoginRequest {
                password: "hunter2".to_string(),
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT, "correct password must succeed");

        let resp = login(
            State(auth.clone()),
            ConnectInfo(peer),
            Json(LoginRequest {
                password: "wrong-again".to_string(),
            }),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "the next failure after a success must start at 401, not 429 — success must reset state"
        );
    }

    /// Test 6: the delay is advertised, never enforced by blocking — the
    /// handler returns promptly even once the computed delay has reached
    /// the multi-minute cap.
    #[tokio::test]
    async fn login_never_blocks_on_computed_backoff() {
        let auth = state_with_password("hunter2");
        let peer = SocketAddr::from(([127, 0, 0, 1], 45204));

        // Drive well past the point where the curve caps at LOGIN_BACKOFF_CAP.
        for _ in 0..15 {
            let _ = login(
                State(auth.clone()),
                ConnectInfo(peer),
                Json(LoginRequest {
                    password: "wrong".to_string(),
                }),
            )
            .await;
        }

        let start = std::time::Instant::now();
        let resp = login(
            State(auth.clone()),
            ConnectInfo(peer),
            Json(LoginRequest {
                password: "wrong".to_string(),
            }),
        )
        .await;
        let elapsed = start.elapsed();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(
            resp.headers().get(header::RETRY_AFTER).is_some(),
            "429 must still advertise Retry-After even at the cap"
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "handler must return promptly regardless of the computed delay; took {elapsed:?}"
        );
    }

    // -------------------------------------------------------------------
    // Phase 49.1 Plan 04 Task 2 (D-07 weaknesses 2/3): global keying and
    // restart-surviving attempt state.
    // -------------------------------------------------------------------

    /// Test 1: two different peer IPs contribute to the SAME attempt
    /// counter — five failures from `127.0.0.1` followed by one failure
    /// from a DIFFERENT `ConnectInfo` address returns 429, not 401. This
    /// is the source-rotation weakness: IronHermes has exactly one
    /// operator password, so a global counter is the correct primitive.
    #[tokio::test]
    async fn login_global_counter_spans_source_addresses() {
        let auth = state_with_password("hunter2");
        let peer_a = SocketAddr::from(([127, 0, 0, 1], 45300));
        let peer_b = SocketAddr::from(([10, 0, 0, 7], 45301));

        for i in 0..5 {
            let resp = login(
                State(auth.clone()),
                ConnectInfo(peer_a),
                Json(LoginRequest {
                    password: "wrong".to_string(),
                }),
            )
            .await;
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "attempt {i} from peer_a must be 401"
            );
        }

        let resp = login(
            State(auth.clone()),
            ConnectInfo(peer_b),
            Json(LoginRequest {
                password: "wrong".to_string(),
            }),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "the 6th failure, from a DIFFERENT address, must still trip the global counter"
        );
    }

    /// Test 2: the per-IP detail is still recorded for diagnostics (which
    /// addresses attempted, and when), but it never gates the verdict —
    /// asserted directly against `AuthState::login_attempt_sources`, and
    /// re-confirmed by a subsequent call still returning 401 (3 total
    /// failures across two addresses is under `LOGIN_MAX_ATTEMPTS`).
    #[tokio::test]
    async fn login_per_ip_diagnostics_recorded_but_not_gating() {
        let auth = state_with_password("hunter2");
        let peer_a = SocketAddr::from(([127, 0, 0, 1], 45302));
        let peer_b = SocketAddr::from(([203, 0, 113, 9], 45303));

        for _ in 0..2 {
            let _ = login(
                State(auth.clone()),
                ConnectInfo(peer_a),
                Json(LoginRequest {
                    password: "wrong".to_string(),
                }),
            )
            .await;
        }
        let _ = login(
            State(auth.clone()),
            ConnectInfo(peer_b),
            Json(LoginRequest {
                password: "wrong".to_string(),
            }),
        )
        .await;

        {
            let sources = auth.login_attempt_sources.read();
            assert_eq!(
                sources.get(&peer_a.ip()).map(|e| e.count),
                Some(2),
                "peer_a's diagnostic count must reflect its own 2 attempts"
            );
            assert_eq!(
                sources.get(&peer_b.ip()).map(|e| e.count),
                Some(1),
                "peer_b's diagnostic count must reflect its own 1 attempt"
            );
        }

        let resp = login(
            State(auth.clone()),
            ConnectInfo(peer_a),
            Json(LoginRequest {
                password: "wrong".to_string(),
            }),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "3 total failures across 2 addresses is still under budget — the per-IP records above are bookkeeping only"
        );
    }

    /// Test 3: attempt state round-trips through persistence — a FRESH
    /// `AuthState` constructed from the SAME home directory (simulating a
    /// process restart) continues the backoff rather than restarting at
    /// 401.
    #[tokio::test]
    async fn login_attempt_state_survives_reconstruction() {
        let home = tempfile::tempdir().expect("tempdir").keep();
        let peer = SocketAddr::from(([127, 0, 0, 1], 45304));

        {
            let auth_a = state_with_password_at("hunter2", &home);
            for _ in 0..6 {
                let _ = login(
                    State(auth_a.clone()),
                    ConnectInfo(peer),
                    Json(LoginRequest {
                        password: "wrong".to_string(),
                    }),
                )
                .await;
            }
        } // auth_a dropped — simulates the process exiting.

        let auth_b = state_with_password_at("hunter2", &home);
        let resp = login(
            State(auth_b.clone()),
            ConnectInfo(peer),
            Json(LoginRequest {
                password: "wrong".to_string(),
            }),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "reconstructing AuthState from the same home must continue the backoff, not restart it"
        );
    }

    /// Test 4: the persisted state file is written at mode 0600, and its
    /// serialised form contains EXACTLY the count + timestamp fields — no
    /// credential material.
    #[tokio::test]
    async fn login_attempt_state_file_is_mode_0600_and_credential_free() {
        let home = tempfile::tempdir().expect("tempdir").keep();
        let auth = state_with_password_at("hunter2", &home);
        let peer = SocketAddr::from(([127, 0, 0, 1], 45305));

        let _ = login(
            State(auth.clone()),
            ConnectInfo(peer),
            Json(LoginRequest {
                password: "wrong".to_string(),
            }),
        )
        .await;

        let path = home.join("login_attempts.json");
        let meta = std::fs::metadata(&path).expect("state file must exist after a failed attempt");
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            meta.permissions().mode() & 0o777,
            0o600,
            "login_attempts.json must be written at mode 0600"
        );

        let contents = std::fs::read_to_string(&path).expect("read state file");
        let value: serde_json::Value = serde_json::from_str(&contents).expect("state file must be valid JSON");
        let obj = value.as_object().expect("state file must be a JSON object");
        let keys: std::collections::BTreeSet<&str> = obj.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["count", "last_failure_unix_secs"].into_iter().collect(),
            "persisted state must contain exactly count + timestamp, no credential material"
        );
    }

    /// Test 5: a corrupt or unreadable state file does not panic, and
    /// does NOT silently reset the counter to zero — it fails closed by
    /// treating the state as already at-or-past the threshold (T-49.1-04-07).
    #[tokio::test]
    async fn login_corrupt_attempt_state_fails_closed() {
        let home = tempfile::tempdir().expect("tempdir").keep();
        std::fs::write(home.join("login_attempts.json"), b"not valid json{{{")
            .expect("write corrupt fixture");

        let auth = state_with_password_at("hunter2", &home);
        let peer = SocketAddr::from(([127, 0, 0, 1], 45306));

        // The VERY FIRST attempt on this fresh AuthState must already be
        // rejected — proving the fail-closed treatment kicked in at load
        // time, not after genuinely accumulating 6 failures.
        let resp = login(
            State(auth.clone()),
            ConnectInfo(peer),
            Json(LoginRequest {
                password: "wrong".to_string(),
            }),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "a corrupt state file must fail closed (429), never reset to a fresh budget (401)"
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

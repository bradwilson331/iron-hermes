//! D-05 credential store bridge: `AuthStoreCredentialStore` implements rmcp's
//! `CredentialStore` trait, routing token/DCR persistence through Phase 41's `AuthStore`.
//!
//! # Design
//!
//! - `ns` — the token namespace (e.g. `"cloudflare_mcp_docs"`), keyed under
//!   `tokens.<ns>` in `auth.json`.
//! - `server_url` — the DCR key (e.g. `"https://docs.mcp.cloudflare.com/mcp"`), keyed
//!   under `clients.<server-url>` in `auth.json`.
//!
//! # Security properties
//!
//! - Pitfall 3 avoidance: the struct NEVER holds a cached `auth_store.state` lock.
//!   Only public AuthStore API methods are called (`get_token`, `put_token`,
//!   `get_client`, `put_client`); those methods handle locking internally.
//! - A-1: `AuthStoreCredentialStore` itself holds no secret fields. The token-bearing
//!   types it delegates to (`TokenEntry`, `DcrEntry`) already have manual `[REDACTED]`
//!   Debug implementations in Phase 41's `auth/store.rs`.
//! - T-44-04: no raw token value is embedded in any local struct or log output; all
//!   secrets flow exclusively through the Phase 41 mode-0600 atomic-write path.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use ironhermes_core::auth::{AuthStore, DcrEntry, RefreshFn, TokenEntry};
use oauth2::basic::BasicTokenType;
use oauth2::{AccessToken, RefreshToken, Scope, TokenResponse as _};
use rmcp::transport::auth::{AuthorizationManager, OAuthTokenResponse, VendorExtraTokenFields};
use rmcp::transport::{AuthError, CredentialStore, StoredCredentials};

// ─────────────────────────────────────────────────────────────────────────────
// AuthStoreCredentialStore — public surface
// ─────────────────────────────────────────────────────────────────────────────

/// Bridge between rmcp's [`CredentialStore`] trait and Phase 41's [`AuthStore`].
///
/// Satisfies D-05 (persistence) and the zero-DCR-on-restart guarantee (MCPA-03/B-3):
/// rmcp reads/writes OAuth credentials through this adapter, which lands them in
/// `auth.json` (`clients.<server-url>` + `tokens.<ns>`) via the existing AuthStore
/// API — the single source of truth that `spawn_refresh_task` already monitors.
///
/// # Pitfall 3 (lock deadlock avoidance)
///
/// This struct MUST NOT hold any `auth_store.state` lock as a field, and MUST NOT
/// attempt to lock `auth_store.state` directly. All access goes through the public
/// `AuthStore::get_token` / `put_token` / `get_client` / `put_client` methods, which
/// handle the mutex internally (never held across `.await`).
pub struct AuthStoreCredentialStore {
    store: Arc<AuthStore>,
    /// Token namespace — `tokens.<ns>` in `auth.json`.
    ns: String,
    /// DCR key — `clients.<server_url>` in `auth.json`.
    server_url: String,
}

impl AuthStoreCredentialStore {
    /// Create a new credential store adapter.
    ///
    /// - `store` — the shared `AuthStore` instance.
    /// - `ns` — the token namespace (e.g. `"cloudflare_mcp_docs"`).
    /// - `server_url` — the server URL used as the DCR key
    ///   (e.g. `"https://docs.mcp.cloudflare.com/mcp"`).
    pub fn new(
        store: Arc<AuthStore>,
        ns: impl Into<String>,
        server_url: impl Into<String>,
    ) -> Self {
        Self {
            store,
            ns: ns.into(),
            server_url: server_url.into(),
        }
    }
}

/// Manual Debug — `ns` and `server_url` are non-secret; `store` is omitted
/// because it holds the in-memory token cache (A-1).
impl std::fmt::Debug for AuthStoreCredentialStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthStoreCredentialStore")
            .field("ns", &self.ns)
            .field("server_url", &self.server_url)
            .finish_non_exhaustive()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CredentialStore impl
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
impl CredentialStore for AuthStoreCredentialStore {
    /// Load credentials from `AuthStore`.
    ///
    /// # Pitfall 5 / B-3 (zero DCR on restart)
    ///
    /// `get_client` is called FIRST. If a `DcrEntry` exists, `Some(StoredCredentials)`
    /// is returned with `client_id` populated — even when the token is expired or absent.
    /// This is what makes rmcp's `initialize_from_store()` return `true`, signalling that
    /// DCR has already been done and can be skipped on this startup.
    ///
    /// When no `DcrEntry` exists, returns `Ok(None)` so rmcp performs a fresh DCR.
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let store = Arc::clone(&self.store);
        let ns = self.ns.clone();
        let server_url = self.server_url.clone();

        // get_client FIRST — client_id must be present even if token expired (Pitfall 5 / B-3).
        let dcr = store.get_client(&server_url).await;
        let token = store.get_token(&ns).await;

        match dcr {
            // No DCR entry → trigger a fresh Dynamic Client Registration.
            None => Ok(None),
            Some(dcr_entry) => {
                // Map TokenEntry → OAuthTokenResponse when a token is available.
                // Pitfall 5: even when token is None, return Some(StoredCredentials) with
                // client_id so rmcp skips re-DCR. The access token will be refreshed
                // automatically via `get_access_token()` / `spawn_refresh_task`.
                let (token_response, granted_scopes, token_received_at) = match token {
                    None => (None, Vec::new(), None),
                    Some(entry) => {
                        let scopes = entry.scopes.clone();
                        let received_at = now_unix_secs();
                        let resp = token_entry_to_oauth_response(entry);
                        (Some(resp), scopes, Some(received_at))
                    }
                };
                Ok(Some(StoredCredentials::new(
                    dcr_entry.client_id,
                    token_response,
                    granted_scopes,
                    token_received_at,
                )))
            }
        }
    }

    /// Persist credentials to `AuthStore`.
    ///
    /// # A-6 (expires_in → expires_at mapping)
    ///
    /// rmcp's `OAuthTokenResponse` carries `expires_in` (a `Duration`).
    /// Phase 41's `TokenEntry` stores `expires_at` (a `SystemTime`) so that
    /// `spawn_refresh_task` can compute the fire-at time with its ±30s clock-skew
    /// tolerance. Mapping: `expires_at = SystemTime::now() + expires_in`.
    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let store = Arc::clone(&self.store);
        let ns = self.ns.clone();
        let server_url = self.server_url.clone();

        // Persist DCR client_id under clients.<server-url> (D-05 / B-3).
        let dcr_entry = DcrEntry {
            client_id: credentials.client_id,
            client_secret: None,
            registration_access_token: None,
        };
        store
            .put_client(&server_url, dcr_entry)
            .await
            .map_err(|e| AuthError::InternalError(format!("AuthStore put_client failed: {e:#}")))?;

        // Persist access/refresh/expiry under tokens.<ns> when present (D-05).
        if let Some(token_response) = credentials.token_response {
            let token_entry = oauth_response_to_token_entry(token_response);
            store.put_token(&ns, token_entry).await.map_err(|e| {
                AuthError::InternalError(format!("AuthStore put_token failed: {e:#}"))
            })?;
        }

        Ok(())
    }

    /// Clear credentials (best-effort; non-critical path).
    ///
    /// `AuthStore` has no explicit delete operation — credentials are overwritten
    /// on the next `save`. This is a no-op for now; the critical path is `load`
    /// and `save`, which satisfy D-05 and B-3.
    async fn clear(&self) -> Result<(), AuthError> {
        // Non-critical: AuthStore does not expose a delete API. The next
        // save() will overwrite with fresh credentials. A future phase can
        // add a delete path to AuthStore if needed (e.g. `hermes mcp logout`).
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// D-04 (46.1-03): real RefreshFn builder — background refresh via rmcp
// ─────────────────────────────────────────────────────────────────────────────

/// Open an [`AuthStore`] wired to a REAL rmcp-backed [`RefreshFn`] (D-04, 46.1-03).
///
/// Unlike the stub `refresh_fn` installed by `AuthStore::open()` (which always
/// returns "live OAuth token exchange is not implemented until Phase 43"), the
/// `RefreshFn` built here performs a live OAuth token exchange via rmcp's
/// [`AuthorizationManager`] + [`AuthStoreCredentialStore`] for a given
/// namespace — the same machinery [`crate::transport::connect_http_oauth`]
/// already uses on its hot path. This is what makes
/// `AuthStore::spawn_refresh_task`'s background loop actually refresh the
/// cloudflare_api/bindings/observability/builds namespaces instead of forever
/// hitting the stale Phase-43 stub error (RESEARCH Pitfall 1/2).
///
/// # Arc-cycle break
///
/// The `RefreshFn` needs a handle to the very `AuthStore` it will be installed
/// on (to call `AuthStore::get_token` / build `AuthStoreCredentialStore`), but
/// `AuthStore::open_with_refresh` only returns the `Arc<AuthStore>` AFTER the
/// `RefreshFn` closure has already been constructed and moved in. This cycle is
/// broken with a `OnceLock<Weak<AuthStore>>` shared between the closure and this
/// function: the closure upgrades the `Weak` on each invocation — failing
/// closed (`Err`) if the store has since been dropped, rather than falling back
/// to a second, divergent in-memory store — and this function `.set()`s the
/// downgraded handle immediately after `open_with_refresh` returns.
///
/// `ns_to_url` maps each OAuth namespace (`oauth_provider`) to its MCP server
/// URL. This lookup is required because `AuthorizationManager::new` needs the
/// server URL and `AuthStore` itself only tracks `ns` strings, not URLs.
///
/// Every fallible step's error text is routed through [`crate::security::sanitize_error`]
/// so no token/verifier value can leak into a propagated or logged error.
pub async fn open_auth_store_with_mcp_refresh(
    path: std::path::PathBuf,
    ns_to_url: std::collections::HashMap<String, String>,
) -> anyhow::Result<std::sync::Arc<AuthStore>> {
    let store_handle: std::sync::Arc<std::sync::OnceLock<std::sync::Weak<AuthStore>>> =
        std::sync::Arc::new(std::sync::OnceLock::new());

    let handle_for_closure = std::sync::Arc::clone(&store_handle);
    let refresh_fn: RefreshFn = std::sync::Arc::new(move |ns: String| {
        let handle = std::sync::Arc::clone(&handle_for_closure);
        let ns_to_url = ns_to_url.clone();
        Box::pin(async move {
            // 1. Upgrade the Weak — fail closed if the store is already gone
            //    rather than silently constructing a second, divergent store.
            let store = handle
                .get()
                .and_then(|weak| weak.upgrade())
                .ok_or_else(|| anyhow::anyhow!("auth store no longer available for refresh"))?;

            // 2. Look up the server URL for this namespace — a namespace with
            //    no known URL cannot refresh (RESEARCH Pitfall 2).
            let url = ns_to_url
                .get(&ns)
                .ok_or_else(|| anyhow::anyhow!("no server url for oauth namespace"))?;

            // 3. Drive the real rmcp OAuth refresh: same
            //    AuthorizationManager + AuthStoreCredentialStore +
            //    get_access_token() sequence connect_http_oauth already uses.
            let mut mgr = AuthorizationManager::new(url.as_str()).await.map_err(|e| {
                anyhow::anyhow!(
                    "auth manager init: {}",
                    crate::security::sanitize_error(&e.to_string())
                )
            })?;
            mgr.set_credential_store(AuthStoreCredentialStore::new(
                std::sync::Arc::clone(&store),
                ns.clone(),
                url.clone(),
            ));
            mgr.initialize_from_store().await.map_err(|e| {
                anyhow::anyhow!(
                    "credential load: {}",
                    crate::security::sanitize_error(&e.to_string())
                )
            })?;
            // get_access_token() performs the oauth2 refresh_token exchange when
            // the cached token is near/past expiry and persists the fresh token
            // via AuthStoreCredentialStore::save() (D-05).
            mgr.get_access_token().await.map_err(|e| {
                anyhow::anyhow!(
                    "get access token: {}",
                    crate::security::sanitize_error(&e.to_string())
                )
            })?;

            // 4. Read back the fresh TokenEntry that save() just persisted so
            //    AuthStore::refresh() has a value to cache + notify waiters with.
            store
                .get_token(&ns)
                .await
                .ok_or_else(|| anyhow::anyhow!("token missing after refresh for ns"))
        })
    });

    let store = AuthStore::open_with_refresh(path, refresh_fn).await?;
    let _ = store_handle.set(std::sync::Arc::downgrade(&store));
    Ok(store)
}

// ─────────────────────────────────────────────────────────────────────────────
// Private mapping helpers — TokenEntry ↔ OAuthTokenResponse
// ─────────────────────────────────────────────────────────────────────────────

/// Map a [`TokenEntry`] (AuthStore type) to an [`OAuthTokenResponse`] (rmcp/oauth2 type).
///
/// # A-6 (reverse mapping)
///
/// `expires_at` (an absolute `SystemTime`) is converted back to `expires_in`
/// (a remaining `Duration`) relative to `now`. An expired token yields
/// `Duration::ZERO` — rmcp will trigger a refresh via `get_access_token()`.
fn token_entry_to_oauth_response(entry: TokenEntry) -> OAuthTokenResponse {
    use oauth2::StandardTokenResponse;

    let mut resp: OAuthTokenResponse = StandardTokenResponse::new(
        AccessToken::new(entry.access_token),
        BasicTokenType::Bearer,
        VendorExtraTokenFields::default(),
    );

    if let Some(rt) = entry.refresh_token {
        resp.set_refresh_token(Some(RefreshToken::new(rt)));
    }

    // A-6 reverse: expires_at → expires_in (remaining duration, clamped to zero).
    if let Some(expires_at) = entry.expires_at {
        let expires_in = expires_at
            .duration_since(SystemTime::now())
            .unwrap_or(Duration::ZERO);
        resp.set_expires_in(Some(&expires_in));
    }

    let scopes: Vec<Scope> = entry.scopes.into_iter().map(Scope::new).collect();
    if !scopes.is_empty() {
        resp.set_scopes(Some(scopes));
    }

    resp
}

/// Map an [`OAuthTokenResponse`] (rmcp/oauth2 type) to a [`TokenEntry`] (AuthStore type).
///
/// # A-6 (expires_in → expires_at)
///
/// `expires_in` (a `Duration` from rmcp) is converted to an absolute `expires_at`
/// (`SystemTime::now() + expires_in`) so that Phase 41's `spawn_refresh_task` can
/// fire ~60s before the token expires with its built-in ±30s clock-skew tolerance.
fn oauth_response_to_token_entry(token_response: OAuthTokenResponse) -> TokenEntry {
    let access_token = token_response.access_token().secret().clone();
    let refresh_token = token_response.refresh_token().map(|r| r.secret().clone());

    // A-6: map expires_in (duration) → expires_at (absolute SystemTime).
    let expires_at = token_response.expires_in().map(|d| SystemTime::now() + d);

    let scopes: Vec<String> = token_response
        .scopes()
        .map(|v| v.iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();

    TokenEntry {
        access_token,
        refresh_token,
        expires_at,
        scopes,
    }
}

/// Current UNIX timestamp in seconds (for `StoredCredentials.token_received_at`).
fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

// ─────────────────────────────────────────────────────────────────────────────
// Unit tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    use oauth2::AccessToken;
    use oauth2::StandardTokenResponse;
    use oauth2::basic::BasicTokenType;
    use rmcp::transport::auth::VendorExtraTokenFields;
    use rmcp::transport::{CredentialStore, StoredCredentials};

    use super::{AuthStoreCredentialStore, OAuthTokenResponse};
    use ironhermes_core::auth::AuthStore;

    // ─────────────────────────────────────────────────────────────────
    // Test helpers
    // ─────────────────────────────────────────────────────────────────

    /// Open an empty AuthStore backed by a unique temp directory + `auth.json`.
    ///
    /// `AuthStore::save_to_disk` enforces `chmod 0700` on the *parent directory*.
    /// Using the system temp dir directly as the parent fails with EPERM because it
    /// is owned by the OS. Each test therefore gets its own subdirectory (which we
    /// create and own) so that the chmod succeeds.
    async fn make_test_store(tag: &str) -> Arc<AuthStore> {
        let dir: PathBuf = std::env::temp_dir().join(format!(
            "ironhermes_mcp_adapter_test_{}_{}",
            std::process::id(),
            tag,
        ));
        std::fs::create_dir_all(&dir).expect("could not create per-test temp dir");
        let path = dir.join("auth.json");
        AuthStore::open(path)
            .await
            .expect("test AuthStore::open failed")
    }

    /// Build a minimal `OAuthTokenResponse` with the given access token.
    fn make_token_response(access_token: &str) -> OAuthTokenResponse {
        StandardTokenResponse::new(
            AccessToken::new(access_token.to_string()),
            BasicTokenType::Bearer,
            VendorExtraTokenFields::default(),
        )
    }

    /// Build a `StoredCredentials` with `client_id` and an optional token.
    fn make_credentials(client_id: &str, token: Option<OAuthTokenResponse>) -> StoredCredentials {
        StoredCredentials::new(
            client_id.to_string(),
            token,
            vec!["mcp:tools".to_string()],
            None,
        )
    }

    // ─────────────────────────────────────────────────────────────────
    // Tests
    // ─────────────────────────────────────────────────────────────────

    /// save() → load() round-trip: client_id is preserved and returned.
    #[tokio::test]
    async fn test_round_trip_client_id_preserved() {
        let store = make_test_store("round_trip").await;
        let adapter =
            AuthStoreCredentialStore::new(Arc::clone(&store), "test_ns", "https://example.com/mcp");

        let creds = make_credentials("my-client-id", Some(make_token_response("tok")));
        adapter.save(creds).await.expect("save failed");

        let loaded = adapter.load().await.expect("load failed");
        assert!(loaded.is_some(), "load should return Some after save");
        assert_eq!(
            loaded.unwrap().client_id,
            "my-client-id",
            "client_id must survive round-trip"
        );
    }

    /// Pitfall 5 / B-3: when DcrEntry exists but token is absent, load() still
    /// returns Some(StoredCredentials) with client_id populated.
    /// This ensures rmcp skips re-DCR on the second startup.
    #[tokio::test]
    async fn test_pitfall5_dcr_without_token_still_returns_client_id() {
        let store = make_test_store("pitfall5").await;
        let adapter = AuthStoreCredentialStore::new(
            Arc::clone(&store),
            "test_ns_p5",
            "https://pitfall5.example.com/mcp",
        );

        // Save credentials WITHOUT a token_response (token expired / not yet exchanged).
        let creds = make_credentials("dcr-only-client", None);
        adapter.save(creds).await.expect("save failed");

        // Even without a token, load must return Some with the client_id (Pitfall 5).
        let loaded = adapter.load().await.expect("load failed");
        assert!(
            loaded.is_some(),
            "Pitfall 5: load must return Some when DcrEntry exists, even if token is absent"
        );
        assert_eq!(
            loaded.unwrap().client_id,
            "dcr-only-client",
            "Pitfall 5: client_id must be populated from DcrEntry even without a token"
        );
    }

    /// load() when no DcrEntry exists returns None (triggers fresh DCR).
    #[tokio::test]
    async fn test_no_dcr_returns_none() {
        let store = make_test_store("no_dcr").await;
        let adapter = AuthStoreCredentialStore::new(
            Arc::clone(&store),
            "ns_no_dcr",
            "https://nodcr.example.com/mcp",
        );

        // Nothing saved — load must return None to signal fresh DCR needed.
        let result = adapter.load().await.expect("load failed");
        assert!(result.is_none(), "load with no DcrEntry must return None");
    }

    /// A-6: save() maps expires_in (Duration from OAuthTokenResponse) →
    /// expires_at = SystemTime::now() + expires_in (stored in AuthStore TokenEntry).
    #[tokio::test]
    async fn test_a6_expires_in_mapped_to_expires_at() {
        use std::time::SystemTime;

        let store = make_test_store("a6").await;
        let adapter = AuthStoreCredentialStore::new(
            Arc::clone(&store),
            "ns_a6",
            "https://a6.example.com/mcp",
        );

        let mut token_resp = make_token_response("access-a6");
        let expires_in = Duration::from_secs(3600);
        token_resp.set_expires_in(Some(&expires_in));

        let before_save = SystemTime::now();
        adapter
            .save(make_credentials("a6-client", Some(token_resp)))
            .await
            .expect("save failed");
        let after_save = SystemTime::now();

        let stored = store
            .get_token("ns_a6")
            .await
            .expect("TokenEntry should be stored after save");

        let expires_at = stored
            .expires_at
            .expect("A-6: TokenEntry must have expires_at set");

        // expires_at must be within [before_save + 3600s, after_save + 3600s + 1s].
        let earliest = before_save + expires_in;
        let latest = after_save + expires_in + Duration::from_secs(1);
        assert!(
            expires_at >= earliest,
            "A-6: expires_at {:?} must be >= {:?}",
            expires_at,
            earliest,
        );
        assert!(
            expires_at <= latest,
            "A-6: expires_at {:?} must be <= {:?}",
            expires_at,
            latest,
        );
    }

    /// save() stores DcrEntry under clients.<server-url> in AuthStore.
    #[tokio::test]
    async fn test_save_persists_dcr_entry() {
        let store = make_test_store("dcr_persist").await;
        let server_url = "https://dcr.example.com/mcp";
        let adapter = AuthStoreCredentialStore::new(Arc::clone(&store), "ns_dcr", server_url);

        adapter
            .save(make_credentials("stored-client-id", None))
            .await
            .expect("save failed");

        let dcr = store.get_client(server_url).await;
        assert!(dcr.is_some(), "DcrEntry should be stored after save");
        assert_eq!(
            dcr.unwrap().client_id,
            "stored-client-id",
            "DcrEntry.client_id must match the saved credentials"
        );
    }

    /// save() with a token_response persists the access token to AuthStore.
    #[tokio::test]
    async fn test_save_persists_token_entry() {
        let store = make_test_store("token_persist").await;
        let adapter = AuthStoreCredentialStore::new(
            Arc::clone(&store),
            "ns_token",
            "https://token.example.com/mcp",
        );

        adapter
            .save(make_credentials(
                "tok-client",
                Some(make_token_response("my-access-token")),
            ))
            .await
            .expect("save failed");

        let token = store.get_token("ns_token").await;
        assert!(
            token.is_some(),
            "TokenEntry should be stored after save with token_response"
        );
        assert_eq!(
            token.unwrap().access_token,
            "my-access-token",
            "access_token must survive save"
        );
    }

    /// clear() is a no-op and does not error (non-critical path).
    #[tokio::test]
    async fn test_clear_is_noop() {
        let store = make_test_store("clear").await;
        let adapter = AuthStoreCredentialStore::new(
            Arc::clone(&store),
            "ns_clear",
            "https://clear.example.com/mcp",
        );

        // clear on empty store must not error.
        adapter
            .clear()
            .await
            .expect("clear on empty store must not error");

        // clear after save must also not error.
        adapter
            .save(make_credentials("clear-client", None))
            .await
            .expect("save failed");
        adapter
            .clear()
            .await
            .expect("clear after save must not error");
    }
}

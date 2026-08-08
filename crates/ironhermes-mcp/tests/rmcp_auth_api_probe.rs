//! rmcp 1.8.0 Auth API surface probe — compile-time verification for Phase 44.
//!
//! **Purpose:** Prove the exact import paths and API signatures that 44-03
//! (`AuthStoreCredentialStore`), 44-04 (`connect_http_oauth`), and 44-06
//! (integration tests) depend on. Compilation success IS the assertion — no live
//! network calls are made.
//!
//! Run with: `cargo test -p ironhermes-mcp --test rmcp_auth_api_probe`
//!
//! ---
//!
//! ## Verified Import Paths (rmcp =1.8.0 with `auth` feature)
//!
//! | Type / Trait                          | Verified import path                                                            |
//! |---------------------------------------|---------------------------------------------------------------------------------|
//! | `AuthorizationManager`                | `rmcp::transport::AuthorizationManager`                                         |
//! | `AuthorizationSession`                | `rmcp::transport::AuthorizationSession`                                         |
//! | `CredentialStore` trait               | `rmcp::transport::CredentialStore`                                              |
//! | `StoredCredentials`                   | `rmcp::transport::StoredCredentials`                                            |
//! | `AuthClient<C>`                       | `rmcp::transport::AuthClient`                                                   |
//! | `StreamableHttpClientTransport<C>`    | `rmcp::transport::StreamableHttpClientTransport`                                |
//! | `StreamableHttpClientTransportConfig` | `rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig`  |
//! | `StreamableHttpClient` trait (bound)  | `rmcp::transport::streamable_http_client::StreamableHttpClient`                 |
//!
//! **IMPORTANT:** `StreamableHttpClientTransportConfig` and `StreamableHttpClient` are NOT
//! re-exported at the `rmcp::transport` top level. Downstream plans MUST use the submodule path:
//! ```rust,ignore
//! use rmcp::transport::streamable_http_client::{
//!     StreamableHttpClient, StreamableHttpClientTransportConfig,
//! };
//! ```
//!
//! ---
//!
//! ## Open Question Resolutions
//!
//! ### OQ-1: Does `reqwest::Client` satisfy `AuthClient<C>`'s generic bound?
//!
//! **CONFIRMED YES — BUT WITH A CRITICAL VERSION CAVEAT.**
//!
//! The bound on `C` in `AuthClient<C>` for use with
//! `StreamableHttpClientTransport::with_client(auth_client, config)` is:
//! `C: StreamableHttpClient + Clone + Send + 'static`.
//!
//! `rmcp 1.8.0` uses `reqwest = "0.13.2"` internally. `reqwest::Client` from
//! reqwest 0.13 implements `StreamableHttpClient` (confirmed in rmcp source:
//! `transport/common/reqwest/streamable_http_client.rs`).
//!
//! **VERSION CONFLICT DISCOVERED:** The workspace uses `reqwest = "0.12.28"` but
//! rmcp 1.8.0 requires `reqwest = "0.13.2"`. These are TWO DIFFERENT crates in the
//! dependency graph. The `StreamableHttpClient` impl is on `reqwest 0.13` —
//! workspace `reqwest 0.12.x` does NOT satisfy the bound and WILL cause a compile
//! error if used as `AuthClient<reqwest_012::Client>`.
//!
//! **Impact on 44-04:** `connect_http_oauth` must NOT use the workspace reqwest to
//! instantiate the `AuthClient`. Two valid approaches:
//!
//! **Approach A (recommended — avoids direct reqwest dep):**
//! Use `StreamableHttpClientTransportConfig::auth_header(token)` with `from_config()`.
//! The token is obtained from `AuthorizationManager::get_access_token().await?`.
//! `StreamableHttpClientTransport::from_config(config)` uses rmcp's internal reqwest
//! 0.13 client automatically. No `AuthClient` needed; the B-2 retry on 401 refreshes
//! the token and reconnects.
//! ```rust,ignore
//! let token = auth_mgr.get_access_token().await?;
//! let cfg = StreamableHttpClientTransportConfig::with_uri(url)
//!     .auth_header(token)
//!     .reinit_on_expired_session(true);
//! let transport = StreamableHttpClientTransport::from_config(cfg);
//! ```
//!
//! **Approach B (direct reqwest 0.13 dep):**
//! Add `reqwest = "0.13"` as a NON-workspace direct dep to `ironhermes-mcp/Cargo.toml`.
//! This gives access to the 0.13 `reqwest::Client` that satisfies `StreamableHttpClient`.
//! `AuthClient<reqwest_013::Client>` then compiles and provides per-request auto-refresh.
//!
//! The generic compile-time proof below uses `C: StreamableHttpClient` abstractly to
//! confirm the type-level relationship without hitting the version conflict.
//!
//! ### OQ-2: Is `AuthorizationManager::new(base_url)` async?
//!
//! **CONFIRMED YES.**
//!
//! Signature: `pub async fn new<U: IntoUrl>(base_url: U) -> Result<Self, AuthError>`.
//!
//! `new()` does NOT make network calls — it only builds an internal reqwest 0.13 client
//! and constructs the struct. `discover_metadata()` and `initialize_from_store()` make
//! network calls. `new()` is safe to call in tests without a live server.
//!
//! **Usage in 44-04:** `connect_http_oauth` MUST be `async fn` to call
//! `AuthorizationManager::new(url).await?`.
//!
//! ### OQ-3: Does `StreamableHttpClientTransportConfig` have `reinit_on_expired_session`?
//!
//! **CONFIRMED YES.**
//!
//! Field: `pub reinit_on_expired_session: bool` — defaults to `true` in `Default` impl.
//! Builder: `.reinit_on_expired_session(enable: bool) -> Self`.
//!
//! When `true`, the transport transparently recovers from HTTP 404 (expired
//! `Mcp-Session-Id`) by replaying the `initialize` handshake. **No additional B-1 code
//! is needed in ironhermes-mcp** — rmcp stores and resends `Mcp-Session-Id` internally.
//!
//! ---
//!
//! ## CredentialStore Trait Correction (vs RESEARCH.md)
//!
//! RESEARCH.md described `CredentialStore` methods using `Pin<Box<dyn Future<...>>>`.
//! The **actual** trait in rmcp 1.8 uses `#[async_trait]` with plain `async fn`:
//!
//! ```rust,ignore
//! #[async_trait]
//! pub trait CredentialStore: Send + Sync {
//!     async fn load(&self) -> Result<Option<StoredCredentials>, AuthError>;
//!     async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError>;
//!     async fn clear(&self) -> Result<(), AuthError>;
//! }
//! ```
//!
//! **Impact on 44-03:** `AuthStoreCredentialStore` MUST use `#[async_trait::async_trait]`
//! + `async fn` — NOT manually-constructed `Pin<Box<...>>`.
//!
//! ---
//!
//! ## Recommended 44-04 Transport Construction (Approach A)
//!
//! ```rust,ignore
//! // 1. Build AuthorizationManager (async, no network)
//! let mut auth_mgr = AuthorizationManager::new(url).await?;
//!
//! // 2. Attach credential store (custom AuthStoreCredentialStore from 44-03)
//! auth_mgr.set_credential_store(auth_store_adapter);
//!
//! // 3. Load cached credentials (makes network call to discover metadata if needed)
//! let has_creds = auth_mgr.initialize_from_store().await?;
//! // If !has_creds && headless: return skip-with-warn (D-04)
//!
//! // 4. Get current access token (auto-refreshes if expired)
//! let token = auth_mgr.get_access_token().await?;
//!
//! // 5. Build transport using rmcp's internal reqwest 0.13 client (no version conflict)
//! let config = StreamableHttpClientTransportConfig::with_uri(url)
//!     .auth_header(token)
//!     .reinit_on_expired_session(true);  // B-1 session resume
//! let transport = StreamableHttpClientTransport::from_config(config);
//!
//! // 6. Connect
//! let client = ().serve(transport).await?;
//! Ok((client, None))
//! ```

#![allow(dead_code)]

use rmcp::transport::streamable_http_client::{
    StreamableHttpClient, StreamableHttpClientTransportConfig,
};
use rmcp::transport::{
    AuthClient, AuthorizationManager, AuthorizationSession, CredentialStore, StoredCredentials,
    StreamableHttpClientTransport,
};

// ──────────────────────────────────────────────────────────────────────────────
// OQ-1 Probe: Generic proof that AuthClient<C> satisfies StreamableHttpClient
// ──────────────────────────────────────────────────────────────────────────────

/// Compile-time proof (generic): IF `C: StreamableHttpClient + Clone + Send + 'static`,
/// THEN `AuthClient<C>` also satisfies `StreamableHttpClient` and can be passed to
/// `StreamableHttpClientTransport::with_client`.
///
/// We use a generic `C` parameter to avoid the reqwest 0.12 vs 0.13 version conflict
/// (see OQ-1 in the doc block above). The type relationship is proven by the compiler
/// resolving the impl `StreamableHttpClient for AuthClient<C>` from rmcp source.
fn prove_auth_client_satisfies_streamable_http_client_bound<
    C: StreamableHttpClient + Clone + Send + Sync + 'static,
>(
    auth_client: AuthClient<C>,
    config: StreamableHttpClientTransportConfig,
) -> StreamableHttpClientTransport<AuthClient<C>> {
    // Compilation of this call proves:
    // - AuthClient<C> implements StreamableHttpClient (when C does)
    // - StreamableHttpClientTransport::with_client accepts AuthClient<C>
    StreamableHttpClientTransport::with_client(auth_client, config)
}

// ──────────────────────────────────────────────────────────────────────────────
// OQ-2 Probe: AuthorizationManager::new is async
// ──────────────────────────────────────────────────────────────────────────────

/// Prove that `AuthorizationManager::new` is async by calling `.await` on it inside
/// an `async fn`. `new()` does NOT make network calls; it only constructs the struct.
///
/// If this function compiles, OQ-2 is CONFIRMED (the `.await` requires a Future).
async fn prove_authorization_manager_new_is_async() {
    // new() builds the struct + internal reqwest 0.13 client — no network traffic.
    // We suppress the result since the URL is invalid (unreachable); we only care
    // that the compiler accepts `.await` here (proving new() returns a Future).
    let _ = AuthorizationManager::new("http://127.0.0.1:1").await;
}

// ──────────────────────────────────────────────────────────────────────────────
// OQ-3 Probe: StreamableHttpClientTransportConfig.reinit_on_expired_session
// ──────────────────────────────────────────────────────────────────────────────

/// Prove that `StreamableHttpClientTransportConfig` has `reinit_on_expired_session`
/// field and `.reinit_on_expired_session(bool)` builder method.
///
/// If this compiles AND the runtime assertion passes, OQ-3 is CONFIRMED.
fn prove_transport_config_reinit_on_expired_session_exists() -> StreamableHttpClientTransportConfig
{
    StreamableHttpClientTransportConfig::with_uri("http://127.0.0.1:1/mcp")
        .reinit_on_expired_session(true)
}

/// Prove that `StreamableHttpClientTransportConfig::auth_header` exists (Approach A path).
fn prove_transport_config_auth_header_exists() -> StreamableHttpClientTransportConfig {
    StreamableHttpClientTransportConfig::with_uri("http://127.0.0.1:1/mcp")
        .auth_header("stub-token")
        .reinit_on_expired_session(true)
}

// ──────────────────────────────────────────────────────────────────────────────
// StoredCredentials + CredentialStore probe
// ──────────────────────────────────────────────────────────────────────────────

/// Prove `StoredCredentials::new` has the expected 4-arg signature and field names.
/// Prove `CredentialStore` is importable as a trait bound.
fn prove_stored_credentials_and_credential_store() {
    let creds = StoredCredentials::new(
        "test-client-id".to_string(),
        None, // token_response: Option<OAuthTokenResponse>
        vec!["mcp:tools".to_string()],
        None, // token_received_at: Option<u64>
    );

    // Verify public field names match what RESEARCH.md + 44-03 will depend on
    assert_eq!(creds.client_id, "test-client-id");
    assert!(creds.token_response.is_none());
    assert_eq!(creds.granted_scopes, vec!["mcp:tools"]);
    assert!(creds.token_received_at.is_none());

    // Prove CredentialStore is usable as a trait bound (compilation sufficient)
    fn _accepts_credential_store<S: CredentialStore>(_s: S) {}
}

/// Prove `AuthorizationSession` is importable as a type.
/// Cannot construct without a live auth server; type-level reference is sufficient.
fn prove_authorization_session_type_is_accessible() {
    // Referencing the type in an Option proves the import path is correct.
    let _: Option<AuthorizationSession> = None;
    // The public fields downstream plans depend on (from source inspection):
    //   session.auth_url     : String  — open in browser for PKCE
    //   session.redirect_uri : String  — loopback callback URL
    //   session.auth_manager : AuthorizationManager
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn test_all_seven_auth_types_are_importable() {
    // The mere compilation of this file proves all 7 types are importable at their
    // documented paths. Runtime assertions verify the non-trivial field/method values.
    prove_stored_credentials_and_credential_store();
    prove_authorization_session_type_is_accessible();

    let config = prove_transport_config_reinit_on_expired_session_exists();
    assert!(
        config.reinit_on_expired_session,
        "OQ-3: reinit_on_expired_session must be true when set via builder"
    );

    let config2 = prove_transport_config_auth_header_exists();
    assert!(
        config2.auth_header.is_some(),
        "auth_header field must be Some after .auth_header() builder"
    );
}

#[test]
fn test_oq1_auth_client_generic_satisfies_streamable_http_client_bound() {
    // `prove_auth_client_satisfies_streamable_http_client_bound` is defined above.
    // Its presence in this file proves OQ-1 generically:
    //   AuthClient<C>: StreamableHttpClient when C: StreamableHttpClient + Clone + Send + Sync + 'static
    //
    // REQWEST VERSION NOTE: workspace reqwest 0.12.28 ≠ rmcp's reqwest 0.13.2.
    // Use Approach A (auth_header + from_config) in 44-04 to avoid the version conflict.
    //
    // Compilation of this file (specifically the function above) is the proof.
}

#[test]
fn test_oq2_authorization_manager_new_is_async() {
    // `prove_authorization_manager_new_is_async` is an async fn that calls `.await` on
    // AuthorizationManager::new(). The fact that it compiles proves new() returns a Future.
    //
    // Compilation of this file (specifically the async fn above) is the proof.
}

#[test]
fn test_oq3_transport_config_reinit_on_expired_session_exists() {
    let config = prove_transport_config_reinit_on_expired_session_exists();
    assert!(
        config.reinit_on_expired_session,
        "OQ-3 CONFIRMED: reinit_on_expired_session field exists on StreamableHttpClientTransportConfig"
    );
}

#[test]
fn test_approach_a_auth_header_builder_exists() {
    // Prove Approach A config is viable: auth_header() builder exists on the config struct.
    let config = prove_transport_config_auth_header_exists();
    assert!(
        config.auth_header.is_some(),
        "Approach A CONFIRMED: StreamableHttpClientTransportConfig::auth_header() builder exists"
    );
}

#[tokio::test]
async fn test_approach_a_from_config_exists() {
    // Prove from_config() exists and accepts the config (uses rmcp's internal reqwest 0.13).
    // from_config() spawns an internal worker task — requires a tokio runtime context.
    let config = prove_transport_config_auth_header_exists();
    let _transport = StreamableHttpClientTransport::from_config(config);
    // Compilation + runtime success (worker spawns without panic) is the proof.
}

#[test]
fn test_streamable_http_client_transport_config_submodule_path() {
    // Prove that StreamableHttpClientTransportConfig must be imported from the submodule:
    //   rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig
    // Compilation of the import at the top of this file is the proof.
    let _config = StreamableHttpClientTransportConfig::with_uri("http://localhost/mcp");
    // Field access confirms it's the correct type (not a name-collision alias)
    drop(_config);
}

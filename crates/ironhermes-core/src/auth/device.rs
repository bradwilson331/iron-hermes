//! RFC 8628 device-code OAuth 2.1 flow for IronHermes.
//!
//! Implements:
//! - `DeviceFlowParams` — parameters sourced from `AuthProviderConfig` (Plan 02).
//! - `run_device_flow` — async entry point for headless / CLI-only auth.
//! - `is_headless` — detects SSH sessions and display-less Linux environments
//!   so Plan 05's CLI can auto-select the right flow (D-04).
//!
//! Poll loop invariants (A-5, PITFALLS §A-5):
//! - Start with the server-supplied `interval`.
//! - On each `slow_down` error: `poll_interval += 5s` (cumulative, NEVER reset).
//! - On `authorization_pending`: continue polling; interval unchanged.
//! - Enforce the `expires_in` deadline on every iteration (A-5, A-6).
//! - Log `ns` + error only — never log a token or device code value (A-1).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use super::AuthStore;

// ─────────────────────────────────────────────────────────────────────────────
// DeviceFlowParams
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters for the RFC 8628 device-code flow.
///
/// Typically constructed from `AuthProviderConfig` (Plan 02).
/// `device_authorization_url`, `token_url`, and `client_id` are required.
///
/// `poll_interval_observer` is a test seam — leave `None` in production.
pub struct DeviceFlowParams {
    /// Device authorization endpoint URL
    /// (e.g. `https://accounts.example.com/oauth/device/code`).
    pub device_authorization_url: String,
    /// Token endpoint URL (e.g. `https://accounts.example.com/oauth/token`).
    pub token_url: String,
    /// Registered client ID.
    pub client_id: String,
    /// Requested OAuth scopes.
    pub scopes: Vec<String>,
    /// Test seam: when `Some`, the observer is called with the effective sleep
    /// duration before each poll attempt **instead** of actually sleeping.
    ///
    /// This allows SC#4 to assert the computed interval sequence without wall-clock
    /// delays. In production, leave `None` — `tokio::time::sleep(interval)` is used.
    pub poll_interval_observer: Option<Arc<dyn Fn(Duration) + Send + Sync>>,
}

impl Clone for DeviceFlowParams {
    fn clone(&self) -> Self {
        Self {
            device_authorization_url: self.device_authorization_url.clone(),
            token_url: self.token_url.clone(),
            client_id: self.client_id.clone(),
            scopes: self.scopes.clone(),
            poll_interval_observer: self.poll_interval_observer.clone(),
        }
    }
}

/// Manual Debug — summarises the observer field without revealing its closure address.
/// All other fields are plain strings and are safe to print.
impl std::fmt::Debug for DeviceFlowParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceFlowParams")
            .field("device_authorization_url", &self.device_authorization_url)
            .field("token_url", &self.token_url)
            .field("client_id", &self.client_id)
            .field("scopes", &self.scopes)
            .field(
                "poll_interval_observer",
                &self.poll_interval_observer.as_ref().map(|_| "<observer>"),
            )
            .finish()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// run_device_flow — native only
// ─────────────────────────────────────────────────────────────────────────────

/// Run the RFC 8628 device-code flow, persisting the resulting token into `store` under `ns`.
///
/// Prints the verification URL and user code so the operator can authenticate from
/// a separate device (SC#3 surfacing requirement). Polls the token endpoint at the
/// server-specified interval with cumulative `slow_down` backoff (A-5) until
/// authorized, expired, or an unrecoverable error is returned.
///
/// # Poll loop (A-5)
///
/// - Start with `interval` from the device-authorization response.
/// - `slow_down` → `poll_interval += 5s` (cumulative; never resets to the initial value).
/// - `authorization_pending` → continue at the current `poll_interval`.
/// - `expires_in` deadline passes → bail with "device code expired".
/// - Success → build `TokenEntry`, call `store.put_token(ns, entry)`, return `Ok(())`.
///
/// # Security
///
/// - The device code secret is never logged (A-1). Only `ns` + the error category
///   appear in warning log lines.
/// - `expires_at = now + expires_in` (A-6); clock-skew tolerance is applied by
///   `spawn_refresh_task` when it consumes `expires_at`.
///
/// # Errors
///
/// Returns `Err` on device authorization failure, expiry, access denial, or
/// any unrecoverable error from the token endpoint.
#[cfg(not(target_arch = "wasm32"))]
pub async fn run_device_flow(
    store: &AuthStore,
    ns: &str,
    params: DeviceFlowParams,
) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use oauth2::{
        ClientId, DeviceAuthorizationUrl, Scope, StandardDeviceAuthorizationResponse, TokenUrl,
        basic::BasicClient,
    };

    use super::store::TokenEntry;

    // Build an async reqwest client (redirect-disabled per oauth2 best practices).
    let http_client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("failed to build HTTP client for device flow")?;

    // Parse and validate URLs upfront so errors surface before any network call.
    let device_auth_url = DeviceAuthorizationUrl::new(params.device_authorization_url.clone())
        .context("invalid device_authorization_url")?;
    let token_url = TokenUrl::new(params.token_url.clone()).context("invalid token_url")?;

    // Build oauth2 client — only the device-authorization URL is needed here;
    // the token endpoint is accessed directly via reqwest in the hand-rolled poll loop.
    let oauth_client = BasicClient::new(ClientId::new(params.client_id.clone()))
        .set_device_authorization_url(device_auth_url)
        .set_token_uri(token_url);

    // Step 1: Request device authorization (POST to device_authorization_url).
    let mut device_req = oauth_client.exchange_device_code();
    for scope in &params.scopes {
        device_req = device_req.add_scope(Scope::new(scope.clone()));
    }
    let details: StandardDeviceAuthorizationResponse = device_req
        .request_async(&http_client)
        .await
        .map_err(|e| anyhow::anyhow!("device authorization request failed for ns={ns}: {e}"))?;

    // Step 2: Surface verification URL + user code (SC#3 requirement).
    // The device_code is a secret — never print it (A-1).
    println!("To authorize, open: {}", details.verification_uri());
    println!("Enter user code:    {}", details.user_code().secret());

    // Step 3: Initialise poll state.
    // expires_deadline: once SystemTime::now() >= this, the device code is expired (A-5, A-6).
    let expires_deadline = SystemTime::now() + details.expires_in();
    // Capture the device code secret for the poll request body (never logged — A-1).
    let device_code_secret = details.device_code().secret().to_string();
    // Start at the server-supplied interval (RFC 8628 §3.5 / A-5).
    let mut poll_interval = details.interval();

    // Step 4: Hand-rolled poll loop.
    // The oauth2 crate's built-in auto-poller hides the interval, so SC#4 cannot assert it.
    // We drive the loop manually so the interval is observable and the cumulative
    // slow_down backoff is explicit (plan requirement, A-5).
    loop {
        // Sleep or notify the observer (observer replaces real sleep in tests).
        match &params.poll_interval_observer {
            Some(observer) => observer(poll_interval),
            None => tokio::time::sleep(poll_interval).await,
        }

        // Enforce expiry deadline before each poll (A-5).
        if SystemTime::now() >= expires_deadline {
            anyhow::bail!("device code expired before user authorized (ns={ns})");
        }

        // Single poll attempt: POST to token_url with the device_code grant.
        let response = http_client
            .post(&params.token_url)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device_code_secret.as_str()),
                ("client_id", params.client_id.as_str()),
            ])
            .send()
            .await
            .with_context(|| format!("device token poll request failed for ns={ns}"))?;

        let status = response.status();
        let body: serde_json::Value = response
            .json()
            .await
            .with_context(|| format!("failed to parse token poll response for ns={ns}"))?;

        if status.is_success() {
            // Parse the successful token response.
            let access_token = body["access_token"]
                .as_str()
                .ok_or_else(|| {
                    anyhow::anyhow!("missing access_token in device token response for ns={ns}")
                })?
                .to_string();
            let refresh_token = body["refresh_token"].as_str().map(str::to_string);
            let expires_in_secs = body["expires_in"].as_u64();
            // Parse server-returned scopes; fall back to the requested scopes.
            let scopes = body["scope"]
                .as_str()
                .map(|s| s.split_whitespace().map(str::to_string).collect::<Vec<_>>())
                .unwrap_or_else(|| params.scopes.clone());

            // A-6: expires_at = now + expires_in.
            // Clock-skew tolerance (±30 s) is applied by spawn_refresh_task.
            let expires_at =
                expires_in_secs.map(|secs| SystemTime::now() + Duration::from_secs(secs));

            let entry = TokenEntry {
                access_token,
                refresh_token,
                expires_at,
                scopes,
            };
            store.put_token(ns, entry).await?;
            return Ok(());
        }

        // Error response: classify per RFC 8628 §3.5 and act accordingly.
        let error = body["error"].as_str().unwrap_or("unknown_error");
        match error {
            "slow_down" => {
                // Cumulative +5 s per slow_down (A-5). NEVER resets to the initial value.
                poll_interval += Duration::from_secs(5);
            }
            "authorization_pending" => {
                // User has not yet authorized; continue polling at the unchanged interval.
            }
            "access_denied" => {
                anyhow::bail!("device flow authorization denied by user (ns={ns})");
            }
            "expired_token" => {
                anyhow::bail!("device code expired (ns={ns})");
            }
            other => {
                let desc = body["error_description"].as_str().unwrap_or("");
                // Log ns + error only — never log a token value (A-1).
                tracing::warn!("device flow error for ns={ns}: {other} — {desc}");
                anyhow::bail!("device flow failed for ns={ns}: {other} — {desc}");
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// is_headless — D-04 detection matrix
// ─────────────────────────────────────────────────────────────────────────────

/// Detect whether the current environment is headless (no GUI browser available).
///
/// Used by Plan 05's CLI (`cmd_login`) to auto-select the device-code flow when
/// the operator has not explicitly specified `--browser` or `--device-code`:
/// - SSH sessions: `SSH_CONNECTION` or `SSH_TTY` are set.
/// - Linux without a display server: both `DISPLAY` and `WAYLAND_DISPLAY` are unset.
///
/// macOS and Windows always return `false` — a GUI is assumed to be available.
/// The operator can override auto-selection with `--device-code` or `--browser`.
///
/// D-04: this function is the single source of truth for headless detection;
/// Plan 05's CLI must call it rather than re-implementing the env-var checks.
pub fn is_headless() -> bool {
    // SSH session (any platform).
    if std::env::var("SSH_CONNECTION").is_ok() || std::env::var("SSH_TTY").is_ok() {
        return true;
    }
    // Linux without any display server: both X11 and Wayland absent.
    // (macOS and Windows always have a GUI; $DISPLAY is meaningless there.)
    #[cfg(target_os = "linux")]
    if std::env::var("DISPLAY").is_err() && std::env::var("WAYLAND_DISPLAY").is_err() {
        return true;
    }
    false
}

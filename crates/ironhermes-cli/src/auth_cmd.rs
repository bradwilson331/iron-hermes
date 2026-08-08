//! `hermes auth` CLI subcommand — OAuth token acquisition and status (Phase 43, D-05).
//!
//! Provides three subcommands:
//! - `login`  — Authenticate with a configured OAuth provider (D-04 flow selection).
//! - `status` — Show token presence + expiry per provider (A-1: never prints token values).
//! - `list`   — List all configured providers and their authentication state.
//!
//! Flow selection for `login` (D-04 precedence, highest to lowest):
//!   1. `--browser`     → PKCE loopback flow (forced even on headless)
//!   2. `--device-code` → RFC 8628 device-code flow (forced even on GUI)
//!   3. auto            → `is_headless()` → device-code; otherwise PKCE
//!
//! Security: `cmd_status` and `cmd_list` never print `access_token` or
//! `refresh_token` values (A-1, T-43-12).

use anyhow::{Context as _, Result};
use clap::Subcommand;
use colored::Colorize;
use ironhermes_core::auth::device::DeviceFlowParams;
use ironhermes_core::auth::pkce::PkceFlowParams;
use ironhermes_core::auth::{AuthStore, is_headless};
use ironhermes_core::get_hermes_home;

// ─────────────────────────────────────────────────────────────────────────────
// AuthCommands — clap subcommand tree
// ─────────────────────────────────────────────────────────────────────────────

/// OAuth subcommands for managing provider authentication.
#[derive(Subcommand)]
pub enum AuthCommands {
    /// Authenticate with a configured OAuth provider (browser or device-code flow).
    ///
    /// Flow selection (D-04):
    ///   --browser      → force PKCE loopback flow (requires authorization_url)
    ///   --device-code  → force RFC 8628 device-code flow (requires device_authorization_url)
    ///   (neither)      → auto-detect via is_headless()
    Login {
        /// Provider name as configured under auth.providers.<name> in config.yaml
        #[arg(long)]
        provider: String,
        /// Force device-code (RFC 8628) flow even on a GUI system
        #[arg(long, conflicts_with = "browser")]
        device_code: bool,
        /// Force browser (PKCE loopback) flow even on a headless system
        #[arg(long, conflicts_with = "device_code")]
        browser: bool,
    },
    /// Show token status for one or all configured providers.
    ///
    /// Output includes provider name, authenticated/not-authenticated, and expiry date.
    /// The access_token and refresh_token values are NEVER printed (A-1).
    Status {
        /// Provider name (auth.providers.<name> key); omit to show all configured providers
        #[arg(long)]
        provider: Option<String>,
    },
    /// List all configured providers and their current authentication state.
    List,
}

// ─────────────────────────────────────────────────────────────────────────────
// Dispatcher
// ─────────────────────────────────────────────────────────────────────────────

/// Dispatch an `auth` subcommand to the appropriate handler.
pub async fn handle_auth_command(cmd: AuthCommands) -> Result<()> {
    match cmd {
        AuthCommands::Login {
            provider,
            device_code,
            browser,
        } => cmd_login(provider, device_code, browser).await,
        AuthCommands::Status { provider } => cmd_status(provider).await,
        AuthCommands::List => cmd_list().await,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// cmd_login (D-04, D-05)
// ─────────────────────────────────────────────────────────────────────────────

/// Authenticate with a configured OAuth provider (D-04, D-05).
///
/// # Flow selection (D-04 precedence, highest to lowest)
/// 1. `browser=true`     → PKCE loopback flow
/// 2. `device_code=true` → device-code flow
/// 3. auto               → `is_headless()` → device-code; otherwise PKCE
///
/// # Security
/// The PKCE verifier lives only in memory and is consumed by the exchange; it is
/// never written to `auth.json` or any file (A-3). Token values are not logged (A-1).
async fn cmd_login(provider: String, device_code: bool, browser: bool) -> Result<()> {
    // Load config — unwrap_or_default so pre-auth configs parse cleanly.
    let config = ironhermes_core::config::Config::load().unwrap_or_default();

    // Look up provider configuration (T-43-14: error cleanly, no panic).
    let provider_cfg = config
        .auth
        .providers
        .get(&provider)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "provider not configured: {provider}\n\
                 Add it under auth.providers.{provider} in config.yaml"
            )
        })?
        .clone();

    // D-04: select flow — flags always override is_headless() auto-detection.
    let use_device_code = if browser {
        false // --browser forces PKCE even on headless
    } else if device_code {
        true // --device-code forces device-code even on GUI
    } else {
        is_headless() // auto-detect
    };

    // Open the AuthStore at $IRONHERMES_HOME/auth.json.
    let auth_json = get_hermes_home().join("auth.json");
    let store = AuthStore::open(auth_json)
        .await
        .context("failed to open auth store")?;

    if use_device_code {
        // RFC 8628 device-code flow (SC#3, SC#4).
        run_device_code_login(&store, &provider, &provider_cfg).await?;
    } else {
        // PKCE loopback flow (SC#1, SC#2).
        run_pkce_login(&store, &provider, &provider_cfg).await?;
    }

    println!(
        "{} Successfully authenticated with {}",
        "✓".green().bold(),
        provider.bold()
    );
    Ok(())
}

/// PKCE browser/loopback flow entry point.
///
/// Validates that `authorization_url` is present before starting the flow —
/// a clear error surfaces if the operator chose `--browser` on a provider
/// that only has a device-authorization endpoint (T-43-13).
async fn run_pkce_login(
    store: &AuthStore,
    ns: &str,
    cfg: &ironhermes_core::config::AuthProviderConfig,
) -> Result<()> {
    let authorization_url = cfg
        .authorization_url
        .as_ref()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "provider {ns} is missing authorization_url — \
                 required for browser (PKCE) flow\n\
                 Use --device-code if the provider supports device authorization"
            )
        })?
        .clone();
    let token_url = cfg
        .token_url
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("provider {ns} is missing token_url"))?
        .clone();
    let client_id = cfg
        .client_id
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("provider {ns} is missing client_id"))?
        .clone();

    let params = PkceFlowParams {
        authorization_url,
        token_url,
        client_id,
        scopes: cfg.scopes.clone(),
    };

    // open::that is the production browser launcher. On failure the flow prints
    // the URL so the operator can visit it manually (D-04 in pkce.rs).
    ironhermes_core::auth::pkce::run_pkce_flow(store, ns, params, &|url| {
        open::that(url).map_err(Into::into)
    })
    .await
}

/// RFC 8628 device-code flow entry point.
///
/// Validates that `device_authorization_url` is present before starting —
/// a clear error surfaces if the operator chose `--device-code` on a provider
/// that does not support the device flow (T-43-13).
async fn run_device_code_login(
    store: &AuthStore,
    ns: &str,
    cfg: &ironhermes_core::config::AuthProviderConfig,
) -> Result<()> {
    let device_authorization_url = cfg
        .device_authorization_url
        .as_ref()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "provider {ns} is missing device_authorization_url — \
                 required for device-code flow\n\
                 Use --browser if the provider supports PKCE"
            )
        })?
        .clone();
    let token_url = cfg
        .token_url
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("provider {ns} is missing token_url"))?
        .clone();
    let client_id = cfg
        .client_id
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("provider {ns} is missing client_id"))?
        .clone();

    let params = DeviceFlowParams {
        device_authorization_url,
        token_url,
        client_id,
        scopes: cfg.scopes.clone(),
        poll_interval_observer: None, // production path — real tokio::time::sleep
    };

    ironhermes_core::auth::device::run_device_flow(store, ns, params).await
}

// ─────────────────────────────────────────────────────────────────────────────
// cmd_status (A-1: never prints token values)
// ─────────────────────────────────────────────────────────────────────────────

/// Show token status per provider.
///
/// Prints provider name, authenticated/not-authenticated, and expiry date.
/// The `access_token` and `refresh_token` values are NEVER included in output (A-1, T-43-12).
async fn cmd_status(provider: Option<String>) -> Result<()> {
    let config = ironhermes_core::config::Config::load().unwrap_or_default();
    let auth_json = get_hermes_home().join("auth.json");
    let store = AuthStore::open(auth_json)
        .await
        .context("failed to open auth store")?;

    // Determine which namespaces to display.
    let namespaces: Vec<String> = if let Some(p) = provider {
        vec![p]
    } else {
        let mut keys: Vec<String> = config.auth.providers.keys().cloned().collect();
        keys.sort();
        keys
    };

    if namespaces.is_empty() {
        println!("No providers configured. Add providers under auth.providers in config.yaml.");
        return Ok(());
    }

    for ns in &namespaces {
        // Prefer display_name from config; fall back to the namespace key.
        let display_name = config
            .auth
            .providers
            .get(ns)
            .and_then(|p| p.display_name.as_deref())
            .unwrap_or(ns.as_str());

        match store.get_token(ns).await {
            Some(token) => {
                // A-1: ONLY print presence + expiry — NEVER access_token or refresh_token.
                let expiry_str = format_expiry(token.expires_at);
                println!(
                    "provider: {}  status: {}  expires: {}",
                    display_name.bold(),
                    "authenticated".green(),
                    expiry_str
                );
            }
            None => {
                println!(
                    "provider: {}  status: {}",
                    display_name.bold(),
                    "not authenticated".yellow()
                );
            }
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// cmd_list
// ─────────────────────────────────────────────────────────────────────────────

/// List all configured providers and their authentication state.
async fn cmd_list() -> Result<()> {
    let config = ironhermes_core::config::Config::load().unwrap_or_default();
    let auth_json = get_hermes_home().join("auth.json");
    let store = AuthStore::open(auth_json)
        .await
        .context("failed to open auth store")?;

    if config.auth.providers.is_empty() {
        println!("No providers configured. Add providers under auth.providers in config.yaml.");
        return Ok(());
    }

    // Sort by namespace key for stable output.
    let mut providers: Vec<(&String, &ironhermes_core::config::AuthProviderConfig)> =
        config.auth.providers.iter().collect();
    providers.sort_by_key(|(k, _)| k.as_str());

    for (ns, provider_cfg) in &providers {
        let display = provider_cfg.display_name.as_deref().unwrap_or(ns.as_str());
        let token_status = if store.get_token(ns).await.is_some() {
            "authenticated".green().to_string()
        } else {
            "not authenticated".yellow().to_string()
        };
        println!("{}  ({})  {}", display.bold(), ns, token_status);
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Format a `SystemTime` expiry as a human-readable UTC string.
///
/// Uses the `chrono` crate (already a workspace dep) for formatting.
/// Never returns a token value — only a date/time string (A-1).
fn format_expiry(expires_at: Option<std::time::SystemTime>) -> String {
    use std::time::SystemTime;

    let Some(t) = expires_at else {
        return "no expiry".to_string();
    };
    if t <= SystemTime::now() {
        return "expired".to_string();
    }
    // Convert SystemTime → chrono::DateTime<Utc> (chrono 0.4 supports this conversion).
    let dt: chrono::DateTime<chrono::Utc> = t.into();
    dt.format("%Y-%m-%d %H:%M UTC").to_string()
}

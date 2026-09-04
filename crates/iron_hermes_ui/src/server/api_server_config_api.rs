//! Phase 49.3 Plan 05 (D-05): the REST API server's non-secret config
//! surface over `gateway.platforms["api_server"]` (`PlatformGatewayConfig`)
//! — `enabled`/`host`/`port`/`public_opt_in`, mirroring
//! `platform_config_api.rs`'s `set_buzz_enabled`/`set_buzz_edit` pair
//! (module doc there — "The DTO is an explicit allowlist", "D-10 sibling").
//!
//! # The key never lands here (RESEARCH, D-05, D-06)
//!
//! The REST API server reads `IRONHERMES_API_SERVER_KEY` via raw
//! `std::env::var` at startup (`ironhermes-restgw/src/api_server/mod.rs`),
//! so the key MUST reach the scoped `.env` the separately-spawned gateway
//! process loads via `dotenvy` — never `config.yaml`, never the DTO. This
//! module writes `config.yaml` ONLY (`enabled`/`host`/`port`/
//! `public_opt_in`); the key write is entirely
//! [`crate::server::gateway_env_secret_api::set_gateway_secret`] (Plan 02)'s
//! job, called directly by `api_server_card.rs`. [`ApiServerConfigView`]
//! carries `key_present: bool` — presence only, derived by reading the SAME
//! `.env` file the gateway process itself loads
//! ([`key_present_for_scope`]), deliberately never
//! `PlatformGatewayConfig.api_key` (a config.yaml field the shared struct
//! carries for other platforms' local auth) — that field is invisible to
//! the separately-spawned gateway process for THIS platform and this module
//! never reads or writes it.
//!
//! # Defaults are never blank (E5 empty)
//!
//! [`build_api_server_view`] always surfaces `host`/`port` as
//! `Some(127.0.0.1)`/`Some(8642)` when the stored fields are `None` —
//! mirroring `ApiServerAdapter::new`'s own default resolution
//! (`ironhermes-restgw/src/api_server/mod.rs`'s `DEFAULT_HOST`/
//! `DEFAULT_PORT`, duplicated here as local constants since this crate does
//! not depend on `ironhermes-restgw`) — a Gateway-screen operator never sees
//! a blank host/port field, matching what the adapter will actually bind to
//! if the block does not exist yet.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::server::tools_config_api::ConfigScope;

/// The `gateway.platforms` map key this module reads and writes. Never
/// hardcoded a second time below.
const API_SERVER_PLATFORM_KEY: &str = "api_server";

/// Mirrors `ironhermes-restgw::api_server::DEFAULT_HOST` — duplicated here
/// (not imported) because this crate does not depend on `ironhermes-restgw`.
#[cfg(not(target_arch = "wasm32"))]
const API_SERVER_DEFAULT_HOST: &str = "127.0.0.1";

/// Mirrors `ironhermes-restgw::api_server::DEFAULT_PORT`.
#[cfg(not(target_arch = "wasm32"))]
const API_SERVER_DEFAULT_PORT: u16 = 8642;

/// The env var name the REST API server reads at startup
/// (`ironhermes-restgw/src/api_server/mod.rs`). Never hardcoded a second
/// time below.
#[cfg(not(target_arch = "wasm32"))]
const IRONHERMES_API_SERVER_KEY_ENV_NAME: &str = "IRONHERMES_API_SERVER_KEY";

/// A host string is capped at this many characters — generous for any
/// hostname/IP literal, small enough that a paste accident cannot smuggle a
/// multi-megabyte value into config.yaml.
const MAX_HOST_LEN: usize = 255;

// =============================================================================
// DTOs
// =============================================================================

/// Explicit field allowlist over `gateway.platforms["api_server"]`
/// (`PlatformGatewayConfig`) — see module doc's key-never-lands-here
/// section. This DTO never carries `api_key` at any depth (enforced by
/// [`tests::api_server_config_view_dto_carries_no_secret_bearing_field`]) —
/// only [`ApiServerConfigView::key_present`], a presence-only flag derived
/// from the scoped `.env`, never `config.yaml`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ApiServerConfigView {
    pub configured: bool,
    pub enabled: bool,
    /// Never `None` in practice — [`build_api_server_view`] always resolves
    /// a stored `None` to the adapter's own default (E5 empty: never
    /// blank).
    pub host: Option<String>,
    /// Never `None` in practice — same default-surfacing discipline as
    /// `host`.
    pub port: Option<u16>,
    pub public_opt_in: bool,
    /// Presence-only flag for `IRONHERMES_API_SERVER_KEY` in the scoped
    /// `.env` — never the key value itself.
    pub key_present: bool,
}

// =============================================================================
// Server-only helpers — pure where possible (mirrors
// `platform_config_api.rs`'s test-reachability discipline).
// =============================================================================

/// Presence-only check for the REST API server's bearer key, for `scope` —
/// reads the SAME `.env` file the separately-spawned gateway process itself
/// loads at startup via `dotenvy` (Root: `Config::env_path()`; Profile: that
/// profile's own `.env`) — deliberately NOT `PlatformGatewayConfig.api_key`
/// (module doc's "the key never lands here" section explains why). A
/// missing or unreadable `.env` answers `false`, never an error — the same
/// "absent file is not a failure" contract `read_env_keys` already
/// guarantees. No disk write; read-only.
#[cfg(not(target_arch = "wasm32"))]
fn key_present_for_scope(scope: &ConfigScope) -> bool {
    let env_path = match scope {
        ConfigScope::Root => ironhermes_core::config::Config::env_path(),
        ConfigScope::Profile(name) => {
            crate::server::profile_api::profile_dir_for(name).join(".env")
        }
    };
    crate::server::profile_api::read_env_keys(&env_path)
        .ok()
        .and_then(|map| map.get(IRONHERMES_API_SERVER_KEY_ENV_NAME).cloned())
        .is_some_and(|value| !value.is_empty())
}

/// Pure builder: `entry` is `None` when no `api_server:` block exists in
/// `gateway.platforms` at all — mirrors `build_buzz_view`/
/// `build_telegram_view`'s "'not configured' is its own answer" discipline.
/// `host`/`port` always resolve to the adapter's real defaults when the
/// stored field is `None`, never left blank (module doc's "Defaults are
/// never blank" section). No disk I/O; directly unit-testable.
#[cfg(not(target_arch = "wasm32"))]
fn build_api_server_view(
    entry: Option<&ironhermes_core::config::PlatformGatewayConfig>,
    key_present: bool,
) -> ApiServerConfigView {
    match entry {
        Some(cfg) => ApiServerConfigView {
            configured: true,
            enabled: cfg.enabled,
            host: Some(
                cfg.host
                    .clone()
                    .filter(|h| !h.is_empty())
                    .unwrap_or_else(|| API_SERVER_DEFAULT_HOST.to_string()),
            ),
            port: Some(cfg.port.unwrap_or(API_SERVER_DEFAULT_PORT)),
            public_opt_in: cfg.public_opt_in,
            key_present,
        },
        None => ApiServerConfigView {
            configured: false,
            enabled: false,
            host: Some(API_SERVER_DEFAULT_HOST.to_string()),
            port: Some(API_SERVER_DEFAULT_PORT),
            public_opt_in: false,
            key_present,
        },
    }
}

/// D-10 sibling of `check_buzz_write_gate`/`check_gateway_write_gate` — a
/// new module-local fn rather than a reuse, because those are private to
/// their own modules (matches `gateway_env_secret_api.rs`'s own decision,
/// its module doc). Fail-closed: reads `security.web_config_write_enabled`
/// from a FRESH ROOT `Config::load()` regardless of the scope being edited.
#[cfg(not(target_arch = "wasm32"))]
fn check_api_server_write_gate() -> Result<(), String> {
    let root_config =
        ironhermes_core::config::Config::load().map_err(|e| format!("Config load failed: {e}"))?;
    if !root_config.security.web_config_write_enabled {
        return Err("Config writes are disabled".to_string());
    }
    Ok(())
}

/// Read the REST API server config state for `scope` — never errors on an
/// absent `api_server:` block; that is the `configured: false` answer, not
/// a failure. Mirrors `get_buzz_platform_view`/`get_telegram_platform_view`.
#[server]
pub async fn get_api_server_config(
    scope: ConfigScope,
) -> Result<ApiServerConfigView, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (config, _target) = crate::server::tools_config_api::resolve_scope_target(&scope)
            .map_err(ServerFnError::new)?;
        let key_present = key_present_for_scope(&scope);
        Ok(build_api_server_view(
            config.gateway.platforms.get(API_SERVER_PLATFORM_KEY),
            key_present,
        ))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = scope;
        unreachable!("server fn body never runs on the wasm client")
    }
}

// =============================================================================
// Staged edit: enabled/host/port/public_opt_in — one validated, gated,
// atomic save. `api_key` has NO write fn anywhere in this module (module
// doc's "the key never lands here" section) — the key write is
// `gateway_env_secret_api::set_gateway_secret`'s job, called directly by
// `api_server_card.rs`, never through this DTO.
// =============================================================================

/// The staged-write payload for [`set_api_server_edit`] — deliberately
/// touches nothing but `enabled`/`host`/`port`/`public_opt_in`; `api_key`
/// has no field here at all.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ApiServerEditPayload {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub public_opt_in: bool,
}

/// Validate + trim `payload.host`; returns the normalized host on success.
/// No disk I/O; directly unit-testable.
#[cfg(not(target_arch = "wasm32"))]
fn validate_api_server_edit(payload: &ApiServerEditPayload) -> Result<String, String> {
    let trimmed = payload.host.trim();
    if trimmed.is_empty() {
        return Err("host must not be empty".to_string());
    }
    if trimmed.len() > MAX_HOST_LEN {
        return Err(format!("host exceeds {MAX_HOST_LEN} characters"));
    }
    if trimmed.contains('\n') || trimmed.contains('\r') {
        return Err("host must not contain a newline".to_string());
    }
    if payload.port == 0 {
        return Err("port must be between 1 and 65535".to_string());
    }
    Ok(trimmed.to_string())
}

/// Pure(-ish) core of [`set_api_server_edit`]. Staged-write order: validate
/// (no disk I/O — a rejected field aborts here) -> resolve scope (fresh
/// disk read) -> gate check -> read-modify-write the EXISTING map entry
/// (creating it from `Default` only when genuinely absent, so
/// `#[serde(flatten)] extra`, `api_key`, and every sibling platform entry
/// survive) -> atomic save -> re-read fresh from disk.
#[cfg(not(target_arch = "wasm32"))]
async fn set_api_server_edit_impl(
    scope: ConfigScope,
    payload: ApiServerEditPayload,
) -> Result<ApiServerConfigView, String> {
    let host = validate_api_server_edit(&payload)?;

    let (mut config, target) = crate::server::tools_config_api::resolve_scope_target(&scope)?;
    check_api_server_write_gate()?;

    let mut platform = config
        .gateway
        .platforms
        .get(API_SERVER_PLATFORM_KEY)
        .cloned()
        .unwrap_or_default();
    platform.enabled = payload.enabled;
    platform.host = Some(host);
    platform.port = Some(payload.port);
    platform.public_opt_in = payload.public_opt_in;
    // NEVER touches `platform.api_key` — the REST API server key is
    // written exclusively through `gateway_env_secret_api::set_gateway_secret`
    // (Plan 02) into the scoped .env, never into config.yaml.
    config
        .gateway
        .platforms
        .insert(API_SERVER_PLATFORM_KEY.to_string(), platform);
    crate::server::tools_config_api::save_scoped(&config, &target)?;

    let (reread, _reread_target) = crate::server::tools_config_api::resolve_scope_target(&scope)?;
    // Computed from the RE-READ config, not the pre-write one, so the
    // returned view is consistent with what is actually on disk.
    let key_present = key_present_for_scope(&scope);
    Ok(build_api_server_view(
        reread.gateway.platforms.get(API_SERVER_PLATFORM_KEY),
        key_present,
    ))
}

/// Staged-write commit for enabled/host/port/public_opt_in — one validated,
/// gated, atomic save. Never accepts `api_key` — that field has no write fn
/// anywhere in this module.
#[server]
pub async fn set_api_server_edit(
    scope: ConfigScope,
    payload: ApiServerEditPayload,
) -> Result<ApiServerConfigView, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        set_api_server_edit_impl(scope, payload)
            .await
            .map_err(ServerFnError::new)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (scope, payload);
        unreachable!("server fn body never runs on the wasm client")
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use ironhermes_core::config::{Config, PlatformGatewayConfig, SecurityConfig};

    fn seeded_config(write_enabled: bool) -> Config {
        Config {
            security: SecurityConfig {
                web_config_write_enabled: write_enabled,
                ..Default::default()
            },
            ..Config::default()
        }
    }

    /// Recursively walk a serialized JSON value and assert none of the
    /// secret-bearing key names appears at ANY nesting depth — the same
    /// walker `platform_config_api.rs`/`gateway_env_secret_api.rs` use
    /// (test-only duplication across modules is the established precedent
    /// in this crate).
    fn assert_no_secret_key_at_any_depth(value: &serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                for forbidden in ["value", "token", "app_token", "api_key", "secret"] {
                    assert!(
                        !map.contains_key(forbidden),
                        "serialized DTO must never carry a field named '{forbidden}' at any nesting depth"
                    );
                }
                for v in map.values() {
                    assert_no_secret_key_at_any_depth(v);
                }
            }
            serde_json::Value::Array(items) => {
                for v in items {
                    assert_no_secret_key_at_any_depth(v);
                }
            }
            _ => {}
        }
    }

    // -------------------------------------------------------------------
    // build_api_server_view — pure mapping tests
    // -------------------------------------------------------------------

    #[test]
    fn api_server_config_view_dto_carries_no_secret_bearing_field() {
        let view = ApiServerConfigView {
            configured: true,
            enabled: true,
            host: Some("0.0.0.0".to_string()),
            port: Some(9000),
            public_opt_in: true,
            key_present: true,
        };
        let json = serde_json::to_value(&view).expect("DTO must serialize");
        assert_no_secret_key_at_any_depth(&json);
    }

    #[test]
    fn api_server_defaults_surface_when_absent() {
        let view = build_api_server_view(None, false);
        assert!(!view.configured);
        assert_eq!(view.host, Some(API_SERVER_DEFAULT_HOST.to_string()));
        assert_eq!(view.port, Some(API_SERVER_DEFAULT_PORT));
        assert!(!view.key_present);
    }

    #[test]
    fn api_server_defaults_surface_when_present_but_host_port_unset() {
        let cfg = PlatformGatewayConfig {
            enabled: true,
            host: None,
            port: None,
            public_opt_in: false,
            ..Default::default()
        };
        let view = build_api_server_view(Some(&cfg), true);
        assert!(view.configured);
        assert_eq!(
            view.host,
            Some(API_SERVER_DEFAULT_HOST.to_string()),
            "a stored None host must surface the resolved default, never blank"
        );
        assert_eq!(
            view.port,
            Some(API_SERVER_DEFAULT_PORT),
            "a stored None port must surface the resolved default, never blank"
        );
        assert!(view.key_present);
    }

    #[test]
    fn api_server_configured_entry_carries_its_real_host_port() {
        let cfg = PlatformGatewayConfig {
            enabled: true,
            host: Some("0.0.0.0".to_string()),
            port: Some(9999),
            public_opt_in: true,
            ..Default::default()
        };
        let view = build_api_server_view(Some(&cfg), false);
        assert_eq!(view.host, Some("0.0.0.0".to_string()));
        assert_eq!(view.port, Some(9999));
        assert!(view.public_opt_in);
    }

    // -------------------------------------------------------------------
    // validate_api_server_edit — pure, no disk I/O
    // -------------------------------------------------------------------

    #[test]
    fn validate_rejects_empty_host() {
        let payload = ApiServerEditPayload {
            enabled: true,
            host: "   ".to_string(),
            port: 8642,
            public_opt_in: false,
        };
        assert!(validate_api_server_edit(&payload).is_err());
    }

    #[test]
    fn validate_rejects_zero_port() {
        let payload = ApiServerEditPayload {
            enabled: true,
            host: "127.0.0.1".to_string(),
            port: 0,
            public_opt_in: false,
        };
        assert!(validate_api_server_edit(&payload).is_err());
    }

    #[test]
    fn validate_trims_host() {
        let payload = ApiServerEditPayload {
            enabled: true,
            host: "  0.0.0.0  ".to_string(),
            port: 8642,
            public_opt_in: false,
        };
        let host = validate_api_server_edit(&payload).expect("valid payload");
        assert_eq!(host, "0.0.0.0");
    }

    // -------------------------------------------------------------------
    // key_present_for_scope — disk-backed, .env presence only
    // -------------------------------------------------------------------

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn key_present_for_scope_reads_env_presence_not_config_yaml() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        // No .env yet -> false, even with a config.yaml api_key set (that
        // field is deliberately never consulted).
        let mut cfg = seeded_config(true);
        cfg.gateway.platforms.insert(
            API_SERVER_PLATFORM_KEY.to_string(),
            PlatformGatewayConfig {
                api_key: Some("config-yaml-key-must-be-ignored".to_string()),
                ..Default::default()
            },
        );
        cfg.save().expect("seed root config.yaml");

        let before = key_present_for_scope(&ConfigScope::Root);
        assert!(
            !before,
            "a config.yaml api_key must never make key_present true"
        );

        // Write the .env the way set_gateway_secret would.
        let env_path = ironhermes_core::config::Config::env_path();
        let contents = format!("{IRONHERMES_API_SERVER_KEY_ENV_NAME}='the-real-key'\n");
        crate::server::profile_api::write_env_atomic_0600(&env_path, &contents)
            .expect("seed .env");

        let after = key_present_for_scope(&ConfigScope::Root);
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(after, "an .env-present key must report key_present true");
    }

    // -------------------------------------------------------------------
    // set_api_server_edit_impl — gate / round-trip (disk-backed)
    // -------------------------------------------------------------------

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn set_api_server_edit_refuses_when_gate_closed_before_touching_config() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let cfg = seeded_config(false);
        cfg.save().expect("seed root config.yaml with writes disabled");
        let config_path = ironhermes_core::config::Config::config_path();
        let before = std::fs::read(&config_path).expect("read seeded config");

        let payload = ApiServerEditPayload {
            enabled: true,
            host: "0.0.0.0".to_string(),
            port: 9999,
            public_opt_in: true,
        };
        let result = set_api_server_edit_impl(ConfigScope::Root, payload).await;

        let after = std::fs::read(&config_path).expect("read config after refused write");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(result.is_err(), "write must be refused when the gate is closed");
        assert_eq!(before, after, "a refused write must leave disk bytes unchanged");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn set_api_server_edit_writes_fields_and_never_touches_api_key() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let mut cfg = seeded_config(true);
        let mut api_server = PlatformGatewayConfig {
            enabled: false,
            host: Some("127.0.0.1".to_string()),
            port: Some(8642),
            public_opt_in: false,
            api_key: Some("preexisting-key-should-never-move".to_string()),
            ..Default::default()
        };
        api_server
            .extra
            .insert("an_unknown_key".to_string(), serde_json::json!("keep-me"));
        cfg.gateway
            .platforms
            .insert(API_SERVER_PLATFORM_KEY.to_string(), api_server);
        // Sibling platform entry must survive untouched.
        cfg.gateway.platforms.insert(
            "telegram".to_string(),
            PlatformGatewayConfig {
                enabled: true,
                whitelist: vec!["should-not-move".to_string()],
                ..Default::default()
            },
        );
        cfg.save().expect("seed root config.yaml");

        let payload = ApiServerEditPayload {
            enabled: true,
            host: "0.0.0.0".to_string(),
            port: 9999,
            public_opt_in: true,
        };
        let result = set_api_server_edit_impl(ConfigScope::Root, payload).await;
        let reloaded = ironhermes_core::config::Config::load();
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        let view = result.expect("write must succeed when the gate is open");
        assert!(view.enabled);
        assert_eq!(view.host, Some("0.0.0.0".to_string()));
        assert_eq!(view.port, Some(9999));
        assert!(view.public_opt_in);

        let reloaded = reloaded.expect("reload saved config");
        let reloaded_entry = reloaded
            .gateway
            .platforms
            .get(API_SERVER_PLATFORM_KEY)
            .expect("api_server entry must survive its own write");
        assert_eq!(reloaded_entry.host, Some("0.0.0.0".to_string()));
        assert_eq!(reloaded_entry.port, Some(9999));
        assert!(reloaded_entry.public_opt_in);
        assert_eq!(
            reloaded_entry.api_key,
            Some("preexisting-key-should-never-move".to_string()),
            "set_api_server_edit must never touch api_key — it survives byte-for-byte"
        );
        assert_eq!(
            reloaded_entry.extra.get("an_unknown_key"),
            Some(&serde_json::json!("keep-me")),
            "an unknown key inside the api_server entry must survive a read-modify-write"
        );

        let reloaded_telegram = reloaded
            .gateway
            .platforms
            .get("telegram")
            .expect("sibling platform entry must survive the api_server write");
        assert_eq!(
            reloaded_telegram.whitelist,
            vec!["should-not-move".to_string()],
            "a sibling platform's field must never be touched by an api_server write"
        );
    }
}

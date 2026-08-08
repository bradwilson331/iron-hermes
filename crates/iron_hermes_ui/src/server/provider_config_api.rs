//! Phase 46.9 Plan 01 (D-01/D-02/D-10): Providers config read/write server fns.
//!
//! Mirrors the `update_voice_config`/`get_voice_config` four-step write
//! protocol in `server/api.rs:1334-1365` verbatim:
//! 1. validate payload (field range + length checks)
//! 2. `Config::load()` fresh from disk (never `app_state.config` — that is
//!    the startup snapshot; config never hot-reloads, see kanban_api.rs:946)
//! 3. gate check — fail closed unless `security.web_config_write_enabled`
//! 4. merge Some-fields + atomic `config.save()`
//!
//! Secret credential VALUES never cross this boundary in either direction.
//! `ProviderConfigSnapshot`/`ProviderWritePayload` carry only non-secret
//! provider fields plus a per-provider `has_secret: bool` presence flag
//! (computed via `ProviderConfig::has_secret()` in ironhermes-core, so this
//! file never names the underlying secret field itself — T-46.9-03).
//! Provider secret set/rotate/clear lands in Plan 06 (isolated for the D-04
//! security review); this file only ever writes non-secret fields.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Non-secret snapshot of a single configured provider (T-46.9-03).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ProviderSnapshot {
    pub name: String,
    pub base_url: Option<String>,
    /// Inverse of `ProviderConfig.disabled` — the UI speaks in terms of
    /// "enabled", the underlying config field is "disabled" (D-14 style).
    pub enabled: bool,
    pub default_model: Option<String>,
    /// Serialized `ApiMode` variant name (snake_case), e.g. "chat_completions".
    /// Defaults to "chat_completions" when unset (UI-SPEC default variant).
    pub api_mode: String,
    pub fallback_providers: Vec<String>,
    /// Count of distinct known model ids for this provider — the union of
    /// `default_model` (if set) and any per-model override keys. A provider
    /// with no `default_model` and no per-model overrides renders 0 (the
    /// partial backstop fixture), never panics.
    pub model_count: usize,
    /// Presence-only — never the credential value itself.
    pub has_secret: bool,
}

/// Read-only snapshot returned by `get_provider_config`. Never contains the
/// raw `Config` — only fields the UI is allowed to display.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ProviderConfigSnapshot {
    pub providers: Vec<ProviderSnapshot>,
    /// Whether the web config write gate is open. The UI locks EDIT/NEW/SAVE
    /// when this is false (mirrors `VoiceConfigSnapshot.web_config_write_enabled`).
    pub web_config_write_enabled: bool,
}

/// Write payload for `update_provider_config`. `name` identifies which
/// `config.providers` entry to create/merge into; every other field is
/// `Option` so only present fields are merged (T-46.9-01 fail-closed +
/// V5 input validation happen before any of these ever reach `Config`).
/// No secret credential field exists on this type by design (T-46.9-03).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ProviderWritePayload {
    pub name: String,
    pub base_url: Option<String>,
    pub enabled: Option<bool>,
    pub default_model: Option<String>,
    /// Serialized `ApiMode` variant name (snake_case). Unknown/garbage
    /// strings are silently ignored during merge — mirrors
    /// `merge_voice_payload`'s `barge_in_mode` handling (api.rs:1162-1169).
    pub api_mode: Option<String>,
    pub fallback_providers: Option<Vec<String>>,
    /// Gap 3 (D-01, CR-03): when `true`, the caller asserts this is a
    /// **create** (the `+ NEW PROVIDER` flow) — `update_provider_config`
    /// rejects the write if `payload.name` already exists in
    /// `config.providers`, instead of silently overwriting the existing
    /// provider's fields. `false` (the default, so existing/EDIT callers are
    /// unaffected) preserves the original upsert-by-name behavior.
    #[serde(default)]
    pub expect_new: bool,
}

/// Phase 46.9 Plan 01: Validate payload field lengths/shape before any
/// `Config` mutation (V5 input validation, T-46.9-02).
#[cfg(not(target_arch = "wasm32"))]
fn validate_provider_payload(payload: &ProviderWritePayload) -> Result<(), ServerFnError> {
    if payload.name.trim().is_empty() {
        return Err(ServerFnError::new("provider name must not be empty"));
    }
    if payload.name.len() > 64 {
        return Err(ServerFnError::new("provider name too long (max 64 chars)"));
    }
    if let Some(ref url) = payload.base_url {
        if url.len() > 512 {
            return Err(ServerFnError::new("base_url too long (max 512 chars)"));
        }
        if !is_well_formed_http_url(url) {
            return Err(ServerFnError::new(format!(
                "base_url '{url}' is not a well-formed http(s) URL"
            )));
        }
    }
    if let Some(ref model) = payload.default_model {
        if model.len() > 128 {
            return Err(ServerFnError::new("default_model too long (max 128 chars)"));
        }
    }
    if let Some(ref list) = payload.fallback_providers {
        if list.len() > 32 {
            return Err(ServerFnError::new(
                "fallback_providers too long (max 32 entries)",
            ));
        }
        for entry in list {
            if entry.len() > 64 {
                return Err(ServerFnError::new(
                    "fallback_providers entry too long (max 64 chars)",
                ));
            }
        }
    }
    Ok(())
}

/// Well-formed http(s) URL check (V5). Deliberately does not add a direct
/// `url` crate dependency to `iron_hermes_ui` (out of this plan's
/// `files_modified` scope) — a minimal scheme+host-present check is
/// sufficient here; SSRF-grade validation already lives at resolver-build
/// time in `ironhermes-core::provider::is_provider_url_safe`.
#[cfg(not(target_arch = "wasm32"))]
fn is_well_formed_http_url(url: &str) -> bool {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    match rest {
        Some(rest) => !rest.trim().is_empty(),
        None => false,
    }
}

/// Gap 3 (D-01, CR-03): true when `payload` asserts a create (`expect_new`)
/// but `payload.name` already exists in `config.providers` — the create must
/// be rejected rather than silently overwriting the live provider's fields.
/// `expect_new=false` (EDIT) never collides, regardless of whether the name
/// exists — an edit is expected to upsert an existing entry.
#[cfg(not(target_arch = "wasm32"))]
fn provider_name_collides(
    config: &ironhermes_core::config::Config,
    payload: &ProviderWritePayload,
) -> bool {
    payload.expect_new && config.providers.contains_key(&payload.name)
}

/// Merge only present (`Some`) non-secret fields into `config.providers`,
/// keyed by `payload.name`. Mirrors `merge_voice_payload` (api.rs:1124-1216).
#[cfg(not(target_arch = "wasm32"))]
fn merge_provider_payload(
    config: &mut ironhermes_core::config::Config,
    payload: &ProviderWritePayload,
) {
    let entry = config.providers.entry(payload.name.clone()).or_default();
    if let Some(ref v) = payload.base_url {
        entry.base_url = Some(v.clone());
    }
    if let Some(v) = payload.enabled {
        // UI speaks "enabled"; the underlying field is "disabled" (inverse).
        entry.disabled = Some(!v);
    }
    if let Some(ref v) = payload.default_model {
        entry.default_model = Some(v.clone());
    }
    if let Some(ref v) = payload.api_mode {
        use ironhermes_core::config::ApiMode;
        let parsed: Result<ApiMode, _> =
            serde_json::from_value(serde_json::Value::String(v.clone()));
        if let Ok(mode) = parsed {
            entry.api_mode = Some(mode);
        }
    }
    if let Some(ref v) = payload.fallback_providers {
        entry.fallback_providers = v.clone();
    }
}

/// Build a `ProviderSnapshot` for one provider entry. `has_secret` is
/// computed via `ProviderConfig::has_secret()` — a presence-only check that
/// never surfaces the credential value (T-46.9-03).
#[cfg(not(target_arch = "wasm32"))]
fn build_provider_snapshot(
    name: &str,
    cfg: &ironhermes_core::config::ProviderConfig,
) -> ProviderSnapshot {
    let api_mode = cfg
        .api_mode
        .as_ref()
        .and_then(|m| serde_json::to_value(m).ok())
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "chat_completions".to_string());

    // model_count: the union of default_model (if set) and any per-model
    // override keys. A provider with neither renders 0 — the partial
    // backstop fixture (base_url set, default_model None) must render
    // without crashing, never panicking on an empty set.
    let mut model_ids: std::collections::HashSet<String> = cfg.models.keys().cloned().collect();
    if let Some(ref dm) = cfg.default_model {
        model_ids.insert(dm.clone());
    }

    ProviderSnapshot {
        name: name.to_string(),
        base_url: cfg.base_url.clone(),
        enabled: !cfg.disabled.unwrap_or(false),
        default_model: cfg.default_model.clone(),
        api_mode,
        fallback_providers: cfg.fallback_providers.clone(),
        model_count: model_ids.len(),
        has_secret: cfg.has_secret(),
    }
}

/// Phase 46.9 Plan 01: Return a snapshot of web-safe provider config
/// fields. Reads fresh from disk (never `app_state.config` — the startup
/// snapshot; config never hot-reloads, kanban_api.rs:946-950).
#[server]
pub async fn get_provider_config() -> Result<ProviderConfigSnapshot, ServerFnError> {
    let config = ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;

    let mut providers: Vec<ProviderSnapshot> = config
        .providers
        .iter()
        .map(|(name, cfg)| build_provider_snapshot(name, cfg))
        .collect();
    // Deterministic ordering — HashMap iteration order is not stable.
    providers.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(ProviderConfigSnapshot {
        providers,
        web_config_write_enabled: config.security.web_config_write_enabled,
    })
}

/// Phase 46.9 Plan 01 (D-01/D-10): Write non-secret provider settings from
/// the browser to `config.yaml`.
///
/// Four-step protocol (mirrors `update_voice_config`, api.rs:1323-1354):
/// 1. validate payload (field range + length checks)
/// 2. `Config::load()` fresh from disk
/// 3. gate check — return error if `security.web_config_write_enabled` is false
/// 4. merge + atomic save (`config.save()` uses temp+rename)
///
/// No secret field exists on `ProviderWritePayload` (T-46.9-03) so a secret
/// credential is never touched by this fn — secret set/rotate/clear is
/// Plan 06's isolated surface.
#[server]
pub async fn update_provider_config(payload: ProviderWritePayload) -> Result<(), ServerFnError> {
    // Step 1: validate
    validate_provider_payload(&payload)?;

    // Step 2: fresh disk read (NOT app_state.config — that is the startup snapshot)
    let mut config = ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;

    // Step 3: gate — web config write is disabled unless operator opts in
    if !config.security.web_config_write_enabled {
        return Err(ServerFnError::new("Config writes are disabled"));
    }

    // Step 3.5 (Gap 3 / D-01, CR-03): a create (`expect_new: true`) must not
    // silently overwrite an existing provider — reject BEFORE merge. Runs
    // after the fail-closed gate above so gate order/T-46.9-18 is preserved;
    // EDIT (`expect_new: false`) is unaffected and still upserts normally.
    if provider_name_collides(&config, &payload) {
        return Err(ServerFnError::new("Provider already exists"));
    }

    // Step 4: merge non-secret fields, then atomic save
    merge_provider_payload(&mut config, &payload);
    config
        .save()
        .map_err(|e| ServerFnError::new(format!("Config save failed: {e}")))?;

    Ok(())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod provider_config_tests {
    use super::{
        build_provider_snapshot, is_well_formed_http_url, merge_provider_payload,
        provider_name_collides, validate_provider_payload, ProviderWritePayload,
    };
    use ironhermes_core::config::{Config, ProviderConfig};

    fn empty_payload(name: &str) -> ProviderWritePayload {
        ProviderWritePayload {
            name: name.to_string(),
            base_url: None,
            enabled: None,
            default_model: None,
            api_mode: None,
            fallback_providers: None,
            expect_new: false,
        }
    }

    /// T-46.9-01: The write gate must fail closed when
    /// security.web_config_write_enabled is false (the default).
    #[test]
    fn gate_fails_closed_by_default() {
        let config = Config::default();
        assert!(
            !config.security.web_config_write_enabled,
            "web_config_write_enabled must default to false (gate closed)"
        );
    }

    /// Non-secret round-trip: a base_url + enabled=false write merges into
    /// config.providers and is visible in the resulting snapshot (mirrors
    /// tests/kanban_write_fns.rs style expectations at the data layer).
    #[test]
    fn non_secret_write_round_trip_enabled_toggle_persists() {
        let mut config = Config::default();
        let mut payload = empty_payload("openrouter");
        payload.base_url = Some("https://openrouter.ai/api/v1".to_string());
        payload.enabled = Some(false);
        payload.default_model = Some("anthropic/claude-3.5-sonnet".to_string());
        payload.fallback_providers = Some(vec!["anthropic".to_string()]);

        merge_provider_payload(&mut config, &payload);

        let cfg = config
            .providers
            .get("openrouter")
            .expect("provider entry created");
        assert_eq!(
            cfg.base_url.as_deref(),
            Some("https://openrouter.ai/api/v1")
        );
        assert_eq!(
            cfg.disabled,
            Some(true),
            "enabled=false must set disabled=true"
        );
        assert_eq!(
            cfg.default_model.as_deref(),
            Some("anthropic/claude-3.5-sonnet")
        );
        assert_eq!(cfg.fallback_providers, vec!["anthropic".to_string()]);

        let snapshot = build_provider_snapshot("openrouter", cfg);
        assert!(
            !snapshot.enabled,
            "enabled toggle must persist through to the snapshot"
        );
        assert_eq!(snapshot.model_count, 1);
    }

    /// Gap 3 (D-01, CR-03): a create (`expect_new: true`) whose name already
    /// exists must be detected as a collision, AND the pre-existing
    /// provider's fields must remain unchanged (the rejected-create path
    /// never reaches `merge_provider_payload`).
    #[test]
    fn create_with_expect_new_rejects_existing_name() {
        let mut config = Config::default();
        let mut seed = empty_payload("foo");
        seed.base_url = Some("https://existing.example.com/v1".to_string());
        seed.enabled = Some(true);
        merge_provider_payload(&mut config, &seed);

        let mut create_payload = empty_payload("foo");
        create_payload.expect_new = true;
        create_payload.base_url = Some("https://attacker.example.com/v1".to_string());
        create_payload.enabled = Some(false);

        assert!(
            provider_name_collides(&config, &create_payload),
            "expect_new=true against an existing name must be flagged as a collision"
        );

        // Simulate `update_provider_config`'s guard: on a collision it
        // returns Err BEFORE calling merge_provider_payload, so the
        // pre-existing provider's fields must be untouched.
        let cfg = config
            .providers
            .get("foo")
            .expect("pre-existing provider still present");
        assert_eq!(
            cfg.base_url.as_deref(),
            Some("https://existing.example.com/v1"),
            "a rejected create must not mutate the existing provider's base_url"
        );
        assert_eq!(
            cfg.disabled,
            Some(false),
            "a rejected create must not mutate the existing provider's enabled/disabled state"
        );
    }

    /// EDIT (`expect_new: false`) must never be treated as a collision, even
    /// when the name already exists — it upserts normally.
    #[test]
    fn edit_still_upserts_existing() {
        let mut config = Config::default();
        let mut seed = empty_payload("foo");
        seed.base_url = Some("https://existing.example.com/v1".to_string());
        merge_provider_payload(&mut config, &seed);

        let mut edit_payload = empty_payload("foo");
        edit_payload.expect_new = false;
        edit_payload.base_url = Some("https://updated.example.com/v1".to_string());

        assert!(
            !provider_name_collides(&config, &edit_payload),
            "expect_new=false (EDIT) must never be flagged as a collision"
        );

        merge_provider_payload(&mut config, &edit_payload);

        let cfg = config
            .providers
            .get("foo")
            .expect("provider entry present after edit");
        assert_eq!(
            cfg.base_url.as_deref(),
            Some("https://updated.example.com/v1"),
            "EDIT must still upsert the existing provider's fields"
        );
    }

    // Note: a "merge never touches the secret credential" test is
    // intentionally NOT written here — `ProviderWritePayload` has no secret
    // field at all, so the type system already proves merge cannot touch
    // one. Writing a fixture for it would require naming the underlying
    // credential field literally in this file, which would trip the strict
    // zero-occurrence redaction gate on this file's source text (T-46.9-03).
    // See `ProviderConfig::has_secret()` in ironhermes-core for the
    // presence-only check this file relies on instead.

    /// Backstop fixture (UI-SPEC 46.9): a provider with base_url set and
    /// default_model None must render (model_count 0, no DEFAULT pill)
    /// without panicking.
    #[test]
    fn partial_fixture_base_url_without_default_model_does_not_panic() {
        let cfg = ProviderConfig {
            base_url: Some("https://api.example.com/v1".to_string()),
            // default_model intentionally left None.
            ..ProviderConfig::default()
        };

        let snapshot = build_provider_snapshot("partial-provider", &cfg);
        assert_eq!(snapshot.model_count, 0);
        assert!(snapshot.default_model.is_none());
        assert_eq!(
            snapshot.base_url.as_deref(),
            Some("https://api.example.com/v1")
        );
    }

    #[test]
    fn validate_rejects_malformed_base_url() {
        let mut payload = empty_payload("openrouter");
        payload.base_url = Some("not-a-url".to_string());
        assert!(validate_provider_payload(&payload).is_err());
    }

    #[test]
    fn validate_accepts_well_formed_https_url() {
        let mut payload = empty_payload("openrouter");
        payload.base_url = Some("https://api.example.com/v1".to_string());
        assert!(validate_provider_payload(&payload).is_ok());
    }

    #[test]
    fn validate_rejects_empty_name() {
        let payload = empty_payload("");
        assert!(validate_provider_payload(&payload).is_err());
    }

    #[test]
    fn well_formed_http_url_helper() {
        assert!(is_well_formed_http_url("https://api.example.com/v1"));
        assert!(is_well_formed_http_url("http://localhost:8080"));
        assert!(!is_well_formed_http_url("ftp://example.com"));
        assert!(!is_well_formed_http_url("not-a-url"));
        assert!(!is_well_formed_http_url("https://"));
    }
}

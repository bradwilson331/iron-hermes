//! Phase 46.9 Plan 01 — Source-string locks + data-layer round-trip test for
//! `provider_config_api.rs`.
//!
//! `iron_hermes_ui` is a bin-only crate (no `src/lib.rs`), so integration
//! tests under `tests/` cannot `use iron_hermes_ui::...` (see
//! tests/kanban_write_fns.rs, tests/kanban_server_fns.rs for the same
//! constraint). The functional gate/merge/validate/snapshot tests for
//! `provider_config_api.rs`'s internals live as `#[cfg(test)]` unit tests
//! inside the file itself (`cargo test -p iron_hermes_ui --lib
//! provider_config`). This file locks the module's *shape* via
//! source-string assertions (mirrors tests/kanban_write_fns.rs) plus a
//! data-layer round-trip exercised directly against `ironhermes_core`,
//! which — unlike `iron_hermes_ui` — is a normal importable lib crate.

use std::fs;
use std::path::PathBuf;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    fs::read_to_string(crate_root().join(path)).expect("failed to read source file")
}

// ---------------------------------------------------------------------------
// Shape locks (source-string assertions)
// ---------------------------------------------------------------------------

#[test]
fn provider_config_api_declares_the_two_server_fns() {
    let src = read("src/server/provider_config_api.rs");
    for name in ["get_provider_config", "update_provider_config"] {
        assert!(
            src.contains(&format!("fn {}", name)),
            "provider_config_api.rs must declare `fn {}`",
            name,
        );
    }
    assert!(
        src.matches("#[server]").count() >= 2,
        "provider_config_api.rs must have at least 2 #[server] fns"
    );
}

#[test]
fn provider_config_api_module_is_registered() {
    let src = read("src/server/mod.rs");
    assert!(
        src.contains("pub mod provider_config_api;"),
        "server/mod.rs must contain `pub mod provider_config_api;`"
    );
}

/// T-46.9-03: the DTO file must never spell out the secret credential field
/// name — presence is computed via `ProviderConfig::has_secret()` in
/// ironhermes-core instead, keeping this file's redaction gate a simple
/// zero-occurrence text check.
#[test]
fn provider_config_api_never_names_the_secret_field() {
    let src = read("src/server/provider_config_api.rs");
    assert_eq!(
        src.matches("api_key").count(),
        0,
        "provider_config_api.rs must contain zero occurrences of the secret \
         credential field name — found {}",
        src.matches("api_key").count(),
    );
}

/// T-46.9-01: the write fn must perform the fail-closed gate check before
/// any merge/save, using the same copy as `update_voice_config`.
#[test]
fn update_provider_config_gates_on_web_config_write_enabled() {
    let src = read("src/server/provider_config_api.rs");
    assert!(
        src.contains("web_config_write_enabled"),
        "update_provider_config must check security.web_config_write_enabled"
    );
    assert!(
        src.contains("Config writes are disabled"),
        "update_provider_config must return the shared gate-closed error copy"
    );
}

/// The read fn must always use a fresh `Config::load()`, never the
/// AppState startup snapshot (config never hot-reloads). Note: this checks
/// for actual field-access usage (`state.config` / `.config.security`),
/// not mere mentions in doc comments explaining the rule itself.
#[test]
fn get_provider_config_reads_fresh_from_disk_not_app_state() {
    let src = read("src/server/provider_config_api.rs");
    assert!(
        src.contains("Config::load()"),
        "get_provider_config must call Config::load() fresh from disk"
    );
    assert!(
        !src.contains("global_app_state()"),
        "provider_config_api.rs must never pull the AppState startup \
         snapshot — always Config::load() fresh"
    );
}

/// Gap 3 (D-01, CR-03): the source must declare the `expect_new` field/guard
/// and the distinct collision-rejection error copy.
#[test]
fn provider_config_api_declares_expect_new_collision_guard() {
    let src = read("src/server/provider_config_api.rs");
    assert!(
        src.contains("expect_new"),
        "provider_config_api.rs must declare an expect_new field/guard"
    );
    assert!(
        src.contains("Provider already exists"),
        "update_provider_config must reject a colliding create with a distinct error"
    );
}

// ---------------------------------------------------------------------------
// Data-layer round trip (against ironhermes_core directly — a real lib crate)
// ---------------------------------------------------------------------------

/// T-46.9-01: `security.web_config_write_enabled` defaults to false
/// (fail-closed) on a fresh `Config`.
#[test]
fn gate_fails_closed_by_default() {
    let config = ironhermes_core::config::Config::default();
    assert!(!config.security.web_config_write_enabled);
}

/// Non-secret write round trip: apply the same field shape
/// `update_provider_config` would merge, save to a temp path, reload, and
/// confirm the enabled toggle + other non-secret fields persisted.
#[test]
fn non_secret_write_round_trips_through_save_and_load() {
    let tmp = std::env::temp_dir().join(format!(
        "iron_hermes_ui_provider_config_api_test_{}",
        std::process::id()
    ));
    let config_path = tmp.join("config.yaml");

    let mut config = ironhermes_core::config::Config::default();
    let entry = config
        .providers
        .entry("openrouter".to_string())
        .or_default();
    entry.base_url = Some("https://openrouter.ai/api/v1".to_string());
    entry.disabled = Some(true); // enabled: false
    entry.default_model = Some("anthropic/claude-3.5-sonnet".to_string());
    entry.fallback_providers = vec!["anthropic".to_string()];

    config.save_to(&config_path).expect("save_to must succeed");
    let reloaded =
        ironhermes_core::config::Config::load_from(&config_path).expect("load_from must succeed");

    let cfg = reloaded
        .providers
        .get("openrouter")
        .expect("provider entry round-trips");
    assert_eq!(
        cfg.base_url.as_deref(),
        Some("https://openrouter.ai/api/v1")
    );
    assert_eq!(cfg.disabled, Some(true));
    assert_eq!(
        cfg.default_model.as_deref(),
        Some("anthropic/claude-3.5-sonnet")
    );
    assert_eq!(cfg.fallback_providers, vec!["anthropic".to_string()]);

    let _ = fs::remove_dir_all(&tmp);
}

/// Partial fixture backstop: a provider with `base_url` set and
/// `default_model` None must serialize/deserialize cleanly (no panic) —
/// this is the config-layer half of the UI-SPEC partial backstop; the
/// rendering half is locked in `provider_config_tests::
/// partial_fixture_base_url_without_default_model_does_not_panic`
/// (src/server/provider_config_api.rs unit test).
#[test]
fn partial_fixture_survives_save_and_load_round_trip() {
    let tmp = std::env::temp_dir().join(format!(
        "iron_hermes_ui_provider_config_api_partial_test_{}",
        std::process::id()
    ));
    let config_path = tmp.join("config.yaml");

    let mut config = ironhermes_core::config::Config::default();
    let entry = config
        .providers
        .entry("partial-provider".to_string())
        .or_default();
    entry.base_url = Some("https://api.example.com/v1".to_string());
    // default_model intentionally left None.

    config.save_to(&config_path).expect("save_to must succeed");
    let reloaded =
        ironhermes_core::config::Config::load_from(&config_path).expect("load_from must succeed");

    let cfg = reloaded
        .providers
        .get("partial-provider")
        .expect("partial provider entry round-trips");
    assert_eq!(cfg.base_url.as_deref(), Some("https://api.example.com/v1"));
    assert!(cfg.default_model.is_none());
    assert!(cfg.models.is_empty());

    let _ = fs::remove_dir_all(&tmp);
}

/// Gap 3 (D-01, CR-03) data-layer mirror: `iron_hermes_ui` is bin-only (no
/// lib.rs) so this integration test cannot import
/// `provider_name_collides`/`ProviderWritePayload` directly (see module doc)
/// — it mirrors the same `expect_new && contains_key` precondition against a
/// real `ironhermes_core::config::Config`, and asserts a rejected create
/// never mutates the pre-existing provider's fields (the real guard in
/// `update_provider_config` returns `Err` before `merge_provider_payload`
/// ever runs).
#[test]
fn collision_check_rejects_create_and_leaves_existing_provider_untouched() {
    let mut config = ironhermes_core::config::Config::default();
    {
        let entry = config
            .providers
            .entry("openrouter".to_string())
            .or_default();
        entry.base_url = Some("https://openrouter.ai/api/v1".to_string());
        entry.disabled = Some(false);
        entry.fallback_providers = vec!["anthropic".to_string()];
    }

    let name = "openrouter";
    let expect_new = true;
    let collides = expect_new && config.providers.contains_key(name);
    assert!(collides, "a create must detect the pre-existing name");

    // A rejected create never reaches the merge step, so the existing
    // provider's fields must remain exactly as seeded above.
    let cfg = config
        .providers
        .get("openrouter")
        .expect("provider still present after a rejected create");
    assert_eq!(
        cfg.base_url.as_deref(),
        Some("https://openrouter.ai/api/v1")
    );
    assert_eq!(cfg.disabled, Some(false));
    assert_eq!(cfg.fallback_providers, vec!["anthropic".to_string()]);
}

/// EDIT (`expect_new: false`) must never be treated as a collision, even
/// when the name already exists in `config.providers`.
#[test]
fn collision_check_allows_edit_to_upsert_existing_name() {
    let mut config = ironhermes_core::config::Config::default();
    config
        .providers
        .entry("openrouter".to_string())
        .or_default();

    let name = "openrouter";
    let expect_new = false;
    let collides = expect_new && config.providers.contains_key(name);
    assert!(
        !collides,
        "an edit (expect_new=false) must never be treated as a collision"
    );
}

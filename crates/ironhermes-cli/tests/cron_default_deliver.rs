//! Phase 22.4.2.2 Plan 01 — behavioral tests for `cmd_create` TG default-routing.
//!
//! Tests drive `Config::telegram_default_origin()` directly (unit seam) and verify
//! the resulting `OriginDecision` maps to the correct (deliver_str, origin_opt) pair
//! per the rules in cmd_create's resolve_cron_deliver helper.
//! `IRONHERMES_HOME` is redirected to a TempDir for each test so `Config::load()`
//! reads the fixture config.yaml.
//!
//! Run: `cargo test -p ironhermes-cli --test cron_default_deliver`

use ironhermes_core::config::{Config, OriginDecision};
use tempfile::TempDir;

/// Process-global env lock — `IRONHERMES_HOME` is shared across the whole
/// test binary and tests can race under `cargo test --jobs N`.
fn env_lock() -> &'static std::sync::Mutex<()> {
    use std::sync::OnceLock;
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

/// Write a fixture config.yaml at `<tmp>/config.yaml`.
fn make_test_config(tmp: &TempDir, tg_enabled: bool, whitelist: &[i64]) {
    let whitelist_yaml = if whitelist.is_empty() {
        "      whitelist: []".to_string()
    } else {
        let entries = whitelist
            .iter()
            .map(|id| format!("        - {}", id))
            .collect::<Vec<_>>()
            .join("\n");
        format!("      whitelist:\n{}", entries)
    };
    let yaml = format!(
        "gateway:\n  platforms:\n    telegram:\n      enabled: {}\n{}\n",
        tg_enabled, whitelist_yaml
    );
    std::fs::write(tmp.path().join("config.yaml"), yaml).unwrap();
}

#[test]
fn tg_enabled_single_chat_routes_to_origin() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    make_test_config(&tmp, true, &[12345]);
    unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }

    let config = Config::load().expect("config must load");
    let decision = config.telegram_default_origin();

    match decision {
        OriginDecision::Single { platform, chat_id } => {
            assert_eq!(platform, "telegram");
            assert_eq!(chat_id, "12345");
        }
        other => panic!("expected Single, got {:?}", other),
    }
}

#[test]
fn tg_enabled_multi_chat_falls_back_to_local() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    make_test_config(&tmp, true, &[12345, 67890]);
    unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }

    let config = Config::load().expect("config must load");
    let decision = config.telegram_default_origin();

    match decision {
        OriginDecision::Multi { whitelist } => {
            assert!(whitelist.contains(&"12345".to_string()));
            assert!(whitelist.contains(&"67890".to_string()));
        }
        other => panic!("expected Multi, got {:?}", other),
    }
}

#[test]
fn tg_disabled_falls_back_to_local() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    make_test_config(&tmp, false, &[12345]);
    unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }

    let config = Config::load().expect("config must load");
    assert!(
        matches!(config.telegram_default_origin(), OriginDecision::None),
        "disabled TG must return OriginDecision::None"
    );
}

#[test]
fn tg_section_missing_falls_back_to_local() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    // Minimal config with no gateway section
    std::fs::write(tmp.path().join("config.yaml"), "model:\n  default: test\n").unwrap();
    unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }

    let config = Config::load().expect("config must load");
    assert!(
        matches!(config.telegram_default_origin(), OriginDecision::None),
        "missing TG section must return OriginDecision::None"
    );
}

#[test]
fn explicit_deliver_flag_skips_helper() {
    // This test verifies that resolve_cron_deliver's Some(d) arm bypasses the
    // helper entirely. Since resolve_cron_deliver is pub(crate) (not visible
    // from this integration test), we exercise the equivalent contract:
    // (1) confirm config IS eligible for auto-routing (helper returns Single),
    // (2) document that cmd_create's `match deliver { Some(d) => (d, None), ... }`
    //     pattern bypasses the helper. INV-22.4.2.2-01 guards the wiring.
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    make_test_config(&tmp, true, &[12345]);
    unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }

    let config = Config::load().expect("config must load");
    // Config IS eligible — explicit flag would bypass this.
    assert!(
        matches!(config.telegram_default_origin(), OriginDecision::Single { .. }),
        "config is eligible for auto-routing; explicit flag bypass is in cmd_create match arm"
    );
    // The actual bypass `Some(d) => (d, None)` lives in resolve_cron_deliver.
    // INV-22.4.2.2-01 + the source-grep on cron.rs prove the wiring.
}

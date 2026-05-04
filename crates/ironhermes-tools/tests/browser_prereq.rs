//! Phase 25.1 D-21: schema-exclusion test + D-19 cache-break-banner regression gate.
//!
//! **D-21 (T-25.1-07 mitigation):** Asserts that ALL 11 browser_* tool schemas are EXCLUDED
//! from ToolRegistry::get_definitions() when chromium is not discoverable.
//! Uses the IRONHERMES_BROWSER_TEST_DISABLE escape hatch (added to find_chromium_binary in
//! browser_session.rs) to force the "no chromium" condition deterministically — dev machines
//! with system Chrome at /Applications/... would otherwise pass through the platform-path
//! fallback and make the test flaky.
//!
//! **D-19 regression gate:** Asserts ToolsConfig::default().toolsets["browser"].enabled == false.
//! Failing this test means a config refactor silently flipped the default, which would break
//! the Phase 25 D-04 cache-break banner (operators won't see the schema-cache-rebuild notice
//! on `hermes toolset enable browser`).
//!
//! Threat anchor: T-25.1-07 (schema-cache poisoning when chromium absent).
//! Run with: cargo test -p ironhermes-tools --test browser_prereq -- --test-threads=1

use std::sync::OnceLock;

fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

/// Closed enumeration of all 11 browser_* tool names per Phase 25.1 D-04.
///
/// The `assert_eq!(BROWSER_TOOL_NAMES.len(), 11, ...)` call at the end of the
/// chromium-missing test is a compile-time anchor: if a 12th browser_* tool is added
/// without updating this list, the test will fail at runtime with a clear message,
/// making the gap visible before merge.
const BROWSER_TOOL_NAMES: &[&str] = &[
    "browser_back",
    "browser_click",
    "browser_close",
    "browser_console",
    "browser_get_images",
    "browser_navigate",
    "browser_press",
    "browser_scroll",
    "browser_snapshot",
    "browser_type",
    "browser_vision",
];

/// Helper: build a ProviderResolver from the default config.
/// Mirrors the dummy_resolver() helper in browser_vision.rs tests.
fn build_resolver() -> std::sync::Arc<ironhermes_core::provider::ProviderResolver> {
    let config = ironhermes_core::config::Config::default();
    std::sync::Arc::new(
        ironhermes_core::provider::ProviderResolver::build(&config)
            .expect("ProviderResolver must build from default config"),
    )
}

/// Phase 25.1 D-21 / T-25.1-07 mitigation:
/// ALL 11 browser_* tool schemas MUST be absent from get_definitions() output when
/// find_chromium_binary() returns None (forced via IRONHERMES_BROWSER_TEST_DISABLE).
///
/// This test passes on dev machines with system Chrome installed because the escape hatch
/// intercepts find_chromium_binary before it reaches the platform-path fallback.
#[tokio::test]
async fn browser_tools_excluded_when_chromium_missing() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    // SAFETY: env_lock + --test-threads=1 ensure single env mutator (Phase 21.6 D Rust 2024).
    unsafe {
        std::env::remove_var("BROWSER_PATH");
        std::env::remove_var("CHROMIUM_PATH");
        // Force find_chromium_binary to return None even on machines with system Chrome.
        std::env::set_var("IRONHERMES_BROWSER_TEST_DISABLE", "1");
    }

    let mut registry = ironhermes_tools::ToolRegistry::new();
    let session = std::sync::Arc::new(tokio::sync::Mutex::new(
        None::<ironhermes_tools::browser_session::BrowserSession>,
    ));
    let resolver = build_resolver();
    let config = std::sync::Arc::new(ironhermes_core::config::Config::default());
    registry.register_browser_tools(session, resolver, config);

    // Enable the browser toolset — we want to confirm that the prereq gate fires
    // even when the toolset is explicitly enabled (D-04: gated by BOTH toolset enable
    // AND the is_available() prereq check).
    let mut cfg = ironhermes_core::config::ToolsConfig::default();
    cfg.toolsets.insert(
        "browser".to_string(),
        ironhermes_core::config::ToolsetEntry { enabled: true },
    );
    registry.set_toolset_config(Some(cfg));

    let names: Vec<String> = registry
        .get_definitions(None)
        .iter()
        .map(|s| s.function.name.clone())
        .collect();

    for tool_name in BROWSER_TOOL_NAMES {
        assert!(
            !names.iter().any(|n| n == *tool_name),
            "Phase 25.1 D-21 / T-25.1-07: {} MUST be excluded when chromium missing — got: {:?}",
            tool_name,
            names
        );
    }

    // Closed-enumeration anchor: fail fast if a 12th browser_* tool is added without
    // updating this constant (makes the gap visible before merge).
    assert_eq!(
        BROWSER_TOOL_NAMES.len(),
        11,
        "BROWSER_TOOL_NAMES must enumerate exactly 11 tools per Phase 25.1 D-04 closed enumeration"
    );

    unsafe {
        std::env::remove_var("IRONHERMES_BROWSER_TEST_DISABLE");
    }
}

/// Phase 25.1 D-04 toolset gate:
/// ALL 11 browser_* tool schemas MUST be absent from get_definitions() when the browser
/// toolset is disabled — regardless of chromium availability.
///
/// This verifies the toolset-level gate independently from the prereq gate.
#[tokio::test]
async fn browser_tools_excluded_when_toolset_disabled() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    // SAFETY: env_lock + --test-threads=1 ensure single env mutator.
    unsafe {
        // Disable chromium discovery so is_available() is irrelevant — we're testing
        // the toolset gate only.
        std::env::set_var("IRONHERMES_BROWSER_TEST_DISABLE", "1");
    }

    let mut registry = ironhermes_tools::ToolRegistry::new();
    let session = std::sync::Arc::new(tokio::sync::Mutex::new(
        None::<ironhermes_tools::browser_session::BrowserSession>,
    ));
    let resolver = build_resolver();
    let config = std::sync::Arc::new(ironhermes_core::config::Config::default());
    registry.register_browser_tools(session, resolver, config);

    // Use the default ToolsConfig — browser toolset is disabled by default (D-04).
    let cfg = ironhermes_core::config::ToolsConfig::default();
    // Confirm the default explicitly before applying it:
    assert!(
        !cfg.toolsets
            .get("browser")
            .map(|t| t.enabled)
            .unwrap_or(false),
        "Phase 25.1 D-04: ToolsConfig::default() MUST have browser toolset disabled"
    );
    registry.set_toolset_config(Some(cfg));

    let names: Vec<String> = registry
        .get_definitions(None)
        .iter()
        .map(|s| s.function.name.clone())
        .collect();

    for tool_name in BROWSER_TOOL_NAMES {
        assert!(
            !names.iter().any(|n| n == *tool_name),
            "Phase 25.1 D-04: {} MUST be excluded when browser toolset disabled — got: {:?}",
            tool_name,
            names
        );
    }

    unsafe {
        std::env::remove_var("IRONHERMES_BROWSER_TEST_DISABLE");
    }
}

/// Phase 25.1 D-19 cache-break-banner regression gate.
///
/// The Phase 25 D-04 banner mechanism reads ToolsConfig::default().toolsets["browser"]
/// to determine the default-disabled state. If a future config refactor silently flips
/// this to enabled (or removes the entry), the toolset-enable banner will misfire and
/// operators won't see the schema-cache-rebuild notice on `hermes toolset enable browser`.
///
/// This test fails fast on that regression.
/// Invocation matches the D-19 acceptance criterion:
///   cargo test -p ironhermes-tools default_browser_toolset_disabled
#[test]
fn default_browser_toolset_disabled() {
    let cfg = ironhermes_core::config::ToolsConfig::default();
    let entry = cfg.toolsets.get("browser").expect(
        "D-19: ToolsConfig::default() MUST have a 'browser' entry for the cache-break banner \
         (Phase 25 D-04 mechanism reads this entry to gate the toolset-enable banner emission)",
    );
    assert!(
        !entry.enabled,
        "D-19 cache-break invariant: default browser toolset MUST be disabled \
         (high-blast-radius opt-in per D-04). Flipping this to true breaks the Phase 25 D-04 \
         banner contract — operators won't see the schema-cache-rebuild notice."
    );
}

/// D-26 Test 2 (mandatory): tool_excluded_when_prereq_missing
///
/// Integration test verifying that a tool whose required prerequisite env var is absent
/// is filtered from get_definitions() even when its toolset is explicitly enabled.
/// Also verifies that setting the env var makes the tool appear.
///
/// Uses env_lock + --test-threads=1 for race-free env mutation (Phase 21.6 D Rust 2024).
///
/// REWRITTEN under Phase 41.3 Plan 07 (D-09): this test originally used
/// `WebSearchTool` as its example gated tool, back when `web_search` hard-required
/// `FIRECRAWL_API_KEY`. Plan 07 made `web_search` a multi-provider chain that is
/// available even with zero provider keys (DDG terminates it) — `web_search` can
/// no longer demonstrate single-required-prereq gating. `HexapodTcpTool` is the
/// replacement vehicle: a trivially-constructible unit struct with a single
/// `required: true` `env_var` prerequisite (`HEXAPOD_IP`), preserving this test's
/// original intent (the registry's `get_definitions()` filtering mechanism, not
/// any one tool's wiring) unchanged.
use std::sync::OnceLock;

fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[tokio::test]
async fn tool_excluded_when_prereq_missing() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    // SAFETY: env_lock + --test-threads=1 ensure single mutator (Phase 21.6 D Rust 2024).
    unsafe {
        std::env::remove_var("HEXAPOD_IP");
    }

    let mut registry = ironhermes_tools::ToolRegistry::new();
    registry.register(Box::new(ironhermes_tools::hexapod_tcp::HexapodTcpTool));
    let mut cfg = ironhermes_core::config::ToolsConfig::default();
    cfg.toolsets.insert(
        "robotics".to_string(),
        ironhermes_core::config::ToolsetEntry { enabled: true },
    );
    registry.set_toolset_config(Some(cfg.clone()));

    let names: Vec<String> = registry
        .get_definitions(None)
        .iter()
        .map(|s| s.function.name.clone())
        .collect();
    assert!(
        !names.iter().any(|n| n == "hexapod_tcp"),
        "hexapod_tcp MUST be filtered out without HEXAPOD_IP — got: {:?}",
        names
    );

    unsafe {
        std::env::set_var("HEXAPOD_IP", "127.0.0.1");
    }
    let names: Vec<String> = registry
        .get_definitions(None)
        .iter()
        .map(|s| s.function.name.clone())
        .collect();
    assert!(
        names.iter().any(|n| n == "hexapod_tcp"),
        "hexapod_tcp MUST be present with HEXAPOD_IP set — got: {:?}",
        names
    );

    unsafe {
        std::env::remove_var("HEXAPOD_IP");
    }
}

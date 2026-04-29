/// D-26 Test 2 (mandatory): tool_excluded_when_prereq_missing
///
/// Integration test verifying that a tool whose required prerequisite env var is absent
/// is filtered from get_definitions() even when its toolset is explicitly enabled.
/// Also verifies that setting the env var makes the tool appear.
///
/// Uses env_lock + --test-threads=1 for race-free env mutation (Phase 21.6 D Rust 2024).
use std::sync::OnceLock;

fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[tokio::test]
async fn tool_excluded_when_prereq_missing() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    // SAFETY: env_lock + --test-threads=1 ensure single mutator (Phase 21.6 D Rust 2024).
    unsafe { std::env::remove_var("FIRECRAWL_API_KEY"); }

    let mut registry = ironhermes_tools::ToolRegistry::new();
    registry.register(Box::new(ironhermes_tools::web_search::WebSearchTool));
    let mut cfg = ironhermes_core::config::ToolsConfig::default();
    cfg.toolsets.insert("web".to_string(),
        ironhermes_core::config::ToolsetEntry { enabled: true });
    registry.set_toolset_config(Some(cfg.clone()));

    let names: Vec<String> = registry.get_definitions(None)
        .iter().map(|s| s.function.name.clone()).collect();
    assert!(!names.iter().any(|n| n == "web_search"),
        "web_search MUST be filtered out without FIRECRAWL_API_KEY — got: {:?}", names);

    unsafe { std::env::set_var("FIRECRAWL_API_KEY", "test_value"); }
    let names: Vec<String> = registry.get_definitions(None)
        .iter().map(|s| s.function.name.clone()).collect();
    assert!(names.iter().any(|n| n == "web_search"),
        "web_search MUST be present with FIRECRAWL_API_KEY set — got: {:?}", names);

    unsafe { std::env::remove_var("FIRECRAWL_API_KEY"); }
}

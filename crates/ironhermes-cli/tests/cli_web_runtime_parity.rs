use std::fs;
use std::path::PathBuf;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_main_rs() -> String {
    fs::read_to_string(crate_root().join("src/main.rs")).expect("failed to read main.rs")
}

fn read_rata_event_loop() -> String {
    fs::read_to_string(crate_root().join("src/tui_rata/event_loop.rs"))
        .expect("failed to read tui_rata/event_loop.rs")
}

#[test]
fn cli_entry_points_use_shared_runtime_factory() {
    let source = read_main_rs();
    let count = source.matches("build_app_runtime_bundle(").count();
    assert!(
        count >= 3,
        "run_single/run_chat/run_gateway must call build_app_runtime_bundle (>=3), got {count}"
    );
    assert!(
        source.contains("AppRuntimeFactoryInput"),
        "main.rs must construct AppRuntimeFactoryInput"
    );
}

#[test]
fn ratatui_build_app_deps_uses_shared_runtime_factory() {
    let source = read_rata_event_loop();
    assert!(
        source.contains("build_app_runtime_bundle("),
        "tui_rata build_app_deps must call build_app_runtime_bundle"
    );
    assert!(
        source.contains("DelegateTaskWiring"),
        "tui_rata build_app_deps must pass DelegateTaskWiring"
    );
}

#[test]
fn hook_registry_and_context_engine_contracts_remain_wired() {
    let source = read_main_rs();
    assert!(
        source.contains(".with_hook_registry("),
        "AgentLoop builders must still call .with_hook_registry(...)"
    );
    assert!(
        source.contains("Some(hook_registry"),
        "attach_context_engine must still receive Some(hook_registry...)"
    );
}

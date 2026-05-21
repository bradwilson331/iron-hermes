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

/// Phase 28.1-04: run_single, run_chat, and run_gateway all use
/// AgentRuntime::from_config (which calls build_app_runtime_bundle and
/// DelegateTaskWiring internally). Direct callers no longer see those symbols.
#[test]
fn cli_entry_points_use_shared_runtime_factory() {
    let source = read_main_rs();
    // Phase 28.1-04: all 3 CLI entry points now use AgentRuntime::from_config.
    // build_app_runtime_bundle is called internally by from_config, not directly.
    let count = source.matches("AgentRuntime::from_config").count();
    assert!(
        count >= 3,
        "run_single/run_chat/run_gateway must all use AgentRuntime::from_config (>=3), got {count}"
    );
    assert!(
        source.contains("AgentRuntimeInput"),
        "main.rs must construct AgentRuntimeInput for AgentRuntime::from_config"
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

/// Phase 28.1-04: hook_registry and attach_context_engine are now owned by
/// AgentRuntime::run_turn internally. run_chat's run_agent_turn and run_single
/// both delegate through run_turn. Assert the delegation pattern is present.
#[test]
fn hook_registry_and_context_engine_contracts_remain_wired() {
    let source = read_main_rs();
    // run_turn (inside AgentRuntime) calls .with_hook_registry and
    // attach_context_engine. Assert run_turn is called by CLI entry points.
    let run_turn_count = source.matches(".run_turn(").count();
    assert!(
        run_turn_count >= 2,
        "CLI entry points must delegate to AgentRuntime::run_turn (>=2 call sites), \
         got {run_turn_count}; run_turn owns hook_registry + attach_context_engine wiring"
    );
    // hook_registry is still wired inside AgentRuntime (agent_runtime.rs uses it).
    // Verify it is referenced in main.rs (sourced from runtime.hook_registry()).
    assert!(
        source.contains("hook_registry"),
        "main.rs must reference hook_registry (sourced from runtime.hook_registry())"
    );
}

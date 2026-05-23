//! Static-grep regression tests for CLI runtime parity wiring.
//!
//! These tests read main.rs source text and assert that both `run_chat`
//! and `run_single` contain the required wiring calls. They guard against
//! future refactors that might accidentally remove a tool registration or
//! hook wiring call.

use std::fs;
use std::path::PathBuf;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_main_rs() -> String {
    fs::read_to_string(crate_root().join("src/main.rs")).expect("Failed to read main.rs")
}

/// Extract the body of a top-level `async fn NAME` block from main.rs.
/// Uses brace-balanced extraction (matching run_chat_invariants.rs style).
fn extract_function_body(source: &str, fn_name: &str) -> String {
    let needle = format!("async fn {}", fn_name);
    let start = source
        .find(&needle)
        .unwrap_or_else(|| panic!("function `async fn {}` not found in main.rs", fn_name));
    let bytes = source.as_bytes();
    let mut i = start;
    while i < bytes.len() && bytes[i] != b'{' {
        i += 1;
    }
    if i >= bytes.len() {
        panic!("opening brace for {} not found", fn_name);
    }
    let body_start = i;
    let mut depth = 0i32;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return source[body_start..=i].to_string();
                }
            }
            _ => {}
        }
        i += 1;
    }
    panic!("closing brace for {} not found", fn_name);
}

/// Phase 28.1-04: run_chat and run_single now use AgentRuntime::from_config
/// (matching run_gateway since Phase 28.1-02). All 3 entry points delegate through
/// AgentRuntime which internally calls build_app_runtime_bundle + constructs the
/// DelegateTaskWiring. Direct callers no longer see those symbols.
#[test]
fn run_chat_and_run_single_use_shared_runtime_factory() {
    let source = read_main_rs();

    for fn_name in &["run_chat", "run_single"] {
        let body = extract_function_body(&source, fn_name);

        // Phase 28.1-04: build_app_runtime_bundle and DelegateTaskWiring are now
        // called inside AgentRuntime::from_config, not by the entry points directly.
        // Assert that each entry point uses AgentRuntime::from_config instead.
        assert!(
            body.contains("AgentRuntime::from_config"),
            "{fn_name} must build the shared runtime via AgentRuntime::from_config \
             (Phase 28.1-04: replaces direct build_app_runtime_bundle + DelegateTaskWiring)"
        );
        // AgentRuntimeInput is the struct passed to from_config.
        assert!(
            body.contains("AgentRuntimeInput"),
            "{fn_name} must construct AgentRuntimeInput for AgentRuntime::from_config"
        );
    }
}

/// Phase 28.1-04: run_agent_turn now builds a TurnRequest and calls
/// runtime.run_turn — it no longer constructs an AgentLoop directly.
/// hook_registry and attach_context_engine are owned by run_turn internally.
#[test]
fn run_agent_turn_wires_hook_registry() {
    let source = read_main_rs();
    let body = extract_function_body(&source, "run_agent_turn");

    // Phase 28.1-04: hook_registry wiring moved inside AgentRuntime::run_turn.
    // Assert run_agent_turn delegates through run_turn.
    assert!(
        body.contains("run_turn("),
        "run_agent_turn must call runtime.run_turn() (Phase 28.1-04: hook_registry \
         is wired inside run_turn, not by the caller)"
    );
}

/// Phase 28.1-04: attach_context_engine is now called inside AgentRuntime::run_turn.
/// run_agent_turn and run_single both delegate to run_turn which owns this call.
#[test]
fn attach_context_engine_receives_hook_registry_not_none() {
    let source = read_main_rs();

    // run_agent_turn delegates to runtime.run_turn which calls attach_context_engine
    // internally with the runtime's hook_registry.
    let agent_turn_body = extract_function_body(&source, "run_agent_turn");
    assert!(
        agent_turn_body.contains("run_turn("),
        "run_agent_turn must delegate to runtime.run_turn (which calls attach_context_engine \
         with hook_registry internally — Phase 28.1-04)"
    );

    // run_single also delegates to runtime.run_turn.
    let single_body = extract_function_body(&source, "run_single");
    assert!(
        single_body.contains("run_turn("),
        "run_single must delegate to runtime.run_turn (which calls attach_context_engine \
         with hook_registry internally — Phase 28.1-04)"
    );
}

#[test]
fn run_gateway_uses_shared_runtime_active_skills() {
    // Phase 28.1-02: run_gateway now constructs the shared AgentRuntime via
    // AgentRuntime::from_config (which builds the AppRuntimeBundle internally and
    // applies active_skills), then reads active_skills back off the runtime and
    // forwards them into the GatewayRunner.
    let source = read_main_rs();
    let body = extract_function_body(&source, "run_gateway");
    assert!(
        body.contains("AgentRuntime::from_config"),
        "run_gateway must build the shared runtime via AgentRuntime::from_config"
    );
    assert!(
        body.contains("runtime.active_skills()"),
        "run_gateway must read active_skills from the shared runtime"
    );
    assert!(
        body.contains("runner.set_active_skills(active_skills)"),
        "run_gateway must forward active_skills into GatewayRunner"
    );
}

//! Phase 22 -- static-grep regression tests for CLI tool + hook parity.
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
    fs::read_to_string(crate_root().join("src/main.rs"))
        .expect("Failed to read main.rs")
}

/// Extract the body of a top-level `async fn NAME` block from main.rs.
/// Uses brace-balanced extraction (matching run_chat_invariants.rs style).
fn extract_function_body(source: &str, fn_name: &str) -> String {
    let needle = format!("async fn {}", fn_name);
    let start = source.find(&needle).unwrap_or_else(|| {
        panic!("function `async fn {}` not found in main.rs", fn_name)
    });
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

#[test]
fn run_chat_and_run_single_wire_all_tools() {
    let source = read_main_rs();

    for fn_name in &["run_chat", "run_single"] {
        let body = extract_function_body(&source, fn_name);

        // Tool registrations (per D-02)
        assert!(
            body.contains("register_cronjob_tool"),
            "{fn_name} must call register_cronjob_tool"
        );
        assert!(
            body.contains("register_skills_tool"),
            "{fn_name} must call register_skills_tool"
        );
        // Plan 21.7-06: the legacy `_with_active_skills` variant was replaced
        // by `_with_process_registry` at all three CLI + gateway call sites
        // to bundle the ProcessRegistry wiring alongside active-skills env
        // bypass (D-29). Accept either spelling — INV-21.7-03 in
        // invariants_21_7.rs locks the precise count.
        assert!(
            body.contains("register_execute_code_tool_with_active_skills")
                || body.contains("register_execute_code_tool_with_process_registry"),
            "{fn_name} must call register_execute_code_tool_with_{{active_skills|process_registry}}"
        );

        // Guardrails (per D-02)
        assert!(
            body.contains("add_guardrail"),
            "{fn_name} must call add_guardrail"
        );
        assert!(
            body.contains("set_error_detail"),
            "{fn_name} must call set_error_detail"
        );

        // HookRegistry (per D-05)
        assert!(
            body.contains("HookRegistry::new"),
            "{fn_name} must construct HookRegistry"
        );

        // JSONL listener (per D-06)
        assert!(
            body.contains("create_jsonl_listener"),
            "{fn_name} must register JSONL listener"
        );

        // Webhook listener (per D-07)
        assert!(
            body.contains("create_webhook_listener"),
            "{fn_name} must register webhook listener"
        );

        // Retry queue drain
        assert!(
            body.contains("drain_retry_queue"),
            "{fn_name} must drain retry queue"
        );
    }
}

#[test]
fn run_agent_turn_wires_hook_registry() {
    let source = read_main_rs();
    let body = extract_function_body(&source, "run_agent_turn");

    assert!(
        body.contains(".with_hook_registry("),
        "run_agent_turn must call .with_hook_registry() on AgentLoop"
    );
}

#[test]
fn attach_context_engine_receives_hook_registry_not_none() {
    let source = read_main_rs();

    // Check run_agent_turn's attach_context_engine call
    let agent_turn_body = extract_function_body(&source, "run_agent_turn");
    assert!(
        agent_turn_body.contains("Some(hook_registry"),
        "run_agent_turn must pass Some(hook_registry...) to attach_context_engine, not None"
    );

    // Check run_single's attach_context_engine call
    let single_body = extract_function_body(&source, "run_single");
    assert!(
        single_body.contains("Some(hook_registry"),
        "run_single must pass Some(hook_registry...) to attach_context_engine, not None"
    );
}

#[test]
fn active_skills_shared_between_skills_and_execute_code() {
    let source = read_main_rs();

    for fn_name in &["run_chat", "run_single"] {
        let body = extract_function_body(&source, fn_name);

        // active_skills must be created
        assert!(
            body.contains("let active_skills"),
            "{fn_name} must create active_skills Arc"
        );

        // active_skills must be cloned to both register calls
        let active_clones = body.matches("active_skills.clone()").count();
        assert!(
            active_clones >= 2,
            "{fn_name} must clone active_skills at least twice (skills_tool + execute_code), found {active_clones}"
        );
    }
}

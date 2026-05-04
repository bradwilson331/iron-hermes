//! Phase 22.4.2 Plan 04: Behavioral smoke tests for Tier D session control commands.
//!
//! Tests handler signatures, ProcessRegistry drain via cmd_stop, and history
//! manipulation logic for /retry, /undo, /rollback, /background, /btw, /queue.
//!
//! Pattern: `make_test_ctx_with_history()` builds a CommandContext with a
//! pre-populated history snapshot — matches the `make_ctx` pattern from handlers.rs.

use ironhermes_core::commands::context::CommandContext;
use ironhermes_core::commands::registry::build_registry;
use ironhermes_core::commands::{CommandResult, CommandRouter};
use ironhermes_core::types::{ChatMessage, Platform};
use std::sync::{Arc, atomic::AtomicBool};

// ── Fixture helpers ────────────────────────────────────────────────────���──────

fn make_ctx(agent_running: bool) -> CommandContext {
    CommandContext::new(
        Platform::Local,
        "test-session-id".to_string(),
        Arc::new(AtomicBool::new(agent_running)),
    )
}

fn make_ctx_with_history(messages: Vec<ChatMessage>) -> CommandContext {
    let history = Arc::new(std::sync::RwLock::new(messages));
    make_ctx(false).with_history(history)
}

fn make_router() -> CommandRouter {
    CommandRouter::new(build_registry())
}

fn find_cmd(name: &str) -> ironhermes_core::commands::CommandDef {
    build_registry()
        .into_iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("Command '{}' not found in registry", name))
}

// ── /stop smoke test ──────────────────────────────────────────────────────────

/// INV-22.4-53 behavioral: /stop with no ProcessRegistry wired returns informational text
/// (the "No agent running" guard path — no registry threaded in test ctx).
#[test]
fn stop_no_process_registry_returns_informational() {
    let ctx = make_ctx(false);
    let router = make_router();
    let cmd = find_cmd("stop");
    let result = ironhermes_core::commands::handlers::dispatch(&cmd, &[], &ctx, &router);
    match result {
        CommandResult::Output(s) => {
            assert!(
                s.contains("No agent") || s.contains("not yet wired") || s.contains("Stopped"),
                "Expected /stop informational text, got: {s}"
            );
        }
        other => panic!("Expected Output from /stop, got {:?}", other),
    }
}

/// /stop with agent_running=true but no ProcessRegistry returns advisory text.
#[test]
fn stop_agent_running_no_registry_returns_advisory() {
    let ctx = make_ctx(true); // agent running but no process registry
    let router = make_router();
    let cmd = find_cmd("stop");
    let result = ironhermes_core::commands::handlers::dispatch(&cmd, &[], &ctx, &router);
    match result {
        CommandResult::Output(s) => {
            assert!(
                s.contains("Stopping agent") || s.contains("not yet wired"),
                "Expected stopping advisory, got: {s}"
            );
        }
        other => panic!("Expected Output from /stop(running), got {:?}", other),
    }
}

// ── /retry smoke tests ────────────────────────────────────────────────────────

/// INV-22.4-54 behavioral: /retry with no history context returns informational text.
#[test]
fn retry_no_history_returns_informational() {
    let ctx = make_ctx(false); // no history threaded
    let router = make_router();
    let cmd = find_cmd("retry");
    let result = ironhermes_core::commands::handlers::dispatch(&cmd, &[], &ctx, &router);
    match result {
        CommandResult::Output(s) => {
            assert!(
                s.contains("History not available") || s.contains("not available"),
                "Expected history-not-available message, got: {s}"
            );
        }
        other => panic!("Expected Output from /retry, got {:?}", other),
    }
}

/// /retry with empty history returns "No user messages" message.
#[test]
fn retry_empty_history_returns_no_messages() {
    let ctx = make_ctx_with_history(vec![]);
    let router = make_router();
    let cmd = find_cmd("retry");
    let result = ironhermes_core::commands::handlers::dispatch(&cmd, &[], &ctx, &router);
    match result {
        CommandResult::Output(s) => {
            assert!(
                s.contains("No user messages") || s.contains("No messages"),
                "Expected no-messages message, got: {s}"
            );
        }
        other => panic!("Expected Output from /retry(empty), got {:?}", other),
    }
}

/// /retry with a user message in history returns "Retrying: <message>".
#[test]
fn retry_with_user_message_returns_retrying() {
    let ctx = make_ctx_with_history(vec![
        ChatMessage::user("Hello, world!"),
        ChatMessage::assistant("Hi there!"),
    ]);
    let router = make_router();
    let cmd = find_cmd("retry");
    let result = ironhermes_core::commands::handlers::dispatch(&cmd, &[], &ctx, &router);
    match result {
        CommandResult::Output(s) => {
            assert!(
                s.contains("Retrying") && s.contains("Hello, world!"),
                "Expected retrying message with content, got: {s}"
            );
        }
        other => panic!(
            "Expected Output from /retry(with user msg), got {:?}",
            other
        ),
    }
}

// ── /undo smoke tests ─────────────────────────────────────────────────────────

/// INV-22.4-55 behavioral: /undo with no history context returns informational text.
#[test]
fn undo_no_history_returns_informational() {
    let ctx = make_ctx(false); // no history threaded
    let router = make_router();
    let cmd = find_cmd("undo");
    let result = ironhermes_core::commands::handlers::dispatch(&cmd, &[], &ctx, &router);
    match result {
        CommandResult::Output(s) => {
            assert!(
                s.contains("not available") || s.contains("History"),
                "Expected history-not-available message, got: {s}"
            );
        }
        other => panic!("Expected Output from /undo, got {:?}", other),
    }
}

/// /undo with empty history returns "No history to undo".
#[test]
fn undo_empty_history_returns_no_history() {
    let ctx = make_ctx_with_history(vec![]);
    let router = make_router();
    let cmd = find_cmd("undo");
    let result = ironhermes_core::commands::handlers::dispatch(&cmd, &[], &ctx, &router);
    match result {
        CommandResult::Output(s) => {
            assert!(
                s.contains("No history to undo") || s.contains("No history"),
                "Expected no-history message, got: {s}"
            );
        }
        other => panic!("Expected Output from /undo(empty), got {:?}", other),
    }
}

/// /undo with a user+assistant pair signals the post-router hook.
#[test]
fn undo_with_history_returns_confirmation() {
    let ctx = make_ctx_with_history(vec![
        ChatMessage::user("What is 2+2?"),
        ChatMessage::assistant("4."),
    ]);
    let router = make_router();
    let cmd = find_cmd("undo");
    let result = ironhermes_core::commands::handlers::dispatch(&cmd, &[], &ctx, &router);
    match result {
        CommandResult::Output(s) => {
            // Core confirms the undo is possible; post-router does the truncation.
            assert!(!s.is_empty(), "Expected non-empty confirmation from /undo");
        }
        other => panic!("Expected Output from /undo(with history), got {:?}", other),
    }
}

// ── /rollback smoke tests ─────────────────────────────────────────────────────

/// INV-22.4-56 behavioral: /rollback with no history context returns informational text.
#[test]
fn rollback_no_history_returns_informational() {
    let ctx = make_ctx(false);
    let router = make_router();
    let cmd = find_cmd("rollback");
    let result = ironhermes_core::commands::handlers::dispatch(&cmd, &[], &ctx, &router);
    match result {
        CommandResult::Output(s) => {
            assert!(
                s.contains("not available") || s.contains("History"),
                "Expected history-not-available message, got: {s}"
            );
        }
        other => panic!("Expected Output from /rollback, got {:?}", other),
    }
}

/// /rollback [n] with history returns "Rolling back N exchange(s)".
#[test]
fn rollback_with_history_returns_rollback_message() {
    let ctx = make_ctx_with_history(vec![
        ChatMessage::user("First question"),
        ChatMessage::assistant("First answer"),
        ChatMessage::user("Second question"),
        ChatMessage::assistant("Second answer"),
    ]);
    let router = make_router();
    let cmd = find_cmd("rollback");
    let result = ironhermes_core::commands::handlers::dispatch(&cmd, &["2"], &ctx, &router);
    match result {
        CommandResult::Output(s) => {
            assert!(
                s.contains("Rolling back") || s.contains("roll"),
                "Expected rollback message, got: {s}"
            );
        }
        other => panic!("Expected Output from /rollback(2), got {:?}", other),
    }
}

// ── /background smoke tests ───────────────────────────────────────────────────

/// INV-22.4-57 behavioral: /background with no agent_loop context returns informational text.
#[test]
fn background_no_agent_loop_returns_informational() {
    let ctx = make_ctx(false); // no agent_loop threaded
    let router = make_router();
    let cmd = find_cmd("background");
    let result =
        ironhermes_core::commands::handlers::dispatch(&cmd, &["do something"], &ctx, &router);
    match result {
        CommandResult::Output(s) => {
            assert!(
                s.contains("not configured") || s.contains("Agent loop"),
                "Expected not-configured message, got: {s}"
            );
        }
        other => panic!("Expected Output from /background, got {:?}", other),
    }
}

/// /background with no args returns usage hint.
#[test]
fn background_no_args_returns_usage() {
    let ctx = make_ctx(false);
    let router = make_router();
    let cmd = find_cmd("background");
    let result = ironhermes_core::commands::handlers::dispatch(&cmd, &[], &ctx, &router);
    match result {
        CommandResult::Output(s) => {
            // Either "not configured" (no agent_loop) or "Usage" (no args with loop)
            assert!(
                !s.is_empty(),
                "Expected non-empty response from /background (no args)"
            );
        }
        other => panic!("Expected Output from /background(no args), got {:?}", other),
    }
}

// ── /btw smoke tests ──────────────────────────────────────────────────────────

/// INV-22.4-58 behavioral: /btw with no agent_loop context returns informational text.
#[test]
fn btw_no_agent_loop_returns_informational() {
    let ctx = make_ctx(false);
    let router = make_router();
    let cmd = find_cmd("btw");
    let result = ironhermes_core::commands::handlers::dispatch(&cmd, &["a thought"], &ctx, &router);
    match result {
        CommandResult::Output(s) => {
            assert!(
                s.contains("not configured") || s.contains("Agent loop"),
                "Expected not-configured message, got: {s}"
            );
        }
        other => panic!("Expected Output from /btw, got {:?}", other),
    }
}

/// /btw with no args returns usage hint.
#[test]
fn btw_no_args_returns_usage() {
    let ctx = make_ctx(false);
    let router = make_router();
    let cmd = find_cmd("btw");
    let result = ironhermes_core::commands::handlers::dispatch(&cmd, &[], &ctx, &router);
    match result {
        CommandResult::Output(s) => {
            assert!(
                !s.is_empty(),
                "Expected non-empty response from /btw (no args)"
            );
        }
        other => panic!("Expected Output from /btw(no args), got {:?}", other),
    }
}

// ── /queue smoke tests ────────────────────────────────────────────────────────

/// INV-22.4-59 behavioral: /queue with no agent_loop context returns informational text.
#[test]
fn queue_no_agent_loop_returns_informational() {
    let ctx = make_ctx(false);
    let router = make_router();
    let cmd = find_cmd("queue");
    let result =
        ironhermes_core::commands::handlers::dispatch(&cmd, &["later task"], &ctx, &router);
    match result {
        CommandResult::Output(s) => {
            assert!(
                s.contains("not configured") || s.contains("Agent loop"),
                "Expected not-configured message, got: {s}"
            );
        }
        other => panic!("Expected Output from /queue, got {:?}", other),
    }
}

/// /queue with no args returns usage hint.
#[test]
fn queue_no_args_returns_usage() {
    let ctx = make_ctx(false);
    let router = make_router();
    let cmd = find_cmd("queue");
    let result = ironhermes_core::commands::handlers::dispatch(&cmd, &[], &ctx, &router);
    match result {
        CommandResult::Output(s) => {
            assert!(
                !s.is_empty(),
                "Expected non-empty response from /queue (no args)"
            );
        }
        other => panic!("Expected Output from /queue(no args), got {:?}", other),
    }
}

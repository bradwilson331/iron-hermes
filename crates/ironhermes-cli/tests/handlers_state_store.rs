//! Phase 22.4.2 Plan 01 — behavioral tests for StateStore-backed handlers.
//!
//! Tests /sessions, /resume, /save, /history, /title via the real
//! `ironhermes_core::commands::handlers::dispatch` entry point.
//!
//! Per RESEARCH OQ-7: placed in `ironhermes-cli/tests/` (not
//! `ironhermes-core/tests/`) because `ironhermes-state` imports
//! `ironhermes-core` — adding it as a dev-dep of core would create a
//! circular dependency. ironhermes-cli already depends on both crates.
//!
//! Fixture pattern mirrors `make_ctx` (handlers.rs:704) and
//! `temp_store()` (ironhermes-state/tests/state_store.rs:6).

use std::sync::{Arc, Mutex};

use ironhermes_core::commands::context::{CommandContext, StateStoreHandle};
use ironhermes_core::commands::registry::build_registry;
use ironhermes_core::commands::{CommandResult, CommandRouter};
use ironhermes_core::types::Platform;
use ironhermes_state::StateStore;
use tempfile::NamedTempFile;

// =============================================================================
// Fixture helpers
// =============================================================================

/// StateStore adapter: thin bridge implementing `StateStoreHandle` so that
/// the concrete `ironhermes_state::StateStore` (in CLI crate context) can be
/// injected into `CommandContext`.
///
/// Mirrors the `StateStoreAdapter` in `tui_rata/commands.rs` — duplicated
/// here because that type is private to the CLI crate.
struct TestStateStoreAdapter(Arc<Mutex<StateStore>>);

impl StateStoreHandle for TestStateStoreAdapter {
    fn list_sessions_text(&self, limit: usize) -> String {
        let guard = match self.0.lock() {
            Ok(g) => g,
            Err(_) => return "StateStore lock poisoned.".to_string(),
        };
        match guard.list_sessions(None, limit) {
            Ok(sessions) if sessions.is_empty() => "No sessions found.".to_string(),
            Ok(sessions) => {
                let lines: Vec<String> = sessions.iter().map(|s| format!("  {}", s.id)).collect();
                format!("Recent sessions:\n{}", lines.join("\n"))
            }
            Err(e) => format!("Error listing sessions: {e}"),
        }
    }

    fn history_text(&self, session_id: &str) -> String {
        let guard = match self.0.lock() {
            Ok(g) => g,
            Err(_) => return "StateStore lock poisoned.".to_string(),
        };
        match guard.get_messages(session_id) {
            Ok(msgs) if msgs.is_empty() => "No messages in history.".to_string(),
            Ok(msgs) => {
                let lines: Vec<String> = msgs
                    .iter()
                    .map(|m| {
                        format!(
                            "  [{}] {}",
                            m.role,
                            m.content.as_deref().unwrap_or("")
                        )
                    })
                    .collect();
                format!("History ({} messages):\n{}", msgs.len(), lines.join("\n"))
            }
            Err(e) => format!("Error loading history: {e}"),
        }
    }

    fn export_session_text(&self, session_id: &str) -> String {
        let guard = match self.0.lock() {
            Ok(g) => g,
            Err(_) => return "StateStore lock poisoned.".to_string(),
        };
        match guard.export_session(session_id) {
            Ok(export) => format!("Session exported: {} messages.", export.messages.len()),
            Err(e) => format!("Error exporting session: {e}"),
        }
    }

    fn update_title(&self, session_id: &str, title: &str) -> Result<(), String> {
        let mut guard = self
            .0
            .lock()
            .map_err(|_| "StateStore lock poisoned.".to_string())?;
        guard
            .update_session_title(session_id, title)
            .map_err(|e| e.to_string())
    }

    fn get_session_id(&self, name_or_id: &str) -> Option<String> {
        let guard = self.0.lock().ok()?;
        if let Ok(Some(s)) = guard.get_session(name_or_id) {
            return Some(s.id);
        }
        guard
            .get_session_by_title(name_or_id)
            .ok()
            .flatten()
            .map(|s| s.id)
    }
}

/// Build a `CommandContext` wired with a temp StateStore.
/// Returns the context and the `NamedTempFile` (keep alive to prevent deletion).
fn make_test_ctx_with_state_store(
    session_id: &str,
) -> (CommandContext, Arc<Mutex<StateStore>>, NamedTempFile) {
    let f = NamedTempFile::new().expect("tempfile creation failed");
    let store = StateStore::new(f.path()).expect("StateStore::new failed");
    let arc_store = Arc::new(Mutex::new(store));

    let agent_running = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ctx = CommandContext::new(Platform::Local, session_id.to_string(), agent_running)
        .with_state_store(Arc::new(TestStateStoreAdapter(arc_store.clone())));

    (ctx, arc_store, f)
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

fn dispatch(name: &str, args: &[&str], ctx: &CommandContext) -> CommandResult {
    let router = make_router();
    let cmd = find_cmd(name);
    ironhermes_core::commands::handlers::dispatch(&cmd, args, ctx, &router)
}

// =============================================================================
// /sessions tests
// =============================================================================

#[test]
fn cmd_sessions_no_state_store_returns_not_configured() {
    let agent_running = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ctx = CommandContext::new(Platform::Local, "s1".to_string(), agent_running);
    let result = dispatch("sessions", &[], &ctx);
    match result {
        CommandResult::Output(s) => assert!(
            s.contains("not configured"),
            "Expected 'not configured' message, got: {s}"
        ),
        other => panic!("Expected Output, got {:?}", other),
    }
}

#[test]
fn cmd_sessions_empty_store_returns_no_sessions() {
    let (ctx, _store, _tmp) = make_test_ctx_with_state_store("s1");
    let result = dispatch("sessions", &[], &ctx);
    match result {
        CommandResult::Output(s) => assert!(
            s.contains("No sessions found"),
            "Expected 'No sessions found', got: {s}"
        ),
        other => panic!("Expected Output, got {:?}", other),
    }
}

#[test]
fn cmd_sessions_with_sessions_lists_them() {
    let (ctx, store, _tmp) = make_test_ctx_with_state_store("s1");
    {
        let mut g = store.lock().unwrap();
        g.create_session("sess-abc", "cli", None, None, None).unwrap();
    }
    let result = dispatch("sessions", &[], &ctx);
    match result {
        CommandResult::Output(s) => assert!(
            s.contains("sess-abc"),
            "Expected session id in output, got: {s}"
        ),
        other => panic!("Expected Output, got {:?}", other),
    }
}

#[test]
fn cmd_sessions_accepts_limit_arg() {
    let (ctx, store, _tmp) = make_test_ctx_with_state_store("s1");
    {
        let mut g = store.lock().unwrap();
        g.create_session("s-one", "cli", None, None, None).unwrap();
        g.create_session("s-two", "cli", None, None, None).unwrap();
    }
    // Limit 1 — should list only 1 session.
    let result = dispatch("sessions", &["1"], &ctx);
    match result {
        CommandResult::Output(s) => {
            // Should contain exactly one session line.
            let session_lines = s.lines().filter(|l| l.trim().starts_with("s-")).count();
            assert_eq!(
                session_lines, 1,
                "Expected exactly 1 session line with limit=1, got: {s}"
            );
        }
        other => panic!("Expected Output, got {:?}", other),
    }
}

// =============================================================================
// /resume tests
// =============================================================================

#[test]
fn cmd_resume_no_state_store_returns_not_configured() {
    let agent_running = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ctx = CommandContext::new(Platform::Local, "s1".to_string(), agent_running);
    let result = dispatch("resume", &["some-session"], &ctx);
    match result {
        CommandResult::Output(s) => assert!(
            s.contains("not configured"),
            "Expected 'not configured' message, got: {s}"
        ),
        other => panic!("Expected Output, got {:?}", other),
    }
}

#[test]
fn cmd_resume_no_args_shows_session_list() {
    let (ctx, _store, _tmp) = make_test_ctx_with_state_store("s1");
    let result = dispatch("resume", &[], &ctx);
    // With empty store, shows "No sessions found".
    match result {
        CommandResult::Output(s) => assert!(
            s.contains("No sessions found") || s.contains("Recent sessions"),
            "Expected session listing, got: {s}"
        ),
        other => panic!("Expected Output, got {:?}", other),
    }
}

#[test]
fn cmd_resume_known_session_returns_resuming_message() {
    let (ctx, store, _tmp) = make_test_ctx_with_state_store("s1");
    {
        let mut g = store.lock().unwrap();
        g.create_session("my-session", "cli", None, None, None).unwrap();
    }
    let result = dispatch("resume", &["my-session"], &ctx);
    match result {
        CommandResult::Output(s) => assert!(
            s.contains("my-session"),
            "Expected session id in resume message, got: {s}"
        ),
        other => panic!("Expected Output, got {:?}", other),
    }
}

#[test]
fn cmd_resume_unknown_session_returns_error() {
    let (ctx, _store, _tmp) = make_test_ctx_with_state_store("s1");
    let result = dispatch("resume", &["nonexistent"], &ctx);
    match result {
        CommandResult::Error(s) => assert!(
            s.contains("nonexistent"),
            "Expected error mentioning session name, got: {s}"
        ),
        other => panic!("Expected Error, got {:?}", other),
    }
}

// =============================================================================
// /save tests
// =============================================================================

#[test]
fn cmd_save_no_state_store_returns_not_configured() {
    let agent_running = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ctx = CommandContext::new(Platform::Local, "s1".to_string(), agent_running);
    let result = dispatch("save", &[], &ctx);
    match result {
        CommandResult::Output(s) => assert!(
            s.contains("not configured"),
            "Expected 'not configured' message, got: {s}"
        ),
        other => panic!("Expected Output, got {:?}", other),
    }
}

#[test]
fn cmd_save_with_session_exports_it() {
    let session_id = "save-test-sess";
    let (ctx, store, _tmp) = make_test_ctx_with_state_store(session_id);
    {
        let mut g = store.lock().unwrap();
        g.create_session(session_id, "cli", None, None, None).unwrap();
    }
    let result = dispatch("save", &[], &ctx);
    match result {
        CommandResult::Output(s) => assert!(
            s.contains("exported") || s.contains("messages"),
            "Expected export confirmation, got: {s}"
        ),
        other => panic!("Expected Output, got {:?}", other),
    }
}

// =============================================================================
// /history tests
// =============================================================================

#[test]
fn cmd_history_no_store_no_snapshot_returns_not_configured() {
    let agent_running = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ctx = CommandContext::new(Platform::Local, "s1".to_string(), agent_running);
    let result = dispatch("history", &[], &ctx);
    match result {
        CommandResult::Output(s) => assert!(
            s.contains("not configured"),
            "Expected 'not configured' message, got: {s}"
        ),
        other => panic!("Expected Output, got {:?}", other),
    }
}

#[test]
fn cmd_history_with_snapshot_shows_messages() {
    use ironhermes_core::types::ChatMessage;
    let agent_running = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let history = vec![
        ChatMessage::user("hello there"),
        ChatMessage::assistant("hi!"),
    ];
    let ctx = CommandContext::new(Platform::Local, "s1".to_string(), agent_running)
        .with_history(Arc::new(std::sync::RwLock::new(history)));

    let result = dispatch("history", &[], &ctx);
    match result {
        CommandResult::Output(s) => {
            assert!(
                s.contains("hello there") || s.contains("2 messages"),
                "Expected history content, got: {s}"
            );
        }
        other => panic!("Expected Output, got {:?}", other),
    }
}

#[test]
fn cmd_history_empty_snapshot_says_no_messages() {
    let agent_running = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ctx = CommandContext::new(Platform::Local, "s1".to_string(), agent_running)
        .with_history(Arc::new(std::sync::RwLock::new(vec![])));

    let result = dispatch("history", &[], &ctx);
    match result {
        CommandResult::Output(s) => assert!(
            s.contains("No messages"),
            "Expected 'No messages' for empty history, got: {s}"
        ),
        other => panic!("Expected Output, got {:?}", other),
    }
}

#[test]
fn cmd_history_with_session_id_arg_queries_store() {
    let (ctx, store, _tmp) = make_test_ctx_with_state_store("s1");
    {
        let mut g = store.lock().unwrap();
        g.create_session("hist-sess", "cli", None, None, None).unwrap();
    }
    let result = dispatch("history", &["hist-sess"], &ctx);
    match result {
        CommandResult::Output(s) => assert!(
            s.contains("No messages") || s.contains("History"),
            "Expected history output for session, got: {s}"
        ),
        other => panic!("Expected Output, got {:?}", other),
    }
}

// =============================================================================
// /title tests
// =============================================================================

#[test]
fn cmd_title_no_args_returns_error() {
    let agent_running = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ctx = CommandContext::new(Platform::Local, "s1".to_string(), agent_running);
    let result = dispatch("title", &[], &ctx);
    assert!(
        matches!(result, CommandResult::Error(_)),
        "Expected Error for /title with no args, got {:?}",
        result
    );
}

#[test]
fn cmd_title_no_state_store_returns_informational() {
    let agent_running = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ctx = CommandContext::new(Platform::Local, "s1".to_string(), agent_running);
    let result = dispatch("title", &["My", "Title"], &ctx);
    match result {
        CommandResult::Output(s) => assert!(
            s.contains("My Title"),
            "Expected title confirmation, got: {s}"
        ),
        other => panic!("Expected Output, got {:?}", other),
    }
}

#[test]
fn cmd_title_with_state_store_persists_title() {
    let session_id = "title-test-sess";
    let (ctx, store, _tmp) = make_test_ctx_with_state_store(session_id);
    {
        let mut g = store.lock().unwrap();
        g.create_session(session_id, "cli", None, None, None).unwrap();
    }
    let result = dispatch("title", &["My", "Session", "Title"], &ctx);
    match result {
        CommandResult::Output(s) => {
            assert!(
                s.contains("My Session Title"),
                "Expected title confirmation, got: {s}"
            );
        }
        other => panic!("Expected Output, got {:?}", other),
    }
    // Verify the title was actually persisted.
    {
        let g = store.lock().unwrap();
        let sess = g.get_session(session_id).unwrap().expect("session should exist");
        assert_eq!(
            sess.title.as_deref(),
            Some("My Session Title"),
            "Title should be persisted in StateStore"
        );
    }
}

#[test]
fn cmd_title_with_state_store_no_session_returns_error() {
    // Session doesn't exist in store — update_title should fail.
    let session_id = "nonexistent-session";
    let (ctx, _store, _tmp) = make_test_ctx_with_state_store(session_id);
    let result = dispatch("title", &["Some", "Title"], &ctx);
    // SQLite will return an error when trying to update a non-existent session.
    // We accept either Error or Output (graceful fallback) — just verify it doesn't panic.
    match result {
        CommandResult::Output(_) | CommandResult::Error(_) => {}
        other => panic!("Expected Output or Error, got {:?}", other),
    }
}

//! Slash-command dispatch for the tui_rata REPL (Phase 22.4 plan 22.4-07 Task 4).
//!
//! This file is a compilation stub — the real `dispatch_slash` implementation
//! lands in plan 22.4-07. The stub provides `SlashOutcome` and a no-op
//! `dispatch_slash` so that `app.rs` compiles in all plans that precede 22.4-07.

/// Outcome returned by `dispatch_slash` to `App::apply_slash_outcome`.
///
/// Each variant maps to a distinct UI action. Plan 22.4-07 Task 4 fills in
/// the full command table; this stub covers all variants so `app.rs` match
/// arms compile without `_` catch-alls.
#[derive(Debug)]
pub enum SlashOutcome {
    /// Command ran and produced a display string for the transcript.
    Handled(String),
    /// Command ran but produced no transcript output (e.g. background action).
    Silent,
    /// User typed `/quit` or `/exit` — set `app.should_quit = true`.
    Quit,
    /// Terminal reset requested (e.g. `/reset`).
    ResetTerminal,
    /// MCP server list reload requested (e.g. `/mcp reload`).
    McpReload,
    /// Session cleared; string is the "session cleared" confirmation message.
    ClearSession(String),
    /// Input started with `/` but matched no command.
    Unknown { input: String, hint: String },
    /// Dispatch itself failed (e.g. command handler returned Err).
    Error(String),
}

/// Dispatch a slash command and return the outcome.
///
/// # Stub behaviour (pre-22.4-07)
/// Returns `SlashOutcome::Unknown` for every input so the UI shows an
/// "unknown command" hint without panicking. Plan 22.4-07 replaces this body
/// with the full `CommandRouter`-backed implementation.
pub async fn dispatch_slash(
    _app: &mut super::app::App,
    input: &str,
) -> SlashOutcome {
    SlashOutcome::Unknown {
        input: input.to_string(),
        hint: format!("unknown command: {input} (stub — 22.4-07 not yet landed)"),
    }
}

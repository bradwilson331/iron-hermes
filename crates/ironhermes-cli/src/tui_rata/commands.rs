//! Slash-command dispatch wrapper for tui_rata (Phase 22.4 D-18 item 8).
//!
//! Wraps `ironhermes_core::commands::CommandRouter::resolve` and surfaces
//! typo-suggestion hints via `ironhermes_core::commands::typo::suggest_typo`
//! on the `ResolveResult::NotFound` arm. Ported from classic
//! `tui/commands.rs` dispatch pattern — widget-slot surface is NOT ported
//! (retired per D-09).
//!
//! Integration contract (BLOCKER-NEW-03):
//! - Plan 22.4-05 Task 2 `App::handle_key` Enter arm calls `dispatch_slash`
//!   via `dispatch_or_submit` → `dispatch_slash_blocking` → `dispatch_slash`.
//! - Slash input NEVER enters `app.history` as User role.
//! - `SlashOutcome` variants mapped by `App::apply_slash_outcome` into
//!   System-role transcript entries or `should_quit = true`.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use ironhermes_core::commands::{CommandResult, CommandRouter, ResolveResult};
use ironhermes_core::commands::context::CommandContext;
use ironhermes_core::commands::typo::suggest_typo;
use ironhermes_core::types::Platform;

use crate::tui_rata::app::App;

// ── SlashOutcome ──────────────────────────────────────────────────────────────

/// Outcome returned by `dispatch_slash` to `App::apply_slash_outcome`.
///
/// Each variant maps to a distinct UI action in the tui_rata REPL.
/// Shape is compatible with `app.rs` match arms defined in plan 22.4-05.
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
    /// Input started with `/` but matched no command. `hint` may contain a
    /// "Did you mean `/X`?" suggestion from `suggest_typo`.
    Unknown { input: String, hint: String },
    /// Dispatch itself failed (e.g. command handler returned Err).
    Error(String),
}

// ── dispatch_slash ────────────────────────────────────────────────────────────

/// Dispatch a slash-prefixed input through the `CommandRouter`.
///
/// On `ResolveResult::Exact` or `PrefixMatch`: invokes the minimum-viable
/// handler table (Phase 22.4 scope — full 49-command coverage is a follow-up).
///
/// On `ResolveResult::NotFound`: invokes `suggest_typo` against the full
/// candidate pool (names + aliases) and surfaces "Did you mean `/X`?" per
/// Phase 22.3 D-10 copy contract (D-18 item 8).
pub async fn dispatch_slash(app: &mut App, input: &str) -> SlashOutcome {
    let platform = Platform::Local; // tui_rata runs under CLI/Local platform
    match app.command_router.resolve(input, &platform) {
        ResolveResult::Exact(def) | ResolveResult::PrefixMatch(def) => {
            let ctx = build_command_context(app);
            match invoke_handler(def.name, &ctx).await {
                Ok(result) => map_core_to_slash_outcome(result),
                Err(e) => SlashOutcome::Error(e.to_string()),
            }
        }
        ResolveResult::Ambiguous(candidates) => {
            let hint = format!(
                "Ambiguous command — matches: {}. Type /help for the list.",
                candidates.join(", ")
            );
            SlashOutcome::Unknown { input: input.to_string(), hint }
        }
        ResolveResult::NotFound => {
            // D-18 item 8 — typo suggester integration point.
            let known = collect_known_command_names(&app.command_router);
            let stripped = input
                .trim_start_matches('/')
                .split_whitespace()
                .next()
                .unwrap_or("");
            let known_refs: Vec<&str> = known.iter().map(|s| s.as_str()).collect();
            let hint = match suggest_typo(stripped, &known_refs) {
                Some(candidate) => format!("Did you mean `/{candidate}`?"),
                None => "Type /help for the list of commands.".to_string(),
            };
            SlashOutcome::Unknown { input: input.to_string(), hint }
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build a minimum-viable `CommandContext` from App state.
///
/// `agent_running` is derived from whether a pending turn is active.
fn build_command_context(app: &App) -> CommandContext {
    let agent_running = Arc::new(AtomicBool::new(app.pending_rx.is_some()));
    let mut ctx = CommandContext::new(
        Platform::Local,
        app.session_id.clone(),
        agent_running,
    );
    if let Some(mgr) = &app.mcp_manager {
        ctx = ctx.with_mcp_reloader(mgr.clone());
    }
    ctx
}

/// Collect all command names + aliases from the router for the typo candidate pool.
///
/// `CommandRouter.commands: Vec<CommandDef>` is public (mod.rs:165).
fn collect_known_command_names(router: &CommandRouter) -> Vec<String> {
    let mut names: Vec<String> = router.commands.iter()
        .map(|c| c.name.to_string())
        .collect();
    for cmd in &router.commands {
        for alias in cmd.aliases {
            names.push(alias.to_string());
        }
    }
    names
}

/// Minimum-viable command handler table for Phase 22.4.
///
/// Routes by `def.name` to concrete `CommandResult` variants.
/// The `other` arm returns an informative `Output` (not a panic/abort)
/// for commands not yet wired — documented in SUMMARY §Handler Coverage.
async fn invoke_handler(
    name: &str,
    _ctx: &CommandContext,
) -> Result<CommandResult, anyhow::Error> {
    let result = match name {
        "quit" | "exit" => CommandResult::Quit,
        "clear"         => CommandResult::ClearSession,
        "new"           => CommandResult::NewSession {
            message: "New session started.".to_string(),
        },
        "help"          => CommandResult::Output(render_help()),
        "reset"         => CommandResult::ResetTerminal,
        "reload-mcp"    => CommandResult::McpReload,
        other => CommandResult::Output(format!(
            "(tui_rata: /{other} not yet wired in Phase 22.4 — see plan 22.4-07 §Handler Coverage)"
        )),
    };
    Ok(result)
}

/// Render a brief help listing. Full command table rendered by the `help` handler
/// in follow-up gap-closure.
fn render_help() -> String {
    "Slash commands: /help · /quit · /clear · /new · /reset · /reload-mcp\n\
     Type /help for this list. Phase 22.4 typo suggester will suggest corrections \
     for unrecognised commands (D-10).".to_string()
}

/// Map a `ironhermes_core::commands::CommandResult` to a `SlashOutcome`.
fn map_core_to_slash_outcome(result: CommandResult) -> SlashOutcome {
    match result {
        CommandResult::Output(text)           => SlashOutcome::Handled(text),
        CommandResult::Handled                => SlashOutcome::Silent,
        CommandResult::Error(msg)             => SlashOutcome::Error(msg),
        CommandResult::Quit                   => SlashOutcome::Quit,
        CommandResult::ClearSession           => SlashOutcome::ClearSession("Conversation cleared.".to_string()),
        CommandResult::ResetTerminal          => SlashOutcome::ResetTerminal,
        CommandResult::NewSession { message } => SlashOutcome::ClearSession(message),
        CommandResult::PassThrough            => SlashOutcome::Unknown {
            input: String::new(),
            hint: "Unknown command. Type /help for the list.".to_string(),
        },
        CommandResult::McpReload              => SlashOutcome::McpReload,
    }
}

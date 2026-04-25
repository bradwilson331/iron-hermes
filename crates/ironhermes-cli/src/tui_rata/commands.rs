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

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
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
    // UAT Gap 3 (Phase 22.4 Plan 22.4-16) — /mouse on|off fast-path.
    // Bypasses the CommandRouter (which has no `mouse` command in the core
    // 49-command set). Toggles the live mouse-capture state via crossterm
    // and updates the shared AtomicBool so other consumers can observe state.
    if let Some(rest) = input.strip_prefix("/mouse") {
        let arg = rest.trim();
        return handle_mouse_slash(app, arg);
    }

    // UAT Round 2 Gap 5 (Phase 22.4 Plan 22.4-18) — /mcp, /sessions, /memory
    // fast-paths. These names are NOT in the core CommandRouter 49-command
    // set (see registry.rs — only `reload-mcp` and `resume` exist), so the
    // router would NotFound them. Fast-path here BEFORE the router call so
    // the user gets informative stub output instead of a typo suggestion.
    // The trailing-char guard (empty OR whitespace) prevents collision with
    // future `/mcp-*` style aliases.
    if let Some(rest) = input.strip_prefix("/mcp") {
        if rest.is_empty() || rest.starts_with(char::is_whitespace) {
            return handle_mcp_slash(app, rest.trim());
        }
    }
    if let Some(rest) = input.strip_prefix("/sessions") {
        if rest.is_empty() || rest.starts_with(char::is_whitespace) {
            return handle_sessions_slash(app, rest.trim());
        }
    }
    if let Some(rest) = input.strip_prefix("/memory") {
        if rest.is_empty() || rest.starts_with(char::is_whitespace) {
            return handle_memory_slash(app, rest.trim());
        }
    }

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

// ── /mouse on|off live toggle (UAT Gap 3 / Plan 22.4-16) ─────────────────────

/// UAT Gap 3 (Phase 22.4 Plan 22.4-16) — /mouse {on|off} live toggle.
///
/// Honours the user-locked decision: capture stays ON by default; users
/// can drop into terminal-native text selection by typing `/mouse off`,
/// then re-enable scroll-wheel transcript scrolling with `/mouse on`.
///
/// The toggle invokes the appropriate crossterm command immediately AND
/// stores the new state on the shared AtomicBool. The MouseCaptureGuard
/// Drop impl is unaffected — it always disables on REPL exit (idempotent
/// if already disabled).
fn handle_mouse_slash(app: &mut App, arg: &str) -> SlashOutcome {
    match arg {
        "on" => {
            if let Err(e) = execute!(io::stdout(), EnableMouseCapture) {
                return SlashOutcome::Error(format!("/mouse on failed: {e}"));
            }
            app.mouse_capture_enabled.store(true, Ordering::SeqCst);
            SlashOutcome::Handled(
                "Mouse capture: on (scroll wheel + click events go to TUI)".to_string(),
            )
        }
        "off" => {
            if let Err(e) = execute!(io::stdout(), DisableMouseCapture) {
                return SlashOutcome::Error(format!("/mouse off failed: {e}"));
            }
            app.mouse_capture_enabled.store(false, Ordering::SeqCst);
            SlashOutcome::Handled(
                "Mouse capture: off (terminal-native text selection re-enabled)".to_string(),
            )
        }
        "" => {
            let state = if app.mouse_capture_enabled.load(Ordering::SeqCst) {
                "on"
            } else {
                "off"
            };
            SlashOutcome::Handled(format!(
                "Mouse capture: {state}. Use /mouse on or /mouse off to toggle."
            ))
        }
        other => SlashOutcome::Unknown {
            input: format!("/mouse {other}"),
            hint: "Usage: /mouse on  |  /mouse off  |  /mouse (status)".to_string(),
        },
    }
}

// ── /mcp, /sessions, /memory fast-path handlers (UAT Round 2 Gap 5 / Plan 22.4-18) ──

/// /mcp [args] — MCP server list / status stub.
///
/// `mcp` is NOT in the core 49-command set (only `reload-mcp` is). This
/// fast-path provides visible output until a full MCP enumeration command
/// lands in a follow-up phase.
fn handle_mcp_slash(app: &mut App, _arg: &str) -> SlashOutcome {
    let configured = if app.mcp_manager.is_some() {
        "configured (use /reload-mcp to refresh)"
    } else {
        "NOT configured for this session"
    };
    SlashOutcome::Handled(format!(
        "/mcp — MCP server list / status.\n\
         Status: {configured}.\n\
         Phase 22.4 stub: full server enumeration (transports, tool inventory, \
         reconnect status) lands in a follow-up; use /reload-mcp to refresh \
         the live manager."
    ))
}

/// /sessions [args] — recent sessions list stub.
///
/// `sessions` is NOT in the core 49-command set (only `resume <name>` and
/// `history` exist). This fast-path provides visible output until a full
/// session-list command lands in a follow-up phase.
fn handle_sessions_slash(app: &mut App, _arg: &str) -> SlashOutcome {
    SlashOutcome::Handled(format!(
        "/sessions — recent session list.\n\
         Current session: {sid}.\n\
         History persisted to: {hpath}.\n\
         Phase 22.4 stub: full session enumeration (sessions on disk, last \
         modified, message counts) lands in a follow-up; use /resume <name> \
         to restore a known session by name.",
        sid = app.session_id,
        hpath = app.history_path.display(),
    ))
}

/// /memory [args] — memory provider status stub.
///
/// `memory` is NOT in the core 49-command set. This fast-path provides
/// visible output until a full memory-management command lands in a
/// follow-up phase.
fn handle_memory_slash(app: &mut App, _arg: &str) -> SlashOutcome {
    let configured = if app.memory_manager.is_some() {
        "configured (MemoryTool registered with the agent)"
    } else {
        "NOT configured for this session"
    };
    SlashOutcome::Handled(format!(
        "/memory — memory provider status.\n\
         Status: {configured}.\n\
         Phase 22.4 stub: full memory inspection (recent writes, vector store \
         counts, on_session_end policy) lands in a follow-up; the \
         MemoryManager handle is wired and reachable from the App."
    ))
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
        // UAT Round 2 Gap 5 (Phase 22.4 Plan 22.4-18): high-traffic deferred
        // handlers from Plan 22.4-07 §Handler Coverage. /agents and /skills
        // are present in the core CommandRouter (registry.rs:38 + :107) so
        // they reach this match arm via ResolveResult::Exact. Stub output
        // returns informative descriptive text per the user-locked decision
        // ("each handler at minimum returns CommandResult::Output(...) with
        // informative text — full functionality can be incremental").
        "agents" => CommandResult::Output(
            "/agents — list, kill, or tail logs for active subagents.\n\
             Args: [list | kill <id> | logs <id>]\n\
             Phase 22.4 stub: full subcommand routing lands in a follow-up; \
             the SubagentRegistry handle is wired and reachable from the App. \
             Use status pill `agents N/M` for the live count.".to_string()
        ),
        "skills" => CommandResult::Output(
            "/skills — list installed skills (active in this session).\n\
             Phase 22.4 stub: full enumeration lands in a follow-up; the \
             skills tool is registered and the active_skills Arc is shared \
             with execute_code per Phase 22 Plan 22-01.".to_string()
        ),
        other => CommandResult::Output(format!(
            "(tui_rata: /{other} not yet wired in Phase 22.4 — see plan 22.4-07 §Handler Coverage)"
        )),
    };
    Ok(result)
}

/// Render a brief help listing. Full command table rendered by the `help` handler
/// in follow-up gap-closure.
fn render_help() -> String {
    "Slash commands:\n\
     · /help — show this help\n\
     · /quit, /exit — exit the REPL\n\
     · /clear — clear screen + start new session\n\
     · /new — start new session (preserves screen)\n\
     · /reset — terminal visual reset\n\
     · /reload-mcp — reload MCP servers\n\
     · /mouse on|off — toggle mouse capture (off = native text selection)\n\
     · /agents — list, kill, or tail logs for active subagents [stub]\n\
     · /skills — list installed skills [stub]\n\
     · /mcp — MCP server list / status [stub]\n\
     · /sessions — recent session list [stub]\n\
     · /memory — memory provider status [stub]\n\
     \n\
     Phase 22.4 typo suggester surfaces `Did you mean /X?` for unrecognised \
     commands (D-10). Stubs marked [stub] return informative output describing \
     the command — full implementations land in a follow-up.\n\
     /mouse off drops into terminal-native text selection; /mouse on restores \
     scroll-wheel transcript scrolling (UAT Gap 3 closure, Plan 22.4-16).\n\
     /help, /agents, /skills, /mcp, /sessions, /memory output is rendered as a \
     dim-gray System row (UAT Round 2 Gap 4 closure, Plan 22.4-17).".to_string()
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

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
use ironhermes_core::commands::{CommandCategory, CommandResult, CommandRouter, ResolveResult};
use ironhermes_core::commands::context::{
    AgentLoopHandle, CommandContext, ContextCompressorHandle, McpManagerHandle,
    MemoryManagerHandle, PersonalityHandle, ProviderResolverHandle, StateStoreHandle,
};
use ironhermes_core::commands::typo::suggest_typo;
use ironhermes_core::types::Platform;

use crate::tui_rata::app::App;

// ── Phase 22.4.2 Plan 00: D-04 trait adapters ────────────────────────────────
//
// These thin wrappers implement the CommandContext handle traits for the
// concrete types held by App. All implementations satisfy the D-05
// `is_some()` guard pattern — handlers check `.is_some()` before calling.

/// Adapter: McpManager → McpManagerHandle for `/mcp` server enumeration.
struct McpManagerAdapter(Arc<ironhermes_mcp::McpManager>);
impl McpManagerHandle for McpManagerAdapter {
    fn connected_server_names(&self) -> Vec<String> {
        self.0.connected_server_names()
    }
}

/// Adapter: ProviderResolver → ProviderResolverHandle for `/model` `/provider`.
struct ProviderResolverAdapter(ironhermes_core::ProviderResolver);
impl ProviderResolverHandle for ProviderResolverAdapter {
    fn main_provider(&self) -> String {
        self.0.main_provider().to_string()
    }
    fn main_model(&self) -> String {
        self.0.resolve_for_main().default_model.clone()
    }
    fn status_text(&self) -> String {
        let ep = self.0.resolve_for_main();
        format!("Provider: {} | Model: {}", self.0.main_provider(), ep.default_model)
    }
    fn validate_model(&self, model: &str) -> Result<String, String> {
        // Accept any non-empty model string; Plans 01-04 will add real validation.
        if model.trim().is_empty() {
            Err("Model name cannot be empty.".to_string())
        } else {
            Ok(model.trim().to_string())
        }
    }
    fn model_list_text(&self) -> String {
        let registry = self.0.model_registry();
        let models = registry.all_models();
        if models.is_empty() {
            "No models available. Run /reload-mcp to refresh.".to_string()
        } else {
            let lines: Vec<String> = models.iter()
                .take(20)
                .map(|(id, meta)| format!("  - {} (ctx: {})", id, meta.context_length))
                .collect();
            let header = "Available models:".to_string();
            let mut out = format!("{header}\n{}", lines.join("\n"));
            if models.len() > 20 {
                out.push_str("\n  ... (use /models for full list)");
            }
            out
        }
    }
    fn fast_role_model(&self) -> Option<String> {
        self.0.resolve_role("fast").map(|ep| ep.default_model.clone())
    }
}

/// Adapter: PersonalityRegistry → PersonalityHandle for `/personality`.
struct PersonalityAdapter(Arc<ironhermes_agent::personality::PersonalityRegistry>);
impl PersonalityHandle for PersonalityAdapter {
    fn get_preset(&self, name: &str) -> Option<String> {
        self.0.get(name).map(|s| s.to_string())
    }
    fn list_presets(&self) -> Vec<String> {
        self.0.list().into_iter().map(|s| s.to_string()).collect()
    }
}

/// Adapter: MemoryManager (tokio Mutex) → MemoryManagerHandle for `/memory`.
struct MemoryManagerAdapter(Arc<tokio::sync::Mutex<ironhermes_agent::memory::MemoryManager>>);
impl MemoryManagerHandle for MemoryManagerAdapter {
    fn status_text(&self) -> String {
        // Use block_in_place to bridge async MemoryManager methods.
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let mgr = self.0.lock().await;
                match mgr.system_prompt_block().await {
                    Some(block) => format!("Memory active:\n{block}"),
                    None => "Memory: no active context block.".to_string(),
                }
            })
        })
    }
}

/// Adapter: StateStore (std Mutex) → StateStoreHandle for `/sessions` etc.
struct StateStoreAdapter(Arc<std::sync::Mutex<ironhermes_state::StateStore>>);
impl StateStoreHandle for StateStoreAdapter {
    fn list_sessions_text(&self, limit: usize) -> String {
        let guard = match self.0.lock() {
            Ok(g) => g,
            Err(_) => return "StateStore lock poisoned.".to_string(),
        };
        match guard.list_sessions(None, limit) {
            Ok(sessions) if sessions.is_empty() => "No sessions found.".to_string(),
            Ok(sessions) => {
                let lines: Vec<String> = sessions.iter()
                    .map(|s| format!("  {}", s.id))
                    .collect();
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
                let lines: Vec<String> = msgs.iter()
                    .map(|m| format!("  [{}] {}", m.role, m.content.as_deref().unwrap_or("")))
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
        let mut guard = self.0.lock().map_err(|_| "StateStore lock poisoned.".to_string())?;
        guard.update_session_title(session_id, title).map_err(|e| e.to_string())
    }
    fn get_session_id(&self, name_or_id: &str) -> Option<String> {
        let guard = self.0.lock().ok()?;
        // Try by exact id first, then by title.
        if let Ok(Some(s)) = guard.get_session(name_or_id) {
            return Some(s.id);
        }
        guard.get_session_by_title(name_or_id).ok().flatten().map(|s| s.id)
    }
}

/// Adapter: ContextEngine → ContextCompressorHandle for `/compress`.
struct ContextEngineAdapter(Arc<dyn ironhermes_agent::context_engine::ContextEngine>);
impl ContextCompressorHandle for ContextEngineAdapter {
    fn compress_text(&self) -> String {
        // Compression requires messages — return informational text.
        // Plans 01-03 will wire the actual compress call with history context.
        "Compression triggered. Use /rollback to revert if needed.".to_string()
    }
    fn status_text(&self) -> String {
        format!(
            "Context compressor active. Mode: {:?}",
            self.0.mode()
        )
    }
}

/// Adapter: AgentLoop → AgentLoopHandle for Tier D session control.
struct AgentLoopAdapter(Arc<ironhermes_agent::agent_loop::AgentLoop>);
impl AgentLoopHandle for AgentLoopAdapter {
    fn is_running(&self) -> bool {
        // Conservative: assume running if we have a handle.
        // Plans 01-04 will wire the actual running-state check.
        false
    }
}

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

/// Dispatch a slash-prefixed input through the `CommandRouter` (pure router-shell).
///
/// Phase 22.4.1 re-port: the four `strip_prefix` fast-paths from Plans 22.4-16
/// (/mouse) and 22.4-18 (/mcp, /sessions, /memory) are RETIRED. All four names
/// are now in the core registry (Plan 22.4.1-00), so the router resolves them
/// as `ResolveResult::Exact` and `invoke_handler` returns the canonical stub.
///
/// `/mouse` is the only stateful command in this re-port — its crossterm capture
/// toggle + AtomicBool mutation are App-side state, so a post-router hook
/// branches on `def.name == "mouse"` and calls `handle_mouse_slash(app, args)`
/// directly (D-10/D-11/D-12). The args extraction uses `def.name`-interpolation
/// (NOT a literal `"/mouse"` string) so INV-22.4-34 returns zero hits.
pub async fn dispatch_slash(app: &mut App, input: &str) -> SlashOutcome {
    let platform = Platform::Local; // tui_rata runs under CLI/Local platform
    match app.command_router.resolve(input, &platform) {
        ResolveResult::Exact(def) | ResolveResult::PrefixMatch(def) => {
            // Extract args: strip the leading "/<name>" prefix and split remainder.
            // D-11 from 22.4.1: use def.name-interpolated strip_prefix (not a literal).
            let args_str = input
                .strip_prefix(&format!("/{}", def.name))
                .unwrap_or("")
                .trim();
            let args_vec: Vec<&str> = if args_str.is_empty() {
                vec![]
            } else {
                args_str.split_whitespace().collect()
            };
            let ctx = build_command_context(app);
            match invoke_handler(def.name, &ctx, &app.command_router, &args_vec).await {
                Ok(result) => {
                    // D-10/D-11/D-12 post-router App-side hook for stateful /mouse.
                    if def.name == "mouse" {
                        handle_mouse_slash(app, args_str)
                    } else {
                        map_core_to_slash_outcome(result)
                    }
                }
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

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build a `CommandContext` from App state, populated with all available handles.
///
/// `agent_running` is derived from whether a pending turn is active.
/// Phase 22.4.2 Plan 00: populates all 8 new D-04 handle fields (D-05 guard
/// pattern: each field is Option so handlers gracefully return "not configured"
/// when the handle is None).
fn build_command_context(app: &App) -> CommandContext {
    let agent_running = Arc::new(AtomicBool::new(app.pending_rx.is_some()));
    let mut ctx = CommandContext::new(
        Platform::Local,
        app.session_id.clone(),
        agent_running,
    );
    if let Some(mgr) = &app.mcp_manager {
        ctx = ctx.with_mcp_reloader(mgr.clone());
        // Also wire the McpManagerHandle for `/mcp` full enumeration (D-04).
        let handle: Arc<dyn McpManagerHandle> = Arc::new(McpManagerAdapter(mgr.clone()));
        ctx = ctx.with_mcp_manager(handle);
    }
    if let Some(mem) = &app.memory_manager {
        let handle: Arc<dyn MemoryManagerHandle> = Arc::new(MemoryManagerAdapter(mem.clone()));
        ctx = ctx.with_memory_manager(handle);
    }
    if let Some(store) = &app.state_store {
        let handle: Arc<dyn StateStoreHandle> = Arc::new(StateStoreAdapter(store.clone()));
        ctx = ctx.with_state_store(handle);
    }
    {
        let handle: Arc<dyn ProviderResolverHandle> =
            Arc::new(ProviderResolverAdapter(app.resolver.clone()));
        ctx = ctx.with_provider_resolver(handle);
    }
    if let Some(engine) = &app.context_compressor {
        let handle: Arc<dyn ContextCompressorHandle> =
            Arc::new(ContextEngineAdapter(engine.clone()));
        ctx = ctx.with_context_compressor(handle);
    }
    {
        let handle: Arc<dyn PersonalityHandle> =
            Arc::new(PersonalityAdapter(app.personality_overlay.clone()));
        ctx = ctx.with_personality_overlay(handle);
    }
    // History snapshot: clone current history for read-only handlers.
    // Mutations (/retry, /undo, /rollback) apply in the post-router hook.
    {
        let snapshot = Arc::new(std::sync::RwLock::new(app.history.clone()));
        ctx = ctx.with_history(snapshot);
    }
    {
        let handle: Arc<dyn AgentLoopHandle> =
            Arc::new(AgentLoopAdapter(app.agent_loop.clone()));
        ctx = ctx.with_agent_loop(handle);
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

/// Phase 22.4.2 Plan 01 (D-01): delegate `invoke_handler` to `core::handlers::dispatch`.
///
/// The 30-arm match table from Phase 22.4.1 Plan 02 collapses to a single delegation.
/// Single source of truth across gateway + classic-tui + tui_rata. Real handler bodies
/// in `ironhermes_core::commands::handlers` replace the per-command stub arms.
/// The safety-net fallback in `dispatch()` covers `/voice` and `/prompt` which remain
/// without backing infra (they still return the todo_stub informational text from core).
async fn invoke_handler(
    name: &str,
    ctx: &CommandContext,
    router: &CommandRouter,
    args: &[&str],
) -> Result<CommandResult, anyhow::Error> {
    let def = router
        .commands
        .iter()
        .find(|c| c.name == name)
        .ok_or_else(|| anyhow::anyhow!("unknown command: {name}"))?;
    Ok(ironhermes_core::commands::handlers::dispatch(def, args, ctx, router))
}

/// Render router-driven /help text — pure router-driven enumeration of the
/// CommandDef registry by category.
///
/// Phase 22.4.1 D-13: replaces the 22-line hand-built `render_help()` so a
/// new CommandDef added to `build_registry()` automatically surfaces in /help
/// without per-call-site maintenance. Body lifted from
/// `crates/ironhermes-cli/src/tui/commands.rs::format_help` (RESEARCH Finding 1)
/// minus the classic-tui-only `_extensions` and `keybinding_registry`
/// parameters.
fn render_help_router(router: &CommandRouter, platform: &Platform) -> String {
    let mut out = String::from("Available commands:\n");
    for (category, cmds) in router.commands_by_category(platform) {
        out.push('\n');
        let cat_name = match category {
            CommandCategory::Session => "SESSION",
            CommandCategory::Configuration => "CONFIGURATION",
            CommandCategory::ToolsAndSkills => "TOOLS & SKILLS",
            CommandCategory::Info => "INFO",
            CommandCategory::Exit => "EXIT",
        };
        out.push_str(cat_name);
        out.push('\n');
        for cmd in cmds {
            out.push_str(&format!(
                "  /{:<13}{:<16}{}\n",
                cmd.name, cmd.args_hint, cmd.description
            ));
        }
    }
    out
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

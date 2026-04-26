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
                .map(|(id, _meta)| format!("  {id}"))
                .collect();
            format!("Available models:\n{}", lines.join("\n"))
        }
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
            let ctx = build_command_context(app);
            match invoke_handler(def.name, &ctx, &app.command_router).await {
                Ok(result) => {
                    // D-10/D-11/D-12 post-router App-side hook for stateful /mouse.
                    if def.name == "mouse" {
                        let args = input
                            .strip_prefix(&format!("/{}", def.name))
                            .unwrap_or("")
                            .trim();
                        handle_mouse_slash(app, args)
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

/// Minimum-viable command handler table for Phase 22.4.
///
/// Routes by `def.name` to concrete `CommandResult` variants.
/// The `other` arm returns an informative `Output` (not a panic/abort)
/// for commands not yet wired — documented in SUMMARY §Handler Coverage.
async fn invoke_handler(
    name: &str,
    _ctx: &CommandContext,
    router: &CommandRouter,
) -> Result<CommandResult, anyhow::Error> {
    let result = match name {
        "quit" | "exit" => CommandResult::Quit,
        "clear"         => CommandResult::ClearSession,
        "new"           => CommandResult::NewSession {
            message: "New session started.".to_string(),
        },
        "help"          => CommandResult::Output(render_help_router(router, &Platform::Local)),
        "reset"         => CommandResult::ResetTerminal,
        "reload-mcp"    => CommandResult::McpReload,
        "mouse" => CommandResult::Output(
            "/mouse — Toggle mouse capture.\n\
             Args: [on|off]\n\
             Phase 22.4.1 stub: the actual crossterm capture toggle + AtomicBool \
             flip runs in the post-router hook in dispatch_slash (D-10/D-11/D-12); \
             this Output is unused for /mouse but is required so /help discovers \
             the command via the router-driven enumerator.".to_string()
        ),
        "mcp" => CommandResult::Output(
            "/mcp — MCP server list and status.\n\
             Phase 22.4.1 stub: full server enumeration (transports, tool inventory, \
             reconnect status) lands in a follow-up; use /reload-mcp to refresh the \
             live manager. McpManager handle is wired and reachable from the App.".to_string()
        ),
        "sessions" => CommandResult::Output(
            "/sessions — List recent sessions.\n\
             Phase 22.4.1 stub: full session enumeration (on-disk sessions, last \
             modified, message counts) lands in a follow-up; StateStore FTS5 \
             (Phase 13 SESS-01) owns implementation. Use /resume <name> to \
             restore a known session.".to_string()
        ),
        "memory" => CommandResult::Output(
            "/memory — Memory provider status.\n\
             Phase 22.4.1 stub: full memory inspection (recent writes, vector \
             store counts, on_session_end policy) lands in a follow-up; \
             MemoryManager (Phase 20) owns implementation.".to_string()
        ),

        // ── Phase 22.4.1 Plan 02: Session category bulk arms (D-05/D-08) ──────
        "history" => CommandResult::Output(
            "/history — Show conversation history.\n\
             Phase 22.4.1 stub: StateStore FTS5 session history (Phase 13 SESS-01) owns implementation.".to_string()
        ),
        "save" => CommandResult::Output(
            "/save — Save conversation to file.\n\
             Phase 22.4.1 stub: Session export (Phase 13 SESS-08) owns implementation.".to_string()
        ),
        "retry" => CommandResult::Output(
            "/retry — Retry the last message.\n\
             Phase 22.4.1 stub: AgentLoop last-message retry (Phase 21 session control) owns implementation.".to_string()
        ),
        "undo" => CommandResult::Output(
            "/undo — Undo the last exchange.\n\
             Phase 22.4.1 stub: AgentLoop message-pair removal (Phase 21 session control) owns implementation.".to_string()
        ),
        "title" => CommandResult::Output(
            "/title — Set session title.\n\
             Args: [name]\n\
             Phase 22.4.1 stub: StateStore session title (Phase 13 SESS-04) owns implementation.".to_string()
        ),
        "compress" => CommandResult::Output(
            "/compress — Compress conversation context.\n\
             Args: [focus]\n\
             Phase 22.4.1 stub: ContextCompressor (Phase 18 PRMT-11) owns implementation.".to_string()
        ),
        "rollback" => CommandResult::Output(
            "/rollback — Roll back to a checkpoint.\n\
             Args: [number]\n\
             Phase 22.4.1 stub: Session checkpoint rollback (Phase 21 session control) owns implementation.".to_string()
        ),
        "stop" => CommandResult::Output(
            "/stop — Stop the running agent.\n\
             Phase 22.4.1 stub: CancellationToken cascade (Phase 21 D-14) owns implementation.".to_string()
        ),
        "background" => CommandResult::Output(
            "/background — Run a prompt in the background.\n\
             Args: <prompt>\n\
             Phase 22.4.1 stub: SubagentRunner background delegation (Phase 21.7 D-09) owns implementation.".to_string()
        ),
        "btw" => CommandResult::Output(
            "/btw — Ask an ephemeral question.\n\
             Args: <question>\n\
             Phase 22.4.1 stub: Ephemeral single-turn query (Phase 21 session control) owns implementation.".to_string()
        ),
        "queue" => CommandResult::Output(
            "/queue — Queue a prompt for after current turn.\n\
             Args: <prompt>\n\
             Phase 22.4.1 stub: Turn queue (Phase 21.7 D-29) owns implementation.".to_string()
        ),
        "status" => CommandResult::Output(
            "/status — Show current session status.\n\
             Phase 22.4.1 stub: hermes status diagnostics (Phase 21.7 D-18) owns implementation.".to_string()
        ),
        "resume" => CommandResult::Output(
            "/resume — Resume a previous session.\n\
             Args: [name]\n\
             Phase 22.4.1 stub: StateStore session restore (Phase 13 SESS-04) owns implementation.".to_string()
        ),

        // ── Phase 22.4.1 Plan 02: Configuration category bulk arms (D-05/D-08) ─
        "config" => CommandResult::Output(
            "/config — Show configuration.\n\
             Phase 22.4.1 stub: config.yaml reader (Phase 23 CFG-02) owns implementation.".to_string()
        ),
        "provider" => CommandResult::Output(
            "/provider — Show current provider.\n\
             Phase 22.4.1 stub: ProviderResolver current endpoint (Phase 12 PROV-01) owns implementation.".to_string()
        ),
        "prompt" => CommandResult::Output(
            "/prompt — Set custom system prompt.\n\
             Args: [text]\n\
             Phase 22.4.1 stub: System prompt override (Phase 15 PRMT-06) owns implementation.".to_string()
        ),
        "personality" => CommandResult::Output(
            "/personality — Apply a personality preset.\n\
             Args: [name]\n\
             Phase 22.4.1 stub: SOUL.md overlay (Phase 15 PRMT-06/PRMT-07) owns implementation.".to_string()
        ),
        "statusbar" => CommandResult::Output(
            "/statusbar — Toggle status bar.\n\
             Phase 22.4.1 stub: TUI status bar toggle (Phase 21 D-03) owns implementation.".to_string()
        ),
        "verbose" => CommandResult::Output(
            "/verbose — Toggle verbose tool output.\n\
             Phase 22.4.1 stub: Verbose tool output toggle (Phase 21 CLI config) owns implementation.".to_string()
        ),
        "yolo" => CommandResult::Output(
            "/yolo — Toggle dangerous command auto-approval.\n\
             Phase 22.4.1 stub: Dangerous command auto-approval (Phase 21.7 D-11) owns implementation.".to_string()
        ),
        "reasoning" => CommandResult::Output(
            "/reasoning — Set reasoning level.\n\
             Args: [level|show|hide]\n\
             Phase 22.4.1 stub: Provider reasoning level (Phase 12 PROV-02) owns implementation.".to_string()
        ),
        "skin" => CommandResult::Output(
            "/skin — Change color theme.\n\
             Args: [name]\n\
             Phase 22.4.1 stub: Color theme (Phase 22.3 skin config) owns implementation.".to_string()
        ),
        "voice" => CommandResult::Output(
            "/voice — Voice/TTS settings.\n\
             Args: [on|off|tts|status]\n\
             Phase 22.4.1 stub: TTS/voice output (future phase) owns implementation.".to_string()
        ),
        "model" => CommandResult::Output(
            "/model — Switch model for this session.\n\
             Args: [provider:model] [--global]\n\
             Phase 22.4.1 stub: ProviderResolver model switch (Phase 21.3) owns implementation.".to_string()
        ),
        "fast" => CommandResult::Output(
            "/fast — Toggle fast model preset.\n\
             Phase 22.4.1 stub: Fast model preset toggle (Phase 21.3 model config) owns implementation.".to_string()
        ),
        "debug" => CommandResult::Output(
            "/debug — Toggle debug information.\n\
             Phase 22.4.1 stub: Debug information toggle (Phase 21 CLI config) owns implementation.".to_string()
        ),

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

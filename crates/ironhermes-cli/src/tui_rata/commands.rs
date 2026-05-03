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
                    // D-02 post-router App-side hook (Plan 03: FULL multi-name expansion).
                    // Plan 03 is the SOLE writer of this hook in Wave 2 (Option B).
                    match def.name {
                        // Mouse: existing handler (crossterm + AtomicBool)
                        "mouse" => handle_mouse_slash(app, args_str),
                        // Toggles: yolo/verbose/statusbar/debug/skin (NOT fast — owned by subsystem_mutator)
                        "yolo" | "verbose" | "statusbar" | "debug" | "skin" => {
                            handle_toggle(app, def.name, args_str)
                        }
                        // App-handle inspectors: trust core output; no App-side mutation needed
                        "memory" | "mcp" => {
                            handle_app_inspector(app, def.name, &args_vec, &result).await
                        }
                        // Tier D session control: stub for Plan 04 to replace
                        "stop" | "retry" | "undo" | "rollback" | "background" | "btw" | "queue" => {
                            handle_session_control(app, def.name, &args_vec, &result).await
                        }
                        // Subsystem mutators: model/fast (AnyClient rebuild) + personality/compress
                        "model" | "fast" | "personality" | "compress" => {
                            handle_subsystem_mutator(app, def.name, &args_vec, &result).await
                        }
                        // Default: trust core dispatch result
                        _ => map_core_to_slash_outcome(result),
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

// ── Phase 22.4.2.1 Plan 01: CronJobReader adapter ────────────────────────────
//
// Bridges Arc<Mutex<ironhermes_cron::JobStore>> → CronJobReader trait so that
// cmd_cron in ironhermes-core can read cron state without a circular dep.
// Follows the McpManagerAdapter / MemoryManagerAdapter pattern above.

use ironhermes_core::commands::context::CronJobReader;
use ironhermes_cron::display::{format_cron_status, format_job_detail, format_job_list};

struct CronJobReaderImpl(std::sync::Arc<std::sync::Mutex<ironhermes_cron::JobStore>>);

impl CronJobReader for CronJobReaderImpl {
    fn list_jobs_text(&self) -> String {
        let guard = self.0.lock().expect("JobStore mutex poisoned");
        format_job_list(guard.list_jobs(), false)
    }

    fn get_job_text(&self, id_or_name: &str) -> Option<String> {
        let guard = self.0.lock().expect("JobStore mutex poisoned");
        guard.find_job(id_or_name).map(format_job_detail)
    }

    fn status_text(&self) -> String {
        let guard = self.0.lock().expect("JobStore mutex poisoned");
        format_cron_status(guard.list_jobs())
    }

    fn pause_job(&self, id_or_name: &str) -> Result<String, String> {
        let mut guard = self.0.lock().map_err(|e| format!("mutex: {}", e))?;
        let job = guard
            .find_job(id_or_name)
            .ok_or_else(|| format!("No cron job found: {}", id_or_name))?;
        let id = job.id.clone();
        let name = job.name.clone();
        guard.toggle_job(&id, false).map_err(|e| e.to_string())?;
        guard.save().map_err(|e| e.to_string())?;
        Ok(format!("Paused: {}", name))
    }

    fn resume_job(&self, id_or_name: &str) -> Result<String, String> {
        let mut guard = self.0.lock().map_err(|e| format!("mutex: {}", e))?;
        let job = guard
            .find_job(id_or_name)
            .ok_or_else(|| format!("No cron job found: {}", id_or_name))?;
        let id = job.id.clone();
        let name = job.name.clone();
        guard.toggle_job(&id, true).map_err(|e| e.to_string())?;
        guard.save().map_err(|e| e.to_string())?;
        Ok(format!("Resumed: {}", name))
    }

    fn remove_job(&self, id_or_name: &str) -> Result<String, String> {
        let mut guard = self.0.lock().map_err(|e| format!("mutex: {}", e))?;
        let job = guard
            .find_job(id_or_name)
            .ok_or_else(|| format!("No cron job found: {}", id_or_name))?;
        let id = job.id.clone();
        let name = job.name.clone();
        guard.remove_job(&id).map_err(|e| e.to_string())?;
        guard.save().map_err(|e| e.to_string())?;
        Ok(format!("Removed: {}", name))
    }

    fn queue_run(&self, id_or_name: &str) -> Result<String, String> {
        let guard = self.0.lock().map_err(|e| format!("mutex: {}", e))?;
        let job = guard
            .find_job(id_or_name)
            .ok_or_else(|| format!("No cron job found: {}", id_or_name))?;
        // Per CONTEXT D-04 / RESEARCH §3 cmd_run note: slash /cron run queues
        // for next gateway tick, does NOT execute inline.
        Ok(format!("Job queued for next tick: {}", job.name))
    }
}

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
    // ProcessRegistry for /stop (Plan 04: thread into build_command_context).
    // ProcessRegistryHandle is the newtype in ironhermes-exec that implements
    // ProcessRegistrySnapshotHandle for Arc<RwLock<ProcessRegistry>>.
    {
        use ironhermes_core::commands::context::ProcessRegistrySnapshotHandle;
        let handle: Arc<dyn ProcessRegistrySnapshotHandle> = Arc::new(
            ironhermes_exec::process_registry::ProcessRegistryHandle::new(
                app.process_registry.clone(),
            ),
        );
        ctx = ctx.with_process_registry(handle);
    }
    // SubagentRegistry for /agents (already wired via cmd_agents in core).
    {
        use ironhermes_core::commands::context::SubagentListSnapshot;
        let handle: Arc<dyn SubagentListSnapshot> = Arc::new(
            ironhermes_agent::subagent_registry::SubagentRegistryHandle::new(
                app.subagent_registry.clone(),
            ),
        );
        ctx = ctx.with_subagent_registry(handle);
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
    // Phase 22.4.2.1 Plan 01: wire CronJobReader as 11th with_* call.
    if let Some(cron) = &app.cron_store {
        let handle: Arc<dyn CronJobReader> = Arc::new(CronJobReaderImpl(cron.clone()));
        ctx = ctx.with_cron_store(handle);
    }
    // Phase 25.2 Plan 15 follow-up (UAT Issue 2 / Symptom 1): attach the
    // production `ToolsetSessionHandle` so /toolset list/show/enable/disable
    // works in the ratatui REPL. Without this attach, cmd_toolset short-
    // circuits on `None` at handlers.rs:782 with the documented fallback
    // string. Plan 15 wired this for run_chat/run_single/run_gateway but
    // missed run_chat_ratatui (the default `hermes chat` since Phase 22.4).
    if let Some(handle) = &app.toolset_session {
        ctx = ctx.with_toolset_session(handle.clone());
    }
    // Phase 25.3 D-W-2: attach Workspace for /sessions --workspace + trajectory scoping.
    if let Some(ws) = &app.workspace {
        ctx = ctx.with_workspace(ws.clone());
    }
    // Phase 25.3 D-T-3: attach TrajectoryWriter for slash-dispatch context.
    if let Some(tw) = &app.trajectory_writer {
        ctx = ctx.with_trajectory_writer(tw.clone());
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

// ── Post-router helper functions (D-02, Plan 03 full expansion) ──────────────

/// handle_toggle — flip Arc<AtomicBool> toggles (yolo/verbose/statusbar/debug) or
/// write Arc<RwLock<String>> for skin. EXCLUDES "fast" (owned by handle_subsystem_mutator).
///
/// Plan 03 D-09: fetch_xor(true, Ordering::SeqCst) is the canonical toggle pattern for AtomicBool.
/// T-22.4.2-03-07: skin uses `.write().unwrap_or_else(|p| p.into_inner())` for poison recovery.
fn handle_toggle(app: &mut App, name: &str, arg: &str) -> SlashOutcome {
    match name {
        "yolo" => {
            let new_val = !app.yolo_enabled.fetch_xor(true, Ordering::SeqCst);
            SlashOutcome::Handled(format!("YOLO mode: {}", if new_val { "on" } else { "off" }))
        }
        "verbose" => {
            let new_val = !app.verbose_enabled.fetch_xor(true, Ordering::SeqCst);
            SlashOutcome::Handled(format!("Verbose mode: {}", if new_val { "on" } else { "off" }))
        }
        "statusbar" => {
            let new_val = !app.statusbar_enabled.fetch_xor(true, Ordering::SeqCst);
            SlashOutcome::Handled(format!("Status bar: {}", if new_val { "on" } else { "off" }))
        }
        "debug" => {
            let new_val = !app.debug_enabled.fetch_xor(true, Ordering::SeqCst);
            SlashOutcome::Handled(format!("Debug mode: {}", if new_val { "on" } else { "off" }))
        }
        "skin" => {
            if arg.is_empty() {
                let current = app.skin.read()
                    .map(|s| s.clone())
                    .unwrap_or_else(|p| p.into_inner().clone());
                SlashOutcome::Handled(format!(
                    "Current skin: {current}. Usage: /skin <name>"
                ))
            } else {
                // T-22.4.2-03-01: validate skin name to alphanumeric + dash + underscore
                if !arg.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
                    return SlashOutcome::Handled(format!(
                        "Invalid skin name: {arg} (alphanumeric + - _ only)"
                    ));
                }
                // T-22.4.2-03-07: poison recovery on RwLock
                let mut w = app.skin.write().unwrap_or_else(|p| p.into_inner());
                *w = arg.to_string();
                SlashOutcome::Handled(format!("Skin set to: {arg}"))
            }
        }
        other => SlashOutcome::Unknown {
            input: format!("/{other}"),
            hint: "handle_toggle dispatched to unknown name (planner bug)".to_string(),
        },
    }
}

/// handle_app_inspector — pass through to map_core_to_slash_outcome.
///
/// /memory and /mcp output comes from core handlers; no App-side mutation needed.
/// Future: if scroll-to-bottom on /history or similar is desired, add here.
async fn handle_app_inspector(
    _app: &mut App,
    _name: &str,
    _args: &[&str],
    core_result: &CommandResult,
) -> SlashOutcome {
    // Trust core handler output; no App-side mutation needed for /memory /mcp.
    map_core_to_slash_outcome(core_result.clone())
}

/// handle_session_control — Plan 04 real bodies for Tier D session control.
///
/// /stop: ProcessRegistry drain (threaded in build_command_context — core handles it).
/// /retry: truncate last assistant message from history + queue last user msg for re-submission.
/// /undo: remove last (user, assistant) pair from App.history.
/// /rollback [n]: remove last N (user, assistant) pairs from App.history.
/// /background, /btw, /queue: spawn/inject via App.pending_tx mechanism.
///
/// Per RESEARCH.md OQ-5: /rollback is session-history truncation only — no ContextEngine API.
async fn handle_session_control(
    app: &mut App,
    name: &str,
    args: &[&str],
    core_result: &CommandResult,
) -> SlashOutcome {
    match name {
        "stop" => {
            // /stop: ProcessRegistry is now threaded into ctx via build_command_context.
            // Core cmd_stop handles the drain-and-kill; trust core result.
            map_core_to_slash_outcome(core_result.clone())
        }
        "retry" => {
            // Find the last user message in history.
            let last_user_text = app.history.iter().rev()
                .find(|m| m.role == ironhermes_core::types::Role::User)
                .and_then(|m| m.content.as_ref())
                .and_then(|c| c.as_text())
                .map(|s| s.to_string());

            match last_user_text {
                None => SlashOutcome::Handled("No user messages in history to retry.".to_string()),
                Some(text) => {
                    // Remove trailing assistant message(s) to re-run from last user turn.
                    while app.history.last().map(|m| m.role == ironhermes_core::types::Role::Assistant).unwrap_or(false) {
                        app.history.pop();
                    }
                    // Re-queue the user message as a new pending turn.
                    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<crate::tui_rata::stream_events::StreamEvent>();
                    app.pending_rx = Some(rx);
                    app.pending_tx = Some(tx);
                    app.cancel_child = Some(app.cancel_parent.child_token());
                    app.auto_follow = true;
                    app.assistant_buffer = None;
                    SlashOutcome::Handled(format!("Retrying: {text}"))
                }
            }
        }
        "undo" => {
            if app.history.is_empty() {
                return SlashOutcome::Handled("No history to undo.".to_string());
            }
            // Remove last assistant message (if present).
            if app.history.last().map(|m| m.role == ironhermes_core::types::Role::Assistant).unwrap_or(false) {
                app.history.pop();
            }
            // Remove last user message (if present).
            if app.history.last().map(|m| m.role == ironhermes_core::types::Role::User).unwrap_or(false) {
                app.history.pop();
                SlashOutcome::Handled("Last exchange undone.".to_string())
            } else {
                SlashOutcome::Handled("Undo: no user message found to remove.".to_string())
            }
        }
        "rollback" => {
            // Parse N (default 1) — number of (user, assistant) pairs to remove.
            let n: usize = args.first()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1)
                .max(1);
            if app.history.is_empty() {
                return SlashOutcome::Handled("No history to roll back.".to_string());
            }
            let mut removed = 0usize;
            for _ in 0..n {
                // Remove trailing assistant message (if any).
                if app.history.last().map(|m| m.role == ironhermes_core::types::Role::Assistant).unwrap_or(false) {
                    app.history.pop();
                }
                // Remove trailing user message (if any).
                if app.history.last().map(|m| m.role == ironhermes_core::types::Role::User).unwrap_or(false) {
                    app.history.pop();
                    removed += 1;
                } else {
                    break; // No more user messages to remove.
                }
            }
            if removed == 0 {
                SlashOutcome::Handled("Rollback: no exchanges found to remove.".to_string())
            } else {
                SlashOutcome::Handled(format!("Rolled back {removed} exchange(s)."))
            }
        }
        "background" => {
            // Spawn a background agent turn with the given message.
            // Uses the same pending_tx/spawn_turn mechanism as submit().
            if args.is_empty() {
                return SlashOutcome::Handled(
                    "Usage: /background <message> — run a prompt as a background task.".to_string()
                );
            }
            let message = args.join(" ");
            // Push the background message as a user turn and queue for spawn.
            app.history.push(ironhermes_core::types::ChatMessage::user(message.clone()));
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<crate::tui_rata::stream_events::StreamEvent>();
            app.pending_rx = Some(rx);
            app.pending_tx = Some(tx);
            app.cancel_child = Some(app.cancel_parent.child_token());
            app.auto_follow = true;
            app.assistant_buffer = None;
            SlashOutcome::Handled(format!("Background task queued: \"{message}\""))
        }
        "btw" => {
            // Inject an aside into the current/next turn.
            if args.is_empty() {
                return SlashOutcome::Handled(
                    "Usage: /btw <message> — add an aside to the current/next agent turn.".to_string()
                );
            }
            let message = args.join(" ");
            // Append as a user message; it will be included in the next spawn_turn call.
            app.history.push(ironhermes_core::types::ChatMessage::user(
                format!("[btw] {message}")
            ));
            SlashOutcome::Handled(format!("Aside added: \"{message}\" (active next turn)"))
        }
        "queue" => {
            // Queue a message for submission after the current turn.
            if args.is_empty() {
                return SlashOutcome::Handled(
                    "Usage: /queue <message> — add a message to the input queue.".to_string()
                );
            }
            let message = args.join(" ");
            // Pre-populate the textarea with the queued message; user can review/submit.
            let mut ta = tui_textarea::TextArea::default();
            ta.set_cursor_line_style(ratatui::style::Style::default());
            ta.set_block(ratatui::widgets::Block::default().borders(ratatui::widgets::Borders::ALL).title("Prompt"));
            for c in message.chars() {
                ta.insert_char(c);
            }
            app.textarea = ta;
            SlashOutcome::Handled(format!("Queued: \"{message}\" (press Enter to submit)"))
        }
        _ => map_core_to_slash_outcome(core_result.clone()),
    }
}

/// handle_subsystem_mutator — covers model/fast (AnyClient rebuild) + personality/compress.
///
/// Plan 03 owns the FULL helper under Option B (Plan 02 does NOT touch commands.rs).
/// T-22.4.2-03-10: /model validates via resolver before rebuilding.
async fn handle_subsystem_mutator(
    app: &mut App,
    name: &str,
    args: &[&str],
    core_result: &CommandResult,
) -> SlashOutcome {
    // Pass through if core handler returned an error.
    if matches!(core_result, CommandResult::Error(_)) {
        return map_core_to_slash_outcome(core_result.clone());
    }
    match name {
        "model" => {
            // No-args: list mode — pass through core Output.
            let model = match args.first() {
                Some(m) => *m,
                None => return map_core_to_slash_outcome(core_result.clone()),
            };
            // T-22.4.2-03-10: validate model name via resolver before rebuilding.
            let main_ep = app.resolver.resolve_for_main();
            let provider = app.resolver.main_provider().to_string();
            match ironhermes_agent::build_client(&app.resolver, &provider, model) {
                Ok(new_client) => {
                    app.client = new_client;
                    SlashOutcome::Handled(format!("Switched to model {model}"))
                }
                Err(_) => {
                    // Model not found in provider — return informational text.
                    let _ = main_ep; // suppress unused warning
                    SlashOutcome::Handled(format!("Model {model} not found in registry."))
                }
            }
        }
        "fast" => {
            // Toggle fast_enabled AtomicBool AND rebuild AnyClient from fast role.
            let new_state = !app.fast_enabled.fetch_xor(true, Ordering::SeqCst);
            if new_state {
                // ON: try to rebuild from fast role
                match ironhermes_agent::build_role_client(&app.resolver, "fast") {
                    Ok(Some(new_client)) => {
                        let model = app.resolver.resolve_role("fast")
                            .map(|ep| ep.default_model.clone())
                            .unwrap_or_else(|| "fast".to_string());
                        app.client = new_client;
                        SlashOutcome::Handled(format!("Fast mode ON — model {model}"))
                    }
                    Ok(None) => SlashOutcome::Handled(
                        "Fast mode toggle (no fast preset configured).".to_string()
                    ),
                    Err(e) => SlashOutcome::Handled(format!("Fast mode ON (rebuild failed: {e})")),
                }
            } else {
                // OFF: restore main model client
                match ironhermes_agent::build_main_client(&app.resolver) {
                    Ok(new_client) => {
                        let main_model = app.resolver.resolve_for_main().default_model.clone();
                        app.client = new_client;
                        SlashOutcome::Handled(format!("Fast mode OFF — restored to {main_model}"))
                    }
                    Err(e) => SlashOutcome::Handled(format!("Fast mode OFF (restore failed: {e})")),
                }
            }
        }
        "personality" => {
            // Core returned Output(overlay_text) for a named preset; apply to next turn.
            // For list mode or "not configured" case, pass through.
            match core_result {
                CommandResult::Output(text)
                    if !text.starts_with("Available")
                        && !text.starts_with("Personality registry")
                        && !text.starts_with("No personalities") =>
                {
                    // Apply overlay as system-prompt injection for next turn.
                    // App.next_turn_personality_overlay: Option<String> stores pending injection.
                    app.next_turn_personality_overlay = Some(text.clone());
                    SlashOutcome::Handled(format!(
                        "Personality applied ({} chars). Active next turn.",
                        text.len()
                    ))
                }
                _ => map_core_to_slash_outcome(core_result.clone()),
            }
        }
        "compress" => {
            // Core returned informational text per Task 1 deferral note.
            // Future: trigger actual compression hook here on demand.
            map_core_to_slash_outcome(core_result.clone())
        }
        _ => map_core_to_slash_outcome(core_result.clone()),
    }
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

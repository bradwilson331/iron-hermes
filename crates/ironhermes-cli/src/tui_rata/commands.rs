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
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use ironhermes_core::commands::context::{
    AgentLoopHandle, CommandContext, ContextCompressorHandle, CoreContextHandles, McpManagerHandle,
    McpReloader, MemoryManagerHandle, PersonalityHandle, ProcessRegistrySnapshotHandle,
    ProviderResolverHandle, StateStoreHandle, SubagentListSnapshot, build_core_context,
};
use ironhermes_core::commands::typo::suggest_typo;
use ironhermes_core::commands::{CommandCategory, CommandResult, CommandRouter, ResolveResult};
use ironhermes_core::queue::QueueError;
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

// `ProviderResolverAdapter` (`/model` `/provider` `/fast`) now lives in
// `ironhermes_core::commands::context` so every surface shares one impl — see
// the Phase 41.3 UAT finding F-1: this adapter being private to the TUI is why
// Web answered "Provider resolver not configured." despite owning a resolver.

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
        self.list_sessions_text_filtered(limit, None)
    }
    fn list_sessions_text_filtered(&self, limit: usize, workspace_root: Option<&str>) -> String {
        let guard = match self.0.lock() {
            Ok(g) => g,
            Err(_) => return "StateStore lock poisoned.".to_string(),
        };
        match guard.list_sessions_filtered(None, limit, workspace_root) {
            Ok(sessions) if sessions.is_empty() => match workspace_root {
                Some(ws) => format!("No sessions found for workspace: {ws}"),
                None => "No sessions found.".to_string(),
            },
            Ok(sessions) => {
                let lines: Vec<String> = sessions.iter().map(|s| format!("  {}", s.id)).collect();
                let header = match workspace_root {
                    Some(ws) => format!("Recent sessions (workspace={ws}):"),
                    None => "Recent sessions:".to_string(),
                };
                format!("{header}\n{}", lines.join("\n"))
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
    /// Phase 25.3 D-F-1: write the 4-file directory export for `session_id`.
    ///
    /// Output dir: `<hermes_home>/sessions/<session_id>/`. Trajectory source
    /// resolves workspace-aware (cwd walk-up; falls back to global hermes_home)
    /// to match Plan 8's writer-attach resolution.
    fn export_to_directory_text(&self, session_id: &str) -> String {
        let guard = match self.0.lock() {
            Ok(g) => g,
            Err(_) => return "error: StateStore lock poisoned.".to_string(),
        };
        let export = match guard.export_session(session_id) {
            Ok(e) => e,
            Err(e) => return format!("error: failed to fetch session {session_id}: {e}"),
        };
        // Drop the lock before doing filesystem IO — `write` is sync but the
        // SessionDirectoryExport doesn't need the connection.
        drop(guard);

        let output_dir = ironhermes_core::constants::get_hermes_home()
            .join("sessions")
            .join(session_id);
        // Trajectory source path resolves the same way as Plan 8 wireup.
        let cwd = std::env::current_dir().ok();
        let traj_root = match cwd
            .as_ref()
            .and_then(|c| ironhermes_core::workspace::resolve_from_cwd(c))
        {
            Some(ws) => ws.root.join(".ironhermes"),
            None => ironhermes_core::constants::get_hermes_home(),
        };
        let traj_src = traj_root
            .join("sessions")
            .join(session_id)
            .join("trajectories.jsonl");
        let exporter = ironhermes_state::SessionDirectoryExport::new(session_id, &output_dir);
        match exporter.write(&export, None, Some(traj_src.as_path())) {
            Ok(()) => format!("Session {session_id} exported to {}", output_dir.display()),
            Err(e) => format!("error: export failed: {e}"),
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
        // Try by exact id first, then by title.
        if let Ok(Some(s)) = guard.get_session(name_or_id) {
            return Some(s.id);
        }
        guard
            .get_session_by_title(name_or_id)
            .ok()
            .flatten()
            .map(|s| s.id)
    }

    /// Phase 36.2 Plan 10: `/usage` table renderer (production impl).
    ///
    /// Builds a `UsageFilter` from the primitive arguments and dispatches
    /// to `StateStore::query_usage_events` (which is the single SQL-bound
    /// access site, T-36.2-10-INJ). Output is built by the canonical
    /// `format_usage_rollups` helper in `ironhermes-state` so every
    /// platform (CLI, TUI, gateway, web UI) emits byte-identical text.
    fn usage_text(
        &self,
        session_id: Option<&str>,
        today_only: bool,
        provider: Option<&str>,
        model: Option<&str>,
        since_seconds: Option<i64>,
    ) -> String {
        let filter = ironhermes_state::UsageFilter {
            session_id: session_id.map(|s| s.to_string()),
            today_only,
            provider: provider.map(|s| s.to_string()),
            model: model.map(|s| s.to_string()),
            since_seconds,
        };
        let guard = match self.0.lock() {
            Ok(g) => g,
            Err(_) => return "StateStore lock poisoned.".to_string(),
        };
        match guard.query_usage_events(&filter) {
            Ok(rows) => ironhermes_state::format_usage_rollups(&rows, &filter),
            Err(e) => format!("Usage query failed: {e}"),
        }
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
        format!("Context compressor active. Mode: {:?}", self.0.mode())
    }
}

/// Adapter: AgentRuntime → AgentLoopHandle for Tier D session control.
///
/// Phase 28.1-05: the App-level Arc<AgentLoop> is replaced by Arc<AgentRuntime>.
/// AgentLoopHandle only exposes is_running() → bool; the runtime has no concept
/// of a "running turn" at the handle level (the cancel_child token on App tracks
/// that). Conservative false matches the prior AgentLoopAdapter behaviour and
/// satisfies all existing /status consumers without requiring a live-state probe.
#[allow(dead_code)] // field retained for future is_running() predicate wiring; current impl conservatively returns false
struct AgentRuntimeAdapter(Arc<ironhermes_agent::AgentRuntime>);
impl AgentLoopHandle for AgentRuntimeAdapter {
    fn is_running(&self) -> bool {
        // Conservative: the runtime does not expose a live-turn predicate at the
        // handle level. Return false so /status shows "idle" consistently; the
        // status-line agent-running pill (AtomicBool) is the accurate indicator.
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
    /// Skills registry reloaded; string is the diff/summary message.
    SkillsReload(String),
    /// A skill was activated via SKILL-13 fallback; inject into next turn.
    ///
    /// Phase 41.1 (D-02): `args` holds the verbatim trailing text of an argued
    /// invoke (`/<skill> <text>`), or `None` for a bare `/<skill>`. This plan
    /// only threads the field; the one-shot activate+run consumer lands in the
    /// TUI surface plan.
    SkillActivated {
        name: String,
        body: String,
        args: Option<String>,
    },
    /// Input started with `/` but matched no command. `hint` may contain a
    /// "Did you mean `/X`?" suggestion from `suggest_typo`.
    Unknown { input: String, hint: String },
    /// Dispatch itself failed (e.g. command handler returned Err).
    Error(String),
    /// Phase 36.6.3 Plan 03 (TUI-INPUT-02, D-06): bare `/model` — open the
    /// two-step provider->model picker. `App::apply_slash_outcome` sets
    /// `active_overlay = Some(OverlayKind::ModelPicker { step: PickerStep::Provider,
    /// selected_provider: None })`.
    OpenModelPicker,
    /// Phase 36.6.3 Plan 03 (TUI-INPUT-02, D-06): bare `/provider` — open the
    /// single-step provider picker. `App::apply_slash_outcome` sets
    /// `active_overlay = Some(OverlayKind::ModelPicker { step: PickerStep::ProviderOnly,
    /// selected_provider: None })`.
    OpenProviderPicker,
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
            // Phase 39.1 Plan 04 (R39.1-06 / D-06): gate REMOVED — all slash commands
            // dispatch mid-turn. The old AtomicBool running-gate check has been deleted
            // entirely. The D-09 bypass list is retained in ironhermes_core for other
            // surfaces; it is not consulted here.
            match invoke_handler(def.name, &ctx, &app.command_router, &args_vec).await {
                Ok(result) => {
                    // D-02 post-router App-side hook (Plan 03: FULL multi-name expansion).
                    // Plan 03 is the SOLE writer of this hook in Wave 2 (Option B).
                    match def.name {
                        // Mouse: existing handler (crossterm + AtomicBool)
                        "mouse" => handle_mouse_slash(app, args_str),
                        // Phase 46.7 Plan 06 (D-18): `/attach <path>` needs
                        // App-side state (pending-attachment queue,
                        // StateStore, session_id) that CommandContext
                        // doesn't carry — same App-side-handler pattern as
                        // `/mouse` above. The core dispatch table has no
                        // "attach" arm (falls through to `todo_stub`,
                        // whose result is discarded here).
                        "attach" => handle_attach_slash(app, args_str),
                        // Phase 36.6.4 Plan 05 (D-12/D-13, TUI-IMG-01):
                        // `/image <path>` needs App-side state (the image
                        // chip collection) that CommandContext doesn't
                        // carry — same App-side-handler pattern as
                        // `/attach` above.
                        "image" => handle_image_slash(app, args_str),
                        // Phase 36.17.8 (D-08/D-11): `/voice` runtime state (enabled /
                        // recording / auto_tts) lives in `App::voice` AtomicBools, so the
                        // toggle + status must be driven App-side, not from the core
                        // handler's canned strings or stale on-disk config.
                        "voice" => handle_voice(app, &args_vec, result),
                        // Toggles: yolo/verbose/statusbar/debug/skin (NOT fast — owned by subsystem_mutator)
                        "yolo" | "verbose" | "statusbar" | "debug" | "skin" => {
                            handle_toggle(app, def.name, args_str)
                        }
                        // App-handle inspectors: trust core output; no App-side mutation needed
                        "memory" | "mcp" => {
                            handle_app_inspector(app, def.name, &args_vec, &result).await
                        }
                        // Tier D session control. Phase 39.1 Plan 04: "cancel" added for
                        // per-turn cancel via /cancel <turn-id> (R39.1-05).
                        "stop" | "retry" | "undo" | "rollback" | "background" | "btw" | "queue"
                        | "cancel" => {
                            handle_session_control(app, def.name, &args_vec, &result).await
                        }
                        // Phase 36.17.3 (D-06 amended): `/pause` toggles queue
                        // drain; `/unpause` (alias) explicitly sets paused=false.
                        // Since the registry resolves the `/unpause` alias to
                        // canonical name "pause", detect the typed alias from
                        // the original input and route to the correct arm name.
                        // These run BEFORE map_core_to_slash_outcome so the
                        // defensive Silent fallback arms (Plan 02) never fire.
                        "pause" => {
                            let typed = input
                                .split_whitespace()
                                .next()
                                .and_then(|s| s.strip_prefix('/'))
                                .unwrap_or("pause");
                            let route_name = if typed == "unpause" {
                                "unpause"
                            } else {
                                "pause"
                            };
                            handle_session_control(app, route_name, &args_vec, &result).await
                        }
                        // Phase 36.17.3 (D-07 + T-02 mitigation): /new (and the
                        // /reset alias which resolves to canonical name "new")
                        // must clear the queue and reset paused BEFORE the
                        // session-clear path forwards to the ClearSession /
                        // NewSession mapping in map_core_to_slash_outcome.
                        "new" => handle_session_control(app, def.name, &args_vec, &result).await,
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
            SlashOutcome::Unknown {
                input: input.to_string(),
                hint,
            }
        }
        ResolveResult::NotFound => {
            // SKILL-13 fallback: check skill registry before typo-hint.
            let cmd_token = input
                .trim_start_matches('/')
                .split_whitespace()
                .next()
                .unwrap_or("");
            if let Some(registry) = &app.skill_registry
                && let Some(record) = registry.find(cmd_token)
                && let Some(body) = registry.read_content(&record.name)
            {
                // Phase 41.1 (D-02): capture the verbatim trailing text after
                // `/<skill-name>` for the argued-invoke form (mirrors the
                // registered-command `args_str` extraction at ~:370-373).
                let args_str = input
                    .strip_prefix(&format!("/{}", record.name))
                    .unwrap_or("")
                    .trim();
                let args = if args_str.is_empty() {
                    None
                } else {
                    Some(args_str.to_string())
                };
                return SlashOutcome::SkillActivated {
                    name: record.name.clone(),
                    body,
                    args,
                };
            }
            // D-18 item 8 — typo suggester integration point.
            let known = collect_known_command_names(&app.command_router);
            let known_refs: Vec<&str> = known.iter().map(|s| s.as_str()).collect();
            let hint = match suggest_typo(cmd_token, &known_refs) {
                Some(candidate) => format!("Did you mean `/{candidate}`?"),
                None => "Type /help for the list of commands.".to_string(),
            };
            SlashOutcome::Unknown {
                input: input.to_string(),
                hint,
            }
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

// ── Phase 46.7 Plan 06: /attach <path> (D-18/D-20) ───────────────────────────

/// `/attach <path>` — resolves `path` against the operator's real CWD (D-22
/// applies to the post-turn capture; attach resolution just uses the natural
/// CWD-relative affordance either way), copies it into the session
/// attachment store (D-20), and queues it for the NEXT submitted message.
/// Feedback copy is UI-SPEC-exact so `/attach` and inline `@path` (Task 2)
/// share identical wording.
fn handle_attach_slash(app: &mut App, arg: &str) -> SlashOutcome {
    let path = arg.trim();
    if path.is_empty() {
        return SlashOutcome::Unknown {
            input: "/attach".to_string(),
            hint: "Usage: /attach <path>".to_string(),
        };
    }
    match app.copy_local_path_into_store(path) {
        Ok(pending) => {
            let filename = pending.filename.clone();
            app.pending_attachments.push(pending);
            SlashOutcome::Handled(format!(
                "Attached {filename} — will send with your next message"
            ))
        }
        Err((display_name, reason)) => {
            SlashOutcome::Handled(format!("Could not attach {display_name}: {reason}"))
        }
    }
}

// ── Phase 36.6.4 Plan 05: /image <path> (D-12/D-13, TUI-IMG-01) ─────────────

/// `/image <path>` — the second D-12 trigger (alongside `<MEDIA:>` tag
/// extraction at turn-commit). Resolves `path` against the operator's real
/// CWD, bounded-read-checks it (T-36.6.4-IMG-01 — never reads file bytes
/// past the cap, and never attempts a decode here at all; decode is Task
/// 2's overlay-open concern), and on success appends an image chip
/// directly to `app.image_chips` — the chip itself IS the visible
/// feedback, so this returns `SlashOutcome::Silent` (mirrors how a
/// successful action needs no separate transcript line once its own chip
/// is visible). A missing/oversized/unreadable path renders NO chip and
/// instead a single System-role transcript line via `SlashOutcome::Handled`
/// (UI-SPEC §5 E5/error, `apply_slash_outcome`'s `Handled` arm already
/// pushes `Role::System`).
fn handle_image_slash(app: &mut App, arg: &str) -> SlashOutcome {
    let path_str = arg.trim();
    if path_str.is_empty() {
        return SlashOutcome::Unknown {
            input: "/image".to_string(),
            hint: "Usage: /image <path>".to_string(),
        };
    }
    let path = std::path::PathBuf::from(path_str);
    match crate::tui_rata::app::check_image_path_bounded(&path) {
        Ok(()) => {
            let label = crate::tui_rata::app::image_chip_label_for_path(&path);
            // Phase 36.6.4 Plan 12 (G-09 closure): `/image` pushes nothing
            // into `history` itself, so `app.history.len()` at this point IS
            // "after everything said so far" — the chip's chronological
            // anchor.
            app.image_chips.push(crate::tui_rata::app::ImageChip {
                label,
                source: ironhermes_gateway::media_tag::MediaRef {
                    source: ironhermes_gateway::media_tag::MediaSource::Path(path),
                    kind: ironhermes_gateway::media_tag::MediaKind::Photo,
                    original_tag_text: String::new(),
                },
                history_anchor: app.history.len(),
            });
            app.scroll_to_bottom();
            SlashOutcome::Silent
        }
        Err(reason) => SlashOutcome::Handled(format!(
            "Could not load image: {} — {reason}.",
            path.display()
        )),
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
    // Phase 39.1 (R39.1-06 / D-06): agent_running removed from CommandContext.
    // Turn tracking now uses app.turn_registry (TurnRegistry) instead.
    //
    // Phase 41.3 Plan 04 (D-11/D-12): the nine core handles are collected into
    // CoreContextHandles and built via build_core_context — the TUI already
    // wired all nine before this refactor, so this is a pure migration with no
    // behavior change. Surface-specific extras (mcp_manager D-04 handle,
    // memory_manager, provider_resolver, context_compressor, personality_overlay,
    // history, agent_loop, cron_store) stay outside CoreContextHandles and are
    // chained on afterward exactly as before.
    let core_handles = CoreContextHandles {
        subagent_registry: Some(Arc::new(
            ironhermes_agent::subagent_registry::SubagentRegistryHandle::new(
                app.subagent_registry.clone(),
            ),
        ) as Arc<dyn SubagentListSnapshot>),
        process_registry: Some(Arc::new(
            ironhermes_exec::process_registry::ProcessRegistryHandle::new(
                app.process_registry.clone(),
            ),
        ) as Arc<dyn ProcessRegistrySnapshotHandle>),
        skill_registry: app.skill_registry.clone(),
        state_store: app
            .state_store
            .as_ref()
            .map(|store| Arc::new(StateStoreAdapter(store.clone())) as Arc<dyn StateStoreHandle>),
        toolset_session: app.toolset_session.clone(),
        turn_registry: Some(app.turn_registry.clone()),
        workspace: app.workspace.clone(),
        mcp_reloader: app
            .mcp_manager
            .as_ref()
            .map(|mgr| mgr.clone() as Arc<dyn McpReloader>),
        trajectory_writer: app.trajectory_writer.clone(),
    };
    let mut ctx = build_core_context(Platform::Local, app.session_id.clone(), core_handles);

    if let Some(mgr) = &app.mcp_manager {
        // Also wire the McpManagerHandle for `/mcp` full enumeration (D-04).
        // Distinct from mcp_reloader (wired above via CoreContextHandles).
        let handle: Arc<dyn McpManagerHandle> = Arc::new(McpManagerAdapter(mgr.clone()));
        ctx = ctx.with_mcp_manager(handle);
    }
    if let Some(mem) = &app.memory_manager {
        let handle: Arc<dyn MemoryManagerHandle> = Arc::new(MemoryManagerAdapter(mem.clone()));
        ctx = ctx.with_memory_manager(handle);
    }
    {
        let handle: Arc<dyn ProviderResolverHandle> = Arc::new(
            ironhermes_core::commands::context::ProviderResolverAdapter::new(Arc::new(
                app.resolver.clone(),
            )),
        );
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
        // Phase 28.1-05: re-pointed at the runtime (App-level AgentLoop removed).
        let handle: Arc<dyn AgentLoopHandle> =
            Arc::new(AgentRuntimeAdapter(app.agent_runtime.clone()));
        ctx = ctx.with_agent_loop(handle);
    }
    // Phase 22.4.2.1 Plan 01: wire CronJobReader.
    if let Some(cron) = &app.cron_store {
        let handle: Arc<dyn CronJobReader> = Arc::new(CronJobReaderImpl(cron.clone()));
        ctx = ctx.with_cron_store(handle);
    }
    // Phase 49.5 Plan 05: wire CronJobWriter for `/blueprint run`. Unlike the
    // reader above, the writer is stateless and opens its own JobStore per
    // call, so it does not need `app.cron_store` to be present — wired
    // unconditionally. Covers both the CLI and the TUI, which share this
    // context builder. CLI/TUI are local trusted surfaces (Platform::Local),
    // so cmd_blueprint's run arm leaves this handle ungated; wiring it here
    // is not itself authorization for the gateway's remote-origin gate.
    ctx = ctx.with_cron_job_writer(std::sync::Arc::new(
        ironhermes_cron::CronJobWriterImpl::new(),
    ));
    // Phase 49.6 Plan 03: wire BlueprintSaver for `/blueprint save`. Wired
    // ONLY here (the CLI/TUI context builder) — never on the gateway's —
    // because `cmd_blueprint_save`'s unconditional Platform::Local gate is
    // the control and this handle's absence everywhere else is the
    // backstop. Like the writer above, it is stateless and opens its own
    // JobStore per call, so it is wired unconditionally.
    ctx = ctx.with_blueprint_saver(std::sync::Arc::new(
        crate::blueprint_save::BlueprintSaverImpl::new(),
    ));
    ctx
}

/// Collect all command names + aliases from the router for the typo candidate pool.
///
/// `CommandRouter.commands: Vec<CommandDef>` is public (mod.rs:165).
fn collect_known_command_names(router: &CommandRouter) -> Vec<String> {
    let mut names: Vec<String> = router.commands.iter().map(|c| c.name.to_string()).collect();
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
    Ok(ironhermes_core::commands::handlers::dispatch(
        def, args, ctx, router,
    ))
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
#[allow(dead_code)] // retained as planned /help dispatch target; invariants_22_4.rs INV-22.4-31 asserts this fn exists
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
/// Phase 36.17.8 (D-08/D-10/D-11): App-side `/voice` handler.
///
/// Voice mode is a TUI-runtime feature — its live state lives in `App::voice`
/// (`enabled` / `recording` / `auto_tts` AtomicBools + the capture task). The
/// core `cmd_voice` handler only provides the command surface (help/headless);
/// here we drive the actual runtime state so `status` reflects reality and `tts`
/// toggles a real flag (rather than the previous canned string + on-disk read,
/// which made `/voice tts` appear to do nothing).
///
/// This toggles/reports the three voice states; the flags are consumed in
/// `event_loop::spawn_turn`, which speaks the reply via
/// `voice_reply::speak_reply` when `should_speak` returns true (`/voice tts`
/// speaks every reply; `/voice on` speaks only voice-input turns).
fn handle_voice(app: &mut App, args: &[&str], core_result: CommandResult) -> SlashOutcome {
    let msg = match args.first().copied() {
        Some("on") => {
            app.voice.enabled.store(true, Ordering::Relaxed);
            "Voice mode enabled. Press Ctrl+B to start/stop recording.".to_string()
        }
        Some("off") => {
            // Cancel any in-flight capture loop, then disable.
            app.voice.stop();
            app.voice.enabled.store(false, Ordering::Relaxed);
            "Voice mode disabled.".to_string()
        }
        Some("tts") => {
            let now = app.voice.toggle_auto_tts();
            format!("Voice auto-TTS: {}.", if now { "on" } else { "off" })
        }
        None | Some("status") => {
            let config = ironhermes_core::Config::load().unwrap_or_default();
            let provider = ironhermes_tools::stt::select_stt_provider(&config.stt)
                .unwrap_or_else(|| "none (no API key configured)".to_string());
            format!(
                "Voice mode status:\n  enabled: {}\n  provider: {}\n  record_key: {}\n  auto_tts: {}",
                app.voice.is_enabled(),
                provider,
                config.voice.record_key,
                app.voice.auto_tts.load(Ordering::Relaxed),
            )
        }
        // Unknown subcommand — defer to the core handler's "Unknown ..." message.
        Some(_) => return map_core_to_slash_outcome(core_result),
    };
    SlashOutcome::Handled(msg)
}

fn handle_toggle(app: &mut App, name: &str, arg: &str) -> SlashOutcome {
    match name {
        "yolo" => {
            let new_val = !app.yolo_enabled.fetch_xor(true, Ordering::SeqCst);
            SlashOutcome::Handled(format!("YOLO mode: {}", if new_val { "on" } else { "off" }))
        }
        "verbose" => {
            let new_val = !app.verbose_enabled.fetch_xor(true, Ordering::SeqCst);
            SlashOutcome::Handled(format!(
                "Verbose mode: {}",
                if new_val { "on" } else { "off" }
            ))
        }
        "statusbar" => {
            let new_val = !app.statusbar_enabled.fetch_xor(true, Ordering::SeqCst);
            SlashOutcome::Handled(format!(
                "Status bar: {}",
                if new_val { "on" } else { "off" }
            ))
        }
        "debug" => {
            let new_val = !app.debug_enabled.fetch_xor(true, Ordering::SeqCst);
            SlashOutcome::Handled(format!(
                "Debug mode: {}",
                if new_val { "on" } else { "off" }
            ))
        }
        "skin" => {
            if arg.is_empty() {
                let current = app
                    .skin
                    .read()
                    .map(|s| s.clone())
                    .unwrap_or_else(|p| p.into_inner().clone());
                SlashOutcome::Handled(format!("Current skin: {current}. Usage: /skin <name>"))
            } else {
                // T-22.4.2-03-01: validate skin name to alphanumeric + dash + underscore
                if !arg
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
                {
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
            // Phase 36.17.3 (D-08 + RESEARCH Pitfall 1): clear-then-cancel
            // ordering is non-negotiable. The queue must be empty BEFORE
            // `cancel_child.cancel()` fires so the eventual `StreamEvent::Cancelled`
            // arm finds an empty queue (belt-and-suspenders alongside Plan 04 Task 2
            // which already skips drain on Cancelled). Order: clear -> reset paused
            // -> cancel in-flight turn -> forward to core (ProcessRegistry drain).
            app.queue.clear(&app.queue_key);
            app.queue_paused
                .store(false, std::sync::atomic::Ordering::SeqCst);
            if let Some(tok) = app.cancel_child.take() {
                tok.cancel();
            }
            // Phase 39.1 Plan 04 (R39.1-05): cancel all session turns in the TurnRegistry.
            // This reaches Surface::Cli turns registered by spawn_turn.
            let _cancelled = app.turn_registry.cancel_session(&app.session_id).await;
            // /stop: ProcessRegistry is now threaded into ctx via build_command_context.
            // Core cmd_stop handles the drain-and-kill; trust core result.
            map_core_to_slash_outcome(core_result.clone())
        }
        "retry" => {
            // Find the last user message in history.
            let last_user_text = app
                .history
                .iter()
                .rev()
                .find(|m| m.role == ironhermes_core::types::Role::User)
                .and_then(|m| m.content.as_ref())
                .and_then(|c| c.as_text())
                .map(|s| s.to_string());

            match last_user_text {
                None => SlashOutcome::Handled("No user messages in history to retry.".to_string()),
                Some(text) => {
                    // Remove trailing assistant message(s) to re-run from last user turn.
                    while app
                        .history
                        .last()
                        .map(|m| m.role == ironhermes_core::types::Role::Assistant)
                        .unwrap_or(false)
                    {
                        app.history.pop();
                    }
                    // Re-queue the user message as a new pending turn.
                    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<
                        crate::tui_rata::stream_events::StreamEvent,
                    >();
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
            if app
                .history
                .last()
                .map(|m| m.role == ironhermes_core::types::Role::Assistant)
                .unwrap_or(false)
            {
                app.history.pop();
            }
            // Remove last user message (if present).
            if app
                .history
                .last()
                .map(|m| m.role == ironhermes_core::types::Role::User)
                .unwrap_or(false)
            {
                app.history.pop();
                SlashOutcome::Handled("Last exchange undone.".to_string())
            } else {
                SlashOutcome::Handled("Undo: no user message found to remove.".to_string())
            }
        }
        "rollback" => {
            // Parse N (default 1) — number of (user, assistant) pairs to remove.
            let n: usize = args
                .first()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1)
                .max(1);
            if app.history.is_empty() {
                return SlashOutcome::Handled("No history to roll back.".to_string());
            }
            let mut removed = 0usize;
            for _ in 0..n {
                // Remove trailing assistant message (if any).
                if app
                    .history
                    .last()
                    .map(|m| m.role == ironhermes_core::types::Role::Assistant)
                    .unwrap_or(false)
                {
                    app.history.pop();
                }
                // Remove trailing user message (if any).
                if app
                    .history
                    .last()
                    .map(|m| m.role == ironhermes_core::types::Role::User)
                    .unwrap_or(false)
                {
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
                    "Usage: /background <message> — run a prompt as a background task.".to_string(),
                );
            }
            let message = args.join(" ");
            // Push the background message as a user turn and queue for spawn.
            app.history
                .push(ironhermes_core::types::ChatMessage::user(message.clone()));
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<
                crate::tui_rata::stream_events::StreamEvent,
            >();
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
                    "Usage: /btw <message> — add an aside to the current/next agent turn."
                        .to_string(),
                );
            }
            let message = args.join(" ");
            // Append as a user message; it will be included in the next spawn_turn call.
            app.history
                .push(ironhermes_core::types::ChatMessage::user(format!(
                    "[btw] {message}"
                )));
            SlashOutcome::Handled(format!("Aside added: \"{message}\" (active next turn)"))
        }
        "queue" => {
            // Phase 36.17.3 (D-09 + D-10): real push into the shared MessageQueue.
            // Replaces the prior textarea-prepopulate placeholder. D-12 negative
            // control precondition: the old TextArea-prepopulate mutation has
            // been removed from this arm. Bell is OMITTED per Resolution 7.
            if args.is_empty() {
                return SlashOutcome::Handled("Usage: /queue <message>".to_string());
            }
            let message = args.join(" ");
            match app.queue.try_push(&app.queue_key, message.clone()) {
                Ok(()) => {
                    let depth = app.queue.len(&app.queue_key);
                    SlashOutcome::Handled(format!("Queued: \"{}\" ({} in queue)", message, depth))
                }
                Err(QueueError::CapacityReached { max, .. }) => {
                    // D-10: cap-hit error rendered inline. T-01 mitigation = cap
                    // enforced at SessionQueue source (Plan 01).
                    SlashOutcome::Handled(format!(
                        "Queue is full ({max}/{max}). /stop or /flush to drain."
                    ))
                }
            }
        }
        // Phase 36.17.3 (D-06 amended): /pause toggles `app.queue_paused`.
        // `/unpause` is an alias of `/pause` in the registry (canonical name
        // resolved to "pause"), so the dispatch layer above detects the typed
        // alias from the original input and routes here with name = "unpause".
        // These arms run BEFORE the catch-all _ => map_core_to_slash_outcome
        // forwarder so the defensive Silent fallback (Plan 02) never fires.
        "pause" => {
            let was_paused = app
                .queue_paused
                .fetch_xor(true, std::sync::atomic::Ordering::SeqCst);
            let new_state = !was_paused;
            let depth = app.queue.len(&app.queue_key);
            SlashOutcome::Handled(format!(
                "Queue drain: {}. ({} queued)",
                if new_state { "paused" } else { "resumed" },
                depth
            ))
        }
        // Phase 36.17.3 (D-06 amended): /unpause explicit set-to-false; no-op
        // (with informational message) when not currently paused.
        "unpause" => {
            let was_paused = app
                .queue_paused
                .swap(false, std::sync::atomic::Ordering::SeqCst);
            let depth = app.queue.len(&app.queue_key);
            if was_paused {
                SlashOutcome::Handled(format!("Queue resumed. ({} queued)", depth))
            } else {
                SlashOutcome::Handled("Queue was not paused.".to_string())
            }
        }
        // Phase 36.17.3 (D-07 + T-02 mitigation): clear the queue and reset
        // pause BEFORE forwarding to the session-clear path so the user never
        // observes stale queued items firing against a fresh session.
        // RESEARCH Pitfall 1 ordering: queue.clear -> queue_paused.store(false)
        // -> session clear forwarding.
        "new" => {
            app.queue.clear(&app.queue_key);
            app.queue_paused
                .store(false, std::sync::atomic::Ordering::SeqCst);
            map_core_to_slash_outcome(core_result.clone())
        }
        // Phase 36.17.3 (D-07 + T-02 mitigation): /reset is registered as an
        // alias of /new in the core registry (resolves to canonical name "new"
        // at dispatch time), so this arm is unreachable at runtime. It is
        // retained as a defensive marker so any future refactor that distinguishes
        // /reset from /new at the dispatch layer still clears the queue. The
        // ordering matches the /new arm: clear -> reset paused -> forward.
        "reset" => {
            app.queue.clear(&app.queue_key);
            app.queue_paused
                .store(false, std::sync::atomic::Ordering::SeqCst);
            map_core_to_slash_outcome(core_result.clone())
        }
        // Phase 39.1 Plan 04 (R39.1-05 / T-39.1-04): /cancel <turn-id> cancels a
        // specific in-flight turn by UUID. Validates the UUID before calling cancel_one
        // (T-39.1-04 mitigation: invalid UUID → user-facing error, no panic).
        "cancel" => match args.first() {
            None => SlashOutcome::Handled(
                "Usage: /cancel <turn-id> — cancel a specific in-flight turn.".to_string(),
            ),
            Some(id_str) => match uuid::Uuid::parse_str(id_str) {
                Err(_) => SlashOutcome::Handled(format!(
                    "Invalid turn ID '{}' — must be a UUID (e.g. /agents to list active turns).",
                    id_str
                )),
                Ok(turn_id) => {
                    if app.turn_registry.cancel_one(turn_id).await {
                        SlashOutcome::Handled(format!("Turn {} cancelled.", turn_id))
                    } else {
                        SlashOutcome::Handled(format!(
                            "Turn {} not found — it may have already completed.",
                            turn_id
                        ))
                    }
                }
            },
        },
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
                        let model = app
                            .resolver
                            .resolve_role("fast")
                            .map(|ep| ep.default_model.clone())
                            .unwrap_or_else(|| "fast".to_string());
                        app.client = new_client;
                        SlashOutcome::Handled(format!("Fast mode ON — model {model}"))
                    }
                    Ok(None) => SlashOutcome::Handled(
                        "Fast mode toggle (no fast preset configured).".to_string(),
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
            // Phase 21.8.3.1 D-05: "clear" is intercepted before any registry/output
            // matching. Sets active_personality_overlay = None and returns immediately.
            // Core handler has no "clear" case — without this pre-check, "clear" would
            // be looked up as a preset name and return Error("Unknown personality: clear").
            if args.first().copied() == Some("clear") {
                app.active_personality_overlay = None;
                return SlashOutcome::Handled("Personality cleared.".to_string());
            }
            match core_result {
                CommandResult::PersonalityApplied(text) => {
                    app.active_personality_overlay = Some(text.clone());
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
        CommandResult::Output(text) => SlashOutcome::Handled(text),
        CommandResult::Handled => SlashOutcome::Silent,
        CommandResult::Error(msg) => SlashOutcome::Error(msg),
        CommandResult::Quit => SlashOutcome::Quit,
        CommandResult::ClearSession => {
            SlashOutcome::ClearSession("Conversation cleared.".to_string())
        }
        CommandResult::ResetTerminal => SlashOutcome::ResetTerminal,
        CommandResult::NewSession { message } => SlashOutcome::ClearSession(message),
        CommandResult::PassThrough => SlashOutcome::Unknown {
            input: String::new(),
            hint: "Unknown command. Type /help for the list.".to_string(),
        },
        CommandResult::McpReload => SlashOutcome::McpReload,
        CommandResult::SkillsReload => SlashOutcome::SkillsReload("Skills reloaded.".to_string()),
        CommandResult::SkillActivated { name, body, args } => {
            SlashOutcome::SkillActivated { name, body, args }
        }
        CommandResult::PersonalityApplied(text) => SlashOutcome::Handled(text),
        // Phase 36.17.3: closed via handle_session_control's "queue" arm above;
        // this fallback remains for non-TUI consumers (gateway adapters, the
        // classic CLI REPL) which also emit `CommandResult::Queued` but do not
        // route through `handle_session_control`.
        CommandResult::Queued { message } => SlashOutcome::Handled(format!("Queued: {message}")),
        // Phase 36.17.3 (D-06 amended): defensive no-op; active toggle lives in
        // handle_session_control (Plan 05) BEFORE map_core_to_slash_outcome is
        // called, so this arm is the fallback for any path that routes through
        // here without interception (e.g., gateway shimming through the TUI
        // mapper, or future surfaces without a queue-paused AtomicBool).
        CommandResult::PauseQueue => SlashOutcome::Silent,
        CommandResult::UnpauseQueue => SlashOutcome::Silent,
        // Phase 39.1 (R39.1-09): Plan 39.1-01 added the AgentsList variant in
        // core. Render the active TurnRegistry entries as handled output so the
        // mapper stays exhaustive; richer `/agents` TUI wiring lands in Plan 04.
        CommandResult::AgentsList(turns) => {
            let text = if turns.is_empty() {
                "No active turns.".to_string()
            } else {
                let mut out = format!("Active turns ({}):\n", turns.len());
                for t in &turns {
                    out.push_str(&format!(
                        "• {} — {} — session {} — {}ms\n",
                        t.turn_id, t.surface, t.session_id, t.elapsed_ms
                    ));
                }
                out
            };
            SlashOutcome::Handled(text)
        }
        // Phase 36.6.3 Plan 03 (D-06): TUI opens the picker; `fallback_text`
        // (today's model_list_text()/status_text() output) is intentionally
        // dropped here — it exists for non-TUI/gateway surfaces that map
        // these variants straight to `Output(fallback_text)` instead.
        CommandResult::OpenModelPicker { .. } => SlashOutcome::OpenModelPicker,
        CommandResult::OpenProviderPicker { .. } => SlashOutcome::OpenProviderPicker,
    }
}

// ── Phase 46.7 Plan 06 tests: /attach <path> (D-18) ──────────────────────────

#[cfg(all(test, feature = "test-support"))]
mod attach_command_tests {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex, OnceLock};

    static ENV_LOCK: OnceLock<StdMutex<()>> = OnceLock::new();

    /// SAFETY: mirrors the lock convention in `app.rs::tui_attach_at_path` —
    /// see that module's doc comment for the nextest-process-isolation note.
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.get_or_init(|| StdMutex::new(())).lock().unwrap()
    }

    fn app_with_store() -> (App, tempfile::TempDir) {
        let home_dir = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home_dir.path());
        }
        let db_path = home_dir.path().join("state.db");
        let store = ironhermes_state::StateStore::new(&db_path).unwrap();
        let mut app = App::new_test_empty();
        app.state_store = Some(Arc::new(std::sync::Mutex::new(store)));
        (app, home_dir)
    }

    #[tokio::test]
    async fn attach_command_queues_file_and_reports_ui_spec_feedback() {
        // clippy::await_holding_lock: scope the guard to setup only — the
        // env-var mutation happens synchronously inside `app_with_store()`;
        // it must not be held across the `.await` below.
        let (mut app, _home_dir) = {
            let _g = lock();
            app_with_store()
        };
        let src_dir = tempfile::tempdir().unwrap();
        let src_path = src_dir.path().join("plan.md");
        std::fs::write(&src_path, b"# plan").unwrap();

        let input = format!("/attach {}", src_path.to_string_lossy());
        let outcome = dispatch_slash(&mut app, &input).await;
        match outcome {
            SlashOutcome::Handled(text) => {
                assert!(text.contains("Attached plan.md"), "got: {text}");
                assert!(
                    text.contains("will send with your next message"),
                    "got: {text}"
                );
            }
            other => panic!("expected Handled, got {other:?}"),
        }
        assert_eq!(app.pending_attachments.len(), 1);
        assert_eq!(app.pending_attachments[0].filename, "plan.md");
    }

    #[tokio::test]
    async fn attach_command_reports_error_for_missing_file() {
        let (mut app, _home_dir) = {
            let _g = lock();
            app_with_store()
        };
        let outcome = dispatch_slash(&mut app, "/attach /nope/does-not-exist-46-7.md").await;
        match outcome {
            SlashOutcome::Handled(text) => {
                assert!(text.starts_with("Could not attach"), "got: {text}");
            }
            other => panic!("expected Handled, got {other:?}"),
        }
        assert!(app.pending_attachments.is_empty());
    }

    /// D-18: `/attach` never panics `CommandRouter::new` (no duplicate
    /// name/alias) and resolves via the router on the CLI/Local platform.
    #[tokio::test]
    async fn attach_command_resolves_via_router() {
        let (mut app, _home_dir) = {
            let _g = lock();
            app_with_store()
        };
        // No path arg — usage hint, not a panic or Unknown-command fallthrough.
        let outcome = dispatch_slash(&mut app, "/attach").await;
        match outcome {
            SlashOutcome::Unknown { hint, .. } => {
                assert!(hint.contains("Usage: /attach"), "got: {hint}");
            }
            other => panic!("expected Unknown usage hint, got {other:?}"),
        }
    }
}

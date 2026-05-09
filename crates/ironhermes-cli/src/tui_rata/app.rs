//! Central App state for the tui_rata REPL (Phase 22.4).
//!
//! Structural template: tmon/src/main.rs App struct + scroll helpers.
//! IronHermes additions for the D-18 14-item parity list.
//!
//! # Design notes
//! - `hint` in `StatusLineState` is a `String`; empty = no hint shown.
//! - TextArea import uses `tui_textarea_2` (workspace alias for tui-textarea-2 0.10.2).
//! - `dispatch_slash` is a stub in `commands.rs`; plan 22.4-07 Task 4 fills it.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use tui_textarea::TextArea;

use crate::tui_rata::double_ctrl_c::{CtrlCDecision, DoubleCtrlCState};
use crate::tui_rata::history::{DEFAULT_MAX, ReplHistory};
use crate::tui_rata::status_line::StatusLineState;
use crate::tui_rata::stream_events::StreamEvent;

// Concrete paths — grep-verified iteration 2.
use ironhermes_agent::AnyClient;
use ironhermes_agent::agent_loop::AgentLoop;
use ironhermes_agent::budget::BudgetHandle;
use ironhermes_agent::context_engine::ContextEngine;
use ironhermes_agent::memory::MemoryManager;
use ironhermes_agent::personality::PersonalityRegistry;
use ironhermes_agent::subagent_registry::SubagentRegistry;
use ironhermes_core::ProviderResolver;
use ironhermes_core::commands::CommandRouter;
use ironhermes_core::commands::context::ToolsetSessionHandle;
use ironhermes_core::types::{ChatMessage, MessageContent, Role};
use ironhermes_exec::process_registry::ProcessRegistry;
use ironhermes_hooks::HookRegistry;
use ironhermes_mcp::McpManager;
use ironhermes_state::StateStore;
use ironhermes_tools::ToolRegistry;

// ── AppDeps ───────────────────────────────────────────────────────────────────

/// Dependency bundle passed into `App::new`.
///
/// Keeps the constructor signature stable as the parity list grows.
/// Plan 22.4-07 constructs this in the event-loop bootstrap.
pub struct AppDeps {
    pub agent_loop: Arc<AgentLoop>,
    pub hook_registry: Arc<HookRegistry>,
    pub mcp_manager: Option<Arc<McpManager>>,
    pub memory_manager: Option<Arc<Mutex<MemoryManager>>>,
    pub subagent_registry: Arc<RwLock<SubagentRegistry>>,
    pub process_registry: Arc<RwLock<ProcessRegistry>>,
    pub command_router: Arc<CommandRouter>,
    pub session_id: String,
    pub history_path: PathBuf,
    pub status_initial: StatusLineState,
    pub cancel_parent: CancellationToken,
    // Plan 22.4-07 additions: needed by spawn_turn to build per-turn AgentLoops
    pub client: AnyClient,
    pub registry: Arc<RwLock<ToolRegistry>>,
    pub budget: BudgetHandle,
    pub context_length: usize,
    pub config_compression: f64,
    pub max_turns: usize,
    /// UAT Gap 2 (Phase 22.4 Plan 22.4-15) — pre-resolved fallback client per
    /// PROV-07 parity with classic main.rs:631-637. spawn_turn clones this and
    /// chains `.with_fallback(fb)` on the per-turn AgentLoop when present.
    pub fallback_client: Option<AnyClient>,
    /// Phase 25.1 GAP-8 closure (plan 25.1-19): shared browser session Arc.
    /// Mirrors `run_chat` (main.rs:1173-1176): one Arc per AgentLoop instance,
    /// lazy-spawned on first browser_* call (D-03), cloned into the App-level
    /// AgentLoop builder AND the per-turn AgentLoop in `spawn_turn`. Without
    /// this field the rata REPL omits all 11 browser_* tools (GAP-8 root cause).
    pub browser_session: std::sync::Arc<
        tokio::sync::Mutex<Option<ironhermes_tools::browser_session::BrowserSession>>,
    >,
    /// UAT Gap 3 (Phase 22.4 Plan 22.4-16) — shared mouse-capture state.
    /// `/mouse on|off` slash command flips this AtomicBool AND executes the
    /// corresponding crossterm command. Initial value `true` matches the
    /// EnableMouseCapture call at run_chat_ratatui startup. The
    /// MouseCaptureGuard Drop impl unconditionally disables on REPL exit.
    pub mouse_capture_enabled: Arc<AtomicBool>,

    // ── Phase 22.4.2 Plan 00: D-08 four subsystem handles ───────────────────
    /// StateStore for `/sessions` `/resume` `/save` `/history` `/title`.
    pub state_store: Option<Arc<std::sync::Mutex<StateStore>>>,
    /// ProviderResolver for `/model` `/provider` `/fast`.
    pub resolver: ProviderResolver,
    /// ContextEngine for `/compress` (Phase 18 PRMT-11).
    pub context_compressor: Option<Arc<dyn ContextEngine>>,
    /// PersonalityRegistry for `/personality` (Phase 15 PRMT-06/PRMT-07).
    pub personality_overlay: Arc<PersonalityRegistry>,

    // ── Phase 22.4.2 Plan 00: D-09 six session-toggle Arc fields ────────────
    /// `/yolo` toggle — upgraded from `bool` to `Arc<AtomicBool>` (D-09).
    /// (Replaces the plain `yolo_enabled: bool` field.)
    pub yolo_enabled: Arc<AtomicBool>,
    /// `/verbose` toggle (D-09).
    pub verbose_enabled: Arc<AtomicBool>,
    /// `/statusbar` toggle — initial value `true` (D-09).
    pub statusbar_enabled: Arc<AtomicBool>,
    /// `/debug` toggle (D-09).
    pub debug_enabled: Arc<AtomicBool>,
    /// `/fast` preset toggle (D-09).
    pub fast_enabled: Arc<AtomicBool>,
    /// `/skin <name>` setter (D-09).
    pub skin: Arc<std::sync::RwLock<String>>,

    /// Phase 25.2 Plan 15 follow-up — production `ToolsetSessionHandle` for the
    /// ratatui REPL's slash dispatch (`/toolset list/show/enable/disable`).
    /// Plan 15 wired the handle in `run_chat`/`run_single`/`run_gateway` but
    /// missed `tui_rata::run_chat_ratatui`, which is the default `hermes chat`
    /// entry since Phase 22.4. Without this field, `build_command_context`
    /// returns a `CommandContext` whose `toolset_session: None` falls through
    /// to the "toolset session handle not configured" guard at
    /// `crates/ironhermes-core/src/commands/handlers.rs:782`.
    pub toolset_session: Option<Arc<dyn ToolsetSessionHandle>>,

    /// Phase 25.3 D-W-2: resolved Workspace for session-scoped project resolution.
    /// `build_app_deps` calls `ironhermes_core::workspace::resolve_from_cwd(&cwd)`
    /// at session start (frozen-snapshot). `build_command_context` attaches via
    /// `.with_workspace(...)` so the slash-dispatch CommandContext sees the root.
    pub workspace: Option<Arc<ironhermes_core::workspace::Workspace>>,
    /// Phase 25.3 D-T-3: TrajectoryWriter handle for per-tool-call JSONL ledger.
    /// `build_app_deps` opens the writer at workspace-scoped or global path and
    /// wraps it in `TrajectoryWriterHandleImpl`. `build_command_context` attaches
    /// via `.with_trajectory_writer(...)`.
    pub trajectory_writer:
        Option<Arc<dyn ironhermes_core::commands::context::TrajectoryWriterHandle>>,

    /// Phase 25.3-13 CR-04: pre-built system message containing the durable
    /// [Workspace: <root>] Identity-slot line. Seeded into App.history at
    /// App::new so the per-turn AgentLoop sees it via messages_snapshot.
    /// Without this seed, the LLM sees no system prompt and [Workspace: <root>]
    /// is invisible on the default `hermes chat` surface.
    pub system_message: Option<ChatMessage>,

    /// Phase 21.8.2: skill registry for `/skills` slash command + SKILL-13 fallback.
    pub skill_registry: Option<Arc<ironhermes_core::SkillRegistry>>,
}

// ── App ───────────────────────────────────────────────────────────────────────

/// Central REPL application state (D-18 14-item parity list + scroll state).
///
/// All fields are `pub` so `ui.rs` (plan 22.4-06) can read them directly
/// without accessor indirection.
pub struct App {
    // — transcript / history ─────────────────────────────────────────────────
    pub history: Vec<ChatMessage>,
    pub textarea: TextArea<'static>,
    pub transcript_scroll: u16,
    pub auto_follow: bool,

    // — streaming bridge ─────────────────────────────────────────────────────
    pub pending_rx: Option<UnboundedReceiver<StreamEvent>>,
    pub pending_tx: Option<UnboundedSender<StreamEvent>>,
    pub assistant_buffer: Option<String>,

    // — lifecycle ────────────────────────────────────────────────────────────
    pub should_quit: bool,
    pub session_id: String,

    // — REPL history persistence ─────────────────────────────────────────────
    pub history_store: ReplHistory,
    pub history_path: PathBuf,

    // — status line ──────────────────────────────────────────────────────────
    pub status: StatusLineState,
    pub knight_rider_tick: u64,

    // — ctrl-c / cancellation ────────────────────────────────────────────────
    pub double_ctrl_c: DoubleCtrlCState,
    pub cancel_parent: CancellationToken,
    pub cancel_child: Option<CancellationToken>,

    // — feature flags (Phase 22.4.2 Plan 00: D-09 upgrades) ─────────────────
    /// `/yolo` toggle — upgraded from `bool` to `Arc<AtomicBool>` (D-09).
    pub yolo_enabled: Arc<AtomicBool>,
    /// `/verbose` toggle (D-09).
    pub verbose_enabled: Arc<AtomicBool>,
    /// `/statusbar` toggle — initial `true` (D-09).
    pub statusbar_enabled: Arc<AtomicBool>,
    /// `/debug` toggle (D-09).
    pub debug_enabled: Arc<AtomicBool>,
    /// `/fast` preset toggle (D-09).
    pub fast_enabled: Arc<AtomicBool>,
    /// `/skin <name>` setter (D-09).
    pub skin: Arc<std::sync::RwLock<String>>,

    // — D-18 parity handles (Arc-held) ───────────────────────────────────────
    pub agent_loop: Arc<AgentLoop>,
    pub hook_registry: Arc<HookRegistry>,
    pub mcp_manager: Option<Arc<McpManager>>,
    pub memory_manager: Option<Arc<Mutex<MemoryManager>>>,
    pub subagent_registry: Arc<RwLock<SubagentRegistry>>,
    pub process_registry: Arc<RwLock<ProcessRegistry>>,
    pub command_router: Arc<CommandRouter>,
    // Plan 22.4-07: spawn_turn needs these to build per-turn AgentLoops
    pub client: AnyClient,
    pub registry: Arc<RwLock<ToolRegistry>>,
    pub budget: BudgetHandle,
    pub context_length: usize,
    pub config_compression: f64,
    pub max_turns: usize,
    /// UAT Gap 2 (Phase 22.4 Plan 22.4-15) — see AppDeps.fallback_client.
    pub fallback_client: Option<AnyClient>,
    /// Phase 25.1 GAP-8 closure (plan 25.1-19): shared browser session Arc.
    /// Mirrors `run_chat` (main.rs:1173-1176): one Arc per AgentLoop instance,
    /// lazy-spawned on first browser_* call (D-03), cloned into the App-level
    /// AgentLoop builder AND the per-turn AgentLoop in `spawn_turn`. Without
    /// this field the rata REPL omits all 11 browser_* tools (GAP-8 root cause).
    pub browser_session: std::sync::Arc<
        tokio::sync::Mutex<Option<ironhermes_tools::browser_session::BrowserSession>>,
    >,
    /// UAT Gap 3 (Phase 22.4 Plan 22.4-16) — see AppDeps.mouse_capture_enabled.
    pub mouse_capture_enabled: Arc<AtomicBool>,

    // ── Phase 22.4.2 Plan 00: D-08 four subsystem handles ───────────────────
    /// StateStore for `/sessions` `/resume` `/save` `/history` `/title`.
    pub state_store: Option<Arc<std::sync::Mutex<StateStore>>>,
    /// ProviderResolver for `/model` `/provider` `/fast`.
    pub resolver: ProviderResolver,
    /// ContextEngine for `/compress` (Phase 18 PRMT-11).
    pub context_compressor: Option<Arc<dyn ContextEngine>>,
    /// PersonalityRegistry for `/personality` (Phase 15 PRMT-06/PRMT-07).
    pub personality_overlay: Arc<PersonalityRegistry>,
    /// Pending personality overlay text to inject as system-prompt on next spawn_turn.
    /// Set by tui_rata post-router hook `handle_subsystem_mutator` on `/personality <name>`.
    /// Consumed (and cleared) by spawn_turn bootstrap (Plan 03 scope: set only; consume deferred).
    pub next_turn_personality_overlay: Option<String>,

    // ── Phase 22.4.2.1 Plan 01: CronJobReader wiring ────────────────────────
    /// JobStore handle for `/cron` slash UI. None by default (deferred runtime
    /// init per D-02 — gateway is the primary cron host; tui_rata field exists
    /// so the wiring path is ready when a future plan loads the store).
    pub cron_store: Option<std::sync::Arc<std::sync::Mutex<ironhermes_cron::JobStore>>>,

    /// Phase 25.2 Plan 15 follow-up — see `AppDeps.toolset_session` doc.
    pub toolset_session: Option<Arc<dyn ToolsetSessionHandle>>,

    /// Phase 25.3 D-W-2: resolved Workspace — see `AppDeps.workspace` doc.
    pub workspace: Option<Arc<ironhermes_core::workspace::Workspace>>,
    /// Phase 25.3 D-T-3: TrajectoryWriter handle — see `AppDeps.trajectory_writer` doc.
    pub trajectory_writer:
        Option<Arc<dyn ironhermes_core::commands::context::TrajectoryWriterHandle>>,

    /// Phase 21.8.2: skill registry for `/skills` slash command + SKILL-13 fallback.
    /// Wired into CommandContext via `build_command_context` in tui_rata/commands.rs.
    pub skill_registry: Option<Arc<ironhermes_core::SkillRegistry>>,
}

impl App {
    /// Construct App from dependency bundle. Loads REPL history from disk;
    /// falls back to empty history on error (missing file is not fatal).
    pub fn new(deps: AppDeps) -> Self {
        let history_store = ReplHistory::load(&deps.history_path, DEFAULT_MAX)
            .unwrap_or_else(|_| ReplHistory::with_default_max());
        let mut textarea = TextArea::default();
        textarea.set_cursor_line_style(Style::default());
        // UAT Gap 1 (Phase 22.4 Plan 22.4-14): bordered "Prompt" block so the
        // input area is visually defined. render_cursor in ui.rs adds +1/+1
        // offsets to account for the top + left borders.
        textarea.set_block(Block::default().borders(Borders::ALL).title("Prompt"));

        // Phase 25.3-13 CR-04: seed the system message into history so the per-turn
        // AgentLoop sees it via messages_snapshot. Without this seed, the LLM sees
        // no system prompt and [Workspace: <root>] is invisible. Subsequent /clear
        // and /reset handlers may clear this; the documented run_chat behavior is
        // that the system message is part of the FIRST session only — post-clear
        // turns use whatever history exists post-clear.
        let mut history: Vec<ChatMessage> = Vec::new();
        if let Some(sys) = deps.system_message {
            history.push(sys);
        }

        Self {
            history,
            textarea,
            transcript_scroll: 0,
            auto_follow: true,
            pending_rx: None,
            pending_tx: None,
            assistant_buffer: None,
            should_quit: false,
            session_id: deps.session_id,
            history_store,
            history_path: deps.history_path,
            status: deps.status_initial,
            knight_rider_tick: 0,
            double_ctrl_c: DoubleCtrlCState::new(),
            cancel_parent: deps.cancel_parent,
            cancel_child: None,
            // Phase 22.4.2 Plan 00: D-09 toggle Arcs (cloned from deps)
            yolo_enabled: deps.yolo_enabled,
            verbose_enabled: deps.verbose_enabled,
            statusbar_enabled: deps.statusbar_enabled,
            debug_enabled: deps.debug_enabled,
            fast_enabled: deps.fast_enabled,
            skin: deps.skin,
            agent_loop: deps.agent_loop,
            hook_registry: deps.hook_registry,
            mcp_manager: deps.mcp_manager,
            memory_manager: deps.memory_manager,
            subagent_registry: deps.subagent_registry,
            process_registry: deps.process_registry,
            command_router: deps.command_router,
            client: deps.client,
            registry: deps.registry,
            budget: deps.budget,
            context_length: deps.context_length,
            config_compression: deps.config_compression,
            max_turns: deps.max_turns,
            fallback_client: deps.fallback_client,
            browser_session: deps.browser_session,
            mouse_capture_enabled: deps.mouse_capture_enabled,
            // Phase 22.4.2 Plan 00: D-08 subsystem handles
            state_store: deps.state_store,
            resolver: deps.resolver,
            context_compressor: deps.context_compressor,
            personality_overlay: deps.personality_overlay,
            // Phase 22.4.2 Plan 03: pending personality overlay for next spawn_turn
            next_turn_personality_overlay: None,
            // Phase 22.4.2.1 Plan 01: cron store — None by default (gateway is primary cron host)
            cron_store: None,
            // Phase 25.2 Plan 15 follow-up: toolset session handle for /toolset slash UI
            toolset_session: deps.toolset_session,
            // Phase 25.3 D-W-2 / D-T-3: Workspace + TrajectoryWriter for slash dispatch
            workspace: deps.workspace,
            trajectory_writer: deps.trajectory_writer,
            // Phase 21.8.2: forward skill_registry from deps.
            skill_registry: deps.skill_registry,
        }
    }

    // ── Scroll helpers (verbatim from tmon) ───────────────────────────────────

    /// Disable auto-follow and scroll up by `lines` rows.
    pub fn scroll_up(&mut self, lines: u16) {
        self.auto_follow = false;
        self.transcript_scroll = self.transcript_scroll.saturating_sub(lines);
    }

    /// Scroll down by `lines` rows (auto-follow re-enables via `reconcile_scroll`).
    pub fn scroll_down(&mut self, lines: u16) {
        self.transcript_scroll = self.transcript_scroll.saturating_add(lines);
    }

    /// Jump to the top of the transcript.
    pub fn scroll_to_top(&mut self) {
        self.auto_follow = false;
        self.transcript_scroll = 0;
    }

    /// Human-readable scroll indicator for the border title.
    pub fn scroll_indicator(&self, area: Rect) -> String {
        if self.auto_follow {
            "live".to_string()
        } else {
            let max = self.transcript_max_scroll(area);
            format!("scroll {}/{}", self.transcript_scroll, max)
        }
    }

    /// Clamp `transcript_scroll` to `max`; re-enable auto-follow if at bottom.
    pub fn reconcile_scroll(&mut self, area: Rect) {
        let max = self.transcript_max_scroll(area);
        if self.auto_follow {
            self.transcript_scroll = max;
        } else if self.transcript_scroll >= max {
            self.transcript_scroll = max;
            self.auto_follow = true;
        }
    }

    /// Maximum scroll offset for the given viewport.
    pub fn transcript_max_scroll(&self, area: Rect) -> u16 {
        let total = self.transcript_line_count(area.width as usize) as u32;
        let visible = area.height.saturating_sub(2) as u32;
        total.saturating_sub(visible).min(u16::MAX as u32) as u16
    }

    /// Total wrapped-line count across all history entries + streaming buffer.
    pub fn transcript_line_count(&self, width: usize) -> usize {
        let mut total = 0usize;
        for msg in &self.history {
            let body = render_message_body(msg);
            for line in body.lines() {
                total = total.saturating_add(wrapped_line_count(line, width));
            }
        }
        if let Some(buf) = &self.assistant_buffer {
            for line in buf.lines() {
                total = total.saturating_add(wrapped_line_count(line, width));
            }
        }
        total
    }

    // ── Event routing ─────────────────────────────────────────────────────────

    /// Top-level event dispatcher: routes crossterm events to the appropriate
    /// handler. `transcript_area` is needed for mouse scroll bounds.
    pub fn handle_event(&mut self, event: crossterm::event::Event, transcript_area: Rect) {
        use crossterm::event::Event;
        match event {
            Event::Key(k) => self.handle_key(k),
            Event::Mouse(m) => self.handle_mouse(m, transcript_area),
            _ => {}
        }
    }

    /// Key event handler.
    ///
    /// **Threat T-22.4-05-01 (DoS):** `KeyEventKind::Press` filter is first —
    /// release/repeat events are discarded to prevent double-dispatch.
    ///
    /// **BLOCKER-NEW-03:** Enter arm first checks for `/` prefix; slash input is
    /// routed to `dispatch_slash` and NEVER enters `app.history` as a User message.
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
        if key.kind != KeyEventKind::Press {
            return; // T-22.4-05-01: discard release/repeat
        }
        match (key.code, key.modifiers) {
            // Ctrl+C — double-press state machine (D-10..D-14)
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => self.handle_ctrl_c_key(),

            // Shift/Alt+Enter — insert newline without submitting (D-08)
            (KeyCode::Enter, m)
                if m.contains(KeyModifiers::SHIFT) || m.contains(KeyModifiers::ALT) =>
            {
                self.textarea.insert_newline();
            }

            // Enter — slash precheck (BLOCKER-NEW-03) then submit
            (KeyCode::Enter, _) => self.dispatch_or_submit(),

            // History recall (D-06)
            (KeyCode::Up, _) => {
                if let Some(entry) = self.history_store.prev().map(|s| s.to_string()) {
                    self.load_history_entry(&entry);
                }
            }
            (KeyCode::Down, _) => match self.history_store.next().map(|s| s.to_string()) {
                Some(entry) => self.load_history_entry(&entry),
                None => self.clear_textarea(),
            },

            // Scroll (D-05 / tmon parity)
            (KeyCode::PageUp, _) => self.scroll_up(10),
            (KeyCode::PageDown, _) => self.scroll_down(10),

            // Esc — clear textarea
            (KeyCode::Esc, _) => self.clear_textarea(),

            // All other keys — forward to TextArea widget
            _ => {
                let _ = self.textarea.input(key);
            }
        }
    }

    /// Mouse event handler — scrolls transcript when within `area` bounds.
    ///
    /// **Threat T-22.4-05-07 (Tampering):** bounds check prevents scroll events
    /// outside the transcript pane from affecting scroll state.
    pub fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent, area: Rect) {
        use crossterm::event::MouseEventKind;
        let within = mouse.column >= area.x
            && mouse.column < area.x + area.width
            && mouse.row >= area.y
            && mouse.row < area.y + area.height;
        if !within {
            return;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => self.scroll_up(3),
            MouseEventKind::ScrollDown => self.scroll_down(3),
            _ => {}
        }
    }

    /// BLOCKER-NEW-03 router: slash input → `dispatch_slash` (never `app.history`).
    /// Non-slash input → `submit()` (LLM turn).
    fn dispatch_or_submit(&mut self) {
        let text = self.textarea.lines().join("\n");
        if text.starts_with('/') {
            self.dispatch_slash_blocking(&text);
            self.clear_textarea();
            return;
        }
        self.submit();
    }

    /// Invoke `dispatch_slash` on the tokio runtime.
    ///
    /// Outside a tokio runtime (test path), records intent in the status hint
    /// without panicking.
    fn dispatch_slash_blocking(&mut self, input: &str) {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                let input_s = input.to_string();
                let outcome = tokio::task::block_in_place(|| {
                    handle.block_on(async {
                        crate::tui_rata::commands::dispatch_slash(self, &input_s).await
                    })
                });
                self.apply_slash_outcome(outcome);
            }
            Err(_) => {
                // Outside tokio runtime — test path. Record intent in hint.
                self.status.hint = format!("slash (test): {input}");
            }
        }
    }

    /// Apply a `SlashOutcome` to the app state.
    ///
    /// System messages are pushed with `Role::System` — slash output NEVER
    /// appears as `Role::User` (T-22.4-05-10).
    fn apply_slash_outcome(&mut self, outcome: crate::tui_rata::commands::SlashOutcome) {
        use crate::tui_rata::commands::SlashOutcome;
        match outcome {
            SlashOutcome::Handled(text) => {
                let mut msg = ChatMessage::user(&text);
                msg.role = Role::System;
                self.history.push(msg);
            }
            SlashOutcome::Silent => {}
            SlashOutcome::Quit => {
                self.should_quit = true;
            }
            SlashOutcome::ResetTerminal => {}
            SlashOutcome::McpReload => {}
            SlashOutcome::ClearSession(text) => {
                self.history.clear();
                self.assistant_buffer = None;
                let mut system = ChatMessage::user(&text);
                system.role = Role::System;
                self.history.push(system);
            }
            SlashOutcome::Unknown { input: _, hint } => {
                let mut system = ChatMessage::user(&hint);
                system.role = Role::System;
                self.history.push(system);
                self.status.hint = hint;
            }
            SlashOutcome::Error(err) => {
                let body = format!("error: {err}");
                let mut system = ChatMessage::user(&body);
                system.role = Role::System;
                self.history.push(system);
                self.status.hint = format!("error: {err}");
            }
        }
    }

    /// Ctrl+C handler — delegates to the double-ctrl-c state machine (D-10..D-14).
    fn handle_ctrl_c_key(&mut self) {
        let decision = self
            .double_ctrl_c
            .on_ctrl_c(Instant::now(), self.cancel_child.is_some());
        match decision {
            CtrlCDecision::CancelTurn => {
                if let Some(tok) = self.cancel_child.take() {
                    tok.cancel();
                }
                self.status.hint = "cancelled".to_string();
            }
            CtrlCDecision::ExitCleanly => {
                self.should_quit = true;
            }
            CtrlCDecision::ShowPromptHint => {
                self.status.hint = "Ctrl+C again to quit".to_string();
            }
        }
    }

    /// Signal-handler entry point (SIGINT from event_loop). Delegates to
    /// `handle_ctrl_c_key` so the state machine is authoritative.
    pub fn handle_ctrl_c_signal(&mut self) {
        self.handle_ctrl_c_key();
    }

    // ── Streaming bridge ──────────────────────────────────────────────────────

    /// Handle an incoming `StreamEvent` from the agent turn channel.
    ///
    /// All 8 D-17 canonical variants are handled (T-22.4-05-02).
    pub fn handle_stream_event(&mut self, event: StreamEvent) {
        match event {
            StreamEvent::Started => {
                self.assistant_buffer = Some(String::new());
                self.status.hint = "connecting...".to_string();
            }
            StreamEvent::Delta(d) => {
                if let Some(buf) = self.assistant_buffer.as_mut() {
                    buf.push_str(&d);
                } else {
                    self.assistant_buffer = Some(d);
                }
            }
            StreamEvent::ToolCall { name } => {
                self.status.hint = format!("tool: {name}");
            }
            StreamEvent::ToolProgress { name, phase } => {
                self.status.hint = format!("{name}: {phase}");
            }
            StreamEvent::ToolResult { name, ok } => {
                let icon = if ok { "✓" } else { "✗" };
                self.status.hint = format!("{icon} {name}");
            }
            StreamEvent::Finished => {
                self.commit_assistant_buffer();
                self.pending_rx = None;
                self.cancel_child = None;
                self.status.hint = String::new();
            }
            StreamEvent::Error(e) => {
                self.commit_assistant_buffer();
                self.status.hint = format!("error: {e}");
                self.pending_rx = None;
                self.cancel_child = None;
            }
            StreamEvent::Cancelled => {
                self.commit_assistant_buffer();
                self.status.hint = "cancelled".to_string();
                self.pending_rx = None;
                self.cancel_child = None;
            }
        }
    }

    /// Flush `assistant_buffer` into `history` as an assistant message.
    fn commit_assistant_buffer(&mut self) {
        if let Some(buf) = self.assistant_buffer.take() {
            if !buf.is_empty() {
                self.history.push(assistant_message(buf));
            }
        }
    }

    /// Tick callback — advance knight-rider animation counter.
    pub fn on_tick(&mut self) {
        self.knight_rider_tick = self.knight_rider_tick.wrapping_add(1);
    }

    // ── Submit ────────────────────────────────────────────────────────────────

    /// Submit the current textarea content.
    ///
    /// - Empty input → no-op.
    /// - Slash input → **defensive re-check** (paranoid redundancy over
    ///   `dispatch_or_submit`) — routes to `dispatch_slash_blocking` without
    ///   creating a pending channel (T-22.4-05-10).
    /// - Plain input → push to history, create `(tx, rx)` channel, set
    ///   `pending_rx`/`pending_tx` for plan 22.4-07's `spawn_turn`.
    pub fn submit(&mut self) {
        let text = self.textarea.lines().join("\n");
        if text.is_empty() {
            return;
        }
        // Defensive re-check: slash input must never enter history as User.
        if text.starts_with('/') {
            self.dispatch_slash_blocking(&text);
            self.clear_textarea();
            return;
        }
        self.history_store.push(text.clone());
        self.history_store.reset_cursor();
        self.history.push(user_message(text));
        self.clear_textarea();

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();
        self.pending_rx = Some(rx);
        self.pending_tx = Some(tx);
        self.cancel_child = Some(self.cancel_parent.child_token());
        self.auto_follow = true;
        self.assistant_buffer = None;
    }

    // ── Textarea helpers ──────────────────────────────────────────────────────

    /// Replace textarea with a fresh empty widget.
    fn clear_textarea(&mut self) {
        self.textarea = TextArea::default();
        self.textarea.set_cursor_line_style(Style::default());
        // UAT Gap 1 (Phase 22.4 Plan 22.4-14): reinstall the bordered "Prompt"
        // block on every reset so the visual frame survives submit + Esc + slash
        // dispatch cycles.
        self.textarea
            .set_block(Block::default().borders(Borders::ALL).title("Prompt"));
    }

    /// Load a history entry into the textarea (arrow-key recall).
    pub fn load_history_entry(&mut self, entry: &str) {
        let mut ta = TextArea::default();
        ta.set_cursor_line_style(Style::default());
        // UAT Gap 1 (Phase 22.4 Plan 22.4-14): keep the bordered "Prompt" frame
        // when arrow-key history recall replaces the textarea.
        ta.set_block(Block::default().borders(Borders::ALL).title("Prompt"));
        for (i, line) in entry.lines().enumerate() {
            if i > 0 {
                ta.insert_newline();
            }
            ta.insert_str(line);
        }
        self.textarea = ta;
    }

    // ── Transcript rendering ──────────────────────────────────────────────────

    /// Build a `Text<'static>` for the transcript paragraph widget.
    ///
    /// System messages are suppressed (role_style returns `None` for System).
    /// Streaming buffer is appended in green at the end.
    pub fn transcript_text(&self) -> Text<'static> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        for msg in &self.history {
            let (role_label, color) = role_style(msg);
            let Some(color) = color else { continue };
            // UAT Round 2 Gap 4 (Phase 22.4 Plan 22.4-17): System rows render in
            // dim DarkGray so slash-command confirmations (/help, /clear, /new,
            // /mouse on|off, typo-suggester output) are observable yet visually
            // demoted from real conversation rows. See role_style() above.
            let style = if matches!(msg.role, Role::System) {
                Style::default().fg(color).add_modifier(Modifier::DIM)
            } else {
                Style::default().fg(color)
            };
            let body = render_message_body(msg);
            for (i, line_text) in body.lines().enumerate() {
                if i == 0 {
                    lines.push(Line::from(vec![
                        Span::styled(format!("{role_label}: "), style),
                        Span::raw(line_text.to_string()),
                    ]));
                } else {
                    lines.push(Line::from(Span::raw(line_text.to_string())));
                }
            }
        }
        if let Some(buf) = &self.assistant_buffer {
            let green = Style::default().fg(Color::Green);
            for (i, line_text) in buf.lines().enumerate() {
                if i == 0 {
                    lines.push(Line::from(vec![
                        Span::styled("Hermes: ".to_string(), green),
                        Span::raw(line_text.to_string()),
                    ]));
                } else {
                    lines.push(Line::from(Span::raw(line_text.to_string())));
                }
            }
        }
        Text::from(lines)
    }

    // ── test-support constructors ─────────────────────────────────────────────

    /// Construct a minimal empty App for snapshot/unit tests.
    /// Requires the `test-support` feature.
    #[cfg(feature = "test-support")]
    pub fn new_test_empty() -> Self {
        Self::new(test_deps())
    }

    /// Construct an App pre-seeded with `(role, body)` message pairs.
    /// Role strings: `"user"`, `"assistant"`, `"tool"`, `"system"`.
    #[cfg(feature = "test-support")]
    pub fn new_test_with_messages(msgs: Vec<(&'static str, &'static str)>) -> Self {
        let mut app = Self::new(test_deps());
        app.history = msgs.into_iter().map(|(r, b)| test_message(r, b)).collect();
        app
    }
}

// ── Free helpers ──────────────────────────────────────────────────────────────

/// Extract the text body from a ChatMessage. Returns empty string for
/// non-Text content variants and for None.
fn render_message_body(msg: &ChatMessage) -> String {
    match &msg.content {
        Some(MessageContent::Text(s)) => s.clone(),
        Some(_) => String::new(),
        None => String::new(),
    }
}

/// Map a message role to a display label and colour.
///
/// UAT Round 2 Gap 4 (Phase 22.4 Plan 22.4-17): `Role::System` previously
/// returned `None` here, which caused the let-else short-circuit in
/// `transcript_text` to silently drop every slash-command confirmation
/// (/help, /clear, /new, /mouse on|off, typo suggester output) from the
/// rendered transcript. The locked Option B fix returns `Some(Color::DarkGray)`
/// so System rows render in a dim gray distinct from User (Cyan) / Hermes
/// (Green) / Tool (Yellow). The DIM `Modifier` is applied at the
/// `transcript_text` Style-construction site so System rows visually demote
/// as metadata, not as conversation. The Option<Color> return type is kept
/// in case a future Role variant truly should be hidden — no current
/// variant uses None.
fn role_style(msg: &ChatMessage) -> (String, Option<Color>) {
    match msg.role {
        Role::User => ("You".to_string(), Some(Color::Cyan)),
        Role::Assistant => ("Hermes".to_string(), Some(Color::Green)),
        Role::Tool => ("Tool".to_string(), Some(Color::Yellow)),
        Role::System => ("System".to_string(), Some(Color::DarkGray)),
    }
}

fn user_message(body: String) -> ChatMessage {
    ChatMessage::user(&body)
}

fn assistant_message(body: String) -> ChatMessage {
    ChatMessage::assistant(&body)
}

/// Compute wrapped line count for `line` at terminal width `width`.
///
/// - Empty line → 1 (blank line still occupies a row).
/// - `width == 0` → 1 (defensive; avoids divide-by-zero).
/// - Otherwise → ceil(char_count / width).
pub(crate) fn wrapped_line_count(line: &str, width: usize) -> usize {
    if line.is_empty() {
        return 1;
    }
    let cols = line.chars().count();
    if width == 0 {
        return 1;
    }
    (cols + width - 1) / width
}

// ── test-support helpers ──────────────────────────────────────────────────────

#[cfg(feature = "test-support")]
fn test_message(role: &str, body: &str) -> ChatMessage {
    match role {
        "assistant" => ChatMessage::assistant(body),
        "tool" => {
            let mut m = ChatMessage::user(body);
            m.role = Role::Tool;
            m
        }
        "system" => {
            let mut m = ChatMessage::user(body);
            m.role = Role::System;
            m
        }
        _ => ChatMessage::user(body),
    }
}

#[cfg(feature = "test-support")]
fn test_deps() -> AppDeps {
    use ironhermes_agent::budget::BudgetHandle;
    use ironhermes_agent::{AnyClient, agent_loop::AgentLoop};
    use ironhermes_core::commands::registry::build_registry;
    use ironhermes_core::{Config, ProviderResolver};
    use ironhermes_tools::ToolRegistry;

    let test_client = AnyClient::ChatCompletions(ironhermes_agent::client::LlmClient::new(
        "http://localhost:11434",
        "test-key",
        "test-model",
    ));
    let test_registry = Arc::new(tokio::sync::RwLock::new(ToolRegistry::new()));
    // ProviderResolver::build with default Config — uses built-in defaults, no env vars needed.
    let test_resolver = ProviderResolver::build(&Config::default())
        .expect("ProviderResolver::build with default Config must not fail in test context");
    // PersonalityRegistry with no custom presets (built-ins always available).
    let test_personality = Arc::new(PersonalityRegistry::load(
        &std::collections::HashMap::new(),
        &ironhermes_core::get_hermes_home(),
    ));

    AppDeps {
        agent_loop: Arc::new(AgentLoop::for_tests()),
        hook_registry: Arc::new(HookRegistry::new(ironhermes_hooks::HooksConfig::default())),
        mcp_manager: None,
        memory_manager: None,
        subagent_registry: Arc::new(tokio::sync::RwLock::new(SubagentRegistry::new())),
        process_registry: Arc::new(tokio::sync::RwLock::new(ProcessRegistry::new_for_session(
            "test-session".to_string(),
        ))),
        command_router: Arc::new(CommandRouter::new(build_registry())),
        session_id: "test-session".to_string(),
        history_path: std::env::temp_dir()
            .join(format!("tui_rata_hist_{}.txt", std::process::id())),
        status_initial: StatusLineState::default(),
        cancel_parent: CancellationToken::new(),
        client: test_client,
        registry: test_registry,
        budget: BudgetHandle::new(10),
        context_length: 8192,
        config_compression: 0.8,
        max_turns: 10,
        fallback_client: None,
        browser_session: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
        mouse_capture_enabled: Arc::new(AtomicBool::new(true)),
        // Phase 22.4.2 Plan 00: D-08 subsystem handles (None/defaults for tests)
        state_store: None,
        resolver: test_resolver,
        context_compressor: None,
        personality_overlay: test_personality,
        // Phase 22.4.2 Plan 00: D-09 toggle Arcs
        yolo_enabled: Arc::new(AtomicBool::new(false)),
        verbose_enabled: Arc::new(AtomicBool::new(false)),
        statusbar_enabled: Arc::new(AtomicBool::new(true)),
        debug_enabled: Arc::new(AtomicBool::new(false)),
        fast_enabled: Arc::new(AtomicBool::new(false)),
        skin: Arc::new(std::sync::RwLock::new("default".to_string())),
        // Phase 25.2 Plan 15 follow-up: tests don't exercise the toolset slash UI
        toolset_session: None,
        // Phase 25.3 D-W-2 / D-T-3: tests don't exercise the workspace or trajectory writer
        workspace: None,
        trajectory_writer: None,
        // Phase 25.3-13 CR-04: tests don't exercise the seeded system message
        system_message: None,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod inv_tests {
    /// INV-25.1-19: Phase 25.1 GAP-8 closure.
    /// Both AppDeps and App MUST carry the browser_session field with the
    /// exact verified type from the interfaces block, and App::new MUST
    /// forward it from deps.
    #[test]
    fn inv_25_1_gap8_app_carries_browser_session_field() {
        let source = include_str!("app.rs");
        let non_comment: String = source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        // The field MUST appear in BOTH AppDeps and App (2 struct definitions).
        let count = non_comment.matches("browser_session: std::sync::Arc<tokio::sync::Mutex<Option<ironhermes_tools::browser_session::BrowserSession>>>").count();
        assert!(
            count >= 2,
            "Phase 25.1 GAP-8 (plan 25.1-19): both AppDeps and App MUST carry the browser_session field; got {} occurrences in non-comment source",
            count
        );
        // App::new MUST forward the field from deps.
        assert!(
            non_comment.contains("browser_session: deps.browser_session"),
            "Phase 25.1 GAP-8 (plan 25.1-19): App::new MUST forward browser_session from deps"
        );
    }
}

#[cfg(all(test, feature = "test-support"))]
mod scroll_tests {
    use super::*;

    fn area(w: u16, h: u16) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        }
    }

    // — wrapped_line_count ──────────────────────────────────────────────────

    #[test]
    fn wrapped_empty_is_one() {
        assert_eq!(wrapped_line_count("", 10), 1);
    }

    #[test]
    fn wrapped_fits_one_row() {
        assert_eq!(wrapped_line_count("hello", 10), 1);
    }

    #[test]
    fn wrapped_exactly_one_row() {
        assert_eq!(wrapped_line_count("helloworld", 10), 1);
    }

    #[test]
    fn wrapped_overflows_one_row() {
        assert_eq!(wrapped_line_count("helloworld!", 10), 2);
    }

    // — scroll helpers ───────────────────────────────────────────────────────

    #[test]
    fn scroll_up_disables_auto_follow() {
        let mut app = App::new_test_empty();
        assert!(app.auto_follow);
        app.scroll_up(1);
        assert!(!app.auto_follow);
    }

    #[test]
    fn scroll_indicator_live_when_auto_follow() {
        let app = App::new_test_empty();
        assert_eq!(app.scroll_indicator(area(80, 24)), "live");
    }

    #[test]
    fn pending_tx_field_initialized_none() {
        let app = App::new_test_empty();
        assert!(app.pending_tx.is_none());
    }

    // — StreamEvent handlers ─────────────────────────────────────────────────

    #[test]
    fn handle_stream_event_delta_accumulates_assistant_buffer() {
        let mut app = App::new_test_empty();
        app.handle_stream_event(StreamEvent::Started);
        app.handle_stream_event(StreamEvent::Delta("hello".to_string()));
        app.handle_stream_event(StreamEvent::Delta(" world".to_string()));
        assert_eq!(app.assistant_buffer.as_deref(), Some("hello world"));
    }

    #[test]
    fn handle_stream_event_finished_clears_pending_rx_and_commits() {
        let mut app = App::new_test_empty();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();
        app.pending_rx = Some(rx);
        app.pending_tx = Some(tx);
        app.assistant_buffer = Some("response text".to_string());
        app.handle_stream_event(StreamEvent::Finished);
        assert!(app.pending_rx.is_none());
        assert!(app.assistant_buffer.is_none());
        assert_eq!(app.history.len(), 1);
        assert_eq!(app.history[0].role, Role::Assistant);
    }

    // — KeyEvent handlers ────────────────────────────────────────────────────

    #[test]
    fn handle_key_press_only_filter_ignores_release() {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
        let mut app = App::new_test_empty();
        // seed textarea
        app.textarea.insert_str("hello");
        let release = KeyEvent {
            code: KeyCode::Esc,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: crossterm::event::KeyEventState::NONE,
        };
        app.handle_key(release);
        // Esc Release must be a no-op — textarea not cleared
        assert_eq!(app.textarea.lines().join(""), "hello");
    }

    #[test]
    fn handle_key_ctrl_c_idle_sets_prompt_hint() {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
        let mut app = App::new_test_empty();
        let ctrl_c = KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        };
        app.handle_key(ctrl_c);
        // No in-flight turn → ShowPromptHint
        assert!(
            !app.status.hint.is_empty(),
            "hint must be set after Ctrl+C at prompt"
        );
        assert!(
            !app.should_quit,
            "should_quit must remain false on first Ctrl+C"
        );
    }

    #[test]
    fn handle_key_ctrl_c_in_flight_cancels_child_token() {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
        let mut app = App::new_test_empty();
        let child = app.cancel_parent.child_token();
        app.cancel_child = Some(child);
        let ctrl_c = KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        };
        app.handle_key(ctrl_c);
        // cancel_child consumed + cancel_parent's child cancelled
        assert!(app.cancel_child.is_none());
    }

    #[test]
    fn handle_key_up_arrow_loads_history_entry() {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
        let mut app = App::new_test_empty();
        app.history_store.push("previous command".to_string());
        let up = KeyEvent {
            code: KeyCode::Up,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        };
        app.handle_key(up);
        assert_eq!(app.textarea.lines().join(""), "previous command");
    }

    // — submit / BLOCKER-NEW-03 coverage ─────────────────────────────────────

    #[test]
    fn slash_submit_routes_to_dispatch_not_history() {
        let mut app = App::new_test_empty();
        app.textarea.insert_str("/help");
        app.submit();
        // slash input must NOT create a User-role history entry
        let user_entries: Vec<_> = app
            .history
            .iter()
            .filter(|m| m.role == Role::User)
            .collect();
        assert!(
            user_entries.is_empty(),
            "slash input must never enter history as User; got: {:?}",
            user_entries
        );
        // No agent turn should be scheduled
        assert!(
            app.pending_rx.is_none(),
            "slash submit must not create pending_rx"
        );
    }

    #[test]
    fn slash_dispatch_or_submit_short_circuits_submit() {
        let mut app = App::new_test_empty();
        app.textarea.insert_str("/quit");
        app.dispatch_or_submit();
        // Outside tokio runtime — dispatch_slash_blocking falls back to hint
        assert!(
            app.pending_rx.is_none(),
            "slash dispatch must not create pending_rx"
        );
        // hint should contain slash marker (test-path fallback)
        assert!(
            app.status.hint.contains("slash") || app.status.hint.contains("/quit"),
            "status.hint must reflect slash handling; got: {:?}",
            app.status.hint
        );
    }

    #[test]
    fn non_slash_submit_creates_pending_rx_and_pending_tx() {
        let mut app = App::new_test_empty();
        app.textarea.insert_str("hello world");
        app.submit();
        assert!(
            app.pending_rx.is_some(),
            "pending_rx must be Some after submit"
        );
        assert!(
            app.pending_tx.is_some(),
            "pending_tx must be Some after submit"
        );
        let user_entries: Vec<_> = app
            .history
            .iter()
            .filter(|m| m.role == Role::User)
            .collect();
        assert_eq!(
            user_entries.len(),
            1,
            "exactly 1 User-role entry after submit"
        );
    }

    // — misc ─────────────────────────────────────────────────────────────────

    #[test]
    fn handle_mouse_outside_area_noop() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let mut app = App::new_test_empty();
        let scroll_before = app.transcript_scroll;
        let auto_before = app.auto_follow;
        let outside = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 200,
            row: 200,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };
        app.handle_mouse(outside, area(80, 24));
        assert_eq!(app.transcript_scroll, scroll_before);
        assert_eq!(app.auto_follow, auto_before);
    }

    #[test]
    fn on_tick_increments_knight_rider_tick() {
        let mut app = App::new_test_empty();
        assert_eq!(app.knight_rider_tick, 0);
        app.on_tick();
        assert_eq!(app.knight_rider_tick, 1);
        app.on_tick();
        assert_eq!(app.knight_rider_tick, 2);
    }
}

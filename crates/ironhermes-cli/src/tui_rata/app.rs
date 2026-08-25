//! Central App state for the tui_rata REPL (Phase 22.4).
//!
//! Structural template: tmon/src/main.rs App struct + scroll helpers.
//! IronHermes additions for the D-18 14-item parity list.
//!
//! # Design notes
//! - `hint` in `StatusLineState` is a `String`; empty = no hint shown.
//! - TextArea import uses `tui_textarea_2` (workspace alias for tui-textarea-2 0.10.2).
//! - `dispatch_slash` is a stub in `commands.rs`; plan 22.4-07 Task 4 fills it.

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use tui_scrollview::ScrollViewState;
use tui_textarea::TextArea;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::tui_rata::approval_gate_tui::ApprovalRequest;
use crate::tui_rata::clarify_dispatcher_tui::ClarifyRequest;
use crate::tui_rata::double_ctrl_c::{CtrlCDecision, DoubleCtrlCState};
use crate::tui_rata::history::{DEFAULT_MAX, ReplHistory};
use crate::tui_rata::overlay::{OverlayKind, PickerStep};
use crate::tui_rata::selection::{self, Selection};
use crate::tui_rata::shell_bang;
use crate::tui_rata::status_line::StatusLineState;
use crate::tui_rata::stream_events::StreamEvent;

// Concrete paths — grep-verified iteration 2.
use ironhermes_agent::AgentRuntime;
use ironhermes_agent::AnyClient;
use ironhermes_agent::context_engine::ContextEngine;
use ironhermes_agent::memory::MemoryManager;
use ironhermes_agent::personality::PersonalityRegistry;
use ironhermes_agent::subagent_registry::SubagentRegistry;
use ironhermes_core::ApprovalOutcome;
use ironhermes_core::ProviderResolver;
use ironhermes_core::commands::CommandRouter;
use ironhermes_core::commands::context::ToolsetSessionHandle;
use ironhermes_core::commands::skill_dispatch::build_skill_invocation;
use ironhermes_core::queue::MessageQueue;
use ironhermes_core::session::SessionKey;
use ironhermes_core::types::{ChatMessage, MessageContent, Platform, Role};
use ironhermes_exec::process_registry::ProcessRegistry;
use ironhermes_gateway::media_tag::{MediaKind, MediaRef, MediaSource, MediaTagExtractor};
use ironhermes_hooks::HookRegistry;
use ironhermes_mcp::McpManager;
use ironhermes_state::StateStore;
use ironhermes_tools::ToolRegistry;
use ironhermes_tools::{ClarifyAnswer, PendingClarifyRegistry};

// ── AppDeps ───────────────────────────────────────────────────────────────────

/// Dependency bundle passed into `App::new`.
///
/// Keeps the constructor signature stable as the parity list grows.
/// Plan 22.4-07 constructs this in the event-loop bootstrap.
pub struct AppDeps {
    /// Phase 28.1-05: AgentRuntime owns the durable agent (budget, registry,
    /// browser session, skills, hook registry). Replaces the loose
    /// agent_loop/budget/context_length/config_compression/max_turns/fallback_client
    /// fields; spawn_turn calls runtime.run_turn per turn.
    pub agent_runtime: Arc<AgentRuntime>,
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
    /// Mutable client for /model and /fast slash commands that rebuild the
    /// AnyClient mid-session. The runtime owns its own client for turns; this
    /// field tracks the slash-command-mutated client so spawn_turn can pass it
    /// to TurnRequest when a model switch is in effect.
    /// NOTE: Phase 28.1-05 decision — keep client on App because /model and
    /// /fast mutate it interactively; routing through the runtime would require
    /// a mutable runtime accessor which is architecturally heavyweight.
    pub client: AnyClient,
    /// ToolRegistry — kept for session-end hooks (registry.read().await.call_session_end_hooks())
    /// and for slash-dispatch CommandContext. Runtime also holds this Arc via
    /// runtime.registry(); App keeps its own clone for the end-hook call site
    /// in run_with_deps without requiring a runtime borrow.
    pub registry: Arc<RwLock<ToolRegistry>>,
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
    /// Phase 36.17.3 (D-03): shared FIFO queue keyed by SessionKey.
    /// The TUI uses a single fixed `SessionKey` (Platform::Local + "local"
    /// chat_id + "local" user_id); the App derives the key in `App::new`.
    pub queue: Arc<dyn MessageQueue<SessionKey>>,
    /// Phase 36.17.3 (D-06 amended): queue drain pause toggle.
    /// Arc-wrapped per PATTERNS §2 / RESEARCH Pitfall 6 so slash handlers
    /// running in shared-context closures can mutate without `&mut App`.
    pub queue_paused: Arc<AtomicBool>,
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

    /// Phase 21.8.2 Plan 03 D-02 / D-Plan03-06: SkillsConfig used by the
    /// SkillsReload event-loop arm to call `SkillRegistry::load_with_config`.
    /// Populated by `build_app_deps` from `config.skills.clone()`.
    pub skills_config: ironhermes_core::config::SkillsConfig,

    /// Phase 21.8.2 Plan 03 D-07 (TUI delivery): pending activated-skill
    /// overlays. The SkillActivated event-loop arm pushes (name, body) here;
    /// the next turn's per-turn prompt_builder assembly reads + drains them.
    pub pending_skill_overlays: Vec<(String, String)>,

    /// Phase 36.3.12 Plan 10 (WR-01): a SINGLE `ApprovalsStore` loaded once by
    /// `build_app_deps` and shared for the lifetime of the TUI process, so a
    /// `[s]ession` approval grant persists across every `spawn_turn` dispatch
    /// instead of being discarded by a fresh `ApprovalsStore::load()` per turn.
    pub approvals_store: Arc<ironhermes_core::ApprovalsStore>,

    /// Phase 36.6.4 Plan 05 (D-13, T-36.6.4-IMG-04): the image `Picker`,
    /// built ONCE by `build_app_deps` inside the narrow post-alt-screen
    /// pre-event-stream startup window — see `App.picker`'s doc comment.
    pub picker: ratatui_image::picker::Picker,
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
    /// Phase 36.6.4 Plan 01 (D-02): the SOLE authority for the transcript
    /// scroll offset — replaces the old `transcript_scroll: u16` field.
    /// `App` is rendered via `&App` (not `&mut App`), so this needs interior
    /// mutability; a `Mutex` mirrors `chip_hit_test`'s existing thread-safety
    /// posture (Phase 46.7 Plan 07 precedent) rather than introducing
    /// `RefCell`. Read via `transcript_scroll()` / mutated via
    /// `set_transcript_scroll()` — keeping BOTH a `u16` field AND this state
    /// alive is precisely the silent-drift mechanism RESEARCH Pitfall 2
    /// names (see `<assumption_delta_decision>` in the Plan 01 PLAN.md).
    pub scroll_view_state: std::sync::Mutex<ScrollViewState>,
    pub auto_follow: bool,
    /// Phase 36.6.4 Plan 01 (D-04/D-07): the active or most-recently-
    /// completed mouse-drag text selection, in virtual content coordinates.
    /// `None` = no selection. Cleared only by a fresh `Down(Left)` press (a
    /// completed selection's highlight persists until the next click, per
    /// D-04's X11 primary-selection model).
    pub selection: Option<Selection>,
    /// Phase 36.6.4 Plan 02 (D-05): vim-style keyboard selection mode — the
    /// SSH-safe fallback for D-04's mouse-drag selection (mouse events do
    /// not reliably survive every SSH/tmux configuration). `Idle` = no
    /// keyboard selection in progress (`selection` above, established by a
    /// mouse drag, is tracked independently and unaffected). `v` (textarea
    /// empty, no overlay) enters `Visual`; `y`/`Esc` return to `Idle`.
    pub selection_mode: selection::SelectionMode,

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
    /// Phase 36.17.3 (D-03): shared FIFO queue keyed by SessionKey.
    pub queue: Arc<dyn MessageQueue<SessionKey>>,
    /// Phase 36.17.3 (D-03): fixed TUI session key. Constructed in `App::new`
    /// from `SessionKey::new(Platform::Local, "local").with_user("local")`
    /// because the TUI never multiplexes chats — see RESEARCH §SessionKey Position.
    pub queue_key: SessionKey,
    /// Phase 36.17.3 (D-06 amended): when true, post-turn drain check skips
    /// popping. Arc-wrapped per RESEARCH Pitfall 6.
    pub queue_paused: Arc<AtomicBool>,
    /// `/skin <name>` setter (D-09).
    pub skin: Arc<std::sync::RwLock<String>>,
    /// Phase 39.1 Plan 04 (R39.1-01 / R39.1-06 / D-06): process-wide TurnRegistry.
    /// Replaces the bare `agent_running: Arc<AtomicBool>` gate (removed). All slash
    /// commands dispatch mid-turn; per-turn cancel and /agents cross-surface listing
    /// are served from this registry (R39.1-05 / R39.1-09 / D-09).
    pub turn_registry: Arc<ironhermes_core::concurrency::TurnRegistry>,
    /// Phase 39.1 Plan 04: two-level semaphore gate (per-session cap + global ceiling).
    pub concurrency: ironhermes_core::concurrency::ConcurrencyLayer,
    /// Phase 39.1 Plan 04: wire-safe in-flight turn cache for the render loop.
    /// Refreshed from `turn_registry.list_session(session_id)` on each tick (D-07).
    pub in_flight: Vec<ironhermes_core::concurrency::TurnSummary>,
    // Kept for CommandContext::new() signature compat until Plan 06 removes the field.
    pub agent_running: Arc<AtomicBool>,

    // — D-18 parity handles (Arc-held) ───────────────────────────────────────
    /// Phase 28.1-05: durable agent runtime. spawn_turn builds TurnRequest and
    /// calls runtime.run_turn per turn. Replaces the per-turn AgentLoop builder.
    pub agent_runtime: Arc<AgentRuntime>,
    pub hook_registry: Arc<HookRegistry>,
    pub mcp_manager: Option<Arc<McpManager>>,
    pub memory_manager: Option<Arc<Mutex<MemoryManager>>>,
    pub subagent_registry: Arc<RwLock<SubagentRegistry>>,
    pub process_registry: Arc<RwLock<ProcessRegistry>>,
    pub command_router: Arc<CommandRouter>,
    /// Mutable client for /model and /fast (see AppDeps.client doc).
    pub client: AnyClient,
    /// ToolRegistry clone for session-end hooks + slash-dispatch CommandContext.
    pub registry: Arc<RwLock<ToolRegistry>>,
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
    /// Active personality overlay text. Injected into per-turn messages_snapshot[0].content
    /// by spawn_turn. Persists across turns; None = default identity.
    /// Set by handle_subsystem_mutator on /personality <name>; cleared by /personality clear.
    pub active_personality_overlay: Option<String>,

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

    /// Phase 21.8.2 Plan 03 D-02 / D-Plan03-06: see AppDeps doc above.
    pub skills_config: ironhermes_core::config::SkillsConfig,

    /// Phase 21.8.2 Plan 03 D-07 (TUI delivery): see AppDeps doc above.
    pub pending_skill_overlays: Vec<(String, String)>,

    /// Phase 36.17.8 (D-08): voice state for Ctrl+B push-to-talk capture loop.
    /// Holds recording/enabled flags, capture task handle, and stop-channel.
    pub voice: crate::tui_rata::voice_state::VoiceState,

    /// Phase 36.17.8 (D-08): channel from the voice capture task to the event loop.
    /// The capture task sends accepted transcripts here; `poll_voice_transcripts`
    /// drains them each tick and calls `submit_voice_text`.
    pub voice_transcript_rx: Option<tokio::sync::mpsc::UnboundedReceiver<String>>,

    /// Phase 36.17.8: `true` when the pending turn's input came from voice
    /// (`submit_voice_text`), `false` for typed input (`submit`). Read by
    /// `spawn_turn` to decide whether `/voice on` should speak this reply.
    pub last_turn_was_voice: bool,

    /// Phase 46.7 Plan 06 (D-18/D-20): files already copied into
    /// `session_attachments_dir(session_id)`, queued to attach to the NEXT
    /// submitted message. Populated by `/attach <path>` (Task 1) and inline
    /// `@path` parsing (Task 2); drained by `submit()`.
    pub pending_attachments: Vec<PendingAttachment>,

    /// Phase 46.7 Plan 06 (D-22): deliverables captured by the turn-scoped
    /// post-turn capture (`event_loop::spawn_turn`). Shared (not per-turn)
    /// so the spawned task can push into it without `&mut App`; Plan 07
    /// reads it to render artifact chips in the transcript.
    pub captured_artifacts:
        Arc<std::sync::Mutex<Vec<ironhermes_tools::chat_capture::CapturedArtifact>>>,

    /// Phase 46.7 Plan 06 (D-15): the exact caption text of the last
    /// submitted turn (post `@path`-stripping), read by `spawn_turn` for
    /// `detect_turn_opt_out`. Set by `submit()`/`submit_voice_text()`.
    pub last_submitted_text: String,

    /// Phase 46.7 Plan 07 (D-19): files actually sent with a submitted turn,
    /// recorded by `build_user_message_with_attachments` at drain time (the
    /// draining `PendingAttachment` list itself doesn't survive submit).
    /// Flat + append-only, mirroring `captured_artifacts`'s existing
    /// precedent — rendered as `[📎 filename size]` chips appended to the
    /// transcript.
    pub sent_attachment_chips: Vec<SentAttachmentChip>,

    /// Phase 36.6.4 Plan 05 (D-12/D-13, TUI-IMG-01): image chips created by
    /// either D-12 trigger — `<MEDIA:>` tag extraction at turn-commit
    /// (`commit_assistant_buffer`) or `/image <path>`. Flat + append-only,
    /// mirroring `sent_attachment_chips`/`captured_artifacts`. Rendered as
    /// `[🖼 {label}]` `Color::Cyan` chips appended to the transcript AFTER
    /// `captured_artifacts` and BEFORE `shell_runs` — `transcript_render_text`
    /// and `rebuild_chip_hit_test` must both walk this in the SAME order.
    pub image_chips: Vec<ImageChip>,

    /// Phase 36.6.4 Plan 05 (D-13): the image `Picker` built ONCE at startup
    /// in the narrow post-alt-screen pre-event-stream window
    /// (`event_loop::build_app_deps`) — never lazily on first overlay open,
    /// so the stdio capability query's response bytes can never be consumed
    /// by the live crossterm event stream instead of the query's own
    /// blocking read (T-36.6.4-IMG-04).
    pub picker: ratatui_image::picker::Picker,

    /// Phase 36.6.4 Plan 05 (T-36.6.4-IMG-01): decode/protocol-build state
    /// for the CURRENTLY open image overlay. `None` = no decode has been
    /// triggered yet for the current overlay (a fresh chip click always
    /// resets this to `None` before setting `active_overlay`) —
    /// `overlay::render_image_viewer` observes `None` and triggers the
    /// ONE-TIME `spawn_blocking` decode; it never re-triggers while
    /// Decoding/Ready/Failed. `Arc<Mutex<>>` so the spawned task can write
    /// the result back without `&mut App` (mirrors `captured_artifacts`).
    pub image_decode: Arc<std::sync::Mutex<Option<ImageDecodeState>>>,

    /// Phase 46.7 Plan 07 (D-17): per-render chip hit-test map, rebuilt from
    /// scratch every render pass by `rebuild_chip_hit_test` (called from
    /// `ui.rs::render_transcript`) and consulted by `handle_mouse`'s
    /// `Down(Left)` arm. `App` is rendered via `&App` (not `&mut App`), so
    /// this needs interior mutability; a `Mutex` (not `RefCell`) mirrors the
    /// existing `captured_artifacts` field's thread-safety posture.
    chip_hit_test: std::sync::Mutex<Vec<(Rect, ChipAction)>>,

    /// Phase 36.6.4 Plan 02: most-recently-rendered transcript pane `Rect`,
    /// cached via interior mutability (mirrors `chip_hit_test`/
    /// `scroll_view_state` — `App` renders through `&App`, never `&mut
    /// App`) so `handle_key`'s keyboard-only yank/visual-mode paths (`y`,
    /// `Ctrl+Y`, `hjkl` row-bound clamping) can reach the SAME
    /// width/geometry `handle_mouse` receives directly as a parameter,
    /// without threading a `Rect` through `handle_key`'s 30+ existing call
    /// sites. Updated every render inside `rebuild_chip_hit_test` (called
    /// once per frame from `ui.rs::render_transcript`) — one render tick of
    /// staleness on a live resize is the same tolerance `chip_hit_test`
    /// itself already accepts.
    transcript_area: std::sync::Mutex<Rect>,

    /// Phase 36.6.4 Plan 10 Task 2 (G-08 closure): single-entry memo behind
    /// `transcript_measurement` — replaced (never accumulated) on every
    /// content or width change, so memory is bounded by one
    /// transcript-sized snapshot. Keyed on `MeasureKey`, itself derived
    /// entirely from the SAME `transcript_render_units()` enumeration the
    /// render walks (`transcript_content_fingerprint`) — there is no
    /// hand-maintained dirty flag, frame counter or revision field to
    /// forget to update. `App` renders via `&App`, so this needs interior
    /// mutability, mirroring `chip_hit_test`/`transcript_area` above.
    transcript_measure_cache: std::sync::Mutex<Option<(MeasureKey, Arc<TranscriptMeasurement>)>>,

    /// Phase 36.6.4 Plan 02 (D-07): the last mouse press's content
    /// position, timestamp, and the click count (1..=3, wraps to 1 at 4) it
    /// resolved to — classified via the pure `selection::classify_click`.
    /// `None` before the first press this session. Read-and-overwritten by
    /// `handle_mouse`'s `Down(Left)` arm on every press; reset to `None` on
    /// a chip-rect press (chip clicks never participate in double/
    /// triple-click counting — see `handle_mouse`'s doc comment).
    last_press: Option<(selection::ContentPos, Instant, u8)>,

    /// Phase 36.6.4 Plan 02 (D-04, UI-SPEC §2): transient copy-confirmation
    /// override for the status-line hint slot — `(toast_text,
    /// expires_at_knight_rider_tick)`. `None` = no active confirmation (the
    /// normal `status.hint` shows unmodified). Set by `yank_selection` on a
    /// successful or truncated write; cleared by `on_tick` once
    /// `knight_rider_tick` reaches the expiry — a one-shot window on the
    /// EXISTING 100ms frame tick (Motion Contract: zero new animated
    /// primitives). Never set on a write failure (that path renders a
    /// transcript line instead, per D-04) or an empty/no-op yank.
    copy_confirmation: Option<(String, u64)>,

    /// Phase 46.7 Plan 07 (D-17): browser-launch hook for
    /// `ChipAction::OpenArtifactUrl`. Defaults to the project's standard
    /// `open::that` launcher (matches the `auth_cmd.rs`/`pkce.rs` precedent).
    /// Swapped for a no-op recorder in `handle_mouse_chip_tests` so unit
    /// tests never actually launch a browser window.
    opener: BrowserOpener,

    /// Phase 36.6.4 Plan 08 (gap-closure: honest clipboard feedback):
    /// clipboard-yank hook, mirroring `opener`'s injectable pattern.
    /// Defaults to `selection::yank` (real OSC52 write, real `pbcopy`
    /// attempt, real environment-based capability detection). Swapped in
    /// `visual_mode_tests` for a closure returning an explicit `Supported`
    /// `ClipboardOutcome::Written` so the toast-wording assertions are
    /// deterministic regardless of the test host's real `TERM_PROGRAM` or
    /// OS (production `selection::yank` reads real env and, on macOS,
    /// invokes real `pbcopy` — neither is a stable input to assert an
    /// exact toast string against in CI).
    clipboard_yank: ClipboardYankFn,

    /// Phase 36.3.12 Plan 10 (WR-01): see `AppDeps.approvals_store` doc — shared
    /// process-lifetime store consulted by every `spawn_turn` gating closure.
    pub approvals_store: Arc<ironhermes_core::ApprovalsStore>,

    /// Phase 36.6.2 Plan 01: which overlay (if any) is currently active.
    /// `None` = no overlay showing (base frame only). Exactly one overlay
    /// may be active at a time (never a stack) — see `overlay.rs`'s
    /// `OverlayKind` and UI-SPEC §4 "Overlay exclusivity".
    pub active_overlay: Option<OverlayKind>,
    /// Phase 36.6.2 Plan 01: live filter query for the Skills Hub overlay.
    /// Cleared on close (Esc or Ctrl+K toggle-off) and on Enter-insert.
    pub skills_hub_filter: String,
    /// Phase 36.6.2 Plan 01: selected index into the Skills Hub's FILTERED
    /// list (`overlay::skills_hub_filtered`, not the full registry) —
    /// clamped on every filter keystroke (T-36.6.2-01-02).
    pub skills_hub_selected: usize,

    /// Phase 36.6.3 Plan 01 (TUI-INPUT-01, D-03): selected index into the
    /// command palette's filtered, SELECTABLE row range
    /// (`palette::visible_command_count`, not the raw match count — the
    /// overflow hint row is never a target). Clamped on every keystroke that
    /// changes the textarea while the palette is showing
    /// (`clamp_palette_selected`). NOT a companion open/closed flag — the
    /// palette's visibility is derived live from the textarea via
    /// `palette::palette_query` (see that fn's doc for why no such flag
    /// exists).
    pub palette_selected: usize,

    /// Phase 36.6.3 Plan 03 (TUI-INPUT-02, D-06/D-07): live filter query for
    /// the `/model`/`/provider` picker overlay. Cleared on open, on Esc
    /// filter-clear/close/step-back, and on a successful apply. Mirrors
    /// `skills_hub_filter`'s ownership convention (mutable state lives on
    /// `App`, never inside the `OverlayKind::ModelPicker` variant itself).
    pub model_picker_filter: String,
    /// Phase 36.6.3 Plan 03: selected index into the picker's FILTERED list
    /// for whichever step is active (`overlay::model_picker_providers_filtered`
    /// at `PickerStep::Provider`/`ProviderOnly`, `model_picker_models_filtered`
    /// at `PickerStep::Model`) — clamped on every filter-mutating keystroke,
    /// reset to 0 on step transitions (mirrors `skills_hub_selected`).
    pub model_picker_selected: usize,

    /// Phase 36.6.2 Plan 02 (D-01): whether the expanded thinking pane is
    /// showing. `false` (default) = today's unchanged 4-chunk layout;
    /// `true` prepends the `Length(8)` thinking pane above the transcript.
    /// Toggled by Ctrl+T.
    pub thinking_expanded: bool,
    /// Phase 36.6.2 Plan 02 (D-02 refinement): buffered real turn-activity
    /// lines — `ToolCall`/`ToolProgress`/`ToolResult` + status transitions
    /// (Started/Finished/Error/Cancelled) — rendered into the expanded
    /// thinking pane. Source-agnostic strings only; NOT literal model
    /// chain-of-thought (no data source exists for that in this pipeline).
    /// Buffering happens regardless of `thinking_expanded`. Cleared at the
    /// start of every turn (`submit`/`submit_voice_text`/`maybe_drain_queue`).
    pub thinking_lines: Vec<String>,

    // ── Phase 36.6.2 Plan 03 (TUI-02): approval/secret/sudo plumbing ─────────
    /// Sender half of the approval channel, cloned by `spawn_turn` to build the
    /// per-turn `TuiApprovalGate`. Wired in `run_app_inner` at startup.
    pub approval_tx: Option<UnboundedSender<ApprovalRequest>>,
    /// Receiver half — drained by `recv_approval_request` in `run_app_inner`'s
    /// `select!`, mirroring the `pending_rx`/`recv_pending` precedent.
    pub approval_rx: Option<UnboundedReceiver<ApprovalRequest>>,
    /// The stashed `oneshot::Sender` for the CURRENTLY-surfaced approval request.
    /// `handle_key`'s `[y]`/`[n]`/`[s]`/Esc arms take it out and fire the
    /// `ApprovalOutcome` back to the awaiting `TuiApprovalGate::request_approval`.
    pub pending_approval_resolve: Option<tokio::sync::oneshot::Sender<ApprovalOutcome>>,
    /// FIFO queue of approval requests that arrived while another overlay was
    /// already active (`active_overlay` holds exactly one at a time). The front
    /// is surfaced when the current overlay resolves; the footer shows
    /// `(+N more pending)` for the rest (UI-SPEC §2 queue discipline).
    pub approval_queue: Vec<ApprovalRequest>,

    // ── Phase 41.1 Plan 10 (G-41.1-1): clarify overlay plumbing ──────────────
    /// Sender half of the clarify channel, cloned by `spawn_turn` to build the
    /// per-turn `TuiClarifyDispatcher`. Wired in `run_app_inner` at startup —
    /// mirrors `approval_tx`.
    pub clarify_tx: Option<UnboundedSender<ClarifyRequest>>,
    /// Receiver half — drained by `recv_clarify_request` in `run_app_inner`'s
    /// `select!`, mirroring `approval_rx`.
    pub clarify_rx: Option<UnboundedReceiver<ClarifyRequest>>,
    /// The SHARED registry every turn's `ClarifyTool` and this `App` use to
    /// route an answer back to the suspended turn that inserted it. MUST be
    /// the SAME `Arc` cloned into `spawn_turn`'s `messaging_wiring` — a
    /// fresh-per-turn `PendingClarifyRegistry::new()` there would never
    /// receive `App`'s answer (see `.planning/debug/41.1-tui-interactive-render-corruption.md`).
    pub clarify_registry: Arc<PendingClarifyRegistry>,
    /// FIFO queue of clarify requests that arrived while another overlay was
    /// already active — mirrors `approval_queue`'s never-drop discipline.
    pub clarify_queue: Vec<ClarifyRequest>,
    /// Selected row index into the currently-surfaced `OverlayKind::Clarify`'s
    /// choices — mirrors `skills_hub_selected`/`palette_selected`.
    pub clarify_selected: usize,

    /// Phase 36.6.2 Plan 04 (TUI-02, D-08/D-09): vertical scroll offset for
    /// the `?` Help overlay. Reset to 0 whenever Help is (re)opened; clamped
    /// in `handle_help_key`'s PageDown arm so it never scrolls past the last
    /// registered keybinding entry (mirrors `transcript_scroll`'s discipline,
    /// scoped to `OverlayKind::Help` instead of the transcript).
    pub help_scroll: u16,

    /// Phase 41.1 Plan 02 (D-01, UI-SPEC §C / key_link): `self.history` indices
    /// whose `Role::User` content is a BARE-invoke synthetic skill trigger —
    /// model-facing turn content that must NEVER render as a user bubble. Only
    /// the DIM run-turn meta chip (a separate `Role::System` line) is
    /// user-visible for a bare invoke. `transcript_text` skips these indices.
    /// Argued invokes are the user's OWN typed words and are never recorded
    /// here (they render normally). Cleared whenever `self.history` is cleared.
    pub skill_run_hidden_indices: std::collections::HashSet<usize>,

    /// Phase 36.6.4 Plan 03 (D-09..D-11, TUI-BANG-01): completed (or
    /// in-flight) `!` shell-command runs. Rendered as directly-styled
    /// transcript lines (NOT a new `Role`) via `shell_bang::shell_block_lines`
    /// — mirrors the `sent_attachment_chips`/`captured_artifacts` chip-append
    /// convention (`transcript_render_text` appends these AFTER the existing
    /// chip rows). Flat + append-only.
    pub shell_runs: Vec<shell_bang::ShellRun>,

    /// Phase 36.6.4 Plan 03 (D-11/D-16 `must_haves.prohibitions`): `self.history`
    /// indices whose content is a shell-run's captured-output text (pushed by
    /// `apply_shell_outcome` so follow-up questions work, D-11) — rendered
    /// EXCLUSIVELY via `shell_runs`/`shell_block_lines`'s custom Magenta/Red
    /// styling, never via the normal per-message System/DarkGray loop in
    /// `transcript_text` (which would double-render it). Mirrors
    /// `skill_run_hidden_indices`'s "model-facing content, not a second
    /// rendered bubble" precedent. Cleared whenever `self.history` is cleared.
    pub shell_history_hidden_indices: std::collections::HashSet<usize>,
}

/// Signature for `App::opener` — factored into a type alias per
/// `clippy::type_complexity`.
type BrowserOpener = Box<dyn Fn(&str) -> std::io::Result<()>>;

/// Signature for `App::clipboard_yank` — factored into a type alias per
/// `clippy::type_complexity`. Mirrors `BrowserOpener`'s injectable-field
/// pattern: production defaults to `selection::yank` (real OSC52/pbcopy
/// I/O and real environment detection); App-level tests that need a
/// deterministic `TerminalClipboardCaps` verdict for the toast wording
/// swap this field, the same way `handle_mouse_chip_tests` swaps `opener`
/// for a no-op recorder (Plan 08, gap-closure: honest clipboard feedback).
type ClipboardYankFn = Box<dyn Fn(&str) -> selection::ClipboardOutcome>;

/// Phase 36.6.4 Plan 07 (G-01/G-02/G-06 closure): the sentinel line
/// `transcript_rendered_plain_rows` appends after the real transcript
/// content, then locates by scanning the scratch render for it — the row it
/// lands on IS the true rendered height above it. Plain ASCII (not a
/// zero/ambiguous-width Unicode codepoint) so `ratatui`'s word-wrap and
/// `unicode-width` measurement never treat it specially. Distinctive enough
/// that a model reply or `!` shell output cannot produce it by accident. It
/// has no whitespace, so it never SPLITS at a word boundary — but at a
/// narrow `width` it CAN character-wrap across several rows like any other
/// oversized word; the search below accounts for that by scanning the
/// concatenation of rows, not a single row in isolation.
const TRANSCRIPT_MEASURE_SENTINEL: &str = "IRONHERMES_TRANSCRIPT_MEASURE_SENTINEL_9f2b7a";

// ── Phase 36.6.4 Plan 10 (G-08 closure): shipping-path work counters ───────
//
// THREAD-LOCAL `Cell<u64>`s, always compiled (never behind `cfg(test)` or
// the `test-support` feature) — a performance budget asserted against a
// differently-compiled path is the same false confidence that shipped the
// G-08 regression. Every counter is written with exactly ONE `bump` per
// measurement (accumulated into a plain local first), so the always-on
// instrumentation costs a handful of thread-local cell writes per
// measurement, never per cell.
//
// POST-MERGE FIX: these were process-global `AtomicU64`s until this fix,
// which made the ten counter-asserting tests below (and in `ui.rs`) racy
// under a default `cargo test` — any test rendering a transcript
// concurrently mutated the SAME globals another test had just reset via
// `reset_transcript_measure_stats()`. Design (b) from the post-merge
// checkpoint: thread-local storage, chosen over a shared `Mutex` because it
// requires no serialization between tests at all and needs no poisoning
// recovery. This is safe because the TUI renders on exactly ONE thread in
// production — `event_loop::run_app_inner`'s single event-loop task calls
// `terminal.draw` synchronously (never across an `.await`), and nothing in
// `measure_transcript_uncached`/`transcript_render_units`/
// `transcript_measurement` spawns a thread — so a thread-local counter
// observes the identical real shipping-path work a process-global one did
// on that thread; `transcript_measure_stats()` read from the render thread
// still reports genuine production work. Every one of the ten failing
// tests (verified by grep) is a plain synchronous `#[test]` — never
// `#[tokio::test]` or otherwise thread-hopping — and Rust's default test
// harness runs each `#[test]` function to completion on its own dedicated
// OS thread, so each test's reset/measure/read sequence now only ever
// touches that one thread's cells: concurrent sibling tests rendering their
// own transcripts can no longer pollute this test's counts.
// `transcript_measure_stats()`/`reset_transcript_measure_stats()` are only
// ever called from tests (grep confirms zero production call sites), so
// there is no cross-thread aggregation behavior in the shipping binary this
// change could silently alter.
thread_local! {
    static TRANSCRIPT_RENDERS: Cell<u64> = const { Cell::new(0) };
    static TRANSCRIPT_SCRATCH_ROWS: Cell<u64> = const { Cell::new(0) };
    static TRANSCRIPT_CELLS_WALKED: Cell<u64> = const { Cell::new(0) };
    static TRANSCRIPT_ROW_LOOKUPS: Cell<u64> = const { Cell::new(0) };
    static TRANSCRIPT_UNIT_BUILDS: Cell<u64> = const { Cell::new(0) };
    static TRANSCRIPT_CACHE_HITS: Cell<u64> = const { Cell::new(0) };
    static TRANSCRIPT_CACHE_MISSES: Cell<u64> = const { Cell::new(0) };
}

/// Add `delta` to a thread-local counter cell — the `Cell<u64>` equivalent
/// of `AtomicU64::fetch_add`, factored out so every call site stays a
/// one-line `TRANSCRIPT_X.with(|c| bump(c, n))`.
fn bump(cell: &Cell<u64>, delta: u64) {
    cell.set(cell.get() + delta);
}

/// Snapshot of the transcript measurement's per-frame work counters —
/// read off the shipping path, not a test double (Phase 36.6.4 Plan 10's
/// performance budget is asserted against this struct's fields).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TranscriptMeasureStats {
    pub renders: u64,
    pub scratch_rows: u64,
    pub cells_walked: u64,
    pub row_lookups: u64,
    pub unit_builds: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
}

/// Read the current cumulative transcript-measurement work counters.
pub fn transcript_measure_stats() -> TranscriptMeasureStats {
    TranscriptMeasureStats {
        renders: TRANSCRIPT_RENDERS.with(Cell::get),
        scratch_rows: TRANSCRIPT_SCRATCH_ROWS.with(Cell::get),
        cells_walked: TRANSCRIPT_CELLS_WALKED.with(Cell::get),
        row_lookups: TRANSCRIPT_ROW_LOOKUPS.with(Cell::get),
        unit_builds: TRANSCRIPT_UNIT_BUILDS.with(Cell::get),
        cache_hits: TRANSCRIPT_CACHE_HITS.with(Cell::get),
        cache_misses: TRANSCRIPT_CACHE_MISSES.with(Cell::get),
    }
}

/// Zero every transcript-measurement work counter (on the CALLING thread —
/// see the thread-local rationale above `TRANSCRIPT_RENDERS`) — tests call
/// this before the window they want to measure so earlier setup work
/// doesn't pollute the assertion.
pub fn reset_transcript_measure_stats() {
    TRANSCRIPT_RENDERS.with(|c| c.set(0));
    TRANSCRIPT_SCRATCH_ROWS.with(|c| c.set(0));
    TRANSCRIPT_CELLS_WALKED.with(|c| c.set(0));
    TRANSCRIPT_ROW_LOOKUPS.with(|c| c.set(0));
    TRANSCRIPT_UNIT_BUILDS.with(|c| c.set(0));
    TRANSCRIPT_CACHE_HITS.with(|c| c.set(0));
    TRANSCRIPT_CACHE_MISSES.with(|c| c.set(0));
}

/// The single product of one linear measurement pass — rows, per-unit
/// offsets and content height all come from here; nothing else in this
/// file computes any of the three independently (Phase 36.6.4 Plan 10,
/// G-08 closure — collapses Plan 07's two sentinel renders into one).
#[derive(Debug, Clone)]
pub struct TranscriptMeasurement {
    pub width: usize,
    pub units: Vec<TranscriptUnit>,
    pub rows: Vec<String>,
    pub offsets: Vec<(usize, usize)>,
}

impl TranscriptMeasurement {
    /// Content height in rows — the sole source `transcript_total_line_count`
    /// (and through it `transcript_max_scroll`) reads.
    pub fn height(&self) -> usize {
        self.rows.len()
    }

    /// Rebuild the `Paragraph` text from `units` — the render path's text
    /// source, so the drawn content and the measured content can never be
    /// two different derivations.
    pub fn text(&self) -> Text<'static> {
        Text::from(self.units.iter().map(|unit| unit.line.clone()).collect::<Vec<_>>())
    }
}

/// The memo's key (Phase 36.6.4 Plan 10, Task 2, G-08 closure). A hit
/// requires ALL FOUR fields to agree — `fingerprint` alone (a 64-bit hash)
/// is deliberately not trusted on its own; `unit_count`/`total_display_width`
/// are cheap independent corroborators a collision would also have to
/// match. `fingerprint` is computed FROM the same
/// `transcript_render_units()` enumeration the render walks (see
/// `transcript_content_fingerprint`), so there is no hand-maintained dirty
/// flag, frame counter or revision field anywhere for this key to forget to
/// update.
#[derive(Debug, Clone, PartialEq)]
struct MeasureKey {
    fingerprint: u64,
    unit_count: usize,
    total_display_width: usize,
    width: usize,
}

/// Sum of every unit's own rendered display width — one of `MeasureKey`'s
/// four corroborating fields. Computed directly over `TranscriptUnit::line`
/// (not the sentinel-interleaved text `measure_transcript_uncached` builds),
/// so this never triggers a render of its own.
fn transcript_units_total_display_width(units: &[TranscriptUnit]) -> usize {
    units
        .iter()
        .map(|unit| {
            unit.line
                .spans
                .iter()
                .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
                .sum::<usize>()
        })
        .sum()
}

/// The memo key's content fingerprint (Phase 36.6.4 Plan 10, Task 2, G-08
/// closure) — hashes, in order: the unit count, then per unit its `group`
/// discriminant, its `history_anchor` (Phase 36.6.4 Plan 12, G-09 closure),
/// every span's `content` bytes, the `plain` field, and whether `action` is
/// present. Computed FROM `transcript_render_units()`'s own output, so
/// appending a sixth content group to that enumeration changes this
/// fingerprint by construction — there is nothing else to remember to
/// update. Hashing `history_anchor` means an enumeration that differs ONLY
/// in unit order (same groups, same content, different anchors) produces a
/// different fingerprint, so a reorder can never be served stale cached
/// geometry.
fn transcript_content_fingerprint(units: &[TranscriptUnit]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    units.len().hash(&mut hasher);
    for unit in units {
        unit.group.hash(&mut hasher);
        unit.history_anchor.hash(&mut hasher);
        for span in &unit.line.spans {
            span.content.as_bytes().hash(&mut hasher);
        }
        unit.plain.hash(&mut hasher);
        unit.action.is_some().hash(&mut hasher);
    }
    hasher.finish()
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
            scroll_view_state: std::sync::Mutex::new(ScrollViewState::new()),
            auto_follow: true,
            selection: None,
            selection_mode: selection::SelectionMode::Idle,
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
            // Phase 36.17.3 (D-03): queue + fixed TUI key + Arc<AtomicBool> pause toggle.
            queue: deps.queue,
            queue_key: SessionKey::new(Platform::Local, "local").with_user("local"),
            queue_paused: deps.queue_paused,
            skin: deps.skin,
            // Phase 39.1 Plan 04: TurnRegistry + ConcurrencyLayer replace AtomicBool gate.
            turn_registry: Arc::new(ironhermes_core::concurrency::TurnRegistry::new()),
            concurrency: ironhermes_core::concurrency::ConcurrencyLayer::new(
                ironhermes_core::config::ConcurrencyConfig::default().session_turn_cap,
                ironhermes_core::config::ConcurrencyConfig::default().global_turn_ceiling,
            ),
            in_flight: Vec::new(),
            // Throwaway — CommandContext::new still requires it; Plan 06 removes the field.
            agent_running: Arc::new(AtomicBool::new(false)),
            agent_runtime: deps.agent_runtime,
            hook_registry: deps.hook_registry,
            mcp_manager: deps.mcp_manager,
            memory_manager: deps.memory_manager,
            subagent_registry: deps.subagent_registry,
            process_registry: deps.process_registry,
            command_router: deps.command_router,
            client: deps.client,
            registry: deps.registry,
            browser_session: deps.browser_session,
            mouse_capture_enabled: deps.mouse_capture_enabled,
            // Phase 22.4.2 Plan 00: D-08 subsystem handles
            state_store: deps.state_store,
            resolver: deps.resolver,
            context_compressor: deps.context_compressor,
            personality_overlay: deps.personality_overlay,
            // Phase 21.8.3.1 D-02: renamed to active_personality_overlay (session-persistent)
            active_personality_overlay: None,
            // Phase 22.4.2.1 Plan 01: cron store — None by default (gateway is primary cron host)
            cron_store: None,
            // Phase 25.2 Plan 15 follow-up: toolset session handle for /toolset slash UI
            toolset_session: deps.toolset_session,
            // Phase 25.3 D-W-2 / D-T-3: Workspace + TrajectoryWriter for slash dispatch
            workspace: deps.workspace,
            trajectory_writer: deps.trajectory_writer,
            // Phase 21.8.2: forward skill_registry from deps.
            skill_registry: deps.skill_registry,
            // Phase 21.8.2 Plan 03: forward new fields.
            skills_config: deps.skills_config,
            pending_skill_overlays: Vec::new(),
            // Phase 36.17.8 (D-08): voice state — idle at session start.
            voice: crate::tui_rata::voice_state::VoiceState::new(),
            // Phase 36.17.8 (D-08): transcript channel — None until first Ctrl+B.
            voice_transcript_rx: None,
            // Phase 36.17.8: no turn submitted yet — default to non-voice.
            last_turn_was_voice: false,
            // Phase 46.7 Plan 06: no attachments queued at session start.
            pending_attachments: Vec::new(),
            captured_artifacts: Arc::new(std::sync::Mutex::new(Vec::new())),
            last_submitted_text: String::new(),
            // Phase 46.7 Plan 07: no chips/hit-test entries at session start.
            sent_attachment_chips: Vec::new(),
            // Phase 36.6.4 Plan 05: no image chips at session start; the
            // Picker was built once in the startup window (deps.picker) —
            // never lazily here.
            image_chips: Vec::new(),
            picker: deps.picker,
            image_decode: Arc::new(std::sync::Mutex::new(None)),
            chip_hit_test: std::sync::Mutex::new(Vec::new()),
            transcript_area: std::sync::Mutex::new(Rect::default()),
            // Phase 36.6.4 Plan 10 Task 2: no measurement cached at session
            // start — the first `transcript_measurement` call is always a
            // cache miss.
            transcript_measure_cache: std::sync::Mutex::new(None),
            last_press: None,
            copy_confirmation: None,
            opener: Box::new(|url: &str| open::that(url)),
            clipboard_yank: Box::new(selection::yank),
            // Phase 36.3.12 Plan 10 (WR-01): forward the process-lifetime store from deps.
            approvals_store: deps.approvals_store,
            // Phase 36.6.2 Plan 01: no overlay active at session start.
            active_overlay: None,
            skills_hub_filter: String::new(),
            skills_hub_selected: 0,
            // Phase 36.6.3 Plan 01: no palette selection at session start —
            // the palette itself isn't showing until the textarea says so.
            palette_selected: 0,
            // Phase 36.6.3 Plan 03: no picker filter/selection at session
            // start — the picker itself isn't showing until active_overlay
            // says so.
            model_picker_filter: String::new(),
            model_picker_selected: 0,
            // Phase 36.6.2 Plan 02: thinking pane starts collapsed with no
            // buffered activity.
            thinking_expanded: false,
            thinking_lines: Vec::new(),
            // Phase 36.6.2 Plan 03: approval channel wired in run_app_inner at
            // startup; no request in flight and an empty queue at session start.
            approval_tx: None,
            approval_rx: None,
            pending_approval_resolve: None,
            approval_queue: Vec::new(),
            // Phase 41.1 Plan 10 (G-41.1-1): clarify channel wired in
            // run_app_inner at startup; the registry is constructed once here
            // and its Arc clone is threaded into spawn_turn's messaging_wiring
            // so the turn and App share the SAME instance.
            clarify_tx: None,
            clarify_rx: None,
            clarify_registry: Arc::new(PendingClarifyRegistry::new()),
            clarify_queue: Vec::new(),
            clarify_selected: 0,
            // Phase 36.6.2 Plan 04: Help overlay starts closed with no scroll.
            help_scroll: 0,
            // Phase 41.1 Plan 02: no hidden synthetic skill triggers at start.
            skill_run_hidden_indices: std::collections::HashSet::new(),
            // Phase 36.6.4 Plan 03: no `!` shell runs at session start.
            shell_runs: Vec::new(),
            shell_history_hidden_indices: std::collections::HashSet::new(),
        }
    }

    // ── Scroll helpers ─────────────────────────────────────────────────────────

    /// Read the current vertical scroll offset. Thin accessor over
    /// `scroll_view_state` — the SOLE offset authority (Phase 36.6.4 Plan 01,
    /// D-02). `&self` (not `&mut self`) so render-path callers (`ui.rs`,
    /// `scroll_indicator`) can call it through `&App`.
    pub fn transcript_scroll(&self) -> u16 {
        self.scroll_view_state
            .lock()
            .map(|guard| guard.offset().y)
            .unwrap_or(0)
    }

    /// Set the vertical scroll offset directly. Used by the thin wrappers
    /// below and by tests that previously poked the old `transcript_scroll`
    /// field directly.
    fn set_transcript_scroll(&mut self, y: u16) {
        // `&mut self` gives direct access via `get_mut()` — no lock needed
        // when we already hold the unique borrow; `get_mut()` still returns
        // a `LockResult` (poison tracking survives `&mut` access), so
        // recover via `into_inner()` on the (practically unreachable, this
        // is a single-threaded UI struct) poisoned case.
        let state = self
            .scroll_view_state
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut offset = state.offset();
        offset.y = y;
        state.set_offset(offset);
    }

    /// Disable auto-follow and scroll up by `lines` rows.
    pub fn scroll_up(&mut self, lines: u16) {
        self.auto_follow = false;
        let new_y = self.transcript_scroll().saturating_sub(lines);
        self.set_transcript_scroll(new_y);
    }

    /// Scroll down by `lines` rows (auto-follow re-enables via `reconcile_scroll`).
    pub fn scroll_down(&mut self, lines: u16) {
        let new_y = self.transcript_scroll().saturating_add(lines);
        self.set_transcript_scroll(new_y);
    }

    /// Jump to the top of the transcript.
    pub fn scroll_to_top(&mut self) {
        self.auto_follow = false;
        self.set_transcript_scroll(0);
    }

    /// Re-engage auto-follow so the viewport snaps to the newest line on
    /// the next render tick. Symmetric counterpart of `scroll_to_top`.
    ///
    /// Used by `apply_slash_outcome` so System-role messages produced by
    /// slash commands (notably `/skills reload` and SKILL-13 fallback) are
    /// visible on the same render tick. Mirrors the agent-turn reference
    /// behavior in `submit()` (sets `auto_follow = true`); also resets the
    /// scroll offset to 0 for symmetry with `scroll_to_top`.
    /// `reconcile_scroll` (called next render from `ui.rs`) will clamp the
    /// offset to `max` because `auto_follow == true`.
    pub fn scroll_to_bottom(&mut self) {
        self.auto_follow = true;
        self.set_transcript_scroll(0);
    }

    /// Human-readable scroll indicator for the border title.
    pub fn scroll_indicator(&self, area: Rect) -> String {
        let max = self.transcript_max_scroll(area);
        self.scroll_indicator_body(max)
    }

    /// Sibling of `scroll_indicator` that takes the frame's ALREADY-COMPUTED
    /// transcript height (Phase 36.6.4 Plan 10, G-08 closure) instead of
    /// deriving `transcript_max_scroll` itself — so the border title never
    /// triggers its own transcript measurement when the caller
    /// (`ui.rs::render_transcript`) already has one for this frame.
    pub fn scroll_indicator_for_height(&self, area: Rect, height: usize) -> String {
        let max = self.transcript_max_scroll_from_height(area, height);
        self.scroll_indicator_body(max)
    }

    /// Shared indicator-text body for `scroll_indicator`/
    /// `scroll_indicator_for_height` — both compute `max` differently but
    /// render the exact same three-way indicator from it.
    fn scroll_indicator_body(&self, max: u16) -> String {
        let current = self.transcript_scroll();
        if self.auto_follow {
            "live".to_string()
        } else if self.pending_rx.is_some() || self.assistant_buffer.is_some() {
            // D-11: paused indicator — derived from existing state (Option B per RESEARCH §Pattern 5).
            // n = unseen scroll units below current viewport. Resets on resize because max changes
            // with area height, which is acceptable per Claude's discretion.
            let n = max.saturating_sub(current);
            format!("paused ({n} new lines below)")
        } else {
            format!("scroll {}/{}", current, max)
        }
    }

    /// Clamp the scroll offset to `max`; re-enable auto-follow if at bottom.
    ///
    /// Phase 36.6.4 Plan 01/02 (Pitfall 2): this is the SAME clamp discipline
    /// as before the scrollview migration, just re-pointed at
    /// `scroll_view_state` — a terminal resize that shrinks `max` below the
    /// current offset snaps back to the new bottom rather than stranding the
    /// view past it.
    pub fn reconcile_scroll(&mut self, area: Rect) {
        let max = self.transcript_max_scroll(area);
        let current = self.transcript_scroll();
        if self.auto_follow {
            self.set_transcript_scroll(max);
        } else if current >= max {
            self.set_transcript_scroll(max);
            self.auto_follow = true;
        }
    }

    /// Maximum scroll offset for the given viewport. Keeps its pre-Plan-10
    /// signature — `reconcile_scroll` (event_loop.rs) and the tests below
    /// call this directly, and it is still the single source of truth for a
    /// standalone `area`.
    pub fn transcript_max_scroll(&self, area: Rect) -> u16 {
        // Pass the inner width (excluding the 1-char border on each side) so that
        // transcript_line_count wraps at the same column ratatui's Paragraph does.
        //
        // Phase 46.7 Plan 07: uses `transcript_total_line_count` (base +
        // chip rows), not the base-only `transcript_line_count`, so a
        // scrolled-to-bottom viewport actually reaches the true bottom when
        // attachment/artifact chips are appended — otherwise auto-follow
        // would clamp short of the chip rows and they'd be unreachable.
        let inner_width = inner_transcript_width(area);
        let height = self.transcript_measurement(inner_width).height();
        self.transcript_max_scroll_from_height(area, height)
    }

    /// Sibling of `transcript_max_scroll` that takes an ALREADY-MEASURED
    /// height (Phase 36.6.4 Plan 10, G-08 closure) instead of deriving one —
    /// used by `scroll_indicator_for_height` so the border title shares the
    /// frame's one measurement instead of triggering its own.
    pub(crate) fn transcript_max_scroll_from_height(&self, area: Rect, height: usize) -> u16 {
        let total = height as u32;
        let visible = area.height.saturating_sub(2) as u32;
        total.saturating_sub(visible).min(u16::MAX as u32) as u16
    }

    /// Shared predicate for whether history row `idx` is hidden from the
    /// normal per-message transcript loop (Phase 36.6.4 Plan 07, G-01
    /// closure). `transcript_text` and `transcript_line_count` both call
    /// this SAME function so the rendered rows and the counted rows can
    /// never diverge — a hidden shell-run copy (D-11) or a bare skill
    /// trigger (D-01/D-02 of 41.1 Plan 02) contributes zero rows to either.
    fn history_row_is_hidden(&self, idx: usize) -> bool {
        self.skill_run_hidden_indices.contains(&idx)
            || self.shell_history_hidden_indices.contains(&idx)
    }

    /// Total wrapped-line count across all history entries + streaming buffer.
    ///
    /// `width` must be the **inner** render width (border excluded, i.e.
    /// `area.width - 2`) so the count matches what ratatui's Paragraph widget
    /// actually wraps at. Callers that pass the outer area width will get a
    /// count that is slightly too low, causing auto-follow to stop short of the
    /// true visual bottom.
    ///
    /// For line `i == 0` of each message the role prefix ("You: " / "Hermes: "
    /// etc.) shares the first row, so the row count is
    /// `ceil((prefix_len + body_chars) / width)` — not `ceil(body / (width -
    /// prefix))`. The two formulas diverge at certain line lengths. See D-06/D-07
    /// in `.planning/phases/21.8.3-tui-streaming-scroll-fix-and-scrollbar/`.
    ///
    /// Phase 36.6.4 Plan 07 (G-01): skips both `skill_run_hidden_indices` and
    /// `shell_history_hidden_indices` via `history_row_is_hidden` — the SAME
    /// predicate `transcript_text` applies — so a hidden history row never
    /// contributes a row here while also never rendering there.
    pub fn transcript_line_count(&self, width: usize) -> usize {
        let mut total = 0usize;
        for (idx, msg) in self.history.iter().enumerate() {
            if self.history_row_is_hidden(idx) {
                continue;
            }
            let (role_label, color) = role_style(msg);
            // Mirror transcript_text() (line 785) — skip messages whose role_style returns None.
            // No role currently returns None post-22.4-17; this is a structural guard for future
            // Role variants. See .planning/phases/21.8.3.../21.8.3-RESEARCH.md Pitfall 1.
            let Some(_color) = color else { continue };
            let body = render_message_body(msg);
            for (i, line) in body.lines().enumerate() {
                let rows = if i == 0 {
                    // First row: prefix + body share the same terminal row.
                    // Build the full first-line string and run word_wrapped_line_count on it.
                    // prefix is ASCII ("You: ", "Hermes: " etc.) so len() == display width.
                    // Fixes D-01: word-wrap semantics + unicode display width (RESEARCH §3).
                    let first_line = format!("{}: {}", role_label, line);
                    word_wrapped_line_count(&first_line, width)
                } else {
                    word_wrapped_line_count(line, width)
                };
                total = total.saturating_add(rows);
            }
        }
        if let Some(buf) = &self.assistant_buffer {
            // assistant_buffer renders with "Hermes: " prefix on line 0 (transcript_text:807-819)
            for (i, line) in buf.lines().enumerate() {
                let rows = if i == 0 {
                    // First row: prefix + body share the same terminal row.
                    // Build the full first-line string and run word_wrapped_line_count on it.
                    // Fixes D-01: word-wrap semantics + unicode display width (RESEARCH §3).
                    let first_line = format!("Hermes: {}", line);
                    word_wrapped_line_count(&first_line, width)
                } else {
                    word_wrapped_line_count(line, width)
                };
                total = total.saturating_add(rows);
            }
        }
        total
    }

    /// Total wrapped-row count of the FULL rendered transcript.
    ///
    /// Phase 36.6.4 Plan 07 (G-01/G-02/G-06 closure): this is now a MEASURED
    /// height, not a hand-maintained sum over `sent_attachment_chips` /
    /// `captured_artifacts` / `image_chips` / `shell_runs`. It defers
    /// entirely to `transcript_rendered_plain_rows`, which renders
    /// `transcript_render_text()` — the SOLE content authority — once, into
    /// a scratch buffer, and reports the row a trailing sentinel line lands
    /// on. No group list is maintained here; a group appended to
    /// `transcript_render_text` is counted automatically, by construction,
    /// without a matching edit in this function.
    ///
    /// `width` MUST be the same inner render width callers pass to
    /// `transcript_line_count`/`rebuild_chip_hit_test` (memory
    /// `feedback_scroll_width_inner`) or the count drifts from what's drawn.
    ///
    /// Phase 36.6.4 Plan 10 (G-08 closure): reads `.height()` directly off
    /// `transcript_measurement` rather than cloning `rows` just to take its
    /// length.
    pub fn transcript_total_line_count(&self, width: usize) -> usize {
        self.transcript_measurement(width).height()
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

        // Esc while vim-style visual mode is active (Phase 36.6.4 Plan 02,
        // D-05) — checked AHEAD of the overlay-close precedence chain below.
        // Visual mode carries no `OverlayKind` (it has no modal chrome), but
        // Esc must still mean "back out of the current modal thing first,"
        // matching the overlay chain's own precedence discipline. Clears the
        // selection and returns to `Idle` — no clipboard write (D-04's
        // silent-cancel contract).
        if key.code == KeyCode::Esc && self.selection_mode == selection::SelectionMode::Visual {
            self.selection_mode = selection::SelectionMode::Idle;
            self.selection = None;
            return;
        }

        // Esc — overlay-close precedence (Phase 36.6.2 Plan 01 foundation),
        // evaluated BEFORE any other arm so an active overlay always wins.
        // `.take()` clears `active_overlay` regardless of which arm matches
        // below, so Plan 03/04 (Approval/Secret/Sudo/Help) can add arms
        // above the `None` default without rewriting it (UI-SPEC §4).
        if key.code == KeyCode::Esc {
            match self.active_overlay.take() {
                Some(OverlayKind::SkillsHub) => {
                    self.skills_hub_filter.clear();
                    self.skills_hub_selected = 0;
                    // CR-01: a queued approval that arrived while the Skills Hub
                    // was open must not be stranded — re-surface it now that
                    // `active_overlay` is clear (T-36.6.2-03-05 queue discipline).
                    self.drain_approval_queue_after_close();
                }
                // Phase 36.6.2 Plan 03 (TUI-02): Esc is fail-closed on every
                // approval-family overlay — Approval/Sudo deny, Secret cancels;
                // all resolve the awaiting request to Denied (never Approved,
                // never a textarea-clear fallthrough). `active_overlay` was just
                // `.take()`n, so `resolve_approval` re-surfaces the next queued
                // request cleanly.
                Some(OverlayKind::Approval { .. })
                | Some(OverlayKind::Sudo { .. })
                | Some(OverlayKind::Secret { .. }) => {
                    self.resolve_approval(ApprovalOutcome::Denied);
                }
                // Phase 41.1 Plan 10 (G-41.1-1): Esc cancels the pending
                // clarify — spawns `clarify_registry.remove` so the suspended
                // tool's own `select!` resolves via its timeout/cancel-token
                // arm (never a fabricated "answer"), then re-surfaces the
                // next queued overlay exactly like every other close path
                // here. `active_overlay` was just `.take()`n, so
                // `drain_approval_queue_after_close`'s guard passes.
                Some(OverlayKind::Clarify { clarify_id, .. }) => {
                    self.cancel_clarify(clarify_id);
                    self.drain_approval_queue_after_close();
                }
                // Phase 36.6.2 Plan 04 (D-08/D-09): Help closes with no side
                // effect — `.take()` already cleared `active_overlay` above.
                // CR-01: same as Skills Hub, a queued approval must re-surface.
                Some(OverlayKind::Help) => {
                    self.drain_approval_queue_after_close();
                }
                // Phase 36.6.3 Plan 03 (TUI-INPUT-02, D-08): filter-clear-first
                // at step 2, then step-back to step 1, then close — "back one
                // level, or close if already at the top" (UI-SPEC). Step 1 and
                // the single-step `/provider` flow always close on Esc,
                // regardless of filter state (matches the Skills Hub
                // precedent — no filter-clear-first there either).
                Some(OverlayKind::ModelPicker {
                    step: PickerStep::Model,
                    selected_provider,
                }) if !self.model_picker_filter.is_empty() => {
                    self.model_picker_filter.clear();
                    self.model_picker_selected = 0;
                    self.active_overlay = Some(OverlayKind::ModelPicker {
                        step: PickerStep::Model,
                        selected_provider,
                    });
                }
                Some(OverlayKind::ModelPicker {
                    step: PickerStep::Model,
                    ..
                }) => {
                    self.model_picker_filter.clear();
                    self.model_picker_selected = 0;
                    self.active_overlay = Some(OverlayKind::ModelPicker {
                        step: PickerStep::Provider,
                        selected_provider: None,
                    });
                }
                Some(OverlayKind::ModelPicker { .. }) => {
                    self.model_picker_filter.clear();
                    self.model_picker_selected = 0;
                    self.drain_approval_queue_after_close();
                }
                // Phase 36.6.4 Plan 05 (D-13): Esc closes with no side
                // effect (mirrors Help/SkillsHub) — `.take()` already
                // cleared `active_overlay` above. Clear the decode state
                // too, so a future open of a (possibly different) image
                // always re-decodes fresh rather than flashing the
                // previous image's stale Ready/Failed state.
                Some(OverlayKind::ImageViewer { .. }) => {
                    if let Ok(mut guard) = self.image_decode.lock() {
                        *guard = None;
                    }
                    self.drain_approval_queue_after_close();
                }
                None => self.clear_textarea(),
            }
            return;
        }

        // Ctrl+K — toggle the Skills Hub (Phase 36.6.2 Plan 01, D-06/D-07/D-08).
        // Unbound today (UI-SPEC "Keybindings — verified conflict-free") —
        // added directly to this hardcoded match per RESEARCH Pitfall 3
        // (KeybindingRegistry::match_key is display-only scaffolding, not a
        // live dispatch path).
        if key.code == KeyCode::Char('k') && key.modifiers == KeyModifiers::CONTROL {
            let closing = matches!(self.active_overlay, Some(OverlayKind::SkillsHub));
            match self.active_overlay {
                Some(OverlayKind::SkillsHub) => self.active_overlay = None,
                None => self.active_overlay = Some(OverlayKind::SkillsHub),
                // Phase 36.6.2 Plan 03 (UI-SPEC §4 overlay exclusivity): Ctrl+K
                // is a no-op while a security-critical approval-family overlay is
                // active — do NOT clear filter/selection or toggle.
                _ => return,
            }
            self.skills_hub_filter.clear();
            self.skills_hub_selected = 0;
            // CR-01: toggling the Skills Hub OFF must re-surface a queued
            // approval; toggling it ON leaves `active_overlay` Some, so the
            // guard inside `drain_approval_queue_after_close` is a no-op there.
            if closing {
                self.drain_approval_queue_after_close();
            }
            return;
        }

        // Ctrl+T — toggle the expanded thinking pane (Phase 36.6.2 Plan 02,
        // D-01/D-02). Unbound today (UI-SPEC "Keybindings — verified
        // conflict-free"). Guarded so it is a no-op while a security-critical
        // overlay is active; this plan's `OverlayKind` only has `SkillsHub`
        // (a browse overlay, not security-critical), so the toggle is always
        // allowed today — Plan 03 adds Approval/Secret/Sudo arms here that
        // fall through to a no-op instead of toggling.
        if key.code == KeyCode::Char('t') && key.modifiers == KeyModifiers::CONTROL {
            if matches!(self.active_overlay, None | Some(OverlayKind::SkillsHub)) {
                self.thinking_expanded = !self.thinking_expanded;
            }
            return;
        }

        // Approval-family key sub-router (Phase 36.6.2 Plan 03, TUI-02). While an
        // Approval/Secret/Sudo overlay is active, keys are the allow/deny decision
        // surface — routed here BEFORE the normal dispatch so `[y]`/`[n]`/`[s]`/
        // typing never leak into the textarea or history recall. Esc is handled by
        // the precedence check above (fail-closed deny/cancel).
        if matches!(
            self.active_overlay,
            Some(OverlayKind::Approval { .. } | OverlayKind::Secret { .. } | OverlayKind::Sudo { .. })
        ) {
            self.handle_approval_key(key);
            return;
        }

        // Clarify-active key sub-router (Phase 41.1 Plan 10, G-41.1-1). Same
        // precedence as the approval-family router above — while a Clarify
        // overlay is active, Up/Down/Enter are the answer surface and must
        // never leak into the textarea or history recall. Esc is handled by
        // the precedence check above (cancel).
        if matches!(self.active_overlay, Some(OverlayKind::Clarify { .. })) {
            self.handle_clarify_key(key);
            return;
        }

        // Skills-Hub-active key sub-router — takes priority over the normal
        // dispatch below (Up/Down/Enter mean something different while the
        // Skills Hub is open: selection movement / insert-trigger, never
        // history recall or turn submission).
        if matches!(self.active_overlay, Some(OverlayKind::SkillsHub)) {
            self.handle_skills_hub_key(key);
            return;
        }

        // Help-active key sub-router (Phase 36.6.2 Plan 04, TUI-02, D-08/D-09).
        // Only PageUp/PageDown scroll `help_scroll` while Help is open; every
        // other key (including `?` itself) is a no-op — Help is not a text
        // entry surface. Esc is handled by the precedence check above.
        if matches!(self.active_overlay, Some(OverlayKind::Help)) {
            self.handle_help_key(key);
            return;
        }

        // Model/Provider-picker-active key sub-router (Phase 36.6.3 Plan 03,
        // TUI-INPUT-02, D-06/D-07). Takes priority over `?`/palette/default
        // dispatch below — Up/Down/Enter/Backspace/Char mean something
        // different while the picker is open (selection movement / filter
        // typing / advance-or-apply), never history recall, turn
        // submission, or the Help-open shortcut. Esc is handled by the
        // precedence check above.
        if matches!(self.active_overlay, Some(OverlayKind::ModelPicker { .. })) {
            self.handle_model_picker_key(key);
            return;
        }

        // `?` — open the Help overlay (Phase 36.6.2 Plan 04, D-08/D-09).
        // Reachable only when no overlay is active (approval-family and
        // Skills Hub both return above) — guarded so it NEVER hijacks normal
        // text entry: with any content already in the textarea, `?` falls
        // through to the `_` arm below and types the literal character.
        if key.code == KeyCode::Char('?') && self.textarea.is_empty() {
            self.active_overlay = Some(OverlayKind::Help);
            self.help_scroll = 0;
            return;
        }

        // `v` — enter vim-style visual selection (Phase 36.6.4 Plan 02,
        // D-05). Same guard shape as the `?` Help shortcut immediately
        // above (textarea empty; overlays already returned above this
        // point): with any content already typed, `v` falls through to the
        // `_` arm below and types the literal character. Anchors both
        // endpoints at the current viewport-derived content position (the
        // top-left content cell of the visible transcript) so a
        // keyboard-only selection always starts somewhere on screen.
        if key.code == KeyCode::Char('v') && self.textarea.is_empty() {
            let offset = self
                .scroll_view_state
                .lock()
                .map(|guard| guard.offset())
                .unwrap_or_else(|_| Position::new(0, 0));
            let anchor = selection::ContentPos::new(offset.y as usize, offset.x as usize);
            self.selection_mode = selection::SelectionMode::Visual;
            self.selection = Some(Selection::new_at(anchor));
            return;
        }

        // `h`/`j`/`k`/`l`/arrows extend the cursor, `y` yanks-and-exits —
        // ONLY while visual mode is active (Phase 36.6.4 Plan 02, D-05).
        // Intercepted here, ahead of the match block's unconditional
        // Up/Down history-recall arms, so vim-style movement wins the
        // instant `v` has anchored a selection; typing these letters
        // outside visual mode is completely unaffected (they fall through
        // to the `_` arm below like any other character).
        if self.selection_mode == selection::SelectionMode::Visual {
            use selection::MoveDir;
            let dir = match key.code {
                KeyCode::Char('h') | KeyCode::Left => Some(MoveDir::Left),
                KeyCode::Char('j') | KeyCode::Down => Some(MoveDir::Down),
                KeyCode::Char('k') | KeyCode::Up => Some(MoveDir::Up),
                KeyCode::Char('l') | KeyCode::Right => Some(MoveDir::Right),
                _ => None,
            };
            if let Some(dir) = dir {
                let area = *self
                    .transcript_area
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let width = inner_transcript_width(area);
                let max_row = self.transcript_total_line_count(width).saturating_sub(1);
                if let Some(sel) = self.selection.as_mut() {
                    sel.cursor = selection::move_cursor(sel.cursor, dir, max_row);
                }
                return;
            }
            if key.code == KeyCode::Char('y') {
                let area = *self
                    .transcript_area
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                self.yank_selection(area);
                self.selection_mode = selection::SelectionMode::Idle;
                return;
            }
        }

        // Palette precedence entry (Phase 36.6.3 Plan 01, TUI-INPUT-01,
        // D-03/D-04). Only reachable when `palette_query` is `Some` (i.e. the
        // palette is currently showing, derived live from `self.textarea` —
        // there is no separate open/closed flag to check here). Up/Down/Tab/
        // Enter are intercepted; every other key (Backspace/Char/etc.) falls
        // through unchanged to the default arm below, which mutates
        // `self.textarea` directly and re-clamps `palette_selected`
        // afterward — the palette has no filter buffer of its own, unlike
        // the Skills Hub's `skills_hub_filter`.
        if crate::tui_rata::palette::palette_query(self).is_some() && self.handle_palette_key(key) {
            return;
        }

        match (key.code, key.modifiers) {
            // Ctrl+B — toggle push-to-talk voice capture (D-08 / Phase 36.17.8)
            (KeyCode::Char('b'), KeyModifiers::CONTROL) => self.handle_record_key(),

            // Ctrl+C — double-press state machine (D-10..D-14)
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => self.handle_ctrl_c_key(),

            // Ctrl+Y — yank the current selection from EITHER mode (Phase
            // 36.6.4 Plan 02, D-04). `Ctrl+C` is taken (Cancel/force-quit)
            // and `Ctrl+Shift+C` is intercepted by most terminals before the
            // app ever sees it — D-04 lands the yank binding on `Ctrl+Y`
            // instead. `yank_selection` is the SAME no-op on an
            // empty/absent selection as the mouse-drag-release path.
            (KeyCode::Char('y'), KeyModifiers::CONTROL) => {
                let area = *self
                    .transcript_area
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                self.yank_selection(area);
            }

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

            // Jump to bottom (D-10) — single arm catches plain End and Ctrl+End via wildcard modifiers.
            (KeyCode::End, _) => self.scroll_to_bottom(),

            // All other keys — forward to TextArea widget
            _ => {
                let _ = self.textarea.input(key);
                // Phase 36.6.3 Plan 01 (Task 2 action 3): re-clamp the
                // palette selection after every keystroke that could have
                // changed the textarea's filter — mirrors
                // `clamp_skills_hub_selected`'s "call after every
                // filter-mutating keystroke" discipline. Cheap no-op unless
                // the palette is actually showing post-mutation
                // (`palette_query` short-circuits before any registry scan).
                if crate::tui_rata::palette::palette_query(self).is_some() {
                    self.clamp_palette_selected();
                }
            }
        }
    }

    /// Key routing while the Skills Hub is the active overlay (Phase 36.6.2
    /// Plan 01, D-06/D-07). Enter inserts the literal trigger text ONLY —
    /// this fn must NEVER call `dispatch_slash` / construct
    /// `SlashOutcome::SkillActivated` (T-36.6.2-01-01, browse-only invariant).
    /// Esc is handled by the precedence check in `handle_key`, not here.
    fn handle_skills_hub_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Backspace => {
                self.skills_hub_filter.pop();
                self.clamp_skills_hub_selected();
            }
            KeyCode::Char(c) => {
                self.skills_hub_filter.push(c);
                self.clamp_skills_hub_selected();
            }
            KeyCode::Up => {
                self.skills_hub_selected = self.skills_hub_selected.saturating_sub(1);
            }
            KeyCode::Down => {
                let len = crate::tui_rata::overlay::skills_hub_filtered(self).len();
                if len > 0 && self.skills_hub_selected + 1 < len {
                    self.skills_hub_selected += 1;
                }
            }
            KeyCode::Enter => {
                let name = crate::tui_rata::overlay::skills_hub_filtered(self)
                    .get(self.skills_hub_selected)
                    .map(|r| r.name.clone());
                if let Some(name) = name {
                    self.textarea.insert_str(format!("/{name}"));
                }
                self.active_overlay = None;
                self.skills_hub_filter.clear();
                self.skills_hub_selected = 0;
            }
            _ => {}
        }
    }

    /// Key routing while the Help overlay is the active overlay (Phase
    /// 36.6.2 Plan 04, TUI-02, D-08/D-09). `PageUp`/`PageDown` scroll
    /// `help_scroll`, reusing the `scroll_up`/`scroll_down` discipline but
    /// targeting `help_scroll` instead of `transcript_scroll`; `PageDown`
    /// clamps at the last registered keybinding entry so it never scrolls
    /// into blank space beyond the content. Every other key (including a
    /// second `?` press) is a no-op — Esc is handled by the precedence
    /// check in `handle_key`.
    fn handle_help_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::PageUp => {
                self.help_scroll = self.help_scroll.saturating_sub(5);
            }
            KeyCode::PageDown => {
                let entry_count = crate::tui_rata::overlay::help_entry_count() as u16;
                let max_scroll = entry_count.saturating_sub(1);
                self.help_scroll = self.help_scroll.saturating_add(5).min(max_scroll);
            }
            _ => {}
        }
    }

    /// Clamp `skills_hub_selected` to the current filtered list's bounds
    /// (T-36.6.2-01-02) — called after every filter-mutating keystroke so a
    /// shrinking filter never leaves the selection pointing past the end.
    fn clamp_skills_hub_selected(&mut self) {
        let len = crate::tui_rata::overlay::skills_hub_filtered(self).len();
        if len == 0 {
            self.skills_hub_selected = 0;
        } else if self.skills_hub_selected >= len {
            self.skills_hub_selected = len - 1;
        }
    }

    // ── Phase 36.6.3 Plan 01: command palette (TUI-INPUT-01, D-03/D-04) ───────

    /// Key routing while the palette is showing (`palette::palette_query(self)
    /// .is_some()`). Returns `true` when the key was fully handled here
    /// (`handle_key`'s caller `return`s without falling through); `false`
    /// lets it fall through to the default arm, which forwards to
    /// `self.textarea` directly — Backspace/Char have no palette-owned
    /// filter buffer to mutate (unlike `handle_skills_hub_key`'s
    /// `skills_hub_filter`; the palette's filter IS the textarea, D-03).
    fn handle_palette_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        use crossterm::event::{KeyCode, KeyModifiers};
        match key.code {
            KeyCode::Up => {
                self.palette_selected = self.palette_selected.saturating_sub(1);
                true
            }
            KeyCode::Down => {
                let prefix = crate::tui_rata::palette::palette_query(self).unwrap_or_default();
                let len = crate::tui_rata::palette::filtered_entries(self, &prefix).len();
                if len > 0 && self.palette_selected + 1 < len {
                    self.palette_selected += 1;
                }
                true
            }
            KeyCode::Tab => {
                self.palette_tab_insert();
                true
            }
            // Task 2 (D-04): plain Enter only — Shift/Alt+Enter must still
            // insert a newline via the default arm's existing
            // `(KeyCode::Enter, m) if m.contains(SHIFT) || m.contains(ALT)`
            // arm (preserving that pre-existing behavior for the rare
            // multi-line-while-palette-showing edge case).
            KeyCode::Enter if key.modifiers == KeyModifiers::NONE => {
                self.palette_enter();
                true
            }
            // Every other key — including Backspace/Char and modified
            // Enter — falls through to `handle_key`'s default arm unchanged.
            _ => false,
        }
    }

    /// Tab — insert the highlighted command as `/{name} ` (TRAILING space,
    /// ready to type args) and reset selection to the top
    /// (artifacts_produced). The trailing space immediately fails
    /// `palette_query`'s no-space predicate, so this also closes the
    /// dropdown: Tab always "completes and hands off to free typing",
    /// unlike Enter's no-trailing-space completion (Task 2), which
    /// deliberately keeps the dropdown open so a second Enter can submit.
    fn palette_tab_insert(&mut self) {
        let Some(prefix) = crate::tui_rata::palette::palette_query(self) else {
            return;
        };
        let matches = crate::tui_rata::palette::filtered_entries(self, &prefix);
        if let Some(entry) = matches.get(self.palette_selected) {
            let name = entry.name().to_string();
            self.replace_textarea_line(&format!("/{name} "));
        }
        self.palette_selected = 0;
    }

    /// Enter while the palette is showing (D-04). If the highlighted
    /// command's completed `/{name}` form DIFFERS from the current token,
    /// insert it — NO trailing space (contrast Tab), so the dropdown stays
    /// open on the now-exact match and a SECOND Enter submits. Otherwise
    /// (the token already equals the one highlighted command's name — an
    /// arg-less command like `/help` fully typed) fall through to the
    /// existing `dispatch_or_submit` path so the command actually runs.
    ///
    /// Scope boundary (this plan): does NOT special-case bare `/model`/
    /// `/provider` to open a picker — that interception + the picker itself
    /// are added by Plan 03, which introduces the `OverlayKind::ModelPicker`
    /// this fn would need to open. Today, `/model`/`/provider` behave like
    /// any other arg-less-when-exact command: Enter completes then submits,
    /// which dispatches through today's existing text-listing handler.
    fn palette_enter(&mut self) {
        let Some(prefix) = crate::tui_rata::palette::palette_query(self) else {
            self.dispatch_or_submit();
            return;
        };
        let matches = crate::tui_rata::palette::filtered_entries(self, &prefix);
        let Some(entry) = matches.get(self.palette_selected) else {
            // No highlighted entry (e.g. the empty-match state) — nothing
            // to insert; submitting surfaces the existing "unknown command"
            // error path, same as today without a palette.
            self.dispatch_or_submit();
            return;
        };
        if entry.name() == prefix {
            // Exact match (command OR skill): fall through to the shared slash
            // path. A skill token routes via dispatch_slash's SKILL-13 fallback
            // → apply_slash_outcome → the Plan 02 D-01 one-shot activate+run.
            self.dispatch_or_submit();
        } else {
            let name = entry.name().to_string();
            self.replace_textarea_line(&format!("/{name}"));
            self.palette_selected = 0;
        }
    }

    /// Replace the ENTIRE textarea buffer with a single line of `text`,
    /// preserving the bordered "Prompt" chrome (mirrors `load_history_entry`'s
    /// reconstruction pattern, UAT Gap 1). Used by the palette's Tab/Enter
    /// insert paths, which only ever operate on a single-line `/{token}`
    /// buffer — `palette_query` never returns `Some` for multi-line input,
    /// so no `\n`-splitting is needed here (contrast `load_history_entry`,
    /// which does need it for recalled multi-line history entries).
    fn replace_textarea_line(&mut self, text: &str) {
        let mut ta = TextArea::default();
        ta.set_cursor_line_style(Style::default());
        ta.set_block(Block::default().borders(Borders::ALL).title("Prompt"));
        ta.insert_str(text);
        self.textarea = ta;
    }

    /// Clamp `palette_selected` to the current filtered list's bounds
    /// (mirrors `clamp_skills_hub_selected`) — called after every
    /// filter-mutating keystroke so a shrinking filter never leaves the
    /// selection pointing past the end.
    fn clamp_palette_selected(&mut self) {
        let Some(prefix) = crate::tui_rata::palette::palette_query(self) else {
            self.palette_selected = 0;
            return;
        };
        let len = crate::tui_rata::palette::filtered_entries(self, &prefix).len();
        if len == 0 {
            self.palette_selected = 0;
        } else if self.palette_selected >= len {
            self.palette_selected = len - 1;
        }
    }

    // ── Phase 36.6.3 Plan 03 (TUI-INPUT-02, D-06/D-07/D-11): model/provider picker ──

    /// Key routing while the `/model`/`/provider` picker is the active
    /// overlay. Char/Backspace mutate `model_picker_filter` (the picker is a
    /// real `OverlayKind`, mirroring `handle_skills_hub_key`'s
    /// `skills_hub_filter` — NOT the palette's live-textarea-is-the-filter
    /// pattern). Up/Down move the selection, clamped to the active step's
    /// filtered list length. Enter advances step 1 -> step 2, or applies at
    /// step 2 / the single-step `/provider` flow. Esc is handled by the
    /// precedence check in `handle_key`.
    fn handle_model_picker_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;

        let Some(OverlayKind::ModelPicker {
            step,
            selected_provider,
        }) = self.active_overlay.clone()
        else {
            return;
        };

        match key.code {
            KeyCode::Backspace => {
                self.model_picker_filter.pop();
                self.clamp_model_picker_selected();
            }
            KeyCode::Char(c) => {
                self.model_picker_filter.push(c);
                self.clamp_model_picker_selected();
            }
            KeyCode::Up => {
                self.model_picker_selected = self.model_picker_selected.saturating_sub(1);
            }
            KeyCode::Down => {
                let len = self.model_picker_current_len(step, &selected_provider);
                if len > 0 && self.model_picker_selected + 1 < len {
                    self.model_picker_selected += 1;
                }
            }
            KeyCode::Enter => self.model_picker_enter(step, selected_provider),
            _ => {}
        }
    }

    /// The active step's FILTERED list length — providers at
    /// `Provider`/`ProviderOnly`, the selected provider's models at `Model`.
    /// Single source of truth for both selection-movement bounds
    /// (`handle_model_picker_key`) and clamping (`clamp_model_picker_selected`).
    fn model_picker_current_len(&self, step: PickerStep, selected_provider: &Option<String>) -> usize {
        match step {
            PickerStep::Provider | PickerStep::ProviderOnly => {
                crate::tui_rata::overlay::model_picker_providers_filtered(self).len()
            }
            PickerStep::Model => {
                let provider = selected_provider.as_deref().unwrap_or("");
                crate::tui_rata::overlay::model_picker_models_filtered(self, provider).len()
            }
        }
    }

    /// Clamp `model_picker_selected` to the active step's filtered list
    /// bounds (mirrors `clamp_skills_hub_selected`) — called after every
    /// filter-mutating keystroke so a shrinking filter never leaves the
    /// selection pointing past the end.
    fn clamp_model_picker_selected(&mut self) {
        let Some(OverlayKind::ModelPicker {
            step,
            selected_provider,
        }) = self.active_overlay.clone()
        else {
            self.model_picker_selected = 0;
            return;
        };
        let len = self.model_picker_current_len(step, &selected_provider);
        if len == 0 {
            self.model_picker_selected = 0;
        } else if self.model_picker_selected >= len {
            self.model_picker_selected = len - 1;
        }
    }

    /// Enter semantics per step: `Provider` (step 1 of `/model`) ADVANCES to
    /// `Model` with the highlighted row's name as `selected_provider`
    /// (resetting filter/selection); `ProviderOnly` (`/provider`) and
    /// `Model` (step 2 of `/model`) APPLY the highlighted row. A missing
    /// highlighted row (e.g. the empty-filter state) is a no-op — nothing to
    /// advance or apply.
    fn model_picker_enter(&mut self, step: PickerStep, selected_provider: Option<String>) {
        match step {
            PickerStep::Provider => {
                let providers = crate::tui_rata::overlay::model_picker_providers_filtered(self);
                let Some(row) = providers.get(self.model_picker_selected) else {
                    return;
                };
                let chosen = row.name.clone();
                self.model_picker_filter.clear();
                self.model_picker_selected = 0;
                self.active_overlay = Some(OverlayKind::ModelPicker {
                    step: PickerStep::Model,
                    selected_provider: Some(chosen),
                });
            }
            PickerStep::ProviderOnly => {
                let providers = crate::tui_rata::overlay::model_picker_providers_filtered(self);
                let Some(row) = providers.get(self.model_picker_selected) else {
                    return;
                };
                let provider = row.name.clone();
                let model = row.default_model.clone();
                self.apply_model_picker_selection(step, provider, model);
            }
            PickerStep::Model => {
                let provider = selected_provider.unwrap_or_default();
                let models =
                    crate::tui_rata::overlay::model_picker_models_filtered(self, &provider);
                let Some(model) = models.get(self.model_picker_selected).cloned() else {
                    return;
                };
                self.apply_model_picker_selection(step, provider, model);
            }
        }
    }

    /// Apply a chosen provider+model (D-06/D-07/D-11): hot-swaps the LIVE
    /// session via `ironhermes_agent::build_client` — the SAME resolver +
    /// AnyClient rebuild `handle_subsystem_mutator`'s `"model"` arm uses for
    /// `/model <name>` (no parallel preview path) — then persists the
    /// choice to `config.yaml` (`persist_model_picker_selection`, D-11).
    ///
    /// On success: close the overlay, clear filter/selection, and push the
    /// Copywriting-contract System-role success line via the SAME
    /// `apply_slash_outcome` sink `dispatch_slash` output flows through. On
    /// failure: the overlay STAYS OPEN (a wrong pick has a solution path —
    /// try again — not a dead-end) and the error surfaces VERBATIM through
    /// that identical `SlashOutcome::Error` path (no new copy).
    fn apply_model_picker_selection(&mut self, step: PickerStep, provider: String, model: String) {
        match ironhermes_agent::build_client(&self.resolver, &provider, &model) {
            Ok(new_client) => {
                self.client = new_client;

                if let Err(e) = persist_model_picker_selection(&provider, &model) {
                    tracing::warn!(
                        error = %e,
                        provider = %provider,
                        model = %model,
                        "36.6.3-03 (D-11): failed to persist provider/model selection to config.yaml"
                    );
                }

                self.active_overlay = None;
                self.model_picker_filter.clear();
                self.model_picker_selected = 0;

                let text = match step {
                    PickerStep::ProviderOnly => {
                        format!("Switched provider to {provider} (model: {model}).")
                    }
                    _ => format!("Switched to {provider}/{model}."),
                };
                self.apply_slash_outcome(crate::tui_rata::commands::SlashOutcome::Handled(text));
            }
            Err(e) => {
                self.apply_slash_outcome(crate::tui_rata::commands::SlashOutcome::Error(
                    e.to_string(),
                ));
            }
        }
    }

    // ── Phase 36.6.2 Plan 03 (TUI-02): approval overlay state machine ─────────

    /// Surface an incoming [`ApprovalRequest`] (called from the
    /// `recv_approval_request` `select!` arm in `run_app_inner`). If ANY overlay
    /// is already active, the request is enqueued (never dropped — UI-SPEC §2
    /// queue discipline / T-36.6.2-03-05); otherwise it becomes the active
    /// overlay and its `oneshot::Sender` is stashed for `handle_key` to fire.
    pub fn surface_approval_request(&mut self, req: ApprovalRequest) {
        if self.active_overlay.is_some() {
            self.approval_queue.push(req);
        } else {
            let kind = req.to_overlay_kind();
            self.active_overlay = Some(kind);
            self.pending_approval_resolve = Some(req.resolve);
        }
    }

    /// Resolve the currently-surfaced approval request with `outcome`, close the
    /// overlay, and surface the next queued overlay (if any). Fail-closed by
    /// construction: callers only ever pass `Approved` on an explicit `[y]`/`[s]`,
    /// and `Denied` on `[n]`/Esc/cancel. A missing resolve sender is a no-op.
    fn resolve_approval(&mut self, outcome: ApprovalOutcome) {
        if let Some(tx) = self.pending_approval_resolve.take() {
            let _ = tx.send(outcome);
        }
        self.active_overlay = None;
        self.surface_next_overlay();
    }

    /// Surface the next queued overlay, if any — approval first, then
    /// clarify. Called only once `active_overlay` is already `None` (every
    /// caller either just cleared it or guards on it via
    /// `drain_approval_queue_after_close`). Phase 41.1 Plan 10 (G-41.1-1):
    /// this unifies what was previously `resolve_approval`'s and
    /// `drain_approval_queue_after_close`'s own inline `approval_queue`
    /// checks into one helper, so EVERY overlay-close path (approval/sudo/
    /// secret resolution, Skills Hub/Help/Model-Picker Esc, Ctrl+K
    /// toggle-off, and the new clarify close path) also surfaces a queued
    /// clarify — an interleaved clarify never silently stalls until
    /// `clarify_timeout_secs` elapses.
    fn surface_next_overlay(&mut self) {
        if !self.approval_queue.is_empty() {
            let next = self.approval_queue.remove(0);
            self.surface_approval_request(next);
        } else if !self.clarify_queue.is_empty() {
            let next = self.clarify_queue.remove(0);
            self.surface_clarify_request(next);
        }
    }

    /// After closing a NON-approval overlay (Skills Hub / Help / Model
    /// Picker / Clarify), surface the next queued approval or clarify request
    /// that arrived while it was open — so a queued request is never
    /// orphaned (CR-01 / UI-SPEC §2 queue discipline, T-36.6.2-03-05).
    /// `surface_approval_request`/`surface_clarify_request` enqueue rather
    /// than activate whenever `active_overlay.is_some()`, so any close path
    /// that clears `active_overlay` without draining the queue strands the
    /// request forever — an approval's `oneshot::Sender` awaiting
    /// `TuiApprovalGate::request_approval` never resolves (hangs the gated
    /// tool call), or a clarify silently waits out its own timeout.
    ///
    /// The `active_overlay.is_none()` guard makes every call site safe:
    /// - Skills Hub / Help / Model Picker Esc, Clarify Esc (cancel), and
    ///   Ctrl+K toggle-off: `active_overlay` was just cleared, so the guard
    ///   passes and the next queued request (if any) re-surfaces.
    /// - The approval-family Esc arm calls `resolve_approval` instead (which
    ///   already re-surfaces the next queued request itself via
    ///   `surface_next_overlay`), never this helper, so there is no
    ///   double-drain there.
    /// - Ctrl+K toggle-ON leaves `active_overlay` `Some(SkillsHub)`, so this
    ///   helper (called only on the close branch) is never reached for that
    ///   path; it also could not double-drain even if called, since the guard
    ///   requires `None`.
    fn drain_approval_queue_after_close(&mut self) {
        if self.active_overlay.is_none() {
            self.surface_next_overlay();
        }
    }

    /// Key routing while an approval-family overlay is active (Phase 36.6.2
    /// Plan 03, TUI-02). Fail-closed: ONLY an explicit `[y]` (or `[s]`) yields
    /// `Approved`; `[n]`, and every other/unexpected key, never approve. Esc is
    /// handled by the precedence check in `handle_key` (deny/cancel). The Secret
    /// buffer is mutated in place and NEVER logged/formatted.
    fn handle_approval_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;

        // Secret: mutate the masked buffer in place; Enter submits, Esc (above)
        // cancels. Any other key is a no-op.
        if matches!(self.active_overlay, Some(OverlayKind::Secret { .. })) {
            match key.code {
                KeyCode::Char(c) => {
                    if let Some(OverlayKind::Secret { masked_input, .. }) =
                        self.active_overlay.as_mut()
                    {
                        masked_input.push(c);
                    }
                }
                KeyCode::Backspace => {
                    if let Some(OverlayKind::Secret { masked_input, .. }) =
                        self.active_overlay.as_mut()
                    {
                        masked_input.pop();
                    }
                }
                KeyCode::Enter => self.resolve_approval(ApprovalOutcome::Approved),
                // Fail-closed: unexpected keys are no-ops (never submit/leak).
                _ => {}
            }
            return;
        }

        // Approval / Sudo: [y]es / [n]o, plus [s]ession for Approval only.
        let is_approval = matches!(self.active_overlay, Some(OverlayKind::Approval { .. }));
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.resolve_approval(ApprovalOutcome::Approved)
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                self.resolve_approval(ApprovalOutcome::Denied)
            }
            KeyCode::Char('s') | KeyCode::Char('S') if is_approval => {
                // D-04: reuse ApprovalsStore with the SAME cache_key scope as the
                // headless flow. Pitfall 4: approve_session is async and handle_key
                // is sync → tokio::spawn fire-and-forget (single-operator TUI has
                // no meaningful race before the next tool-call's cache-key check).
                let cache_key = if let Some(OverlayKind::Approval { cache_key, .. }) =
                    &self.active_overlay
                {
                    cache_key.clone()
                } else {
                    String::new()
                };
                let store = self.approvals_store.clone();
                tokio::spawn(async move {
                    store.approve_session(&cache_key).await;
                });
                self.resolve_approval(ApprovalOutcome::Approved);
            }
            // Fail-closed: any other key (including [s] on Sudo) is a no-op — it
            // must NEVER default to Approved.
            _ => {}
        }
    }

    // ── Phase 41.1 Plan 10 (G-41.1-1): clarify overlay state machine ─────────
    //
    // Mirrors the approval-gate state machine above: same queue discipline
    // (`surface_clarify_request` enqueues iff another overlay is active),
    // same close-then-drain shape. The one structural difference: clarify has
    // no `oneshot::Sender` stashed on `App` — the answer/cancel instead
    // reaches the suspended turn through the SHARED `clarify_registry`
    // (`take`/`remove`), because `PendingClarify`'s sender lives in the
    // registry entry itself, not on the surfaced request.

    /// Surface an incoming [`ClarifyRequest`] (called from the
    /// `recv_clarify_request` `select!` arm in `run_app_inner`). If ANY
    /// overlay is already active, the request is enqueued (never dropped —
    /// mirrors `surface_approval_request`); otherwise it becomes the active
    /// overlay and `clarify_selected` resets to the first choice.
    pub fn surface_clarify_request(&mut self, req: ClarifyRequest) {
        if self.active_overlay.is_some() {
            self.clarify_queue.push(req);
        } else {
            self.active_overlay = Some(OverlayKind::Clarify {
                question: req.question,
                choices: req.choices,
                clarify_id: req.clarify_id,
            });
            self.clarify_selected = 0;
        }
    }

    /// Spawn a fire-and-forget task that answers the suspended turn awaiting
    /// `clarify_id` with `(label, index)`. `PendingClarifyRegistry::take` is
    /// async and `handle_key` is sync, so this mirrors the `[s]ession` spawn
    /// in `handle_approval_key` (`approve_session`). A `None` entry means the
    /// turn already timed out/was cancelled — the send is simply skipped.
    fn answer_clarify(&mut self, clarify_id: String, index: usize, label: String) {
        let registry = self.clarify_registry.clone();
        tokio::spawn(async move {
            if let Some(entry) = registry.take(&clarify_id).await {
                let _ = entry.sender.send(ClarifyAnswer { label, index });
            }
        });
    }

    /// Spawn a fire-and-forget task that cancels the pending clarify — the
    /// suspended `execute_clarify`'s own `select!` then resolves via its
    /// timeout/cancel-token arm (no cross-registry sentinel needed here).
    fn cancel_clarify(&mut self, clarify_id: String) {
        let registry = self.clarify_registry.clone();
        tokio::spawn(async move {
            registry.remove(&clarify_id).await;
        });
    }

    /// Key routing while the Clarify overlay is active (Phase 41.1 Plan 10,
    /// G-41.1-1). Up/Down move the selection (clamped, mirrors
    /// `handle_skills_hub_key`); Enter answers via `answer_clarify` and
    /// closes+drains. Esc is handled by the precedence check in `handle_key`
    /// (cancel), not here.
    fn handle_clarify_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;

        let choices_len = match &self.active_overlay {
            Some(OverlayKind::Clarify { choices, .. }) => choices.len(),
            _ => return,
        };

        match key.code {
            KeyCode::Up => {
                self.clarify_selected = self.clarify_selected.saturating_sub(1);
            }
            KeyCode::Down => {
                if choices_len > 0 && self.clarify_selected + 1 < choices_len {
                    self.clarify_selected += 1;
                }
            }
            KeyCode::Enter => {
                let answer = match &self.active_overlay {
                    Some(OverlayKind::Clarify {
                        choices,
                        clarify_id,
                        ..
                    }) => {
                        let index = self.clarify_selected.min(choices.len().saturating_sub(1));
                        Some((clarify_id.clone(), index, choices[index].clone()))
                    }
                    _ => None,
                };
                if let Some((clarify_id, index, label)) = answer {
                    self.answer_clarify(clarify_id, index, label);
                }
                self.active_overlay = None;
                self.drain_approval_queue_after_close();
            }
            _ => {}
        }
    }

    /// Double/triple-click time budget (Phase 36.6.4 Plan 02, D-07). No
    /// terminal reports the platform double-click setting to a TUI, so this
    /// is a Claude's-discretion value per the plan's `planner_assumptions`
    /// — named and documented rather than a bare magic number, so a wrong
    /// magnitude is a one-line change. 500ms matches the common OS default
    /// double-click interval.
    const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(500);

    /// Mouse event handler — scrolls transcript, opens chips, and drives
    /// text selection when within `area` bounds.
    ///
    /// **Threat T-22.4-05-07 (Tampering):** bounds check prevents scroll events
    /// outside the transcript pane from affecting scroll state.
    ///
    /// Phase 46.7 Plan 07 (D-17): `Down(Left)` opens an artifact URL when the
    /// click lands inside a chip's hit-test rect (built by
    /// `rebuild_chip_hit_test` during the previous render pass) — chip
    /// hit-test keeps priority over starting a selection when the click
    /// lands inside a chip rect.
    ///
    /// Phase 36.6.4 Plan 01 (D-04/D-07/D-08): `Down(Left)` ALSO seeds a new
    /// selection anchor+cursor at the clicked content position (even when a
    /// chip was hit — the chip click still opens the URL AND starts a
    /// selection at that cell, matching ordinary terminal click-drag
    /// semantics). `Drag(Left)` extends the cursor. `Up(Left)` auto-copies
    /// the dragged range to the clipboard over OSC52 (D-04's X11
    /// primary-selection model: extract, write, clear nothing — the
    /// highlight persists until the next click). Mouse capture stays
    /// enabled throughout (D-08) — this is the entire reason selection must
    /// work as in-app rendering rather than relying on the terminal's own
    /// native selection UI.
    pub fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent, area: Rect) {
        use crossterm::event::{MouseButton, MouseEventKind};
        let within = mouse.column >= area.x
            && mouse.column < area.x + area.width
            && mouse.row >= area.y
            && mouse.row < area.y + area.height;
        if !within {
            return;
        }
        let scroll_offset = self
            .scroll_view_state
            .lock()
            .map(|guard| guard.offset())
            .unwrap_or_else(|_| Position::new(0, 0));
        match mouse.kind {
            MouseEventKind::ScrollUp => self.scroll_up(3),
            MouseEventKind::ScrollDown => self.scroll_down(3),
            // D-07/click-count granularity (Phase 36.6.4 Plan 02): the chip
            // hit-test KEEPS PRIORITY — a press inside a chip rect opens the
            // chip and anchors a fresh (empty, invisible) char-range
            // selection exactly like Plan 01 did, never escalating
            // granularity or participating in the double/triple-click
            // count (that ordering already existed in this arm from Plan
            // 01 and must not be inverted here).
            MouseEventKind::Down(MouseButton::Left) => {
                let hit = self
                    .chip_hit_test
                    .lock()
                    .ok()
                    .and_then(|hits| chip_action_at(&hits, mouse.column, mouse.row));
                let hit_present = hit.is_some();
                match &hit {
                    Some(ChipAction::OpenArtifactUrl(url)) => {
                        if let Err(e) = (self.opener)(url) {
                            tracing::warn!(
                                error = %e, url = %url,
                                "Phase 46.7 Plan 07: failed to open artifact URL in browser"
                            );
                        }
                    }
                    // Phase 36.6.4 Plan 05 (D-13): a click sets the active
                    // overlay rather than calling the browser opener. Fresh
                    // decode state per open (not just on Esc-close) — cleared
                    // here so a stale Ready/Failed from a PREVIOUS image
                    // never flashes before the new decode task finishes.
                    Some(ChipAction::OpenImage { label, source }) => {
                        self.active_overlay = Some(OverlayKind::ImageViewer {
                            label: label.clone(),
                            source: source.clone(),
                        });
                        if let Ok(mut guard) = self.image_decode.lock() {
                            *guard = None;
                        }
                    }
                    None => {}
                }
                let pos = selection::content_pos_at(area, scroll_offset, mouse.column, mouse.row);
                if hit_present {
                    self.selection = Some(Selection::new_at(pos));
                    self.last_press = None;
                } else {
                    let now = Instant::now();
                    let (granularity, count) =
                        selection::classify_click(self.last_press, pos, now, Self::DOUBLE_CLICK_WINDOW);
                    self.last_press = Some((pos, now, count));
                    self.selection = Some(self.resolve_click_selection(pos, granularity, area));
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(sel) = self.selection.as_mut() {
                    sel.cursor =
                        selection::content_pos_at(area, scroll_offset, mouse.column, mouse.row);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.yank_selection(area);
            }
            _ => {}
        }
    }

    /// Resolve a fresh `Down(Left)` press into a `Selection` for the given
    /// click `granularity` (Phase 36.6.4 Plan 02, D-07). `Char` reproduces
    /// Plan 01's original zero-length anchor (a `Drag` extends it). `Word`/
    /// `Line` re-render the CURRENT transcript at the press's `area` width
    /// (the SAME `transcript_rendered_plain_rows` extraction `yank_
    /// selection` itself uses, so the selected range can never drift from
    /// what's actually drawn) and resolve boundaries via the pure
    /// `selection::word_range_at`/`line_range_at` helpers — both produce an
    /// ordinary same-row `Selection` that renders and yanks exactly like a
    /// drag-selected range (UI-SPEC §2: no distinct visual treatment for
    /// click-vs-drag origin).
    fn resolve_click_selection(
        &self,
        pos: selection::ContentPos,
        granularity: selection::ClickGranularity,
        area: Rect,
    ) -> Selection {
        let (start_col, end_col) = match granularity {
            selection::ClickGranularity::Char => return Selection::new_at(pos),
            selection::ClickGranularity::Word | selection::ClickGranularity::Line => {
                let width = inner_transcript_width(area);
                let rows = self.transcript_rendered_plain_rows(width);
                let row_text = rows.get(pos.row).cloned().unwrap_or_default();
                match granularity {
                    selection::ClickGranularity::Word => selection::word_range_at(&row_text, pos.col),
                    selection::ClickGranularity::Line => selection::line_range_at(&row_text),
                    selection::ClickGranularity::Char => unreachable!("handled above"),
                }
            }
        };
        Selection {
            anchor: selection::ContentPos::new(pos.row, start_col),
            cursor: selection::ContentPos::new(pos.row, end_col),
        }
    }

    /// BLOCKER-NEW-03 router: slash input → `dispatch_slash` (never `app.history`).
    /// `!` input → `dispatch_bang_blocking` (Phase 36.6.4 Plan 03, D-09..D-11;
    /// never `app.history` either — TUI-HIST-01/D-16). Non-slash/non-`!`
    /// input → `submit()` (LLM turn).
    fn dispatch_or_submit(&mut self) {
        let text = self.textarea.lines().join("\n");
        if text.starts_with('/') {
            // Phase 36.6.4 Plan 03 (D-16, TUI-HIST-01): push the raw slash
            // line into the LOCAL recall buffer — mirrors submit()'s
            // history_store.push+reset_cursor pair (see `submit()` below).
            // This is the ONLY new machinery D-16 needs for slash recall;
            // `ReplHistory::push` already dedupes consecutive entries and
            // enforces its own cap. MUST NOT touch `self.history` (the LLM
            // conversation) — that stays exactly as `dispatch_slash_blocking`
            // already routes it (BLOCKER-NEW-03, invariants_22_4.rs,
            // unchanged by this plan).
            self.history_store.push(text.clone());
            self.history_store.reset_cursor();
            self.dispatch_slash_blocking(&text);
            self.clear_textarea();
            return;
        }
        if text.starts_with('!') {
            // Phase 36.6.4 Plan 03 (D-09..D-11/D-16, TUI-BANG-01/TUI-HIST-01):
            // route to the shell module. Same history_store push as the
            // slash arm above — Up-arrow recall works for `!` commands too.
            // The raw `!{command}` line itself NEVER enters `self.history`;
            // only the shell run's OUTPUT does (D-11), via
            // `apply_shell_outcome` below.
            self.history_store.push(text.clone());
            self.history_store.reset_cursor();
            self.dispatch_bang_blocking(&text);
            self.clear_textarea();
            return;
        }
        self.submit();
    }

    /// Route a `!`-prefixed line to `shell_bang` (Phase 36.6.4 Plan 03,
    /// D-09..D-11). Mirrors `dispatch_slash_blocking`'s tokio-runtime
    /// detection: inside a runtime, blocks on `shell_bang::run` via
    /// `block_in_place` (same synchronous shape the slash path already
    /// uses); outside a runtime (unit tests with no `#[tokio::test]`),
    /// records intent in the status hint without panicking.
    ///
    /// D-10 refusal is checked HERE, before any spawn attempt, so a refused
    /// command never reaches `shell_bang::run` at all — it renders a single
    /// `Role::System` line (UI-SPEC §3 REFUSED state) and returns. This is
    /// NOT the raw `!{command}` line entering `app.history` — the
    /// prohibition (TUI-HIST-01) is about the raw command text becoming a
    /// User-role echo, which never happens on either path.
    fn dispatch_bang_blocking(&mut self, input: &str) {
        let command = input.strip_prefix('!').unwrap_or(input).trim().to_string();
        let first_token = command.split_whitespace().next().unwrap_or("");
        if shell_bang::classify_interactive(first_token) {
            let mut sys = ChatMessage::user(shell_bang::refusal_message(&command));
            sys.role = Role::System;
            self.history.push(sys);
            self.scroll_to_bottom();
            return;
        }

        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                let cmd = command.clone();
                let outcome = tokio::task::block_in_place(|| {
                    handle.block_on(async { shell_bang::run(&cmd).await })
                });
                self.apply_shell_outcome(outcome);
            }
            Err(_) => {
                // Outside tokio runtime — test path. Record intent in hint.
                self.status.hint = format!("shell (test): {command}");
            }
        }
    }

    /// Apply a completed `ShellOutcome` (Phase 36.6.4 Plan 03, D-11):
    /// records a `ShellRun` for the custom-styled transcript render
    /// (`shell_runs`, consumed by `transcript_render_text` via
    /// `shell_bang::shell_block_lines`), and pushes the SAME plain text
    /// (`shell_bang::shell_block_plain`, byte-identical — not re-derived)
    /// into `app.history` as a `Role::System` message so a follow-up
    /// question sees exactly what the operator saw. That message's index is
    /// recorded in `shell_history_hidden_indices` so `transcript_text`'s
    /// normal per-message loop does not ALSO render it (double-render guard).
    ///
    /// Phase 36.6.4 Plan 12 (G-09 closure): the hidden copy is pushed FIRST
    /// so its index is known, then the `ShellRun` is stamped with
    /// `history_anchor: self.history.len()` (the post-push length) — the
    /// styled block renders at the SAME point in time as its own hidden
    /// copy, closing the defect where a `!` block was structurally
    /// incapable of rendering above any later history row.
    fn apply_shell_outcome(&mut self, outcome: shell_bang::ShellOutcome) {
        let mut run = shell_bang::ShellRun {
            command: outcome.command.clone(),
            state: shell_bang::ShellRunState::Done(outcome),
            history_anchor: 0,
        };
        let plain = shell_bang::shell_block_plain(&run).join("\n");
        let mut sys = ChatMessage::user(&plain);
        sys.role = Role::System;
        self.history.push(sys);
        self.shell_history_hidden_indices
            .insert(self.history.len() - 1);
        run.history_anchor = self.history.len();
        self.shell_runs.push(run);
        self.scroll_to_bottom();
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
    ///
    /// Visibility widened to `pub(super)` for unit-test access from
    /// `mod scroll_tests` — Phase 21.8.2 G-01 closure. Still crate-private.
    pub(super) fn apply_slash_outcome(&mut self, outcome: crate::tui_rata::commands::SlashOutcome) {
        use crate::tui_rata::commands::SlashOutcome;
        match outcome {
            SlashOutcome::Handled(text) => {
                let mut msg = ChatMessage::user(&text);
                msg.role = Role::System;
                self.history.push(msg);
                self.scroll_to_bottom();
            }
            SlashOutcome::Silent => {}
            SlashOutcome::Quit => {
                self.should_quit = true;
            }
            SlashOutcome::ResetTerminal => {}
            SlashOutcome::McpReload => {}
            SlashOutcome::SkillsReload(msg) => {
                let mut system = ChatMessage::user(&msg);
                system.role = Role::System;
                self.history.push(system);
                self.scroll_to_bottom();
            }
            // Phase 41.1 Plan 02 (D-01/D-02) — LEAD TRACER: one-shot activate+run.
            // Build the identity-free SkillInvocation (bare vs. argued
            // trigger_text via the shared resolver's `build_skill_invocation`),
            // activate the body through the existing overlay path, then drive
            // the SAME turn-spawn primitive `submit()` uses so
            // event_loop.rs:828-833 picks up `pending_tx` and calls
            // `spawn_turn` — no second message required, no parallel spawn path.
            SlashOutcome::SkillActivated { name, body, args } => {
                // Distinguish bare vs. argued BEFORE `args` is moved into the
                // resolver: `Some(non-empty-after-trim)` is the user's own
                // typed words (renders normally); `None`/whitespace is a bare
                // invoke whose synthetic trigger is hidden from the transcript.
                let args_display = args
                    .as_ref()
                    .map(|a| a.trim().to_string())
                    .filter(|a| !a.is_empty());
                let is_bare = args_display.is_none();
                let invocation = build_skill_invocation(name.clone(), body, args);
                // Existing overlay path: buffer (name, body) for the next
                // turn's prompt assembly (unchanged from today).
                self.pending_skill_overlays
                    .push((name.clone(), invocation.body.clone()));
                // Run-turn meta chip (UI-SPEC §C): a DIM single line preceding
                // the turn — same Role::System-style rendering the now-retired
                // activation acknowledgment line used, new copy only.
                let chip = run_turn_meta_chip(&name, args_display.as_deref());
                let mut chip_msg = ChatMessage::user(&chip);
                chip_msg.role = Role::System;
                self.history.push(chip_msg);
                // Model-facing turn content (D-01/D-02): the bare run-now
                // instruction, or the argued trailing text verbatim. Pushed as
                // Role::User so `spawn_turn`'s `history.clone()` snapshot
                // carries it into the turn.
                self.history
                    .push(user_message(invocation.trigger_text.clone()));
                // key_link: the BARE synthetic trigger is never a user bubble —
                // hide it; only the meta chip above is user-visible. The argued
                // form's trailing text IS the user's words and renders normally.
                if is_bare {
                    self.skill_run_hidden_indices.insert(self.history.len() - 1);
                }
                self.start_pending_turn();
                self.scroll_to_bottom();
            }
            SlashOutcome::ClearSession(text) => {
                self.history.clear();
                // Phase 41.1 Plan 02: hidden-trigger indices point into the
                // now-cleared history — drop them so they can't mis-hide a
                // future message that later occupies the same index.
                self.skill_run_hidden_indices.clear();
                // Phase 36.6.4 Plan 03: same reasoning for shell-run hidden
                // indices.
                self.shell_history_hidden_indices.clear();
                self.active_personality_overlay = None;
                self.assistant_buffer = None;
                let mut system = ChatMessage::user(&text);
                system.role = Role::System;
                self.history.push(system);
                self.scroll_to_bottom();
            }
            SlashOutcome::Unknown { input: _, hint } => {
                let mut system = ChatMessage::user(&hint);
                system.role = Role::System;
                self.history.push(system);
                self.status.hint = hint;
                self.scroll_to_bottom();
            }
            SlashOutcome::Error(err) => {
                let body = format!("error: {err}");
                let mut system = ChatMessage::user(&body);
                system.role = Role::System;
                self.history.push(system);
                self.status.hint = format!("error: {err}");
                self.scroll_to_bottom();
            }
            // Phase 36.6.3 Plan 03 (TUI-INPUT-02, D-06): open the picker at
            // step 1 (two-step `/model`) or the single-step `/provider` flow.
            // Reset filter/selection on open — mirrors Ctrl+K's Skills Hub
            // open path.
            SlashOutcome::OpenModelPicker => {
                self.active_overlay = Some(OverlayKind::ModelPicker {
                    step: PickerStep::Provider,
                    selected_provider: None,
                });
                self.model_picker_filter.clear();
                self.model_picker_selected = 0;
            }
            SlashOutcome::OpenProviderPicker => {
                self.active_overlay = Some(OverlayKind::ModelPicker {
                    step: PickerStep::ProviderOnly,
                    selected_provider: None,
                });
                self.model_picker_filter.clear();
                self.model_picker_selected = 0;
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
                // Phase 36.6.2 Plan 02 (D-02 refinement): buffer the status
                // transition into thinking_lines regardless of
                // thinking_expanded — buffering continues while collapsed
                // so expanding later shows the accumulated feed.
                self.thinking_lines.push("▶ turn started".to_string());
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
                self.thinking_lines.push(format!("→ tool: {name}"));
            }
            StreamEvent::ToolProgress { name, phase } => {
                self.status.hint = format!("{name}: {phase}");
                self.thinking_lines.push(format!("  {name}: {phase}"));
            }
            StreamEvent::ToolResult { name, ok } => {
                let icon = if ok { "✓" } else { "✗" };
                self.status.hint = format!("{icon} {name}");
                self.thinking_lines.push(format!("{icon} {name}"));
            }
            StreamEvent::Finished { total_tokens } => {
                self.thinking_lines.push("✓ turn finished".to_string());
                self.commit_assistant_buffer();
                // D-08: snap-to-bottom safety net — defense-in-depth against future
                // line-count drift. Cheap because reconcile_scroll runs every render tick anyway.
                if self.auto_follow {
                    self.scroll_to_bottom();
                }
                self.pending_rx = None;
                self.cancel_child = None;
                self.status.hint = String::new();
                // Phase 36.2 Plan 07/10 fix: stamp the per-turn token total
                // onto the status bar. `0` means the provider didn't return
                // usage data — preserve the prior count rather than reset to
                // 0 so the pill doesn't visibly regress on providers that
                // omit usage (older Ollama, custom gateways, etc.).
                if total_tokens > 0 {
                    self.status.tokens_used = total_tokens;
                }
                // Phase 36.17.3 (D-04): auto-drain next queued item, if any.
                // Guards inside maybe_drain_queue (paused / in-flight) short-
                // circuit if the conditions aren't right.
                self.maybe_drain_queue();
            }
            StreamEvent::Error(e) => {
                self.thinking_lines.push(format!("✗ error: {e}"));
                self.commit_assistant_buffer();
                self.status.hint = format!("error: {e}");
                self.pending_rx = None;
                self.cancel_child = None;
                // Phase 36.17.3 (Resolution 4): on stream error, set paused=true
                // so the user sees the paused state and can /unpause to recover.
                // The maybe_drain_queue call below is symmetric belt-and-
                // suspenders — its first guard (paused) short-circuits the pop
                // so no item is consumed by an errored turn.
                self.queue_paused
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                self.maybe_drain_queue();
            }
            StreamEvent::Cancelled => {
                self.thinking_lines.push("⨯ cancelled".to_string());
                self.commit_assistant_buffer();
                self.status.hint = "cancelled".to_string();
                self.pending_rx = None;
                self.cancel_child = None;
                // Phase 36.17.3: do NOT drain on cancel (RESEARCH Pitfall 1 —
                // /stop clears queue before firing cancel; a drain here would
                // be either a no-op (queue already empty) or a regression
                // (drain pops a queued item after a deliberate hard-stop).
            }
        }
    }

    /// Flush `assistant_buffer` into `history` as an assistant message.
    ///
    /// Phase 36.6.4 Plan 05 (D-12, TUI-IMG-01): the first D-12 trigger —
    /// runs the gateway's own `MediaTagExtractor` over the completed turn
    /// BEFORE it's pushed into history, so `<MEDIA:>` tags never reach the
    /// transcript as raw text (this codebase would otherwise be the only
    /// surface still doing that; web/Telegram/cron already strip them via
    /// the same extractor). Fed as one whole-buffer `feed()` call — the
    /// extractor's byte-walk algorithm is correct whether it arrives as one
    /// shot or many streamed deltas, and by the time a turn commits here the
    /// buffer IS already fully assembled. Only `MediaKind::Photo` refs
    /// become image chips (this phase's scope); other kinds still have
    /// their tag stripped from the visible text but produce no chip — audio/
    /// video/document rendering is out of scope for `OverlayKind::ImageViewer`.
    fn commit_assistant_buffer(&mut self) {
        if let Some(buf) = self.assistant_buffer.take()
            && !buf.is_empty()
        {
            let mut extractor = MediaTagExtractor::new();
            let mut visible = extractor.feed(&buf);
            visible.push_str(&extractor.flush_tail());
            // Phase 36.6.4 Plan 12 (G-09 closure): collect the photo refs
            // FIRST, push the assistant message the chips belong to, THEN
            // push the chips with `history_anchor: self.history.len()` (the
            // post-push length) — so a chip's anchor names the point in
            // time AFTER the turn that produced it, not before.
            let photo_refs: Vec<_> = extractor
                .take_attachments()
                .into_iter()
                .filter(|media_ref| media_ref.kind == MediaKind::Photo)
                .collect();
            self.history.push(assistant_message(visible));
            let anchor = self.history.len();
            for media_ref in photo_refs {
                let label = image_chip_label_for_source(&media_ref.source);
                self.image_chips.push(ImageChip {
                    label,
                    source: media_ref,
                    history_anchor: anchor,
                });
            }
        }
    }

    /// Phase 36.17.3 (D-04 / D-05 / T-04 mitigation): pop one queued item and
    /// set up the next pending turn. Called from `handle_stream_event` on
    /// `StreamEvent::Finished` (and defensively on `Error`).
    ///
    /// Guards (both mandatory — RESEARCH Pitfall 3 / Pitfall 6):
    /// 1. `queue_paused.load(Relaxed)` → skip drain (D-06).
    /// 2. `pending_tx.is_some()` → skip drain (T-04: prevents double-spawn
    ///    deadlock if drain fires while a turn is still in flight).
    ///
    /// On Some(text): mirror `submit()`'s channel + cancel-token setup so the
    /// event loop picks the queued item up as the next turn.
    fn maybe_drain_queue(&mut self) {
        // Guard 1 (D-06): paused → skip drain.
        if self.queue_paused.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        // Guard 2 (RESEARCH Pitfall 3 — T-04 mitigation): turn already in
        // flight → skip drain. Without this guard, a future double-Finished
        // (or Finished racing a still-active turn) would double-spawn and
        // deadlock the event loop on the second channel.
        if self.pending_tx.is_some() {
            return;
        }
        if let Some(text) = self.queue.pop(&self.queue_key) {
            self.history.push(user_message(text));
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();
            self.pending_rx = Some(rx);
            self.pending_tx = Some(tx);
            self.cancel_child = Some(self.cancel_parent.child_token());
            self.auto_follow = true;
            self.assistant_buffer = None;
            // Phase 36.6.2 Plan 02: a new turn starts — clear the previous
            // turn's buffered activity so the pane doesn't show stale content.
            self.thinking_lines.clear();
        }
    }

    /// Tick callback — advance knight-rider animation counter.
    pub fn on_tick(&mut self) {
        self.knight_rider_tick = self.knight_rider_tick.wrapping_add(1);
        // Phase 36.6.4 Plan 02 (D-04): one-shot copy-confirmation window —
        // reverts the status-line hint slot to normal once its tick budget
        // elapses. Reuses the EXISTING 100ms tick (Motion Contract: no new
        // animated primitive).
        if let Some((_, expires_at)) = self.copy_confirmation
            && self.knight_rider_tick >= expires_at
        {
            self.copy_confirmation = None;
        }
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
    ///
    /// Phase 46.7 Plan 06 (D-18/D-20/D-12): before building the turn, `@path`
    /// tokens (and plausible terminal-drag-drop bare paths) are parsed out of
    /// the composer text and copied into the session attachment store,
    /// combined with anything already queued by `/attach`. When there is
    /// nothing to attach the pre-46.7 plain-text path is unchanged.
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

        let (stripped_text, attach_errors) = self.parse_and_queue_attach_paths(&text);
        let attachments_queued = !self.pending_attachments.is_empty();
        let submit_text = if attachments_queued {
            stripped_text
        } else {
            text.clone()
        };

        self.history_store.push(submit_text.clone());
        self.history_store.reset_cursor();
        if attachments_queued {
            // Phase 36.6.4 Plan 12 (G-09 closure): record where THIS turn's
            // attachment chips start (existing chips from earlier turns are
            // untouched) BEFORE building the message — the builder can only
            // stamp a placeholder anchor since it doesn't yet know where the
            // owning user message will land.
            let chips_from = self.sent_attachment_chips.len();
            let msg = self.build_user_message_with_attachments(&submit_text);
            self.history.push(msg);
            let anchor = self.history.len();
            for chip in &mut self.sent_attachment_chips[chips_from..] {
                chip.history_anchor = anchor;
            }
        } else {
            self.history.push(user_message(submit_text.clone()));
        }
        for (display_name, reason) in attach_errors {
            let body = format!("Could not attach {display_name}: {reason}");
            let mut sys = ChatMessage::user(&body);
            sys.role = Role::System;
            self.history.push(sys);
        }
        self.last_submitted_text = submit_text;
        self.clear_textarea();
        // Phase 36.17.8: typed input — `/voice on` must NOT speak this reply.
        self.last_turn_was_voice = false;

        self.start_pending_turn();
        self.scroll_to_bottom();
    }

    /// Shared per-turn spawn primitive: create the `(tx, rx)` stream channel,
    /// set `pending_rx`/`pending_tx`/`cancel_child`, and reset the streaming
    /// buffers so `event_loop.rs`'s top-of-loop pickup (which gates on both
    /// `pending_tx.is_some()` and `cancel_child`) spawns exactly one turn.
    /// Called by BOTH `submit()` (plain-text turns) and `apply_slash_outcome`'s
    /// `SkillActivated` arm (Phase 41.1 D-01 one-shot activate+run) — there is
    /// intentionally NO second/parallel spawn path (plan 41.1-02 key_link).
    fn start_pending_turn(&mut self) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();
        self.pending_rx = Some(rx);
        self.pending_tx = Some(tx);
        self.cancel_child = Some(self.cancel_parent.child_token());
        self.assistant_buffer = None;
        // Phase 36.6.2 Plan 02: a new turn starts — clear the previous
        // turn's buffered activity so the pane doesn't show stale content.
        self.thinking_lines.clear();
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

    // ── Phase 36.17.8 voice capture ───────────────────────────────────────────

    /// Toggle the voice capture loop (Ctrl+B arm — D-08).
    ///
    /// - First press (while idle): starts `run_conversation_loop` as a tokio task,
    ///   sets `voice.recording = true`, voice pill transitions to `● LISTENING`.
    /// - Second press (while recording): calls `voice.stop()`, resets to Idle.
    ///
    /// The cpal callback is SYNC; we bridge to async via the
    /// `block_in_place` + `Handle::current()` pattern (mirrors cmd_stop).
    /// The capture task itself is fully async (await on tokio::time::sleep).
    ///
    /// Requires voice mode to be enabled (`/voice on`) for the loop to start.
    pub fn handle_record_key(&mut self) {
        use std::sync::atomic::Ordering;

        if self.voice.is_recording() {
            // Second Ctrl+B press — stop capture.
            self.voice.stop();
            tracing::info!("handle_record_key: stopped capture");
            return;
        }

        // Require /voice on (or explicit first Ctrl+B enables it).
        self.voice.enabled.store(true, Ordering::Relaxed);

        // Build stop channel.
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        self.voice.stop_tx = Some(stop_tx);

        // Clone handles for the spawned task.
        let recording_flag = self.voice.recording.clone();
        let phase_handle = self.voice.phase.clone();

        // We need a way to submit the transcript from inside the async task.
        // Use an unbounded channel: the task sends transcripts; App::poll_voice_transcript
        // drains it each tick from the event loop.
        let (transcript_tx, transcript_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        self.voice_transcript_rx = Some(transcript_rx);

        // Load config for max_recording_seconds, VAD thresholds, and provider.
        let config = ironhermes_core::Config::load().unwrap_or_default();
        let home = ironhermes_core::constants::get_hermes_home();
        let max_secs = config.voice.max_recording_seconds;
        // D-09: honour the user's tunable VAD thresholds (default 200 / 3.0 s).
        // Some mics run below 200 RMS even during speech — lowering
        // `voice.silence_threshold` lets end-of-speech detection work.
        let vad_settings = ironhermes_tools::vad::VadSettings {
            silence_threshold: config.voice.silence_threshold,
            silence_duration_secs: config.voice.silence_duration,
        };

        // Build SttRegistry and select a provider.
        use ironhermes_tools::stt::{build_stt_registry, select_stt_provider};
        let registry = build_stt_registry(&config.stt);
        let provider_name = select_stt_provider(&config.stt);
        let provider = provider_name.and_then(|name| registry.get(&name));

        let Some(provider) = provider else {
            // No provider available — push a warning into transcript display.
            tracing::warn!(
                "handle_record_key: no STT provider available (set GROQ_API_KEY or VOICE_TOOLS_OPENAI_KEY)"
            );
            return;
        };

        // Mark recording state before spawning.
        recording_flag.store(true, Ordering::Relaxed);
        if let Ok(mut g) = phase_handle.lock() {
            *g = crate::tui_rata::voice_state::RecordPhase::Listening;
        }

        // Clone the phase handle for the loop's phase callback (separate from the
        // task-finish reset clone above) so the status pill reflects LISTENING →
        // TRANSCRIBING → LISTENING as the loop captures and transcribes.
        let phase_for_loop = self.voice.phase.clone();

        // Spawn the conversation loop as a background tokio task.
        let handle = tokio::task::spawn(async move {
            use ironhermes_tools::capture::CapturePhase;
            ironhermes_tools::capture::run_conversation_loop(
                &home,
                provider,
                stop_rx,
                max_secs,
                vad_settings,
                move |transcript| {
                    let _ = transcript_tx.send(transcript);
                },
                move |phase| {
                    if let Ok(mut g) = phase_for_loop.lock() {
                        *g = match phase {
                            CapturePhase::Listening => {
                                crate::tui_rata::voice_state::RecordPhase::Listening
                            }
                            CapturePhase::Transcribing => {
                                crate::tui_rata::voice_state::RecordPhase::Transcribing
                            }
                        };
                    }
                },
            )
            .await;
            // Task finished — reset flags.
            recording_flag.store(false, Ordering::Relaxed);
            if let Ok(mut g) = phase_handle.lock() {
                *g = crate::tui_rata::voice_state::RecordPhase::Idle;
            }
        });

        self.voice.task_handle = Some(handle);
        tracing::info!("handle_record_key: capture loop started");
    }

    /// Drain pending voice transcripts and submit each one as a user turn.
    ///
    /// Called by the event loop each tick. Transcripts arrive via the
    /// `voice_transcript_rx` channel written by the capture task.
    pub fn poll_voice_transcripts(&mut self) {
        use tokio::sync::mpsc::error::TryRecvError;

        // Collect transcripts into a local vec first so we can drop the
        // mutable borrow on `self.voice_transcript_rx` before calling
        // `submit_voice_text`, which itself borrows `self` mutably (E0499).
        let mut pending: Vec<String> = Vec::new();
        let mut disconnected = false;

        if let Some(ref mut rx) = self.voice_transcript_rx {
            loop {
                match rx.try_recv() {
                    Ok(transcript) => {
                        tracing::info!("poll_voice_transcripts: queued {:?}", transcript);
                        pending.push(transcript);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }

        if disconnected {
            self.voice_transcript_rx = None;
        }

        for transcript in pending {
            self.submit_voice_text(transcript);
        }
    }

    /// Submit a voice transcript as a user turn (mirrors `submit()` without
    /// touching the textarea — transcript comes from the capture task, not typed input).
    pub fn submit_voice_text(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        self.history_store.push(text.clone());
        self.history_store.reset_cursor();
        self.history.push(user_message(text.clone()));
        // Phase 46.7 Plan 06: voice turns don't parse @path (no composer text
        // to scan) but still record the caption for post-turn opt-out detection.
        self.last_submitted_text = text;
        // Phase 36.17.8: voice input — `/voice on` speaks this reply.
        self.last_turn_was_voice = true;

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();
        self.pending_rx = Some(rx);
        self.pending_tx = Some(tx);
        self.cancel_child = Some(self.cancel_parent.child_token());
        self.scroll_to_bottom();
        self.assistant_buffer = None;
        // Phase 36.6.2 Plan 02: a new turn starts — clear the previous
        // turn's buffered activity so the pane doesn't show stale content.
        self.thinking_lines.clear();
    }

    // ── Phase 46.7 Plan 06: TUI attachments (D-18/D-20/D-12) ───────────────────

    /// Parse `@path` tokens (and plausible terminal-drag-drop bare paths) out
    /// of `text`, copy each into the session attachment store (D-20), and
    /// append to `self.pending_attachments` alongside anything already queued
    /// by `/attach` (Task 1). Returns the text with attach tokens stripped
    /// plus `(display_name, reason)` for any that failed to attach.
    fn parse_and_queue_attach_paths(&mut self, text: &str) -> (String, Vec<(String, String)>) {
        let (stripped, candidates) = extract_attach_candidates(text);
        let mut errors = Vec::new();
        for candidate in candidates {
            match self.copy_local_path_into_store(&candidate) {
                Ok(pending) => self.pending_attachments.push(pending),
                Err(e) => errors.push(e),
            }
        }
        (stripped, errors)
    }

    /// Build the turn's `ChatMessage` from `text` plus every queued
    /// `pending_attachments` entry (drained). Re-reads bytes from the
    /// session attachment store (rather than caching them in memory) and
    /// feeds `process_local_attachment` + `build_chat_user_message` (Plan
    /// 02) — the SAME shared delivery pipeline the web-chat surface calls
    /// (D-12). `workspace_path` is `session_attachments_dir(session_id)`;
    /// `PendingAttachment::stored_rel_path` (the `<opaque-id>/<leaf>` on-disk
    /// path) is threaded through as the `process_local_attachment` filename
    /// argument so a `LocalAttachment::WorkspaceFile` note's
    /// `workspace_path.join(filename)` resolves to the REAL on-disk path
    /// (there is no TUI workspace redirect to mirror the web-chat layout — D-22).
    fn build_user_message_with_attachments(&mut self, text: &str) -> ChatMessage {
        let attachments_dir = ironhermes_core::session_attachments_dir(&self.session_id);
        let queued: Vec<PendingAttachment> = self.pending_attachments.drain(..).collect();

        let mut locals: Vec<ironhermes_gateway::multimodal::LocalAttachment> = Vec::new();
        for pending in &queued {
            let path = attachments_dir.join(&pending.stored_rel_path);
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(
                        error = %e, path = %path.display(),
                        "Phase 46.7: failed to re-read queued attachment at submit time"
                    );
                    continue;
                }
            };
            let mime = pending
                .content_type
                .clone()
                .unwrap_or_else(|| "text/plain".to_string());
            match ironhermes_gateway::multimodal::process_local_attachment(
                &bytes,
                &mime,
                &pending.stored_rel_path,
                ironhermes_gateway::multimodal::PdfMode::TextExtract,
            ) {
                Ok(local) => {
                    // Phase 46.7 Plan 07 (D-19): record the display metadata for
                    // the `[📎 ...]` transcript chip BEFORE `pending`/`queued`
                    // (and its `PendingAttachment` filename) drop out of scope —
                    // this is the only surviving record once the turn is sent.
                    //
                    // Phase 36.6.4 Plan 12 (G-09 closure): `history_anchor`
                    // is a PLACEHOLDER here — the owning user message isn't
                    // pushed into `history` until the caller (`submit()`)
                    // does it one line later, so this function cannot
                    // compute the real anchor. `submit()` overwrites it
                    // immediately after that push.
                    self.sent_attachment_chips.push(SentAttachmentChip {
                        filename: pending.filename.clone(),
                        size_bytes: bytes.len() as u64,
                        history_anchor: 0,
                    });
                    locals.push(local);
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e, filename = %pending.filename,
                        "Phase 46.7: failed to process queued attachment"
                    );
                }
            }
        }

        ironhermes_gateway::multimodal::build_chat_user_message(text, &locals, &attachments_dir)
    }

    /// Copy a local file into this session's attachment store (D-20) — the
    /// SAME store + dir layout web uploads use (Plan 01) — so there is one
    /// code path + full-persistence resume for both surfaces (D-12).
    ///
    /// `path_str` is resolved against the operator's real CWD (the TUI never
    /// redirects to a session workspace — D-22). Returns the UI-SPEC error
    /// vocabulary ("file too large" / "unsupported file type" / "read error")
    /// as `(display_name, reason)` on failure, shared verbatim by `/attach`
    /// and `@path` feedback copy.
    pub fn copy_local_path_into_store(
        &self,
        path_str: &str,
    ) -> Result<PendingAttachment, (String, String)> {
        let resolved = resolve_against_cwd(path_str);
        let raw_filename = resolved
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let display_name = if raw_filename.is_empty() {
            path_str.to_string()
        } else {
            raw_filename.clone()
        };

        // T-46.7-18: validate the STORED leaf before it ever touches a
        // filesystem write path. `Path::file_name()` already strips any
        // ".."/"/" components, so a crafted `path_str` ending in ".." (or
        // "/") yields an empty `raw_filename` and is rejected by the same
        // empty-string guard `safe_attachment_leaf` applies.
        let Some(leaf) = ironhermes_core::safe_attachment_leaf(&raw_filename) else {
            return Err((display_name, "unsupported file type".to_string()));
        };
        let leaf = leaf.to_string();

        let bytes = match std::fs::read(&resolved) {
            Ok(b) => b,
            Err(_) => return Err((display_name, "read error".to_string())),
        };

        let content_type = guess_mime_from_extension(&leaf);
        let is_image = content_type.starts_with("image/");
        if !is_image && bytes.len() > ironhermes_gateway::multimodal::NONIMAGE_MAX_BYTES {
            return Err((display_name, "file too large".to_string()));
        }

        let Some(store) = self.state_store.clone() else {
            return Err((display_name, "read error".to_string()));
        };

        let att_dir = uuid::Uuid::new_v4().simple().to_string();
        let dest_dir = ironhermes_core::session_attachments_dir(&self.session_id).join(&att_dir);
        if std::fs::create_dir_all(&dest_dir).is_err() {
            return Err((display_name, "read error".to_string()));
        }
        let dest_path = dest_dir.join(&leaf);
        if std::fs::write(&dest_path, &bytes).is_err() {
            return Err((display_name, "read error".to_string()));
        }

        let stored_rel_path = format!("{att_dir}/{leaf}");

        let insert_result = match store.lock() {
            Ok(mut guard) => guard.add_chat_attachment(
                &self.session_id,
                None,
                &leaf,
                Some(content_type.as_str()),
                bytes.len() as i64,
                &stored_rel_path,
            ),
            Err(_) => return Err((display_name, "read error".to_string())),
        };
        if insert_result.is_err() {
            return Err((display_name, "read error".to_string()));
        }

        Ok(PendingAttachment {
            filename: leaf,
            content_type: Some(content_type),
            stored_rel_path,
        })
    }

    // ── Transcript rendering ──────────────────────────────────────────────────

    /// Render `App.history[idx]`'s lines exactly as `transcript_text`'s old
    /// per-message loop body did, or an empty `Vec` when the row is hidden
    /// or has no visible role style (Phase 36.6.4 Plan 12, G-09 closure —
    /// extracted from `transcript_text` so `transcript_render_units` can
    /// interleave history rows with anchored units one row at a time
    /// instead of appending a flat pre-built block).
    fn history_lines_for(&self, idx: usize) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        let Some(msg) = self.history.get(idx) else {
            return lines;
        };
        // Phase 41.1 Plan 02 (key_link / UI-SPEC §C): a bare-invoke synthetic
        // skill trigger is model-facing turn content only — never a user
        // bubble. Its DIM meta chip (a separate Role::System line) is the
        // sole user-visible artifact, so skip rendering the trigger itself.
        //
        // Phase 36.6.4 Plan 03 (D-11/D-16): a shell-run's captured-output
        // message is rendered EXCLUSIVELY via `shell_runs`/
        // `shell_block_lines`'s custom styling — skip it here or it would
        // double-render.
        //
        // Phase 36.6.4 Plan 07 (G-01): both checks route through the SAME
        // `history_row_is_hidden` predicate `transcript_line_count` uses,
        // so the rendered rows and the counted rows can never diverge.
        if self.history_row_is_hidden(idx) {
            return lines;
        }
        let (role_label, color) = role_style(msg);
        let Some(color) = color else { return lines };
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
        lines
    }

    /// Render the in-flight `assistant_buffer` block exactly as
    /// `transcript_text`'s old trailing block did (Phase 36.6.4 Plan 12,
    /// G-09 closure — extracted so `transcript_render_units` can emit it
    /// LAST among conversation content, after every anchored unit, so the
    /// reply currently arriving is always bottom-most, D-02).
    fn streaming_lines(&self) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
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
        lines
    }

    /// Build a `Text<'static>` for the transcript paragraph widget.
    ///
    /// System messages are suppressed (role_style returns `None` for System).
    /// Streaming buffer is appended in green at the end.
    ///
    /// Phase 36.6.4 Plan 12 (G-09 closure): thin wrapper over
    /// `history_lines_for` (one call per history row) + `streaming_lines`
    /// — preserves this function's exact prior output byte-for-byte; the
    /// per-row extraction lets `transcript_render_units` interleave the
    /// same two building blocks with anchored units instead.
    pub fn transcript_text(&self) -> Text<'static> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        for idx in 0..self.history.len() {
            lines.extend(self.history_lines_for(idx));
        }
        lines.extend(self.streaming_lines());
        Text::from(lines)
    }

    /// Build every transcript line in render order, tagged with its content
    /// group, its `history_anchor` ordering key and (for a clickable chip)
    /// the click action it carries.
    ///
    /// Phase 36.6.4 Plan 12 (G-09 closure): THE ONLY place emission order is
    /// decided, rewritten from Plan 07's group-major concatenation to an
    /// ORDER RULE over each unit's `history_anchor` (see `TranscriptGroup`'s
    /// doc comment for why group-major order was the defect). Both
    /// `transcript_render_text` and `rebuild_chip_hit_test` still derive
    /// from this SAME enumeration, so a group appended here still appears
    /// in the rendered text, the measured height and the chip hit-test
    /// offsets all at once.
    ///
    /// **The rule.** Every anchored unit (attachment/artifact/image chips,
    /// shell runs) is bucketed by `history_anchor.min(history.len())` — the
    /// clamp handles a stale anchor surviving `/clear`, which empties
    /// `history` but leaves the three chip/run collections populated.
    /// Within one bucket, units keep STABLE creation order, and the
    /// non-shell groups keep their pre-Plan-12 relative sequence
    /// (attachment, then artifact, then image) — shell runs are bucketed
    /// last so two groups sharing an anchor still read attachment/
    /// artifact/image/shell, matching the original group-major order when
    /// nothing anchors ahead of the others. The walk then interleaves:
    /// bucket `0` first, then for `i` in `0..history.len()`, history row
    /// `i`'s lines (`history_lines_for`, `history_anchor: i`, skipped when
    /// hidden) followed by bucket `i + 1`. `streaming_lines()` is always
    /// emitted last, at `history_anchor: history.len()`, so the in-flight
    /// reply is always bottom-most among conversation content (D-02).
    ///
    /// `captured_artifacts` (group `ArtifactChips`, D-G09-4) is the one
    /// group with NO creation-time anchor available: it is an
    /// `Arc<Mutex<Vec<CapturedArtifact>>>` cloned into the streaming task
    /// (`event_loop.rs`) and pushed from a closure holding no `&App`, and
    /// artifacts arrive before the assistant message they belong to
    /// exists. It is given a RENDER-TIME anchor of `history.len()` instead
    /// — after all settled history, before the streaming buffer — which is
    /// a one-line reversal to a pinned trailing block if ever preferred.
    pub fn transcript_render_units(&self) -> Vec<TranscriptUnit> {
        TRANSCRIPT_UNIT_BUILDS.with(|c| bump(c, 1));
        let history_len = self.history.len();
        let mut buckets: Vec<Vec<TranscriptUnit>> = (0..=history_len).map(|_| Vec::new()).collect();

        // Phase 36.6.4 Plan 12 Task 2: attachment chips carry a REAL
        // creation-time anchor, stamped by `App::submit` right after the
        // owning user message is pushed.
        for chip in &self.sent_attachment_chips {
            let anchor = chip.history_anchor.min(history_len);
            buckets[anchor].push(TranscriptUnit {
                group: TranscriptGroup::AttachmentChips,
                line: attachment_chip_line(chip),
                plain: None,
                action: None,
                history_anchor: anchor,
            });
        }
        // D-G09-4: `captured_artifacts` is the ONE group with no obtainable
        // creation-time anchor — it is an `Arc<Mutex<Vec<CapturedArtifact>>>`
        // cloned into the streaming task (`event_loop.rs`, around the
        // `captured_artifacts` clone + its later push from a closure that
        // holds no `&App`), and artifacts arrive mid-turn, before the
        // assistant message they belong to even exists. A RENDER-TIME
        // anchor of `history_len` keeps them below all settled history and
        // above the in-flight streaming reply; switching to a pinned
        // trailing block instead is a one-line change if ever preferred.
        if let Ok(artifacts) = self.captured_artifacts.lock() {
            for artifact in artifacts.iter() {
                let anchor = history_len;
                buckets[anchor].push(TranscriptUnit {
                    group: TranscriptGroup::ArtifactChips,
                    line: artifact_chip_line(artifact),
                    plain: Some(artifact_chip_plain(artifact)),
                    action: Some(ChipAction::OpenArtifactUrl(artifact_browser_url(
                        &artifact.artifact_id,
                    ))),
                    history_anchor: anchor,
                });
            }
        }
        // Phase 36.6.4 Plan 12 Task 2: image chips carry a REAL
        // creation-time anchor, stamped by `commit_assistant_buffer` (after
        // the assistant message that produced them) or `handle_image_slash`
        // (`commands.rs`, at `/image <path>` success).
        for chip in &self.image_chips {
            let anchor = chip.history_anchor.min(history_len);
            buckets[anchor].push(TranscriptUnit {
                group: TranscriptGroup::ImageChips,
                line: image_chip_line(chip),
                plain: Some(image_chip_plain(chip)),
                action: Some(ChipAction::OpenImage {
                    label: chip.label.clone(),
                    source: chip.source.clone(),
                }),
                history_anchor: anchor,
            });
        }
        // Phase 36.6.4 Plan 03 (D-09..D-11, TUI-BANG-01): `!` shell blocks —
        // directly-styled lines (NOT a new `Role`). Plan 12: the ONE group
        // with a real anchor in this task — `run.history_anchor` was
        // stamped by `apply_shell_outcome` at the same point in time as its
        // hidden `Role::System` copy.
        for run in &self.shell_runs {
            let anchor = run.history_anchor.min(history_len);
            for line in shell_bang::shell_block_lines(run) {
                buckets[anchor].push(TranscriptUnit {
                    group: TranscriptGroup::ShellRuns,
                    line,
                    plain: None,
                    action: None,
                    history_anchor: anchor,
                });
            }
        }

        let mut units: Vec<TranscriptUnit> = Vec::new();
        units.append(&mut buckets[0]);
        for i in 0..history_len {
            for line in self.history_lines_for(i) {
                units.push(TranscriptUnit {
                    group: TranscriptGroup::History,
                    line,
                    plain: None,
                    action: None,
                    history_anchor: i,
                });
            }
            units.append(&mut buckets[i + 1]);
        }
        for line in self.streaming_lines() {
            units.push(TranscriptUnit {
                group: TranscriptGroup::History,
                line,
                plain: None,
                action: None,
                history_anchor: history_len,
            });
        }
        units
    }

    /// Build the transcript `Text` for the Paragraph widget — every
    /// `transcript_render_units()` line, in order, with the group/action
    /// tags dropped. No group list is maintained here; it is inherited
    /// entirely from `transcript_render_units`.
    pub fn transcript_render_text(&self) -> Text<'static> {
        Text::from(
            self.transcript_render_units()
                .into_iter()
                .map(|unit| unit.line)
                .collect::<Vec<_>>(),
        )
    }

    /// The frame's single entry point for transcript geometry (Phase 36.6.4
    /// Plan 10, G-08 closure). Builds the content enumeration and measures
    /// it once; every consumer — the title indicator, the chip hit-test,
    /// the ScrollView content size, the scrollbar and link extraction — all
    /// read the SAME `TranscriptMeasurement` for a frame instead of each
    /// re-deriving it.
    ///
    /// Task 2 (G-08 closure): a single-entry memo sits behind this exact
    /// signature — no call site changes twice. The units are always built
    /// first (the deliberate cost of an honest key: the fingerprint must be
    /// computed FROM the same enumeration the render would walk), then a
    /// `MeasureKey` is derived from them; a match against the cached key
    /// returns the cached `Arc` without a render, a miss measures, caches
    /// and returns the fresh one. The cache is single-entry — replaced, not
    /// accumulated — so memory is bounded by one transcript-sized snapshot.
    pub fn transcript_measurement(&self, width: usize) -> Arc<TranscriptMeasurement> {
        let units = self.transcript_render_units();
        let key = MeasureKey {
            fingerprint: transcript_content_fingerprint(&units),
            unit_count: units.len(),
            total_display_width: transcript_units_total_display_width(&units),
            width,
        };

        {
            let guard = self
                .transcript_measure_cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some((cached_key, cached)) = guard.as_ref()
                && *cached_key == key
            {
                TRANSCRIPT_CACHE_HITS.with(|c| bump(c, 1));
                return Arc::clone(cached);
            }
        }

        TRANSCRIPT_CACHE_MISSES.with(|c| bump(c, 1));
        let measurement = Arc::new(self.measure_transcript_uncached(units, width));

        let mut guard = self
            .transcript_measure_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = Some((key, Arc::clone(&measurement)));
        measurement
    }

    /// The single interleaved render pass (Phase 36.6.4 Plan 10, Task 1):
    /// collapses Plan 07's two independent sentinel renders
    /// (`transcript_rendered_plain_rows` + `transcript_unit_row_offsets`)
    /// into ONE scratch `Paragraph` render that yields rows, per-unit
    /// `(start_row, end_row)` offsets and content height together.
    ///
    /// Interleaves a `TRANSCRIPT_MEASURE_SENTINEL` `Line` before every
    /// unit's own line, plus one trailing sentinel (N+1 sentinels for N
    /// units) — identical construction to Plan 07's per-unit render.
    /// Because `Paragraph` wraps each `Line` independently, a sentinel
    /// `Line` never changes how any OTHER line wraps.
    ///
    /// Rows are classified with a single forward walk (never a backward
    /// scan): a row begins a sentinel occurrence when it starts with the
    /// sentinel's next expected prefix; the occurrence's remaining rows are
    /// consumed by continuing to match the sentinel's remaining characters
    /// against the following rows. This derives the sentinel's row span
    /// from the render itself — it never calls `word_wrapped_line_count` to
    /// predict it. A row that starts matching but never completes the
    /// match (a false start — e.g. transcript content that happens to
    /// begin with the sentinel's bytes, T-36.6.4-G08-06) is flushed back
    /// into `rows` rather than lost, so a coincidental collision can only
    /// ever cost a mis-classified row, never a dropped one.
    ///
    /// If fewer than N+1 sentinels are found, the scratch bound
    /// under-provisioned for this content: double it and retry, at most
    /// twice. If the last attempt still comes up short, every rendered row
    /// is kept (a harmless blank tail) and `offsets` covers only the units
    /// whose sentinels were actually located — the same safe failure
    /// direction Plan 07 documented, never a clipped real row.
    fn measure_transcript_uncached(&self, units: Vec<TranscriptUnit>, width: usize) -> TranscriptMeasurement {
        if width == 0 || units.is_empty() {
            return TranscriptMeasurement {
                width,
                units,
                rows: Vec::new(),
                offsets: Vec::new(),
            };
        }

        let sentinel = TRANSCRIPT_MEASURE_SENTINEL;
        let sentinel_len = sentinel.len(); // pure ASCII — byte len == char len

        // The interleaved Text's shape is fixed across retries; only the
        // scratch buffer's height changes.
        let mut text = Text::default();
        for unit in &units {
            text.lines.push(Line::from(sentinel.to_string()));
            text.lines.push(unit.line.clone());
        }
        text.lines.push(Line::from(sentinel.to_string()));

        let interleaved_line_count = text.lines.len();
        let total_display_width: usize = text
            .lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
                    .sum::<usize>()
            })
            .sum();

        // TIGHT bound derived only from the content being rendered — no
        // `HEADROOM_MULTIPLIER`/`HEADROOM_FIXED_ROWS` over-provision.
        // Correctness no longer depends on the bound being generous because
        // an under-provisioned render is detected (fewer than N+1
        // sentinels found) and retried with a doubled bound below.
        let base_cap = interleaved_line_count
            .saturating_add(total_display_width.div_ceil(width))
            .saturating_add(8)
            .min(u16::MAX as usize) as u16;

        let mut renders_local: u64 = 0;
        let mut scratch_rows_local: u64 = 0;
        let mut cells_walked_local: u64 = 0;
        let mut row_lookups_local: u64 = 0;

        let mut cap = base_cap;
        let mut rows: Vec<String> = Vec::new();
        let mut offsets: Vec<(usize, usize)> = Vec::new();

        for attempt in 0..3u8 {
            let scratch_area = Rect::new(0, 0, width as u16, cap);
            let mut buf = ratatui::buffer::Buffer::empty(scratch_area);
            let paragraph = ratatui::widgets::Paragraph::new(text.clone())
                .wrap(ratatui::widgets::Wrap { trim: false });
            ratatui::widgets::Widget::render(paragraph, scratch_area, &mut buf);
            renders_local += 1;
            scratch_rows_local += scratch_area.height as u64;
            cells_walked_local += scratch_area.height as u64 * scratch_area.width as u64;

            rows = Vec::with_capacity(scratch_area.height as usize);
            offsets = Vec::with_capacity(units.len());
            let mut starts: Vec<usize> = Vec::with_capacity(units.len());
            let mut matched_chars: usize = 0;
            let mut pending_sentinel_rows: Vec<String> = Vec::new();
            let mut sentinel_index: usize = 0;

            for row in 0..scratch_area.height {
                row_lookups_local += 1;
                // Walk by CONSUMED WIDTH (not cell index) — a wide glyph's
                // continuation cell is skipped, matching every other
                // transcript-row extraction in this file.
                let mut line = String::new();
                let mut col: u16 = 0;
                while col < scratch_area.width {
                    let symbol = buf
                        .cell((col, row))
                        .map(|c| c.symbol().to_string())
                        .unwrap_or_default();
                    let w = (UnicodeWidthStr::width(symbol.as_str()) as u16).max(1);
                    line.push_str(&symbol);
                    col = col.saturating_add(w);
                }

                let remaining = &sentinel[matched_chars..];
                let prefix_len = remaining.len().min(width);
                let expected_prefix = &remaining[..prefix_len];
                if !expected_prefix.is_empty() && line.starts_with(expected_prefix) {
                    pending_sentinel_rows.push(line);
                    matched_chars += prefix_len;
                    if matched_chars >= sentinel_len {
                        // Sentinel occurrence #sentinel_index fully matched
                        // — these rows genuinely were the sentinel, not
                        // transcript content, so they are discarded (never
                        // pushed to `rows`).
                        pending_sentinel_rows.clear();
                        matched_chars = 0;
                        if sentinel_index > 0 {
                            let start = starts[sentinel_index - 1];
                            offsets.push((start, rows.len()));
                        }
                        if sentinel_index < units.len() {
                            starts.push(rows.len());
                        }
                        sentinel_index += 1;
                        if sentinel_index > units.len() {
                            // The trailing (N+1-th) sentinel just completed —
                            // every unit is measured. Stop walking rows: the
                            // scratch buffer's tight-but-not-exact bound
                            // (base_cap) leaves some blank rows past this
                            // point, and without this break they would be
                            // misread as extra content rows, inflating
                            // `rows.len()` past the true height (mirrors the
                            // old `rows.truncate(sentinel_row)` behavior).
                            break;
                        }
                    }
                } else {
                    // Not a sentinel continuation — flush any in-progress
                    // false-start rows back into `rows` (they were real
                    // content, not a completed sentinel occurrence) before
                    // pushing this row too.
                    if !pending_sentinel_rows.is_empty() {
                        rows.append(&mut pending_sentinel_rows);
                        matched_chars = 0;
                    }
                    rows.push(line);
                }
            }
            // A match still in progress when the buffer ran out is a false
            // start (or an under-provisioned buffer) — flush it back into
            // `rows` rather than losing it.
            if !pending_sentinel_rows.is_empty() {
                rows.append(&mut pending_sentinel_rows);
            }

            if sentinel_index > units.len() {
                break; // success — every sentinel located
            }
            if attempt < 2 {
                cap = cap.saturating_mul(2);
            }
        }

        TRANSCRIPT_RENDERS.with(|c| bump(c, renders_local));
        TRANSCRIPT_SCRATCH_ROWS.with(|c| bump(c, scratch_rows_local));
        TRANSCRIPT_CELLS_WALKED.with(|c| bump(c, cells_walked_local));
        TRANSCRIPT_ROW_LOOKUPS.with(|c| bump(c, row_lookups_local));

        TranscriptMeasurement { width, units, rows, offsets }
    }

    /// Half-open `(start_row, end_row)` for each `transcript_render_units()`
    /// entry, at wrap `width` — thin wrapper over `transcript_measurement`
    /// (Phase 36.6.4 Plan 10). Kept for callers/tests that only need the
    /// offsets, not the full measurement.
    pub fn transcript_unit_row_offsets(&self, width: usize) -> Vec<(usize, usize)> {
        self.transcript_measurement(width).offsets.clone()
    }

    /// Build the per-render chip hit-test map for `area` and store it into
    /// `self.chip_hit_test` (Phase 46.7 Plan 07, D-17). Called once per render
    /// pass from `ui.rs::render_transcript`; `handle_mouse` consults the
    /// stored map on `Down(Left)`. Only artifact-link chips get an entry —
    /// plain attachment chips are display-only per the UI-SPEC.
    ///
    /// Phase 36.6.4 Plan 07 Task 2: walks `transcript_render_units()` zipped
    /// with `transcript_unit_row_offsets()` — the SAME enumeration
    /// `transcript_render_text` renders from — instead of maintaining its
    /// own running row cursor over a hand-duplicated group order. A rect's
    /// geometry (tight single-row width via `UnicodeWidthStr`, full-width
    /// fallback for a wrapped multi-row chip, `area.x + 1` / `area.y + 1`
    /// border offsets, visible-window clamp against `transcript_scroll()`)
    /// is unchanged from before this rewrite.
    ///
    /// A chip fully scrolled outside the visible viewport gets no entry — the
    /// map is bounded to what's on screen this frame (T-46.7-22 accepted
    /// disposition) and is fully rebuilt every call (no cross-frame
    /// accumulation).
    ///
    /// Phase 36.6.4 Plan 10 Task 1 (G-08 closure): takes the frame's
    /// already-computed `&TranscriptMeasurement` as a second parameter
    /// instead of re-deriving `transcript_render_units`/
    /// `transcript_unit_row_offsets` itself — `ui.rs::render_transcript`
    /// obtains the measurement once and shares it with this call, so a
    /// frame's hit-test build never costs a second scratch render.
    /// `measurement.width` is used in place of a freshly-derived
    /// `inner_transcript_width(area)` — the two must always agree, since
    /// the caller built the measurement from this same `area`.
    pub fn rebuild_chip_hit_test(&self, area: Rect, measurement: &TranscriptMeasurement) {
        // Phase 36.6.4 Plan 02: cache the render-time area for `handle_key`'s
        // keyboard-only yank/visual-mode paths — see `transcript_area`'s doc
        // comment for why this lives here rather than threading a `Rect`
        // through `handle_key`.
        if let Ok(mut guard) = self.transcript_area.lock() {
            *guard = area;
        }
        let inner_width = measurement.width;
        let visible_rows = area.height.saturating_sub(2) as usize;
        // Phase 36.6.4 Plan 01/02 (Pitfall 2): reads `scroll_view_state` —
        // the SOLE offset authority — never a second cached field, so a
        // chip's hit-test rect can never silently drift from where it's
        // actually drawn after a scroll.
        let scroll = self.transcript_scroll() as usize;

        let units = &measurement.units;
        let offsets = &measurement.offsets;

        let mut hits: Vec<(Rect, ChipAction)> = Vec::new();
        for (unit, (start, end)) in units.iter().zip(offsets.iter()) {
            let Some(action) = &unit.action else {
                continue;
            };
            let (start, end) = (*start, *end);
            let vis_start = start.max(scroll);
            let vis_end = end.min(scroll.saturating_add(visible_rows));
            if vis_start >= vis_end {
                continue;
            }
            let y = area.y.saturating_add(1).saturating_add((vis_start - scroll) as u16);
            let height = (vis_end - vis_start) as u16;
            let row_count = end - start;
            // Single-row chips get a tight rect (the chip's own display
            // width, clamped to the pane); a wrapped multi-row chip falls
            // back to the full inner width for its spanned rows — the "cell
            // range" it actually occupies either way.
            let width = if row_count <= 1 {
                unit.plain
                    .as_deref()
                    .map(|plain| (UnicodeWidthStr::width(plain) as u16).min(inner_width as u16))
                    .unwrap_or(inner_width as u16)
            } else {
                inner_width as u16
            };
            let rect = Rect {
                x: area.x.saturating_add(1),
                y,
                width,
                height,
            };
            hits.push((rect, action.clone()));
        }

        if let Ok(mut guard) = self.chip_hit_test.lock() {
            *guard = hits;
        }
    }

    /// Build the plain-text (unstyled, ANSI-free) wrapped rows of the
    /// CURRENT transcript render, in the exact same order/wrap boundaries
    /// the live `ScrollView` render uses (Phase 36.6.4 Plan 01, D-04).
    ///
    /// Re-derives via a scratch render of the SAME `Paragraph` + wrap width
    /// the live render uses, so text extraction can never drift from what
    /// the operator visually selected. `width` MUST be
    /// `inner_transcript_width(area)` (memory `feedback_scroll_width_inner`),
    /// matching every other transcript-width consumer in this file.
    ///
    /// Phase 36.6.4 Plan 07 (G-01/G-02/G-06 closure): the PRIMARY height
    /// derivation, not a selection-only helper — every other row-count
    /// consumer (`transcript_total_line_count`, and through it
    /// `transcript_max_scroll`, `ui.rs::render_transcript`'s ScrollView
    /// content size and scrollbar) ultimately reads `.len()` of this Vec.
    ///
    /// Phase 36.6.4 Plan 10 (G-08 closure): thin wrapper over
    /// `transcript_measurement` — the scratch render this used to perform
    /// directly is now the ONE shared interleaved pass, memoized from Task
    /// 2 onward. Kept for callers/tests that only need the rows, not the
    /// full measurement (e.g. `yank_selection`, `resolve_click_selection`).
    pub fn transcript_rendered_plain_rows(&self, width: usize) -> Vec<String> {
        self.transcript_measurement(width).rows.clone()
    }

    /// One-shot window (in `knight_rider_tick` units, ~100ms each) the copy
    /// confirmation toast stays in the status-line hint slot before
    /// reverting to the normal hint (Phase 36.6.4 Plan 02, D-04, UI-SPEC §2:
    /// "a fixed ~2s frame-tick window"). 20 ticks * 100ms ≈ 2s.
    const COPY_CONFIRMATION_WINDOW_TICKS: u64 = 20;

    /// Read the active copy-confirmation toast text, if its window hasn't
    /// elapsed (Phase 36.6.4 Plan 02, D-04). `&self` accessor for the
    /// render path (`ui.rs::render_status`), which only ever holds `&App`.
    pub fn copy_confirmation_text(&self) -> Option<&str> {
        self.copy_confirmation.as_ref().map(|(text, _)| text.as_str())
    }

    /// Yank the active selection (if any) to the system clipboard over
    /// OSC52 (Phase 36.6.4 Plan 01, D-04/D-06). No-op on an empty or absent
    /// selection (D-04). On a write failure, renders a single `Role::System`
    /// / `Color::DarkGray` transcript line — never a status-line toast
    /// (UI-SPEC §2) — reusing the existing System-role line convention.
    ///
    /// A successful or truncated write sets `copy_confirmation` (Phase
    /// 36.6.4 Plan 02) rather than writing `status.hint` directly — the
    /// confirmation is a TRANSIENT ~2s toast that `on_tick` reverts, not a
    /// permanent hint replacement. The wording (Phase 36.6.4 Plan 08) is a
    /// pure function of what the app actually OBSERVED — `copy_toast` — not
    /// an unconditional receipt claim: OSC52 never acks, so absent a
    /// `Confirmed` native write, the app can only honestly confirm it
    /// ATTEMPTED the write, not that the clipboard actually received it.
    pub fn yank_selection(&mut self, area: Rect) {
        let Some(sel) = self.selection else {
            return;
        };
        if sel.is_empty() {
            return;
        }
        let width = inner_transcript_width(area);
        let rows = self.transcript_rendered_plain_rows(width);
        let text = selection::selected_text(&rows, &sel);
        let outcome = (self.clipboard_yank)(&text);
        self.apply_clipboard_outcome(outcome);
    }

    /// Apply the result of a yank attempt to app state — factored out of
    /// `yank_selection` (Phase 36.6.4 Plan 02 Task 3; renamed and
    /// re-typed in Plan 08) so the write-failure and wording branches are
    /// unit-testable without forcing a REAL stdout write failure or a real
    /// terminal-capability environment. `copy_toast` (Plan 08) owns the
    /// wording decision; this fn only routes the resulting string (or the
    /// write-failure line, or nothing) into app state.
    fn apply_clipboard_outcome(&mut self, outcome: selection::ClipboardOutcome) {
        match outcome {
            selection::ClipboardOutcome::Written(report, caps) => {
                let toast = selection::copy_toast(report, caps);
                self.copy_confirmation = Some((
                    toast,
                    self.knight_rider_tick
                        .saturating_add(Self::COPY_CONFIRMATION_WINDOW_TICKS),
                ));
            }
            selection::ClipboardOutcome::Empty => {
                // Empty selection — silent no-op (D-04): no write, no toast, no error.
            }
            selection::ClipboardOutcome::WriteFailed(e) => {
                let mut system = ChatMessage::user(format!("Could not copy selection: {e}."));
                system.role = Role::System;
                self.history.push(system);
            }
        }
    }

    /// Trigger the ONE-TIME image decode for the currently open image
    /// overlay (Phase 36.6.4 Plan 05, D-13, Task 2). `&self` — called from
    /// `overlay::render_image_viewer`, which only ever holds `&App`.
    /// `image_decode` is set to `Decoding` synchronously here so a second
    /// call before the spawned task finishes is a no-op from the caller's
    /// perspective (the caller only calls this when it observes `None`).
    ///
    /// Guarded against a missing Tokio runtime (`Handle::try_current`) so a
    /// plain, non-async `#[test]` that renders overlay CHROME only (no
    /// decode round-trip) never panics — it simply leaves the state at
    /// `Decoding` forever, which is exactly the state that class of test
    /// wants to observe. A real session always runs inside the Tokio
    /// runtime the whole TUI is built on, so production behavior is
    /// unaffected.
    pub(crate) fn trigger_image_decode(&self, source: MediaRef, target: Rect) {
        if let Ok(mut guard) = self.image_decode.lock() {
            *guard = Some(ImageDecodeState::Decoding);
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let decode_state = Arc::clone(&self.image_decode);
        let picker = self.picker.clone();
        handle.spawn_blocking(move || {
            let result = decode_image_protocol(&source, &picker, target);
            if let Ok(mut guard) = decode_state.lock() {
                *guard = Some(match result {
                    Ok(protocol) => ImageDecodeState::Ready(Arc::new(protocol)),
                    Err(reason) => ImageDecodeState::Failed(reason),
                });
            }
        });
    }

    // ── test-support constructors ─────────────────────────────────────────────

    /// Snapshot of the current chip hit-test map, for cross-module test
    /// assertions (Phase 36.6.4 Plan 02) — `ui.rs`'s test module renders
    /// through the real `ui()` entry point and needs to inspect the
    /// re-derived rects without reaching into the private `chip_hit_test`
    /// field directly. Test-only surface, gated behind `test-support`.
    #[cfg(feature = "test-support")]
    pub fn chip_hit_test_snapshot(&self) -> Vec<(Rect, ChipAction)> {
        self.chip_hit_test.lock().map(|g| g.clone()).unwrap_or_default()
    }

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

    /// Construct an App with a caller-provided `MessageQueue` for integration
    /// tests (Phase 36.17.3 Plan 03). Plan 06 fills in the test bodies that
    /// drive this constructor; this signature is the harness anchor.
    ///
    /// Mirrors `new_test_empty` (same `test-support` gating, same `test_deps()`
    /// factory) but swaps the default queue for the caller's instance so the
    /// test can assert push/pop/len/clear behavior against a known queue.
    #[cfg(feature = "test-support")]
    pub fn new_test_with_queue(
        queue: std::sync::Arc<
            dyn ironhermes_core::queue::MessageQueue<ironhermes_core::session::SessionKey>,
        >,
    ) -> Self {
        let mut app = Self::new(test_deps());
        app.queue = queue;
        app.queue_key =
            ironhermes_core::session::SessionKey::new(ironhermes_core::Platform::Local, "local")
                .with_user("local");
        app
    }
}

// ── Phase 46.7 Plan 06: TUI attachments (D-18/D-20/D-12) ────────────────────

/// A file already copied into `session_attachments_dir(session_id)` (D-20),
/// queued to attach to the NEXT submitted message. Populated by `/attach`
/// (Task 1) and inline `@path` parsing (Task 2); drained by `submit()`.
#[derive(Debug, Clone)]
pub struct PendingAttachment {
    pub filename: String,
    pub content_type: Option<String>,
    /// Relative to `session_attachments_dir(session_id)` — NEVER absolute
    /// (Plan 01 redirect-safety contract). Includes the opaque-id leaf
    /// component (`<opaque-id>/<leaf>`) so it doubles as the
    /// `process_local_attachment` filename argument at submit time (see
    /// `App::build_user_message_with_attachments`).
    pub stored_rel_path: String,
}

/// Resolve `path_str` against the operator's real CWD (never a redirected
/// session workspace — D-22). Absolute paths pass through unchanged.
fn resolve_against_cwd(path_str: &str) -> std::path::PathBuf {
    let p = std::path::Path::new(path_str);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    }
}

/// Parse `@path` tokens (the explicit D-18 attach directive) and plausible
/// terminal-drag-drop bare paths (most terminals paste a raw filesystem path
/// into the composer as plain text when a file is dropped onto the window)
/// out of `text`. Bare paths are only recognized when they resolve to an
/// EXISTING file on disk, to avoid false-positiving on ordinary prose that
/// happens to start with `/`, `./`, or `../`. Returns the text with every
/// recognized attach token stripped (tokens are directives, not model
/// content) plus the ordered list of raw path strings to attach.
fn extract_attach_candidates(text: &str) -> (String, Vec<String>) {
    let mut candidates = Vec::new();
    let mut out_lines: Vec<String> = Vec::new();

    for line in text.lines() {
        let mut kept: Vec<&str> = Vec::new();
        for token in line.split_whitespace() {
            if let Some(rest) = token.strip_prefix('@')
                && !rest.is_empty()
            {
                candidates.push(rest.to_string());
                continue;
            }
            let looks_like_path =
                token.starts_with('/') || token.starts_with("./") || token.starts_with("../");
            if looks_like_path && resolve_against_cwd(token).is_file() {
                candidates.push(token.to_string());
                continue;
            }
            kept.push(token);
        }
        out_lines.push(kept.join(" "));
    }

    (out_lines.join("\n").trim().to_string(), candidates)
}

/// Extension-based MIME guess for a local attachment. Only the `image/*`
/// prefix and the `application/pdf` value are semantically consulted by
/// `process_local_attachment` (Plan 02) — everything else falls through to
/// its text/code path, so an unrecognized extension safely defaults to
/// `text/plain` rather than needing an exhaustive MIME table.
fn guess_mime_from_extension(filename: &str) -> String {
    let ext = std::path::Path::new(filename)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "pdf" => "application/pdf",
        _ => "text/plain",
    }
    .to_string()
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

/// Phase 41.1 Plan 02 (UI-SPEC §C / Copywriting Contract): the DIM run-turn
/// meta chip that precedes a skill's run turn. Bare invoke → `▶ Ran skill
/// /{name}`; argued invoke → `▶ Ran skill /{name} · "{args}"`, with `args`
/// truncated to 40 chars (char-safe) and an inner `…` appended when truncated
/// (UI-SPEC E3). `args_display` is the trimmed, non-empty trailing text, or
/// `None` for a bare invoke.
fn run_turn_meta_chip(name: &str, args_display: Option<&str>) -> String {
    match args_display {
        None => format!("▶ Ran skill /{name}"),
        Some(args) => {
            const MAX: usize = 40;
            let mut chars = args.chars();
            let head: String = chars.by_ref().take(MAX).collect();
            let truncated = chars.next().is_some();
            if truncated {
                format!("▶ Ran skill /{name} · \"{head}…\"")
            } else {
                format!("▶ Ran skill /{name} · \"{head}\"")
            }
        }
    }
}

fn assistant_message(body: String) -> ChatMessage {
    ChatMessage::assistant(&body)
}

/// Phase 36.6.3 Plan 03 (D-11): persist the picker's chosen provider+model to
/// `$IRONHERMES_HOME/config.yaml`. Reuses `Config::load`/`Config::save`
/// (config.rs, atomic temp+rename) — NEVER hand-rolled YAML, NEVER a fresh
/// partial `Config` (would drop mcp_servers/kanban/identities/etc — see the
/// `Config::load` -> mutate -> `.save()` precedent at
/// `memory_cmd.rs::handle_memory_off`). Mutates ONLY `model.provider` /
/// `model.default`; every other key round-trips untouched.
fn persist_model_picker_selection(provider: &str, model: &str) -> anyhow::Result<()> {
    let mut config = ironhermes_core::Config::load()?;
    config.model.provider = provider.to_string();
    config.model.default = model.to_string();
    config.save()
}

/// Count the terminal rows that `line` occupies when rendered by ratatui's
/// `WordWrapper { trim: false }` at the given column `width`.
///
/// Mirrors the word-boundary logic from ratatui-widgets `WordWrapper::process_input`
/// for the `trim: false` case, using `unicode_width::UnicodeWidthChar` for per-char
/// display widths. This is the corrected replacement for the old `wrapped_line_count`
/// which used character-ceiling-divide and diverged from ratatui on word-wrapped lines.
///
/// Properties:
/// - Empty line → 1  (blank row is still rendered)
/// - `width == 0` → 1  (defensive; avoids divide-by-zero)
/// - Long single word → character-wraps (ratatui's `line_full` fallback)
/// - Leading whitespace preserved (char-level iteration, not `split_whitespace`)
///
/// See D-01 root cause and algorithm in
/// `.planning/phases/36.6.1-.../36.6.1-RESEARCH.md` §3.
pub(crate) fn word_wrapped_line_count(line: &str, width: usize) -> usize {
    if line.is_empty() || width == 0 {
        return 1;
    }

    let mut rows: usize = 1;
    let mut current_row_width: usize = 0; // display columns used on the current row

    // We track two "pending" accumulators mirroring ratatui WordWrapper's
    // `pending_word` and `pending_whitespace` buffers (trim: false).
    let mut pending_word_width: usize = 0;
    let mut pending_ws_width: usize = 0;
    let mut in_word = false;

    let flush_word = |rows: &mut usize,
                      current_row_width: &mut usize,
                      pending_ws_width: &mut usize,
                      pending_word_width: &mut usize,
                      width: usize| {
        if *pending_word_width == 0 {
            return;
        }
        let needed = *current_row_width + *pending_ws_width + *pending_word_width;
        if needed <= width {
            // Word fits on current row (including whitespace separator).
            *current_row_width = needed;
        } else {
            // Word doesn't fit on the current row.
            // If we are not already at the start of a row, wrap to a new row first.
            if *current_row_width > 0 || *pending_ws_width > 0 {
                *rows += 1;
                *current_row_width = 0;
                *pending_ws_width = 0; // whitespace before a break is consumed (trim: false)
            }
            // Now place the word at the start of the (possibly just-started) current row.
            if *pending_word_width <= width {
                // Word fits on a fresh row.
                *current_row_width = *pending_word_width;
            } else {
                // Oversized word: character-wrap it across rows (ratatui line_full path).
                // We are at the start of a row, so the first row is already counted.
                let word_rows = (*pending_word_width).div_ceil(width);
                *rows += word_rows - 1; // additional rows beyond the current one
                let remainder = *pending_word_width % width;
                *current_row_width = if remainder == 0 { width } else { remainder };
            }
        }
        *pending_word_width = 0;
        *pending_ws_width = 0;
    };

    for c in line.chars() {
        let char_w = UnicodeWidthChar::width(c).unwrap_or(0);
        if c.is_whitespace() {
            if in_word {
                // Flush the accumulated word before processing the whitespace.
                flush_word(
                    &mut rows,
                    &mut current_row_width,
                    &mut pending_ws_width,
                    &mut pending_word_width,
                    width,
                );
                in_word = false;
            }
            // Accumulate whitespace (trim: false keeps it on the row).
            // If whitespace alone overflows, it wraps — mirror ratatui's grapheme accumulation.
            if char_w > 0 {
                if current_row_width + pending_ws_width + char_w > width {
                    // Whitespace causes a row overflow: emit a new row.
                    rows += 1;
                    current_row_width = 0;
                    pending_ws_width = char_w;
                } else {
                    pending_ws_width += char_w;
                }
            }
        } else {
            in_word = true;
            pending_word_width += char_w;
        }
    }

    // Flush any remaining word at end-of-line.
    if pending_word_width > 0 {
        flush_word(
            &mut rows,
            &mut current_row_width,
            &mut pending_ws_width,
            &mut pending_word_width,
            width,
        );
    }

    rows
}

// ── Phase 46.7 Plan 07: transcript chips + hit-test (D-17/D-19) ────────────

/// Inner transcript render width — the Paragraph's content width after the
/// 1-char border on each side (`area.width - 2`). `transcript_max_scroll`,
/// `ui.rs::render_transcript`'s scrollbar, and `rebuild_chip_hit_test`'s chip
/// rects ALL derive from this one function rather than re-deriving
/// `area.width - 2` independently, so a hit-test rect can never silently
/// drift from what ratatui's Paragraph actually wraps at (memory
/// `feedback_scroll_width_inner`).
pub(crate) fn inner_transcript_width(area: Rect) -> usize {
    area.width.saturating_sub(2) as usize
}

/// The action a `Down(Left)` click on a chip's hit-test rect triggers.
/// Rebuilt from scratch every render (`App::rebuild_chip_hit_test`) —
/// nothing persists across frames except the `App` state that feeds the
/// rebuild. Only artifact-link chips are clickable (D-17 scope fence);
/// plain attachment chips never produce a `ChipAction`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChipAction {
    /// Open this artifact's viewer URL in the default browser.
    OpenArtifactUrl(String),
    /// Open the image viewer overlay for this chip (Phase 36.6.4 Plan 05,
    /// D-13). Carries the chip's label (used verbatim as the overlay's
    /// left title) and the underlying `MediaRef` the decode step reads.
    OpenImage { label: String, source: MediaRef },
}

/// Phase 36.6.4 Plan 12 (G-09 closure): the five content groups a
/// transcript line can come from. `TranscriptGroup` is a PROVENANCE TAG
/// only, consumed by tests and by `rebuild_chip_hit_test`'s click-action
/// dispatch — it is NOT an ordering authority. Emission order is governed
/// by `TranscriptUnit::history_anchor`: `App::transcript_render_units`
/// walks `App.history` and interleaves every anchored unit at the point in
/// time its anchor names (see that function's doc comment for the exact
/// rule). Before Plan 12 this enum's declared order WAS the sole ordering
/// authority — group-major concatenation made a `!` block structurally
/// incapable of rendering above any later history row (the operator's
/// Round 5 report). A sixth group can be added here without becoming a
/// sixth line in a hardcoded list to remember to update; it just needs a
/// `history_anchor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TranscriptGroup {
    History,
    AttachmentChips,
    ArtifactChips,
    ImageChips,
    ShellRuns,
}

/// One rendered transcript line, tagged with the content group that
/// produced it. `plain` carries the ANSI-free text a clickable chip's
/// hit-test rect sizes itself from (`None` for groups that never need it —
/// history/streaming rows and display-only attachment chips are never
/// individually hit-tested). `action` is `Some` only for a clickable chip
/// (artifact links, image chips); history/streaming/attachment/shell-block
/// lines are always `None`.
///
/// `Debug, Clone` (Phase 36.6.4 Plan 10): `TranscriptMeasurement` holds a
/// `Vec<TranscriptUnit>` and both derives on that struct require them.
///
/// `history_anchor` (Phase 36.6.4 Plan 12, G-09 closure): the ordering key
/// `transcript_render_units` sorts units by — see `TranscriptGroup`'s doc
/// comment. It is copied from the producing record's own
/// `history_anchor` (or is the loop index / `history.len()` for
/// history/streaming rows) and is never used to index into `history`.
#[derive(Debug, Clone)]
pub struct TranscriptUnit {
    pub group: TranscriptGroup,
    pub line: Line<'static>,
    pub plain: Option<String>,
    pub action: Option<ChipAction>,
    pub history_anchor: usize,
}

/// A file actually sent with a submitted turn (D-19). Recorded by
/// `App::build_user_message_with_attachments` at drain time — the draining
/// `PendingAttachment` (and its filename) doesn't survive `submit()`, so
/// this is the only surviving record for the `[📎 ...]` transcript chip.
/// Flat + append-only, mirroring `captured_artifacts`'s existing precedent
/// (not indexed to a specific history message — `/clear` doesn't clear
/// either list today).
#[derive(Debug, Clone)]
pub struct SentAttachmentChip {
    pub filename: String,
    pub size_bytes: u64,
    /// Phase 36.6.4 Plan 12 (G-09 closure): the chip's chronological
    /// ordering key — an index into (never past) `App.history`. Pushed
    /// inside `build_user_message_with_attachments` with a placeholder of
    /// `0`; `App::submit` (the caller, which alone knows where the owning
    /// user message lands) overwrites it immediately after pushing that
    /// message.
    pub history_anchor: usize,
}

/// Human-readable byte size for the `[📎 {filename} {size}]` chip (D-19),
/// matching the UI-SPEC's `2.1 MiB` style: binary (1024-based) units, one
/// decimal place, plain `{n} B` under 1 KiB.
fn human_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    let b = bytes as f64;
    if b < KIB {
        format!("{bytes} B")
    } else if b < MIB {
        format!("{:.1} KiB", b / KIB)
    } else {
        format!("{:.1} MiB", b / MIB)
    }
}

/// Default base URL for the local `iron_hermes_ui` web server (matches
/// `dioxus::cli_config::fullstack_address_or_localhost()`'s dev default —
/// `crates/iron_hermes_ui/src/main.rs`). Overridable via `IRONHERMES_WEB_URL`
/// for operators who bind the server to a non-default host/port. T-46.7-21:
/// the resulting URL always points at the LOCAL same-origin artifact viewer
/// (`/artifacts/{id}`) derived from a server-stamped artifact id, never
/// arbitrary model text.
fn artifact_browser_url(artifact_id: &str) -> String {
    let base =
        std::env::var("IRONHERMES_WEB_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());
    let base = base.trim_end_matches('/');
    format!("{base}/artifacts/{artifact_id}")
}

/// Plain (unstyled) text for a `[📎 ...]` attachment chip — the single
/// source of truth shared by `attachment_chip_line` (styling),
/// `transcript_total_line_count`, and `rebuild_chip_hit_test` (wrap math),
/// so the three never drift from each other.
fn attachment_chip_plain(chip: &SentAttachmentChip) -> String {
    format!("[📎 {} {}]", chip.filename, human_size(chip.size_bytes))
}

/// Plain (unstyled) text for a `[▤ ...]` artifact-link chip — see
/// `attachment_chip_plain`'s doc comment for why this is factored out.
fn artifact_chip_plain(artifact: &ironhermes_tools::chat_capture::CapturedArtifact) -> String {
    format!("[▤ {}]", artifact.title)
}

/// Styled transcript line for a `[📎 ...]` attachment chip. `Color::DarkGray`
/// per the UI-SPEC TUI color mapping — matches the existing `System` role's
/// dim treatment; attachments are metadata, not conversational content.
fn attachment_chip_line(chip: &SentAttachmentChip) -> Line<'static> {
    Line::from(Span::styled(
        attachment_chip_plain(chip),
        Style::default().fg(Color::DarkGray),
    ))
}

/// Styled transcript line for a `[▤ ...]` artifact-link chip. `Color::Cyan`
/// per the UI-SPEC TUI color mapping — the TUI's one "actionable link"
/// color, reused from the existing `User` role.
fn artifact_chip_line(
    artifact: &ironhermes_tools::chat_capture::CapturedArtifact,
) -> Line<'static> {
    Line::from(Span::styled(
        artifact_chip_plain(artifact),
        Style::default().fg(Color::Cyan),
    ))
}

// ── Phase 36.6.4 Plan 05: image chip + overlay (D-12/D-13, TUI-IMG-01) ──────

/// One image chip's rendered identity — the FULL (untruncated) label plus
/// the `MediaRef` needed to open the viewer overlay. Flat + append-only,
/// mirroring `SentAttachmentChip`/`captured_artifacts`.
#[derive(Debug, Clone)]
pub struct ImageChip {
    pub label: String,
    pub source: MediaRef,
    /// Phase 36.6.4 Plan 12 (G-09 closure): the chip's chronological
    /// ordering key. Stamped at creation time — `App::commit_assistant_buffer`
    /// stamps `history.len()` AFTER pushing the assistant message the chip
    /// came from; `commands::handle_image_slash` stamps `app.history.len()`
    /// at the moment `/image <path>` succeeds (that command pushes nothing
    /// into `history` itself, so "now" is exactly "after everything said so
    /// far").
    pub history_anchor: usize,
}

/// Maximum bytes a referenced image file may be before ANY decode is
/// attempted (T-36.6.4-IMG-01, this plan's `must_haves.prohibitions`
/// entry). Checked at BOTH trigger points — `/image <path>`'s synchronous
/// `check_image_path_bounded` (this file) — AND re-asserted at overlay
/// open by `decode_image_protocol` (Task 2), since a `<MEDIA:>` path can
/// grow between chip creation and the operator clicking it.
pub(crate) const IMAGE_FILE_SIZE_CAP_BYTES: u64 = 25 * 1024 * 1024; // 25 MiB

/// Bounded-read check for a LOCAL image path — the `/image <path>`
/// trigger-time gate. Reads only filesystem METADATA, never file bytes.
/// `Ok(())` when the path exists, is a regular file, and is under the cap;
/// `Err(reason)` otherwise (never attempts to open/decode the file here —
/// decode is Task 2's overlay-open concern).
pub(crate) fn check_image_path_bounded(path: &std::path::Path) -> Result<(), String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a regular file".to_string());
    }
    if meta.len() > IMAGE_FILE_SIZE_CAP_BYTES {
        return Err(format!(
            "file is {} bytes, exceeds the {IMAGE_FILE_SIZE_CAP_BYTES} byte limit",
            meta.len()
        ));
    }
    Ok(())
}

/// Label for `/image <path>` — the file's basename (UI-SPEC §5: "the label
/// is the file basename for `/image <path>`"). Returns the FULL untruncated
/// label; `image_chip_plain` truncates at render time.
pub(crate) fn image_chip_label_for_path(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "image".to_string())
}

/// Label for a `<MEDIA:>`-extracted `MediaSource` — the file basename for a
/// `Path` source; for a `Url` source (or a path with no filename component)
/// a short, human-distinguishable "AI image {id}" fallback (UI-SPEC §5:
/// "a short identifier fragment plus a generic prefix for a tag with no
/// human-readable filename"). Returns the FULL untruncated label.
fn image_chip_label_for_source(source: &MediaSource) -> String {
    match source {
        MediaSource::Path(path) => image_chip_label_for_path(path),
        MediaSource::Url(url) => {
            let tail = url
                .rsplit('/')
                .next()
                .filter(|s| !s.is_empty() && s.contains('.'));
            match tail {
                Some(name) => name.to_string(),
                None => {
                    use std::hash::{Hash, Hasher};
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    url.hash(&mut hasher);
                    format!("AI image {:08x}", hasher.finish() as u32)
                }
            }
        }
    }
}

/// Truncate `s` at `max_cells` DISPLAY cells (`unicode-width`, not chars or
/// bytes), appending a trailing `"…"` when truncated. Char-safe and
/// wide-glyph-safe — mirrors `transcript_rendered_plain_rows`'s
/// consumed-width walking discipline, applied to a single label string.
fn truncate_display_cells(s: &str, max_cells: usize) -> String {
    if UnicodeWidthStr::width(s) <= max_cells {
        return s.to_string();
    }
    let budget = max_cells.saturating_sub(1); // reserve 1 cell for "…"
    let mut out = String::new();
    let mut width = 0usize;
    for ch in s.chars() {
        let w = ch.width().unwrap_or(0);
        if width + w > budget {
            break;
        }
        width += w;
        out.push(ch);
    }
    out.push('…');
    out
}

/// Plain (unstyled) text for a `[🖼 ...]` image chip — see
/// `attachment_chip_plain`'s doc comment for why this is factored out.
/// Label truncates at 40 DISPLAY CELLS with a trailing `"…"` (UI-SPEC §5).
fn image_chip_plain(chip: &ImageChip) -> String {
    format!("[🖼 {}]", truncate_display_cells(&chip.label, 40))
}

/// Styled transcript line for a `[🖼 ...]` image chip. `Color::Cyan` — the
/// SAME "this opens something" hue the artifact chip already uses (images
/// are artifact-like: click to open the viewer overlay).
fn image_chip_line(chip: &ImageChip) -> Line<'static> {
    Line::from(Span::styled(
        image_chip_plain(chip),
        Style::default().fg(Color::Cyan),
    ))
}

/// Decode/protocol-build state for the open image overlay (Task 2).
/// `Protocol` is wrapped in an `Arc` so cloning this enum out of the
/// `Mutex` guard (required since the guard can't outlive the render call)
/// is cheap regardless of the underlying protocol payload's own size (a
/// Sixel/Kitty encoding can be large).
#[derive(Clone)]
pub enum ImageDecodeState {
    Decoding,
    Ready(Arc<ratatui_image::protocol::Protocol>),
    Failed(String),
}

/// Decode + protocol-build for one image source (Task 2, D-13). Run
/// entirely off the render thread inside a `spawn_blocking` closure
/// (T-36.6.4-IMG-01: decode must never wedge the render loop). Re-asserts
/// the bounded-read cap — Task 1's `check_image_path_bounded` only covers
/// the synchronous `/image` trigger path, so a `<MEDIA:>` path that grew
/// between chip creation and overlay open still fails cleanly here rather
/// than reading an unbounded file. `MediaSource::Url` is not fetched by
/// this build — returns an honest failure rather than a silent no-op or a
/// live network read at click time (see SUMMARY for the scope note).
pub(crate) fn decode_image_protocol(
    source: &MediaRef,
    picker: &ratatui_image::picker::Picker,
    target: Rect,
) -> Result<ratatui_image::protocol::Protocol, String> {
    let path = match &source.source {
        MediaSource::Path(p) => p,
        MediaSource::Url(_) => {
            return Err("remote image URLs are not supported by the TUI viewer yet".to_string());
        }
    };
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a regular file".to_string());
    }
    if meta.len() > IMAGE_FILE_SIZE_CAP_BYTES {
        return Err(format!(
            "file is {} bytes, exceeds the {IMAGE_FILE_SIZE_CAP_BYTES} byte limit",
            meta.len()
        ));
    }
    let dyn_img = image::ImageReader::open(path)
        .map_err(|e| e.to_string())?
        .decode()
        .map_err(|e| e.to_string())?;
    let size = ratatui::layout::Size::new(target.width.max(1), target.height.max(1));
    picker
        .clone()
        .new_protocol(dyn_img, size, ratatui_image::Resize::Fit(None))
        .map_err(|e| e.to_string())
}

/// Pure hit-test lookup: the `ChipAction` (if any) whose rect contains
/// `(column, row)`. Factored out of `App::handle_mouse` so the dispatch
/// logic is unit-testable without invoking the real OS browser launcher
/// (`App::opener`) — `handle_mouse_chip_tests` exercises this directly plus
/// a full `handle_mouse` round trip through a swapped-in no-op `opener`.
fn chip_action_at(hits: &[(Rect, ChipAction)], column: u16, row: u16) -> Option<ChipAction> {
    hits.iter()
        .find(|(rect, _)| {
            column >= rect.x
                && column < rect.x + rect.width
                && row >= rect.y
                && row < rect.y + rect.height
        })
        .map(|(_, action)| action.clone())
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
    use ironhermes_agent::{AgentRuntime, AnyClient};
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
    // Phase 28.1-05: AgentRuntime::for_tests() builds a minimal runtime backed
    // by a localhost:0 client so the test fixture doesn't need a live endpoint.
    let test_runtime = Arc::new(AgentRuntime::for_tests());

    AppDeps {
        agent_runtime: test_runtime,
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
        // Phase 36.17.3 (D-03 / D-06 amended): test queue + paused toggle.
        queue: Arc::new(ironhermes_gateway::session_queue::SessionQueue::new())
            as Arc<dyn MessageQueue<SessionKey>>,
        queue_paused: Arc::new(AtomicBool::new(false)),
        skin: Arc::new(std::sync::RwLock::new("default".to_string())),
        // Phase 25.2 Plan 15 follow-up: tests don't exercise the toolset slash UI
        toolset_session: None,
        // Phase 25.3 D-W-2 / D-T-3: tests don't exercise the workspace or trajectory writer
        workspace: None,
        trajectory_writer: None,
        // Phase 25.3-13 CR-04: tests don't exercise the seeded system message
        system_message: None,
        // Phase 21.8.2: no skill registry in tests
        skill_registry: None,
        // Phase 21.8.2 Plan 03: default skills config + empty overlays buffer
        skills_config: ironhermes_core::config::SkillsConfig::default(),
        pending_skill_overlays: Vec::new(),
        // Phase 36.3.12 Plan 10 (WR-01): fresh empty store at a throwaway test path —
        // no test in this module exercises session-tier approval persistence.
        approvals_store: Arc::new(ironhermes_core::ApprovalsStore::with_path(
            std::env::temp_dir().join(format!(
                "tui_rata_test_approvals_{}.json",
                std::process::id()
            )),
        )),
        // Phase 36.6.4 Plan 05: deterministic halfblocks constructor — no
        // stdio query performed (the crate's own documented headless-test
        // pattern). Never `from_query_stdio()` in a test context.
        picker: ratatui_image::picker::Picker::halfblocks(),
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
        // rustfmt may wrap the full type signature across several lines, so match
        // on the stable `pub browser_session:` field-declaration prefix instead of
        // the single-line type string (which only the AppDeps/App struct fields
        // carry — App::new's forwarding line and the test helper's `Arc::new(...)`
        // initializer are not `pub` and won't match).
        let field_decls = non_comment.matches("pub browser_session:").count();
        assert!(
            field_decls >= 2,
            "Phase 25.1 GAP-8 (plan 25.1-19): both AppDeps and App MUST declare the browser_session field; got {} declaration(s) in non-comment source",
            field_decls
        );
        // ...and the verified BrowserSession type must back those declarations.
        let type_refs = non_comment
            .matches("ironhermes_tools::browser_session::BrowserSession")
            .count();
        assert!(
            type_refs >= 2,
            "Phase 25.1 GAP-8 (plan 25.1-19): browser_session field MUST use the ironhermes_tools::browser_session::BrowserSession type in both structs; got {} reference(s)",
            type_refs
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

    // — word_wrapped_line_count ──────────────────────────────────────────────

    #[test]
    fn wrapped_empty_is_one() {
        assert_eq!(word_wrapped_line_count("", 10), 1);
    }

    #[test]
    fn wrapped_fits_one_row() {
        assert_eq!(word_wrapped_line_count("hello", 10), 1);
    }

    #[test]
    fn wrapped_exactly_one_row() {
        assert_eq!(word_wrapped_line_count("helloworld", 10), 1);
    }

    #[test]
    fn wrapped_overflows_one_row() {
        // "helloworld!" is a single word of width 11 at width=10 → character-wraps to 2 rows
        assert_eq!(word_wrapped_line_count("helloworld!", 10), 2);
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
        app.handle_stream_event(StreamEvent::Finished { total_tokens: 0 });
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

    // Phase 36.17.8 B1.4 regression: `/voice tts` must flip the RUNTIME auto_tts
    // flag and `/voice status` must reflect it. Previously the core handler
    // returned a canned "toggled" string and status read stale on-disk config, so
    // the toggle appeared to do nothing.
    #[tokio::test]
    async fn voice_tts_toggles_runtime_state_and_status_reflects_it() {
        use crate::tui_rata::commands::{SlashOutcome, dispatch_slash};
        use std::sync::atomic::Ordering;

        let mut app = App::new_test_empty();

        // Baseline: auto_tts off, and status reports it.
        assert!(!app.voice.auto_tts.load(Ordering::Relaxed));
        let SlashOutcome::Handled(s) = dispatch_slash(&mut app, "/voice status").await else {
            panic!("/voice status must be Handled");
        };
        assert!(s.contains("auto_tts: false"), "status before toggle: {s}");

        // The fix: /voice tts flips the runtime flag.
        let SlashOutcome::Handled(s) = dispatch_slash(&mut app, "/voice tts").await else {
            panic!("/voice tts must be Handled");
        };
        assert!(s.contains("on"), "toggle message should report on: {s}");
        assert!(
            app.voice.auto_tts.load(Ordering::Relaxed),
            "auto_tts must be true after /voice tts"
        );

        // ...and status now reflects the new state.
        let SlashOutcome::Handled(s) = dispatch_slash(&mut app, "/voice status").await else {
            panic!("/voice status must be Handled");
        };
        assert!(s.contains("auto_tts: true"), "status after toggle: {s}");
    }

    /// Test 4 (Phase 36.6.4 Plan 05 Task 1, D-12/D-13, TUI-IMG-01): a
    /// nonexistent path produces one System-role transcript line and zero
    /// chips.
    #[tokio::test]
    async fn image_slash_command_missing_path_renders_system_line_and_no_chip() {
        use crate::tui_rata::commands::{SlashOutcome, dispatch_slash};

        let mut app = App::new_test_empty();
        let outcome =
            dispatch_slash(&mut app, "/image /nonexistent/path/does-not-exist.png").await;
        match outcome {
            SlashOutcome::Handled(text) => {
                assert!(
                    text.starts_with("Could not load image:"),
                    "expected the operator-facing load error copy, got: {text}"
                );
            }
            other => panic!("expected SlashOutcome::Handled, got: {other:?}"),
        }
        assert!(
            app.image_chips.is_empty(),
            "a missing path must render NO chip"
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
        use crossterm::event::{MouseEvent, MouseEventKind};
        let mut app = App::new_test_empty();
        let scroll_before = app.transcript_scroll();
        let auto_before = app.auto_follow;
        let outside = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 200,
            row: 200,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };
        app.handle_mouse(outside, area(80, 24));
        assert_eq!(app.transcript_scroll(), scroll_before);
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

    // — apply_slash_outcome scroll re-engagement (Phase 21.8.2 Plan 04, G-01) ──

    #[test]
    fn apply_slash_outcome_skills_reload_re_engages_auto_follow() {
        // Phase 21.8.2 Plan 04 G-01 closure (RED):
        // SkillsReload must call scroll_to_bottom() so the diff line is
        // visible on the same render tick. Reference: submit() at app.rs:718.
        let mut app = App::new_test_empty();
        // Simulate user having scrolled up before issuing /skills reload.
        app.scroll_up(5);
        assert!(
            !app.auto_follow,
            "precondition: scroll_up disabled auto_follow"
        );
        let prev_len = app.history.len();

        let outcome = crate::tui_rata::commands::SlashOutcome::SkillsReload(
            "Skills reloaded: 1 added (test-skill), 0 removed. Total: 5 skills.".to_string(),
        );
        app.apply_slash_outcome(outcome);

        // Bug fix assertion: auto_follow must be re-engaged so the next
        // render tick clamps transcript_scroll to bottom (via reconcile_scroll).
        assert!(
            app.auto_follow,
            "SkillsReload arm of apply_slash_outcome must call scroll_to_bottom() to re-engage auto_follow",
        );
        assert_eq!(
            app.transcript_scroll(), 0,
            "SkillsReload arm must call scroll_to_bottom() which zeros transcript_scroll (symmetric with scroll_to_top)",
        );
        // Sanity: the diff line was actually appended as a System message.
        assert_eq!(
            app.history.len(),
            prev_len + 1,
            "SkillsReload arm must push exactly one message",
        );
        assert_eq!(
            app.history.last().expect("last history entry").role,
            Role::System,
            "SkillsReload arm must push the diff as a Role::System message",
        );
    }

    #[test]
    fn apply_slash_outcome_skill_activated_re_engages_auto_follow() {
        // Phase 41.1 Plan 02 (D-01) — behavior UPDATED from the old activate-only
        // path: SkillActivated now fires a real turn. It must still call
        // scroll_to_bottom() so the run-turn is visible on the same render tick.
        let mut app = App::new_test_empty();
        app.scroll_up(5);
        assert!(
            !app.auto_follow,
            "precondition: scroll_up disabled auto_follow"
        );

        let outcome = crate::tui_rata::commands::SlashOutcome::SkillActivated {
            name: "test-skill".to_string(),
            body: "test body".to_string(),
            args: None,
        };
        app.apply_slash_outcome(outcome);

        assert!(
            app.auto_follow,
            "SkillActivated arm of apply_slash_outcome must call scroll_to_bottom() to re-engage auto_follow",
        );
        assert_eq!(
            app.transcript_scroll(), 0,
            "SkillActivated arm must call scroll_to_bottom() which zeros transcript_scroll (symmetric with scroll_to_top)",
        );
        // The body was activated via the existing overlay path.
        assert_eq!(
            app.pending_skill_overlays.len(),
            1,
            "SkillActivated arm must continue to buffer (name, body) into pending_skill_overlays",
        );
        // A real turn was fired (the event loop spawns iff pending_tx is Some).
        assert!(
            app.pending_tx.is_some(),
            "SkillActivated arm must now fire a real agent turn (pending_tx set)",
        );
    }

    /// LEAD TRACER (D-01): a BARE `/<skill>` fires a real agent turn through the
    /// SAME machinery `submit()` uses. Asserts the actual fields
    /// `event_loop.rs:828-833` consumes to spawn a turn (`pending_tx` +
    /// `cancel_child`) — not just that an activation string was produced — and
    /// that the model-facing turn content is the bare run-now instruction.
    #[test]
    fn skill_activated_bare_invoke_spawns_turn() {
        let mut app = App::new_test_empty();
        assert!(
            app.pending_tx.is_none(),
            "precondition: no turn in flight before activation"
        );

        let outcome = crate::tui_rata::commands::SlashOutcome::SkillActivated {
            name: "gsd-config".to_string(),
            body: "SKILL BODY CONTENT".to_string(),
            args: None,
        };
        app.apply_slash_outcome(outcome);

        // The turn IS submitted: event_loop spawns iff BOTH are set.
        assert!(
            app.pending_tx.is_some(),
            "bare invoke must set pending_tx so event_loop.rs:828-833 spawns the turn"
        );
        assert!(
            app.cancel_child.is_some(),
            "bare invoke must set cancel_child (the second half of the event_loop spawn gate)"
        );
        // The spawned turn's content is the bare run-now instruction, pushed as
        // Role::User so spawn_turn's history snapshot carries it to the model.
        let last_user = app
            .history
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .expect("a Role::User turn message must exist");
        assert_eq!(
            render_message_body(last_user),
            "Run the gsd-config skill now: carry out its instructions immediately.",
            "the bare-invoke turn content must be the run-now instruction (D-02)"
        );
    }

    /// LEAD TRACER (D-02): an ARGUED `/<skill> <text>` uses the trailing text
    /// verbatim as the run-turn content instead of the bare instruction.
    #[test]
    fn skill_activated_with_args_uses_trailing_text() {
        let mut app = App::new_test_empty();

        let outcome = crate::tui_rata::commands::SlashOutcome::SkillActivated {
            name: "gsd-config".to_string(),
            body: "SKILL BODY CONTENT".to_string(),
            args: Some("show me the config".to_string()),
        };
        app.apply_slash_outcome(outcome);

        assert!(
            app.pending_tx.is_some(),
            "argued invoke must also fire a real turn"
        );
        let last_user = app
            .history
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .expect("a Role::User turn message must exist");
        assert_eq!(
            render_message_body(last_user),
            "show me the config",
            "the argued form's turn content must be the trailing text verbatim, not the bare instruction"
        );
    }

    /// Task 2: the run-turn meta-chip renderer — bare copy, argued copy, and
    /// the 40-char truncation with an inner ellipsis (UI-SPEC §C / E3).
    #[test]
    fn run_turn_meta_chip_copy_and_truncation() {
        assert_eq!(
            super::run_turn_meta_chip("gsd-config", None),
            "▶ Ran skill /gsd-config"
        );
        assert_eq!(
            super::run_turn_meta_chip("gsd-config", Some("show me the config")),
            "▶ Ran skill /gsd-config · \"show me the config\"",
            "short argued text renders in full, no ellipsis"
        );
        let long = "x".repeat(50);
        assert_eq!(
            super::run_turn_meta_chip("gsd-config", Some(&long)),
            format!("▶ Ran skill /gsd-config · \"{}…\"", "x".repeat(40)),
            "argued text longer than 40 chars truncates to 40 + inner ellipsis"
        );
    }

    /// Flatten `transcript_text()` into one string for substring assertions.
    fn flatten_transcript(app: &App) -> String {
        app.transcript_text()
            .lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.to_string())
            .collect::<Vec<_>>()
            .join("")
    }

    /// Task 2 (key_link / UI-SPEC §C): a BARE invoke shows ONLY the DIM meta
    /// chip in the transcript — the synthetic run-now trigger is model-facing
    /// content that must never render as a user bubble.
    #[test]
    fn bare_invoke_shows_meta_chip_and_hides_synthetic_trigger() {
        let mut app = App::new_test_empty();
        let outcome = crate::tui_rata::commands::SlashOutcome::SkillActivated {
            name: "gsd-config".to_string(),
            body: "SKILL BODY".to_string(),
            args: None,
        };
        app.apply_slash_outcome(outcome);

        // The trigger IS in history (model-facing turn content) …
        assert!(
            app.history.iter().any(|m| m.role == Role::User
                && render_message_body(m).contains("Run the gsd-config skill now")),
            "the synthetic trigger must be present in history as the turn content"
        );
        // … but it is NOT rendered into the transcript.
        let flat = flatten_transcript(&app);
        assert!(
            flat.contains("▶ Ran skill /gsd-config"),
            "the DIM run-turn meta chip must be visible; transcript:\n{flat}"
        );
        assert!(
            !flat.contains("Run the gsd-config skill now"),
            "the bare synthetic trigger must NOT render as a user bubble; transcript:\n{flat}"
        );
    }

    /// Task 2: the ARGUED form's trailing text IS the user's own words and
    /// renders normally, in addition to the meta chip that precedes the turn.
    #[test]
    fn argued_invoke_shows_meta_chip_and_renders_user_words() {
        let mut app = App::new_test_empty();
        let outcome = crate::tui_rata::commands::SlashOutcome::SkillActivated {
            name: "gsd-config".to_string(),
            body: "SKILL BODY".to_string(),
            args: Some("show me the config".to_string()),
        };
        app.apply_slash_outcome(outcome);

        let flat = flatten_transcript(&app);
        assert!(
            flat.contains("▶ Ran skill /gsd-config · \"show me the config\""),
            "argued meta chip must show the trailing text; transcript:\n{flat}"
        );
        assert!(
            flat.contains("show me the config"),
            "the argued form's own words must render as a normal message; transcript:\n{flat}"
        );
        assert!(
            app.skill_run_hidden_indices.is_empty(),
            "argued invoke must NOT hide anything (the user's words render)"
        );
    }

    // — Phase 36.6.3 Plan 03 (TUI-INPUT-02, D-06): open-wiring ───────────────

    #[test]
    fn apply_slash_outcome_open_model_picker_sets_overlay() {
        let mut app = App::new_test_empty();
        app.model_picker_filter = "stale".to_string();
        app.model_picker_selected = 3;

        app.apply_slash_outcome(crate::tui_rata::commands::SlashOutcome::OpenModelPicker);

        assert_eq!(
            app.active_overlay,
            Some(OverlayKind::ModelPicker {
                step: PickerStep::Provider,
                selected_provider: None,
            }),
            "OpenModelPicker must open the two-step picker at step 1"
        );
        assert_eq!(app.model_picker_filter, "", "filter must reset on open");
        assert_eq!(app.model_picker_selected, 0, "selection must reset on open");
    }

    #[test]
    fn apply_slash_outcome_open_provider_picker_sets_overlay() {
        let mut app = App::new_test_empty();

        app.apply_slash_outcome(crate::tui_rata::commands::SlashOutcome::OpenProviderPicker);

        assert_eq!(
            app.active_overlay,
            Some(OverlayKind::ModelPicker {
                step: PickerStep::ProviderOnly,
                selected_provider: None,
            }),
            "OpenProviderPicker must open the single-step picker"
        );
    }

    /// D-04: bare `/model` typed exactly in the palette + Enter opens the
    /// picker via the EXISTING insert-or-submit path (no palette_enter code
    /// change was needed — `/model` is the sole exact match at
    /// `palette_selected == 0` in registry order, ahead of its
    /// prefix-sibling `/models`).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn palette_enter_opens_model_picker() {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
        let mut app = App::new_test_empty();
        app.textarea.insert_str("/model");
        assert!(crate::tui_rata::palette::palette_query(&app).is_some());

        app.handle_key(KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        });

        assert_eq!(
            app.active_overlay,
            Some(OverlayKind::ModelPicker {
                step: PickerStep::Provider,
                selected_provider: None,
            }),
            "Enter on an exactly-typed bare /model must open the picker, not submit as agent text"
        );
    }

    /// D-04: bare `/provider` typed exactly in the palette + Enter opens the
    /// single-step picker via the same existing path (`/provider` precedes
    /// its subcommand siblings `/provider list` etc. in registry order).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn palette_enter_opens_provider_picker() {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
        let mut app = App::new_test_empty();
        app.textarea.insert_str("/provider");
        assert!(crate::tui_rata::palette::palette_query(&app).is_some());

        app.handle_key(KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        });

        assert_eq!(
            app.active_overlay,
            Some(OverlayKind::ModelPicker {
                step: PickerStep::ProviderOnly,
                selected_provider: None,
            }),
            "Enter on an exactly-typed bare /provider must open the single-step picker"
        );
    }

    // — Phase 36.6.3 Plan 03: picker key routing + apply + D-11 persist ─────

    static MODEL_PICKER_ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> =
        std::sync::OnceLock::new();

    /// SAFETY: mirrors `tui_attach_at_path::lock()` — `std::env::set_var`
    /// mutates process-global state; guarded here as defense-in-depth for
    /// this module's IRONHERMES_HOME-touching tests (which also require
    /// `--test-threads=1`, per project convention on the cross-module env
    /// race).
    fn model_picker_env_lock() -> std::sync::MutexGuard<'static, ()> {
        MODEL_PICKER_ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap()
    }

    fn enter_key() -> crossterm::event::KeyEvent {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
        KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    fn esc_key() -> crossterm::event::KeyEvent {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
        KeyEvent {
            code: KeyCode::Esc,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    /// D-08 Esc semantics: filter-clear-first at step 2, then step-back to
    /// step 1 (resetting `selected_provider`), then close — "back one
    /// level, or close if already at the top". The single-step `/provider`
    /// flow always closes on Esc regardless of filter state.
    #[test]
    fn model_picker_esc_semantics() {
        let mut app = App::new_test_empty();
        app.active_overlay = Some(OverlayKind::ModelPicker {
            step: PickerStep::Model,
            selected_provider: Some("openrouter".to_string()),
        });
        app.model_picker_filter = "abc".to_string();
        app.model_picker_selected = 2;

        // 1. Step 2, non-empty filter -> clear filter first, STAY at step 2
        //    with the SAME selected_provider.
        app.handle_key(esc_key());
        assert_eq!(app.model_picker_filter, "", "filter must clear first");
        assert_eq!(app.model_picker_selected, 0, "selection must reset");
        assert_eq!(
            app.active_overlay,
            Some(OverlayKind::ModelPicker {
                step: PickerStep::Model,
                selected_provider: Some("openrouter".to_string()),
            }),
            "filter-clear-first must NOT step back yet"
        );

        // 2. Step 2, empty filter -> step BACK to step 1, resetting
        //    selected_provider to None.
        app.handle_key(esc_key());
        assert_eq!(
            app.active_overlay,
            Some(OverlayKind::ModelPicker {
                step: PickerStep::Provider,
                selected_provider: None,
            }),
            "empty-filter Esc at step 2 must step back to step 1"
        );

        // 3. Step 1 -> close entirely.
        app.handle_key(esc_key());
        assert_eq!(app.active_overlay, None, "Esc at step 1 must close");

        // 4. Single-step /provider closes on Esc regardless of filter state.
        let mut app2 = App::new_test_empty();
        app2.active_overlay = Some(OverlayKind::ModelPicker {
            step: PickerStep::ProviderOnly,
            selected_provider: None,
        });
        app2.model_picker_filter = "xyz".to_string();
        app2.handle_key(esc_key());
        assert_eq!(
            app2.active_overlay, None,
            "single-step /provider Esc must always close, even with a non-empty filter"
        );
    }

    /// D-06/D-07: selecting a model at step 2 hot-swaps the LIVE session via
    /// the same `ironhermes_agent::build_client` path `/model <name>` uses —
    /// `app.client` reflects the applied model after Enter.
    #[test]
    fn model_picker_apply_hotswaps_session() {
        // Isolate IRONHERMES_HOME: a successful apply also persists to
        // config.yaml (D-11) — never let this test touch the real
        // developer home directory.
        let _guard = model_picker_env_lock();
        let home = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home.path());
        }

        let mut app = App::new_test_empty();
        let expected_model = app
            .resolver
            .resolve("anthropic")
            .expect("anthropic is a built-in provider")
            .default_model
            .clone();
        app.active_overlay = Some(OverlayKind::ModelPicker {
            step: PickerStep::Model,
            selected_provider: Some("anthropic".to_string()),
        });
        app.model_picker_selected = 0; // anthropic's sparse list: [default_model]

        app.handle_key(enter_key());

        assert!(
            app.active_overlay.is_none(),
            "a successful apply must close the overlay"
        );
        assert_eq!(
            app.client.model(),
            expected_model,
            "the live AnyClient must reflect the applied model (hot-swap)"
        );
    }

    /// D-11: selecting provider+model persists to `config.yaml` via
    /// `Config::load` -> mutate ONLY `model.provider`/`model.default` ->
    /// `Config::save` — round-trip-safe (an unrelated key in a DIFFERENT
    /// top-level section survives untouched, proving no fresh partial
    /// `Config` was constructed).
    #[test]
    fn model_picker_apply_persists_config_yaml() {
        let _guard = model_picker_env_lock();
        let home = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home.path());
        }

        // Seed a multi-key config.yaml: initial provider/model, a SECOND
        // selectable model for openrouter, and a distinguishing unrelated
        // key in a different top-level section (agent.system_message).
        let mut seed = ironhermes_core::Config::default();
        seed.model.provider = "openrouter".to_string();
        seed.model.default = "initial-model".to_string();
        seed.agent.system_message = "SENTINEL-UNRELATED-KEY".to_string();
        let mut overrides = std::collections::HashMap::new();
        overrides.insert(
            "chosen-model".to_string(),
            ironhermes_core::config_extras::ProviderModelConfig::default(),
        );
        seed.providers.insert(
            "openrouter".to_string(),
            ironhermes_core::config::ProviderConfig {
                models: overrides,
                ..Default::default()
            },
        );
        seed.save().expect("seed config.yaml must save");

        let mut app = App::new_test_empty();
        app.resolver = ironhermes_core::ProviderResolver::build(&seed)
            .expect("resolver must build from the seeded config");
        app.active_overlay = Some(OverlayKind::ModelPicker {
            step: PickerStep::Model,
            selected_provider: Some("openrouter".to_string()),
        });
        app.model_picker_selected = 0; // sorted override keys first: "chosen-model"

        app.handle_key(enter_key());

        assert!(app.active_overlay.is_none(), "a successful apply must close the overlay");

        let reloaded = ironhermes_core::Config::load().expect("config.yaml must reload");
        assert_eq!(
            reloaded.model.provider, "openrouter",
            "D-11: the active provider key must persist"
        );
        assert_eq!(
            reloaded.model.default, "chosen-model",
            "D-11: the applied model must persist (round-trip write+re-read)"
        );
        assert_eq!(
            reloaded.agent.system_message, "SENTINEL-UNRELATED-KEY",
            "D-11: an unrelated key in a DIFFERENT section must round-trip untouched \
             (proves Config::load -> mutate -> save, never a fresh partial Config)"
        );
    }

    /// A `build_client` failure (e.g. an unknown provider) surfaces
    /// VERBATIM through the SAME `SlashOutcome::Error` -> System-role
    /// transcript path `dispatch_slash` errors already use (no new copy),
    /// and the overlay STAYS OPEN — a wrong pick has a solution path, not a
    /// dead-end. `build_client`'s `Err` path never reaches D-11 persist, so
    /// this test needs no IRONHERMES_HOME isolation.
    #[test]
    fn model_picker_apply_error_surfaces() {
        let mut app = App::new_test_empty();
        app.active_overlay = Some(OverlayKind::ModelPicker {
            step: PickerStep::Model,
            selected_provider: Some("totally-bogus-provider-xyz".to_string()),
        });
        let prev_len = app.history.len();

        app.apply_model_picker_selection(
            PickerStep::Model,
            "totally-bogus-provider-xyz".to_string(),
            "some-model".to_string(),
        );

        assert_eq!(
            app.active_overlay,
            Some(OverlayKind::ModelPicker {
                step: PickerStep::Model,
                selected_provider: Some("totally-bogus-provider-xyz".to_string()),
            }),
            "apply failure must NOT close the overlay"
        );
        assert_eq!(
            app.history.len(),
            prev_len + 1,
            "the error must surface as exactly one transcript entry"
        );
        let last = app.history.last().expect("last history entry");
        assert_eq!(
            last.role,
            Role::System,
            "errors surface as System-role, same as dispatch_slash's error path"
        );
        let body = render_message_body(last);
        assert!(
            body.contains("Unknown provider") && body.contains("totally-bogus-provider-xyz"),
            "error must surface verbatim (no new copy invented), got: {body}"
        );
    }

    // — Phase 21.8.3 RED tests — line-count parity, snap-on-Finished, submit helper, End key ──

    #[test]
    fn transcript_line_count_accounts_for_role_prefix() {
        // D-06: "You: " prefix (5 chars) on line 0 reduces effective width.
        // With width=80 and a body of 80 'x' chars:
        //   current (buggy): effective_width=80 → ceil(80/80)=1
        //   fixed:           effective_width=80-5=75 → ceil(80/75)=2
        let body: &'static str = Box::leak("x".repeat(80).into_boxed_str());
        let app = App::new_test_with_messages(vec![("user", body)]);
        assert_eq!(
            app.transcript_line_count(80),
            2,
            "80-char user message at width=80 must count 2 wrapped rows (prefix reduces effective width to 75)"
        );
    }

    #[test]
    fn system_message_counted_in_line_count() {
        // D-07: System messages are NOW rendered (role_style returns Some(DarkGray)
        // post-22.4-17). transcript_line_count must include them with "System: "
        // prefix (8 chars). With width=80 and body of 80 'y' chars:
        //   current (buggy): counts without prefix → ceil(80/80)=1
        //   fixed:           effective_width=80-8=72 → ceil(80/72)=2
        let body: &'static str = Box::leak("y".repeat(80).into_boxed_str());
        let app = App::new_test_with_messages(vec![("system", body)]);
        assert_eq!(
            app.transcript_line_count(80),
            2,
            "80-char system message at width=80 must count 2 wrapped rows (System: prefix reduces effective width to 72)"
        );
    }

    #[test]
    fn stream_finished_snaps_to_bottom() {
        // D-08: StreamEvent::Finished must call scroll_to_bottom() when auto_follow is true.
        // Pre-fix: Finished arm only commits buffer and clears pending_rx;
        //          transcript_scroll stays at whatever it was → test fails.
        let mut app = App::new_test_empty();
        app.auto_follow = false;
        app.set_transcript_scroll(5);
        app.handle_stream_event(StreamEvent::Started);
        app.handle_stream_event(StreamEvent::Delta("some text".to_string()));
        // Simulate user re-engaging auto_follow before stream finishes
        app.auto_follow = true;
        app.handle_stream_event(StreamEvent::Finished { total_tokens: 0 });
        assert_eq!(
            app.transcript_scroll(), 0,
            "Finished with auto_follow=true must call scroll_to_bottom() which zeros transcript_scroll"
        );
        assert!(
            app.auto_follow,
            "auto_follow must remain true after Finished snap"
        );
    }

    #[test]
    fn submit_calls_scroll_to_bottom() {
        // D-09: submit() must call scroll_to_bottom() instead of bare auto_follow=true.
        // Pre-fix: submit() only sets auto_follow=true at line 742;
        //          transcript_scroll stays at 7 → test fails.
        let mut app = App::new_test_empty();
        app.set_transcript_scroll(7);
        app.auto_follow = false;
        app.textarea.insert_str("hello world");
        app.submit();
        assert_eq!(
            app.transcript_scroll(), 0,
            "submit() must call scroll_to_bottom() which zeros transcript_scroll"
        );
        assert!(
            app.auto_follow,
            "submit() must re-engage auto_follow via scroll_to_bottom()"
        );
    }

    #[test]
    fn end_key_calls_scroll_to_bottom() {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        // D-10: End key (plain) must call scroll_to_bottom().
        // Pre-fix: End falls through to textarea catch-all → transcript_scroll stays at 9.
        let mut app = App::new_test_empty();
        app.set_transcript_scroll(9);
        app.auto_follow = false;
        let end_key = KeyEvent {
            code: KeyCode::End,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        app.handle_key(end_key);
        assert_eq!(
            app.transcript_scroll(), 0,
            "End key must call scroll_to_bottom() which zeros transcript_scroll"
        );
        assert!(
            app.auto_follow,
            "End key must re-engage auto_follow via scroll_to_bottom()"
        );

        // Also verify Ctrl+End (same arm via wildcard modifiers)
        app.set_transcript_scroll(9);
        app.auto_follow = false;
        let ctrl_end = KeyEvent {
            code: KeyCode::End,
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        app.handle_key(ctrl_end);
        assert_eq!(
            app.transcript_scroll(), 0,
            "Ctrl+End must also call scroll_to_bottom()"
        );
        assert!(
            app.auto_follow,
            "Ctrl+End must re-engage auto_follow via scroll_to_bottom()"
        );
    }

    #[test]
    fn auto_follow_tracks_buffer_growth() {
        // D-13c: With auto_follow=true, reconcile_scroll must snap transcript_scroll
        // to the actual rendered bottom when assistant_buffer has grown.
        // Pre-fix: transcript_line_count under-counts (ignores prefix) so max < real
        //          total → reconcile_scroll clamps short of the actual bottom.
        let mut app = App::new_test_empty();
        let a = area(80, 24);
        // Empty history: reconcile_scroll → transcript_scroll == 0
        app.reconcile_scroll(a);
        assert_eq!(app.transcript_scroll(), 0);

        // Push a large assistant_buffer (200 lines)
        app.assistant_buffer = Some("x\n".repeat(200));
        app.auto_follow = true;
        app.reconcile_scroll(a);

        let max = app.transcript_max_scroll(a);
        assert_eq!(
            app.transcript_scroll(), max,
            "reconcile_scroll with auto_follow=true must snap transcript_scroll to transcript_max_scroll (post-fix the max is correct)"
        );
    }

    // ── Phase 36.6.4 Plan 07 (G-01/G-02/G-06 closure) — measured-height tests ──

    #[test]
    fn hidden_shell_messages_contribute_zero_rows_to_measured_height() {
        let width = 80usize;

        // Three shell outcomes applied the NORMAL way: each pushes a hidden
        // `shell_history_hidden_indices` System copy into `history` (D-11)
        // alongside the `shell_runs` entry the custom-styled renderer draws.
        let mut with_hidden = App::new_test_empty();
        for i in 0..3 {
            with_hidden.apply_shell_outcome(shell_bang::ShellOutcome {
                command: format!("echo {i}"),
                stdout: format!("shell output line {i}"),
                stderr: String::new(),
                result: shell_bang::ShellResult::Exited(0),
                truncation: None,
            });
        }
        assert_eq!(
            with_hidden.shell_history_hidden_indices.len(),
            3,
            "sanity: three shell outcomes must record three hidden indices"
        );
        let measured_with_hidden = with_hidden.transcript_total_line_count(width);

        // Ground truth: the SAME three `shell_runs`, with no hidden `history`
        // copy at all — if the hidden copy contributes any row, the two
        // measured heights diverge by exactly that amount.
        let mut without_hidden_history = App::new_test_empty();
        for i in 0..3 {
            let outcome = shell_bang::ShellOutcome {
                command: format!("echo {i}"),
                stdout: format!("shell output line {i}"),
                stderr: String::new(),
                result: shell_bang::ShellResult::Exited(0),
                truncation: None,
            };
            without_hidden_history.shell_runs.push(shell_bang::ShellRun {
                command: outcome.command.clone(),
                state: shell_bang::ShellRunState::Done(outcome),
                history_anchor: 0,
            });
        }
        let measured_without_hidden_history =
            without_hidden_history.transcript_total_line_count(width);

        assert_eq!(
            measured_with_hidden, measured_without_hidden_history,
            "the hidden shell-run System copy in `history` (D-11) must contribute ZERO rows \
             to the measured height — pre-fix this diverged by the hidden messages' own \
             wrapped height (the operator's real session measured a divergence of 46 rows)"
        );
    }

    #[test]
    fn image_chips_are_counted_in_measured_height() {
        let width = 80usize;
        let mut app = App::new_test_empty();
        let before = app.transcript_total_line_count(width);

        for i in 0..3 {
            let anchor = app.history.len();
            app.image_chips.push(ImageChip {
                label: format!("img{i}.png"),
                source: MediaRef {
                    source: MediaSource::Path(PathBuf::from(format!("/tmp/img{i}.png"))),
                    kind: MediaKind::Photo,
                    original_tag_text: format!("<MEDIA: /tmp/img{i}.png>"),
                },
                history_anchor: anchor,
            });
        }
        let after = app.transcript_total_line_count(width);

        assert_eq!(
            after,
            before + 3,
            "three single-row image chips must grow the measured height by exactly three \
             rows — pre-fix image chips grew the height by zero (G-02)"
        );
    }

    #[test]
    fn boundary_width_line_with_trailing_space_measures_one_row() {
        let width = 20usize;
        let mut app = App::new_test_empty();
        // "You: " (5 display cells) + 14 'a's + a trailing space == 20 cells
        // exactly — a line whose display width equals `width` and ends in
        // trailing whitespace. This is the G-06 boundary shape: with the OLD
        // hand-rolled `word_wrapped_line_count` estimate driving the height
        // (rather than a real `Paragraph` render), a boundary-exact line with
        // trailing whitespace is exactly the class of input the estimator was
        // never cross-checked against ratatui for (the existing
        // `word_wrap_tests` regression suite pins several other boundary
        // shapes — see `word_wrapped_line_count_short_empty_exact` — but not
        // this one). This test locks the row count to the REAL measured
        // rendering going forward, so any future estimator reintroduced on
        // this path is caught immediately.
        app.history.push(user_message("aaaaaaaaaaaaaa ".to_string()));

        assert_eq!(
            app.transcript_total_line_count(width),
            1,
            "a line whose display width equals `width` and ends in trailing whitespace must \
             measure exactly one row, not two"
        );
    }

    // ── Phase 36.6.4 Plan 07 Task 2 (one content enumeration) tests ──────────

    /// Populate `app` with a chosen subset of the five transcript groups.
    /// Bit 0=History, 1=AttachmentChips, 2=ArtifactChips, 3=ImageChips,
    /// 4=ShellRuns — used to walk all 32 subsets in
    /// `unit_row_offsets_end_equals_measured_height_for_every_group`.
    fn populate_groups(app: &mut App, mask: u8) {
        if mask & 0b00001 != 0 {
            app.history.push(user_message("hello there".to_string()));
        }
        if mask & 0b00010 != 0 {
            app.sent_attachment_chips.push(SentAttachmentChip {
                filename: "notes.txt".to_string(),
                size_bytes: 1024,
                history_anchor: app.history.len(),
            });
        }
        if mask & 0b00100 != 0 {
            app.captured_artifacts
                .lock()
                .unwrap()
                .push(transcript_chip_tests_support::artifact_for("artifact-1", "Report"));
        }
        if mask & 0b01000 != 0 {
            let anchor = app.history.len();
            app.image_chips.push(ImageChip {
                label: "pic.png".to_string(),
                source: MediaRef {
                    source: MediaSource::Path(PathBuf::from("/tmp/pic.png")),
                    kind: MediaKind::Photo,
                    original_tag_text: "<MEDIA: /tmp/pic.png>".to_string(),
                },
                history_anchor: anchor,
            });
        }
        if mask & 0b10000 != 0 {
            app.apply_shell_outcome(shell_bang::ShellOutcome {
                command: "echo hi".to_string(),
                stdout: "hi".to_string(),
                stderr: String::new(),
                result: shell_bang::ShellResult::Exited(0),
                truncation: None,
            });
        }
    }

    /// The lockstep fence: for every one of the 32 subsets of the five
    /// content groups, the last unit's `end_row` (from
    /// `transcript_unit_row_offsets`, derived via a sentinel-interleaved
    /// render) must equal `transcript_total_line_count` (derived via a
    /// SEPARATE, flat sentinel render — Task 1's `transcript_rendered_
    /// plain_rows`). This is NOT vacuous: the two values come from two
    /// independent renders.
    #[test]
    fn unit_row_offsets_end_equals_measured_height_for_every_group() {
        let width = 40usize;
        for mask in 0u8..32 {
            let mut app = App::new_test_empty();
            populate_groups(&mut app, mask);

            let offsets = app.transcript_unit_row_offsets(width);
            let measured = app.transcript_total_line_count(width);

            match offsets.last() {
                Some(&(_, end)) => assert_eq!(
                    end, measured,
                    "mask={mask:#07b}: last unit's end_row must equal the measured height"
                ),
                None => assert_eq!(
                    measured, 0,
                    "mask={mask:#07b}: no units means the measured height must be 0 too"
                ),
            }
        }
    }

    /// A group appended to `transcript_render_units` without a matching
    /// update to `transcript_unit_row_offsets` (or vice versa) must fail
    /// here: the number of distinct `TranscriptGroup` values present in the
    /// units list must equal the number covered by the offsets Vec (which
    /// is zipped 1:1 with the units).
    #[test]
    fn a_new_group_appears_in_both_render_text_and_hit_test_offsets() {
        let width = 40usize;
        let mut app = App::new_test_empty();
        populate_groups(&mut app, 0b11111); // all five groups present

        let units = app.transcript_render_units();
        let offsets = app.transcript_unit_row_offsets(width);

        let groups_in_units: std::collections::HashSet<TranscriptGroup> =
            units.iter().map(|u| u.group).collect();
        let groups_covered_by_offsets: std::collections::HashSet<TranscriptGroup> = units
            .iter()
            .zip(offsets.iter())
            .map(|(u, _)| u.group)
            .collect();

        assert_eq!(
            groups_in_units.len(),
            groups_covered_by_offsets.len(),
            "every group present in transcript_render_units() must also be covered by \
             transcript_unit_row_offsets() — a group appended to one without the other \
             must fail here"
        );
        assert_eq!(groups_in_units.len(), 5, "test setup: all five groups must be present");
        assert_eq!(
            offsets.len(),
            units.len(),
            "transcript_unit_row_offsets() must return one offset per unit"
        );
    }

    /// With both an artifact chip and an image chip populated, plus enough
    /// history to force a non-zero scroll offset, each visible chip's
    /// hit-test rect `y` must equal the border offset plus the unit's
    /// measured start row, minus the scroll offset — mirroring exactly what
    /// `rebuild_chip_hit_test` computes internally.
    #[test]
    fn chip_hit_test_rects_match_measured_rows_for_image_and_artifact_chips() {
        let mut app = App::new_test_empty();
        for i in 0..30 {
            app.history.push(user_message(format!("line {i}")));
        }
        app.captured_artifacts
            .lock()
            .unwrap()
            .push(transcript_chip_tests_support::artifact_for("artifact-1", "Report"));
        app.image_chips.push(ImageChip {
            label: "pic.png".to_string(),
            source: MediaRef {
                source: MediaSource::Path(PathBuf::from("/tmp/pic.png")),
                kind: MediaKind::Photo,
                original_tag_text: "<MEDIA: /tmp/pic.png>".to_string(),
            },
            history_anchor: app.history.len(),
        });

        let a = area(40, 10); // small viewport forces a non-zero scroll offset
        let inner_width = inner_transcript_width(a);
        app.scroll_to_bottom();
        app.reconcile_scroll(a);
        let scroll = app.transcript_scroll() as usize;
        assert!(scroll > 0, "test setup: expected a non-zero scroll offset");

        let measurement = app.transcript_measurement(inner_width);
        app.rebuild_chip_hit_test(a, &measurement);

        let offsets = app.transcript_unit_row_offsets(inner_width);
        let units = app.transcript_render_units();
        let hits = app.chip_hit_test.lock().unwrap();
        assert!(!hits.is_empty(), "test setup: expected at least one visible chip hit rect");

        let mut checked = 0usize;
        for (unit, (start, _end)) in units.iter().zip(offsets.iter()) {
            let Some(action) = &unit.action else {
                continue;
            };
            let Some((rect, _)) = hits.iter().find(|(_, hit_action)| hit_action == action) else {
                continue; // scrolled out of the visible viewport this frame
            };
            let expected_y =
                a.y.saturating_add(1).saturating_add((*start as u16).saturating_sub(scroll as u16));
            assert_eq!(
                rect.y, expected_y,
                "hit rect's y must equal border offset + measured start row - scroll offset \
                 for action {action:?}"
            );
            checked += 1;
        }
        assert!(checked > 0, "test setup: expected at least one chip to be checked");
    }

    // ── Phase 36.6.4 Plan 10 Task 1 (G-08 closure): one linear pass ──────────

    /// For each of widths 10/20/40/78/195 and a corpus containing a
    /// wide-glyph line, a line whose display width equals the wrap width and
    /// ends in a space, an empty line, a multi-word paragraph and one
    /// unbroken 120-character token, the measurement's `rows` must be
    /// byte-identical to a PLAIN (non-interleaved) render of the same
    /// content — built here in the test, never in `app.rs`, so no second
    /// production derivation is introduced.
    #[test]
    fn interleaved_pass_rows_match_a_plain_render_of_the_same_content() {
        for &width in &[10usize, 20, 40, 78, 195] {
            let mut app = App::new_test_empty();
            app.history.push(user_message("emoji line 🖼🖼🖼 with wide glyphs".to_string()));
            let boundary_line = format!("{} ", "x".repeat(width.saturating_sub(1)));
            app.history.push(user_message(boundary_line));
            app.history.push(user_message(String::new()));
            app.history.push(user_message(
                "multi word paragraph with several separate tokens in it".to_string(),
            ));
            app.history.push(user_message("y".repeat(120)));

            let measurement = app.transcript_measurement(width);

            // Reference: a plain (non-interleaved) render of the SAME units,
            // sized with a generous (never tight) headroom so it can never
            // itself under-measure — then truncated to the measurement's own
            // reported height, which is what actually distinguishes "the
            // measurement is right" from "both derivations under-count the
            // same way".
            let units = app.transcript_render_units();
            let text = Text::from(units.iter().map(|u| u.line.clone()).collect::<Vec<_>>());
            let unwrapped_line_count = text.lines.len();
            let total_display_width: usize = text
                .lines
                .iter()
                .map(|line| {
                    line.spans
                        .iter()
                        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
                        .sum::<usize>()
                })
                .sum();
            let cap = total_display_width
                .div_ceil(width)
                .saturating_mul(4)
                .saturating_add(unwrapped_line_count)
                .saturating_add(128)
                .min(u16::MAX as usize) as u16;
            let scratch_area = Rect::new(0, 0, width as u16, cap);
            let mut buf = ratatui::buffer::Buffer::empty(scratch_area);
            let paragraph = ratatui::widgets::Paragraph::new(text)
                .wrap(ratatui::widgets::Wrap { trim: false });
            ratatui::widgets::Widget::render(paragraph, scratch_area, &mut buf);
            let mut reference_rows: Vec<String> = Vec::with_capacity(scratch_area.height as usize);
            for row in 0..scratch_area.height {
                let mut line = String::new();
                let mut col: u16 = 0;
                while col < scratch_area.width {
                    let symbol = buf
                        .cell((col, row))
                        .map(|c| c.symbol().to_string())
                        .unwrap_or_default();
                    let w = (UnicodeWidthStr::width(symbol.as_str()) as u16).max(1);
                    line.push_str(&symbol);
                    col = col.saturating_add(w);
                }
                reference_rows.push(line);
            }
            reference_rows.truncate(measurement.height());

            assert_eq!(
                measurement.rows, reference_rows,
                "width={width}: interleaved-pass rows must be byte-identical to a plain \
                 render of the same content"
            );
        }
    }

    /// Given N units, the pass must yield exactly N `(start_row, end_row)`
    /// pairs in flat (sentinel-free) row space: `start_0 == 0`, each
    /// `end_i == start_{i+1}`, and the last `end` equals the measured
    /// height — checked over all 32 subsets of the five content groups
    /// (`populate_groups`), so the contiguity fence holds regardless of
    /// which groups are present.
    #[test]
    fn unit_offsets_are_contiguous_and_end_at_the_measured_height() {
        let width = 40usize;
        for mask in 0u8..32 {
            let mut app = App::new_test_empty();
            populate_groups(&mut app, mask);

            let measurement = app.transcript_measurement(width);
            let offsets = &measurement.offsets;
            let unit_count = measurement.units.len();

            assert_eq!(
                offsets.len(),
                unit_count,
                "mask={mask:#07b}: expected one offset per unit"
            );
            if offsets.is_empty() {
                continue;
            }
            assert_eq!(
                offsets[0].0, 0,
                "mask={mask:#07b}: the first unit's start_row must be 0"
            );
            for i in 0..offsets.len() - 1 {
                assert_eq!(
                    offsets[i].1,
                    offsets[i + 1].0,
                    "mask={mask:#07b}: unit {i}'s end_row must equal unit {}'s start_row",
                    i + 1
                );
            }
            assert_eq!(
                offsets.last().unwrap().1,
                measurement.height(),
                "mask={mask:#07b}: the last unit's end_row must equal the measured height"
            );
        }
    }

    /// The quadratic fence: `row_lookups` must grow LINEARLY with unit
    /// count — doubling the units at most ~doubles the counter. Fails on
    /// the pre-fix tree, where the per-sentinel backward scan made this
    /// ratio ~4 (O(units x rows)).
    #[test]
    fn row_lookups_grow_linearly_with_unit_count() {
        let width = 195usize;

        let mut app_600 = App::new_test_empty();
        for i in 0..600 {
            app_600.history.push(user_message(format!("line {i} of a realistic transcript")));
        }
        reset_transcript_measure_stats();
        let _ = app_600.transcript_measurement(width);
        let lookups_600 = transcript_measure_stats().row_lookups;

        let mut app_1200 = App::new_test_empty();
        for i in 0..1200 {
            app_1200.history.push(user_message(format!("line {i} of a realistic transcript")));
        }
        reset_transcript_measure_stats();
        let _ = app_1200.transcript_measurement(width);
        let lookups_1200 = transcript_measure_stats().row_lookups;

        assert!(
            (lookups_1200 as f64) <= 2.3 * (lookups_600 as f64),
            "row_lookups must grow linearly: 600 units -> {lookups_600}, \
             1200 units -> {lookups_1200} (ratio {:.2}, must be <= 2.3)",
            lookups_1200 as f64 / lookups_600.max(1) as f64
        );
    }

    // ── Phase 36.6.4 Plan 10 Task 2 (G-08 closure) — the memo ──────────────

    #[test]
    fn measurement_is_reused_when_content_is_unchanged() {
        let mut app = App::new_test_empty();
        for i in 0..50 {
            app.history.push(user_message(format!("line {i} of a realistic transcript")));
        }
        let width = 80usize;

        reset_transcript_measure_stats();
        let first = app.transcript_measurement(width);
        let second = app.transcript_measurement(width);

        let stats = transcript_measure_stats();
        assert_eq!(
            stats.renders, 1,
            "two calls with no intervening content mutation must perform exactly one render, got {stats:?}"
        );
        assert_eq!(
            stats.cache_hits, 1,
            "the second call must be served from the memo, got {stats:?}"
        );
        assert_eq!(
            first.rows, second.rows,
            "the cached measurement must be identical to the one that produced it"
        );
    }

    /// Named-mutation type for `every_content_mutation_invalidates_the_measurement`
    /// — factored out of the `Vec` literal per `clippy::type_complexity`.
    type NamedMutation = (&'static str, Box<dyn Fn(&mut App)>);

    #[test]
    fn every_content_mutation_invalidates_the_measurement() {
        let width = 80usize;
        let mutations: Vec<NamedMutation> = vec![
            (
                "push a history message",
                Box::new(|app: &mut App| {
                    app.history.push(user_message("a fresh history line".to_string()));
                }),
            ),
            (
                "append to assistant_buffer",
                Box::new(|app: &mut App| {
                    app.assistant_buffer
                        .get_or_insert_with(String::new)
                        .push_str("a streamed token");
                }),
            ),
            (
                "push an image chip",
                Box::new(|app: &mut App| {
                    let history_anchor = app.history.len();
                    app.image_chips.push(ImageChip {
                        label: "img.png".to_string(),
                        source: MediaRef {
                            source: MediaSource::Path(PathBuf::from("/tmp/img.png")),
                            kind: MediaKind::Photo,
                            original_tag_text: "<MEDIA: /tmp/img.png>".to_string(),
                        },
                        history_anchor,
                    });
                }),
            ),
            (
                "push a shell run (with its hidden history copy)",
                Box::new(|app: &mut App| {
                    app.apply_shell_outcome(shell_bang::ShellOutcome {
                        command: "echo hi".to_string(),
                        stdout: "hi".to_string(),
                        stderr: String::new(),
                        result: shell_bang::ShellResult::Exited(0),
                        truncation: None,
                    });
                }),
            ),
            (
                "push an attachment chip",
                Box::new(|app: &mut App| {
                    let history_anchor = app.history.len();
                    app.sent_attachment_chips.push(SentAttachmentChip {
                        filename: "notes.txt".to_string(),
                        size_bytes: 512,
                        history_anchor,
                    });
                }),
            ),
        ];

        let mut app = App::new_test_empty();
        for i in 0..5 {
            app.history.push(user_message(format!("seed line {i}")));
        }

        for (name, apply) in mutations {
            let before = app.transcript_measurement(width);
            apply(&mut app);
            reset_transcript_measure_stats();
            let after = app.transcript_measurement(width);
            let stats = transcript_measure_stats();
            assert_eq!(
                stats.renders, 1,
                "mutation `{name}` must invalidate the memo and force exactly one render, got {stats:?}"
            );
            assert_ne!(
                before.rows.len(),
                after.rows.len(),
                "mutation `{name}` adds rows — the measured height must change"
            );
        }
    }

    #[test]
    fn same_length_different_text_invalidates_the_measurement() {
        let width = 80usize;
        let mut app = App::new_test_empty();
        app.history.push(user_message("aaaaaaaaaa".to_string()));
        let before = app.transcript_measurement(width);

        // Replace the message body with a DIFFERENT string of the SAME byte
        // length — length alone must never be treated as the memo key.
        if let Some(last) = app.history.last_mut() {
            *last = user_message("bbbbbbbbbb".to_string());
        }

        reset_transcript_measure_stats();
        let after = app.transcript_measurement(width);
        let stats = transcript_measure_stats();
        assert_eq!(
            stats.renders, 1,
            "a same-length different-text mutation must still invalidate the memo, got {stats:?}"
        );
        assert_ne!(
            before.rows, after.rows,
            "different text must produce different measured rows even at identical byte length"
        );
    }

    #[test]
    fn width_change_invalidates_the_measurement() {
        let mut app = App::new_test_empty();
        for i in 0..20 {
            app.history
                .push(user_message(format!("line {i} of a transcript that wraps across widths")));
        }

        reset_transcript_measure_stats();
        let _a1 = app.transcript_measurement(80);
        let _b = app.transcript_measurement(120);
        let _a2 = app.transcript_measurement(80);

        let stats = transcript_measure_stats();
        assert_eq!(
            stats.renders, 3,
            "widths A, B, A must render three times — A's rows must never be served at width B, got {stats:?}"
        );
    }

    /// Anti-staleness fence: for a randomised sequence of mutations, the
    /// memoised measurement must be byte-identical to a fresh
    /// `measure_transcript_uncached` over the SAME content and width after
    /// every step. Fixed-seed LCG — no `rand` dependency, per the plan.
    #[test]
    fn cached_rows_are_byte_identical_to_a_fresh_measurement() {
        let width = 80usize;
        let mut app = App::new_test_empty();
        for i in 0..5 {
            app.history.push(user_message(format!("seed line {i}")));
        }

        // 32-bit LCG (Numerical Recipes constants), fixed seed for reproducibility.
        let mut seed: u32 = 0x1234_5678;
        let mut next_u32 = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed
        };

        for step in 0..50 {
            match next_u32() % 5 {
                0 => app.history.push(user_message(format!("mutation line {step}"))),
                1 => {
                    app.assistant_buffer
                        .get_or_insert_with(String::new)
                        .push_str(&format!(" token{step}"));
                }
                2 => {
                    let history_anchor = app.history.len();
                    app.image_chips.push(ImageChip {
                        label: format!("img{step}.png"),
                        source: MediaRef {
                            source: MediaSource::Path(PathBuf::from(format!("/tmp/img{step}.png"))),
                            kind: MediaKind::Photo,
                            original_tag_text: format!("<MEDIA: /tmp/img{step}.png>"),
                        },
                        history_anchor,
                    });
                }
                3 => app.apply_shell_outcome(shell_bang::ShellOutcome {
                    command: format!("echo {step}"),
                    stdout: format!("shell output {step}"),
                    stderr: String::new(),
                    result: shell_bang::ShellResult::Exited(0),
                    truncation: None,
                }),
                _ => {
                    let history_anchor = app.history.len();
                    app.sent_attachment_chips.push(SentAttachmentChip {
                        filename: format!("file{step}.txt"),
                        size_bytes: (step as u64 + 1) * 128,
                        history_anchor,
                    });
                }
            }

            let memoised = app.transcript_measurement(width);
            let units = app.transcript_render_units();
            let fresh = app.measure_transcript_uncached(units, width);

            assert_eq!(
                memoised.rows, fresh.rows,
                "step {step}: memoised rows must equal a fresh measurement of the same content"
            );
            assert_eq!(
                memoised.offsets, fresh.offsets,
                "step {step}: memoised offsets must equal a fresh measurement of the same content"
            );
        }
    }

    // ── Phase 36.6.4 Plan 12 (G-09 closure): chronological transcript order ──

    /// The operator's Round 5 report, minimized: `!ls`, then a question, and
    /// the reply must render BELOW the shell block, not above it. This is
    /// the MANDATORY G-05/Nyquist RED observation — run against the pre-fix
    /// tree BEFORE Task 1's production changes land, its failure output
    /// quoted verbatim in the SUMMARY.
    #[test]
    fn shell_block_renders_above_a_later_assistant_reply() {
        let mut app = App::new_test_empty();
        app.history.push(user_message("first question".to_string()));
        app.apply_shell_outcome(shell_bang::ShellOutcome {
            command: "ls".to_string(),
            stdout: "SHELL_OUTPUT_TOKEN_7f3a".to_string(),
            stderr: String::new(),
            result: shell_bang::ShellResult::Exited(0),
            truncation: None,
        });
        app.history.push(assistant_message("ASSISTANT_REPLY_TOKEN_9c1d".to_string()));

        let units = app.transcript_render_units();

        let last_shell_idx = units
            .iter()
            .enumerate()
            .filter(|(_, u)| u.group == TranscriptGroup::ShellRuns)
            .map(|(i, _)| i)
            .next_back()
            .expect("test setup: a shell run must produce at least one unit");
        let first_reply_idx = units
            .iter()
            .enumerate()
            .find(|(_, u)| {
                u.group == TranscriptGroup::History
                    && u.line
                        .spans
                        .iter()
                        .any(|s| s.content.contains("ASSISTANT_REPLY_TOKEN_9c1d"))
            })
            .map(|(i, _)| i)
            .expect("test setup: the assistant reply must produce a History unit");

        assert!(
            last_shell_idx < first_reply_idx,
            "the `!` block (unit {last_shell_idx}) must render ABOVE the later assistant \
             reply (unit {first_reply_idx}) — on the pre-fix tree the shell block is \
             appended after ALL history, so this assertion fails there"
        );
    }

    /// The generalised order fence the existing presence-only keeper test
    /// (`a_new_group_appears_in_both_render_text_and_hit_test_offsets`)
    /// cannot provide: every unit's `history_anchor` must be non-decreasing
    /// across the whole enumeration.
    #[test]
    fn transcript_units_are_emitted_in_nondecreasing_anchor_order() {
        let mut app = App::new_test_empty();
        populate_groups(&mut app, 0b11111);

        let units = app.transcript_render_units();
        assert!(
            !units.is_empty(),
            "test setup: populate_groups(0b11111) must produce at least one unit"
        );

        let mut prev_anchor = 0usize;
        for (i, unit) in units.iter().enumerate() {
            assert!(
                unit.history_anchor >= prev_anchor,
                "unit {i} (group {:?}) has history_anchor {} which is LESS than the \
                 previous unit's anchor {prev_anchor} — anchors must be non-decreasing",
                unit.group,
                unit.history_anchor
            );
            prev_anchor = unit.history_anchor;
        }
    }

    /// Task 1 acceptance: `history_lines_for`/`streaming_lines` must
    /// reproduce `transcript_text()`'s exact pre-refactor role-prefixed
    /// output for a transcript with no chips and no shell runs.
    #[test]
    fn transcript_text_is_byte_identical_after_the_history_lines_for_extraction() {
        let mut app = App::new_test_empty();
        app.history
            .push(user_message("first line\nsecond line".to_string()));
        app.history.push(assistant_message("a reply".to_string()));
        app.assistant_buffer = Some("streaming reply".to_string());

        let text = app.transcript_text();
        let rendered: Vec<String> = text
            .lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.to_string()).collect::<String>())
            .collect();

        assert_eq!(
            rendered,
            vec![
                "You: first line".to_string(),
                "second line".to_string(),
                "Hermes: a reply".to_string(),
                "Hermes: streaming reply".to_string(),
            ],
            "history_lines_for/streaming_lines must preserve transcript_text()'s exact prior \
             role-prefixed output for a transcript with no chips and no shell runs"
        );
    }

    // ── Phase 36.6.4 Plan 12 Task 2 (G-09 closure): real anchors for image ──
    // ── and attachment chips ─────────────────────────────────────────────

    /// D-G09-2: a `<MEDIA:>` tag extracted from a completed assistant turn
    /// must render its chip AFTER that turn's own History units, and BEFORE
    /// a later user message.
    #[test]
    fn image_chip_from_media_tag_renders_after_its_own_assistant_turn() {
        let mut app = App::new_test_empty();
        app.assistant_buffer =
            Some("Here: <MEDIA: /tmp/x.png> ASSISTANT_TOKEN_2b91".to_string());
        app.commit_assistant_buffer();
        app.history.push(user_message("LATER_QUESTION_TOKEN_4d17".to_string()));

        let units = app.transcript_render_units();

        let assistant_idx = units
            .iter()
            .position(|u| {
                u.group == TranscriptGroup::History
                    && u.line.spans.iter().any(|s| s.content.contains("ASSISTANT_TOKEN_2b91"))
            })
            .expect("test setup: the assistant turn must produce a History unit");
        let chip_idx = units
            .iter()
            .position(|u| u.group == TranscriptGroup::ImageChips)
            .expect("test setup: the media-tag image chip must be present");
        let later_msg_idx = units
            .iter()
            .position(|u| {
                u.group == TranscriptGroup::History
                    && u.line.spans.iter().any(|s| s.content.contains("LATER_QUESTION_TOKEN_4d17"))
            })
            .expect("test setup: the later user message must produce a History unit");

        assert!(
            assistant_idx < chip_idx,
            "the image chip (unit {chip_idx}) must render AFTER its own assistant turn \
             (unit {assistant_idx})"
        );
        assert!(
            chip_idx < later_msg_idx,
            "the image chip (unit {chip_idx}) must render BEFORE the later user message \
             (unit {later_msg_idx})"
        );
    }

    /// D-G09-2: `/image <path>` pushes nothing into `history` itself, so a
    /// chip it creates must render after ALL history rows present at the
    /// moment the command succeeds.
    #[tokio::test]
    async fn image_chip_from_slash_command_renders_after_the_last_history_row() {
        use crate::tui_rata::commands::{SlashOutcome, dispatch_slash};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pic.png");
        std::fs::write(&path, b"not a real png, just bytes for the size check").unwrap();

        let mut app = App::new_test_empty();
        for i in 0..4 {
            app.history.push(user_message(format!("line {i}")));
        }
        let history_len_before = app.history.len();

        let outcome = dispatch_slash(&mut app, &format!("/image {}", path.display())).await;
        assert!(
            matches!(outcome, SlashOutcome::Silent),
            "test setup: a valid image path must yield SlashOutcome::Silent, got: {outcome:?}"
        );
        assert_eq!(
            app.image_chips.len(),
            1,
            "test setup: /image must yield exactly one chip"
        );
        assert_eq!(
            app.image_chips[0].history_anchor, history_len_before,
            "the chip's anchor must equal the history length at the moment /image succeeded"
        );

        let units = app.transcript_render_units();
        let last_history_idx = units
            .iter()
            .enumerate()
            .filter(|(_, u)| u.group == TranscriptGroup::History)
            .map(|(i, _)| i)
            .next_back()
            .expect("test setup: history rows must produce units");
        let chip_idx = units
            .iter()
            .position(|u| u.group == TranscriptGroup::ImageChips)
            .expect("test setup: the image chip unit must be present");

        assert!(
            chip_idx > last_history_idx,
            "the image chip (unit {chip_idx}) must render AFTER the last history row \
             (unit {last_history_idx})"
        );
    }

    /// D-G09-3: an attachment chip must render immediately after the user
    /// message it was sent with — not below every later turn (the same
    /// defect class as the shell-block/image-chip cases, closed by the same
    /// two-step stamp in `App::submit`).
    #[test]
    fn attachment_chip_renders_immediately_after_its_own_user_message() {
        // Deliberately does NOT redirect `IRONHERMES_HOME` (unlike
        // `app_with_store`/`model_picker_apply_persists_config_yaml`) — that
        // env var is process-global and this crate's existing suite ALREADY
        // exhibits cross-test collisions on it independent of this plan
        // (observed: skipping this test alone still fails 5 unrelated
        // `tui_attach_at_path` tests under default parallelism). Adding a
        // 3rd/4th mutator to that shared hazard only widens the window, so
        // this test instead gives `app` a UNIQUE `session_id` — distinct
        // from `test_deps()`'s crate-wide shared `"test-session"` — so its
        // attachment file lives in its own directory no matter what
        // `IRONHERMES_HOME` currently resolves to.
        let mut app = App::new_test_empty();
        app.session_id = format!("plan12-attachment-order-test-{}", std::process::id());
        app.history.push(user_message("earlier turn".to_string()));

        let attachments_dir = ironhermes_core::session_attachments_dir(&app.session_id);
        std::fs::create_dir_all(attachments_dir.join("att1")).unwrap();
        std::fs::write(attachments_dir.join("att1").join("notes.txt"), b"attachment body").unwrap();
        app.pending_attachments.push(PendingAttachment {
            filename: "notes.txt".to_string(),
            content_type: Some("text/plain".to_string()),
            stored_rel_path: "att1/notes.txt".to_string(),
        });

        app.textarea.insert_str("PLEASE_REVIEW_TOKEN_8a3f");
        app.submit();

        assert_eq!(
            app.sent_attachment_chips.len(),
            1,
            "test setup: submit() must have created exactly one attachment chip"
        );

        app.history.push(assistant_message("REVIEW_REPLY_TOKEN_9c22".to_string()));

        let units = app.transcript_render_units();

        let user_msg_idx = units
            .iter()
            .position(|u| {
                u.group == TranscriptGroup::History
                    && u.line.spans.iter().any(|s| s.content.contains("PLEASE_REVIEW_TOKEN_8a3f"))
            })
            .expect("test setup: the submitted user message must produce a History unit");
        let chip_idx = units
            .iter()
            .position(|u| u.group == TranscriptGroup::AttachmentChips)
            .expect("test setup: the attachment chip must be present");
        let reply_idx = units
            .iter()
            .position(|u| {
                u.group == TranscriptGroup::History
                    && u.line.spans.iter().any(|s| s.content.contains("REVIEW_REPLY_TOKEN_9c22"))
            })
            .expect("test setup: the later assistant reply must produce a History unit");

        assert!(
            user_msg_idx < chip_idx,
            "the attachment chip (unit {chip_idx}) must render AFTER its own user message \
             (unit {user_msg_idx})"
        );
        assert!(
            chip_idx < reply_idx,
            "the attachment chip (unit {chip_idx}) must render BEFORE the later assistant \
             reply (unit {reply_idx})"
        );

        // Best-effort cleanup of this test's uniquely-named directory —
        // not required for correctness (a leaked dir is harmless), just
        // tidiness.
        if let Some(session_dir) = attachments_dir.parent() {
            let _ = std::fs::remove_dir_all(session_dir);
        }
    }

    /// D-G09-4: `captured_artifacts` has no obtainable creation-time anchor
    /// (see `transcript_render_units`'s doc comment) — it gets a
    /// RENDER-TIME anchor of `history.len()` instead, which must place it
    /// after all settled history and before the in-flight streaming reply.
    #[test]
    fn artifact_chips_render_after_settled_history_and_before_the_streaming_buffer() {
        let mut app = App::new_test_empty();
        app.history.push(user_message("a question".to_string()));
        app.history
            .push(assistant_message("SETTLED_REPLY_TOKEN_5e60".to_string()));
        app.captured_artifacts
            .lock()
            .unwrap()
            .push(transcript_chip_tests_support::artifact_for("artifact-1", "Report"));
        app.assistant_buffer = Some("STREAMING_TOKEN_7f18".to_string());

        let units = app.transcript_render_units();

        let last_settled_idx = units
            .iter()
            .enumerate()
            .filter(|(_, u)| {
                u.group == TranscriptGroup::History
                    && u.line.spans.iter().any(|s| s.content.contains("SETTLED_REPLY_TOKEN_5e60"))
            })
            .map(|(i, _)| i)
            .next_back()
            .expect("test setup: the settled assistant reply must produce a History unit");
        let artifact_idx = units
            .iter()
            .position(|u| u.group == TranscriptGroup::ArtifactChips)
            .expect("test setup: the artifact chip must be present");
        let streaming_idx = units
            .iter()
            .position(|u| {
                u.group == TranscriptGroup::History
                    && u.line.spans.iter().any(|s| s.content.contains("STREAMING_TOKEN_7f18"))
            })
            .expect("test setup: the streaming buffer must produce a History unit");

        assert!(
            artifact_idx > last_settled_idx,
            "the artifact chip (unit {artifact_idx}) must render AFTER all settled history \
             (unit {last_settled_idx})"
        );
        assert!(
            artifact_idx < streaming_idx,
            "the artifact chip (unit {artifact_idx}) must render BEFORE the in-flight \
             streaming buffer (unit {streaming_idx})"
        );
    }

    /// T-36.6.4-P12-02: `/clear` (`SlashOutcome::ClearSession`) empties
    /// `history` but leaves `shell_runs`/`image_chips`/`sent_attachment_chips`
    /// populated with now-stale anchors exceeding the new (shrunk)
    /// `history.len()`. `transcript_render_units` must clamp rather than
    /// index, and must not panic.
    #[test]
    fn a_stale_anchor_past_history_len_sorts_last_and_does_not_panic() {
        let mut app = App::new_test_empty();
        for i in 0..5 {
            app.history.push(user_message(format!("line {i}")));
        }
        app.apply_shell_outcome(shell_bang::ShellOutcome {
            command: "echo hi".to_string(),
            stdout: "hi".to_string(),
            stderr: String::new(),
            result: shell_bang::ShellResult::Exited(0),
            truncation: None,
        });
        let image_anchor = app.history.len();
        app.image_chips.push(ImageChip {
            label: "pic.png".to_string(),
            source: MediaRef {
                source: MediaSource::Path(PathBuf::from("/tmp/pic.png")),
                kind: MediaKind::Photo,
                original_tag_text: "<MEDIA: /tmp/pic.png>".to_string(),
            },
            history_anchor: image_anchor,
        });
        assert!(
            app.shell_runs[0].history_anchor > 1,
            "test setup: the shell run's anchor must exceed the post-clear history length"
        );
        assert!(
            app.image_chips[0].history_anchor > 1,
            "test setup: the image chip's anchor must exceed the post-clear history length"
        );

        // Replicate `SlashOutcome::ClearSession`'s exact effect (app.rs,
        // `apply_slash_outcome`): `history` clears to ONE System
        // confirmation message; `shell_runs`/`image_chips` are left
        // untouched with now-stale anchors.
        app.history.clear();
        app.skill_run_hidden_indices.clear();
        app.shell_history_hidden_indices.clear();
        let mut system = ChatMessage::user("Session cleared.");
        system.role = Role::System;
        app.history.push(system);
        assert_eq!(
            app.history.len(),
            1,
            "test setup: history must be exactly one message after the simulated /clear"
        );

        // Must not panic — this call is the assertion.
        let units = app.transcript_render_units();

        let surviving_idx = units
            .iter()
            .position(|u| u.group == TranscriptGroup::History)
            .expect("test setup: the surviving System message must produce a History unit");
        let shell_idx = units
            .iter()
            .position(|u| u.group == TranscriptGroup::ShellRuns)
            .expect("the stale-anchored shell run must still be present");
        let image_idx = units
            .iter()
            .position(|u| u.group == TranscriptGroup::ImageChips)
            .expect("the stale-anchored image chip must still be present");

        assert!(
            shell_idx > surviving_idx,
            "the stale-anchored shell run (unit {shell_idx}) must sort AFTER the surviving \
             row (unit {surviving_idx})"
        );
        assert!(
            image_idx > surviving_idx,
            "the stale-anchored image chip (unit {image_idx}) must sort AFTER the surviving \
             row (unit {surviving_idx})"
        );
    }

    // ── Phase 36.6.4 Plan 12 Task 3 (G-09 closure): lock the four ───────────
    // ── enumeration consumers to the anchor-ordered emission ────────────────

    /// Consumer 1 — row offsets. `transcript_unit_row_offsets` must return
    /// exactly one non-overlapping, strictly-increasing offset per unit
    /// through an INTERLEAVED (not group-major) transcript, with the last
    /// unit's `end_row` equal to the measured height — generalising the
    /// existing group-major-only `unit_row_offsets_end_equals_measured_height_for_every_group`
    /// fence to the reordered enumeration.
    #[test]
    fn unit_row_offsets_track_units_through_an_interleaved_transcript() {
        let width = 40usize;
        let mut app = App::new_test_empty();
        app.history.push(user_message("first message".to_string()));
        app.apply_shell_outcome(shell_bang::ShellOutcome {
            command: "echo hi".to_string(),
            stdout: "hi".to_string(),
            stderr: String::new(),
            result: shell_bang::ShellResult::Exited(0),
            truncation: None,
        });
        let anchor = app.history.len();
        app.image_chips.push(ImageChip {
            label: "pic.png".to_string(),
            source: MediaRef {
                source: MediaSource::Path(PathBuf::from("/tmp/pic.png")),
                kind: MediaKind::Photo,
                original_tag_text: "<MEDIA: /tmp/pic.png>".to_string(),
            },
            history_anchor: anchor,
        });
        app.history.push(assistant_message("a later reply".to_string()));

        let units = app.transcript_render_units();
        let offsets = app.transcript_unit_row_offsets(width);
        let measured_height = app.transcript_measurement(width).height();

        assert_eq!(
            offsets.len(),
            units.len(),
            "transcript_unit_row_offsets must return exactly one offset per unit"
        );
        assert!(
            !offsets.is_empty(),
            "test setup: this interleaved transcript must produce at least one unit"
        );

        let mut prev_end = 0usize;
        for (i, &(start, end)) in offsets.iter().enumerate() {
            assert!(
                start >= prev_end,
                "offset {i}: start ({start}) must not overlap the previous unit's end \
                 ({prev_end})"
            );
            assert!(
                end > start,
                "offset {i}: end ({end}) must be strictly greater than start ({start})"
            );
            prev_end = end;
        }
        assert_eq!(
            offsets.last().unwrap().1,
            measured_height,
            "the last unit's end_row must equal the measured height, through an interleaved \
             (not group-major) enumeration"
        );
    }

    /// Consumer 2 — click geometry (silent failure mode, UI-SPEC E1/overflow).
    /// An image chip placed in the MIDDLE of a long transcript, once
    /// scrolled into view, must get a hit-test rect whose `y` tracks its
    /// OWN measured `start_row` — not the row it would have occupied under
    /// the old bottom-pinned group-major order.
    #[test]
    fn chip_hit_test_rect_follows_an_interleaved_chip_after_scroll() {
        let mut app = App::new_test_empty();
        for i in 0..20 {
            app.history.push(user_message(format!("line {i}")));
        }
        let mid_anchor = app.history.len();
        app.image_chips.push(ImageChip {
            label: "middle.png".to_string(),
            source: MediaRef {
                source: MediaSource::Path(PathBuf::from("/tmp/middle.png")),
                kind: MediaKind::Photo,
                original_tag_text: "<MEDIA: /tmp/middle.png>".to_string(),
            },
            history_anchor: mid_anchor,
        });
        for i in 20..40 {
            app.history.push(user_message(format!("line {i}")));
        }

        let a = area(40, 10); // small viewport forces the chip to require a scroll
        let inner_width = inner_transcript_width(a);
        app.scroll_down(15);
        let measurement = app.transcript_measurement(inner_width);
        app.rebuild_chip_hit_test(a, &measurement);

        let scroll = app.transcript_scroll() as usize;
        assert!(scroll > 0, "test setup: expected a non-zero scroll offset");

        let units = app.transcript_render_units();
        let offsets = app.transcript_unit_row_offsets(inner_width);
        let chip_unit_idx = units
            .iter()
            .position(|u| u.group == TranscriptGroup::ImageChips)
            .expect("test setup: the middle image chip must be present");
        let (start_row, _end_row) = offsets[chip_unit_idx];
        assert!(
            start_row >= scroll,
            "test setup: expected the middle chip's start_row ({start_row}) to be within or \
             after the scroll offset ({scroll}) so it is actually visible"
        );

        let hits = app.chip_hit_test_snapshot();
        let (rect, _) = hits
            .iter()
            .find(|(_, act)| {
                matches!(act, ChipAction::OpenImage { label, .. } if label == "middle.png")
            })
            .expect(
                "test setup: the middle chip must be visible in the hit-test map after \
                 scrolling",
            );

        let expected_y = a.y.saturating_add(1).saturating_add((start_row - scroll) as u16);
        assert_eq!(
            rect.y, expected_y,
            "the hit-test rect's y must track the interleaved chip's own measured start_row \
             after scroll, not a bottom-pinned position"
        );
    }

    /// Consumer 3 — link extraction. A bare URL inside a shell block's
    /// stdout, and another inside an assistant reply that arrives
    /// afterwards, must each be found by `hyperlink::extract_links` on the
    /// row that ACTUALLY displays it — the shell block's link on an
    /// earlier row than the later reply's, through the real `ui()` render
    /// entry point.
    #[test]
    fn link_rows_align_with_reordered_rows() {
        use crate::tui_rata::ui::ui;
        use ratatui::{Terminal, backend::TestBackend};

        let mut app = App::new_test_empty();
        app.apply_shell_outcome(shell_bang::ShellOutcome {
            command: "echo url".to_string(),
            stdout: "see https://shell-link.example.com for details".to_string(),
            stderr: String::new(),
            result: shell_bang::ShellResult::Exited(0),
            truncation: None,
        });
        app.history.push(assistant_message(
            "the reply mentions https://reply-link.example.com too".to_string(),
        ));

        let size = ratatui::prelude::Size { width: 80, height: 24 };
        let transcript_area = crate::tui_rata::event_loop::compute_transcript_area(size, false);
        let inner_width = inner_transcript_width(transcript_area);

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| ui(f, &app)).unwrap();

        let plain_rows = app.transcript_rendered_plain_rows(inner_width);

        let shell_row = plain_rows
            .iter()
            .position(|row| {
                crate::tui_rata::hyperlink::extract_links(row)
                    .iter()
                    .any(|l| l.url.contains("shell-link.example.com"))
            })
            .expect("test setup: the shell block's link must appear on some rendered row");
        let reply_row = plain_rows
            .iter()
            .position(|row| {
                crate::tui_rata::hyperlink::extract_links(row)
                    .iter()
                    .any(|l| l.url.contains("reply-link.example.com"))
            })
            .expect("test setup: the reply's link must appear on some rendered row");

        assert!(
            shell_row < reply_row,
            "the shell block's link (row {shell_row}) must render on an EARLIER row than \
             the later assistant reply's link (row {reply_row})"
        );
    }

    /// Consumer 4 — the memo, load-bearing. Two directly-constructed unit
    /// sequences with identical group tags, span bytes, `plain` and
    /// `action` presence but different `history_anchor` values must
    /// produce DIFFERENT `transcript_content_fingerprint`s — otherwise a
    /// reorder the memo cannot see could serve stale cached click geometry.
    /// (The reverted fault-injection that proves the hashing is actually
    /// load-bearing, not merely order-incidental, is recorded verbatim in
    /// the SUMMARY per Plan 10's precedent — it cannot live in this test
    /// without permanently weakening `transcript_content_fingerprint`.)
    #[test]
    fn two_enumerations_differing_only_in_anchor_produce_different_measure_keys() {
        let make_units = |anchor: usize| -> Vec<TranscriptUnit> {
            vec![
                TranscriptUnit {
                    group: TranscriptGroup::History,
                    line: Line::from(Span::raw("You: shared content".to_string())),
                    plain: None,
                    action: None,
                    history_anchor: anchor,
                },
                TranscriptUnit {
                    group: TranscriptGroup::ImageChips,
                    line: Line::from(Span::styled(
                        "[🖼 pic.png]".to_string(),
                        Style::default().fg(Color::Cyan),
                    )),
                    plain: Some("[🖼 pic.png]".to_string()),
                    action: Some(ChipAction::OpenImage {
                        label: "pic.png".to_string(),
                        source: MediaRef {
                            source: MediaSource::Path(PathBuf::from("/tmp/pic.png")),
                            kind: MediaKind::Photo,
                            original_tag_text: "<MEDIA: /tmp/pic.png>".to_string(),
                        },
                    }),
                    history_anchor: anchor,
                },
            ]
        };

        let units_a = make_units(0);
        let units_b = make_units(1);

        // Sanity: identical group tags, span bytes, plain, and action
        // presence — the ONLY difference between the two sequences is
        // `history_anchor`.
        for (a, b) in units_a.iter().zip(units_b.iter()) {
            assert_eq!(a.group, b.group, "test setup: groups must match");
            assert_eq!(a.plain, b.plain, "test setup: plain text must match");
            assert_eq!(
                a.action.is_some(),
                b.action.is_some(),
                "test setup: action presence must match"
            );
            let a_content: Vec<_> = a.line.spans.iter().map(|s| s.content.as_ref()).collect();
            let b_content: Vec<_> = b.line.spans.iter().map(|s| s.content.as_ref()).collect();
            assert_eq!(a_content, b_content, "test setup: span content must match");
        }

        let fp_a = transcript_content_fingerprint(&units_a);
        let fp_b = transcript_content_fingerprint(&units_b);
        assert_ne!(
            fp_a, fp_b,
            "two enumerations differing ONLY in history_anchor must produce DIFFERENT \
             fingerprints — a reorder must never be served stale cached geometry"
        );
    }
}

// ── D-02 Word-wrap unit tests (Phase 36.6.1 Plan 01) ──────────────────────────
//
// These tests verify that `word_wrapped_line_count` matches ratatui's actual
// render output (via TestBackend) for representative inputs, pinning the
// formula's correctness independently of the full UI stack.
//
// Test IDs per VALIDATION.md: 36.6.1-01-01 through 36.6.1-01-05.

#[cfg(all(test, feature = "test-support"))]
mod word_wrap_tests {
    use super::*;
    use ratatui::{
        Terminal,
        backend::TestBackend,
        widgets::{Paragraph, Wrap},
    };

    /// Helper: count rows ratatui actually renders for `text` at column `width`.
    ///
    /// Uses a `TestBackend` tall enough (200 rows) to never clip, renders
    /// `Paragraph::new(text).wrap(Wrap { trim: false })`, then counts non-blank
    /// rows from the top, stopping at the first all-blank row after content starts.
    fn ratatui_line_count(text: &str, width: u16) -> usize {
        let height = 200u16;
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                let para = Paragraph::new(text).wrap(Wrap { trim: false });
                f.render_widget(para, f.area());
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        let mut count = 0usize;
        for row in 0..height {
            let row_blank = (0..width).all(|col| {
                buf.cell((col, row))
                    .map(|c| c.symbol() == " ")
                    .unwrap_or(true)
            });
            if row_blank && row > 0 {
                break;
            }
            if !row_blank {
                count += 1;
            }
        }
        count
    }

    /// 36.6.1-01-01: A long sentence that wraps at width 78 must produce the
    /// same row count as ratatui's actual render.
    #[test]
    fn word_wrapped_line_count_matches_ratatui_for_wrapping_sentence() {
        let line = "This is a long sentence that definitely wraps at a standard terminal \
                    width because it exceeds eighty characters in total length blah blah.";
        let width = 78usize;
        let our_count = word_wrapped_line_count(line, width);
        let ratatui_count = ratatui_line_count(line, width as u16);
        assert_eq!(
            our_count, ratatui_count,
            "word_wrapped_line_count({width}) = {our_count}, ratatui = {ratatui_count}\nline: {line:?}"
        );
    }

    /// 36.6.1-01-02: Short line, empty line, and exact-width line each produce 1 row.
    #[test]
    fn word_wrapped_line_count_short_empty_exact() {
        // Short line
        assert_eq!(word_wrapped_line_count("Hi!", 78), 1);
        assert_eq!(
            word_wrapped_line_count("Hi!", 78),
            ratatui_line_count("Hi!", 78)
        );
        // Empty line
        assert_eq!(word_wrapped_line_count("", 78), 1);
        // Exact-width line (78 'a' chars) — fills one row, no wrap
        let exact = "a".repeat(78);
        assert_eq!(word_wrapped_line_count(&exact, 78), 1);
        assert_eq!(
            word_wrapped_line_count(&exact, 78),
            ratatui_line_count(&exact, 78)
        );
    }

    /// 36.6.1-01-03: A word that straddles the column-78 boundary is pushed to
    /// the next row, producing exactly 2 rows.
    #[test]
    fn word_wrapped_line_count_word_that_straddles_boundary() {
        // 70 'a' + space + "boundary" (8 chars) = 79 cols — "boundary" doesn't fit on row 1
        let line = format!("{} boundary", "a".repeat(70));
        let width = 78usize;
        let our_count = word_wrapped_line_count(&line, width);
        let ratatui_count = ratatui_line_count(&line, width as u16);
        assert_eq!(
            our_count, ratatui_count,
            "word boundary wrap mismatch: our={our_count}, ratatui={ratatui_count}"
        );
        assert_eq!(our_count, 2, "should need exactly 2 rows");
    }

    /// 36.6.1-01-04: A line with 4 leading spaces followed by enough words to
    /// wrap at width 78 must match ratatui's row count.
    ///
    /// Regression test for RESEARCH §6 Risk 1 (leading whitespace): a naive
    /// `split_whitespace`-based simulator would drop the leading spaces and
    /// potentially produce one fewer row than ratatui.
    #[test]
    fn word_wrapped_line_count_leading_whitespace() {
        let line = "    bullet item with enough words to overflow the available column width when wrapped at seventy-eight";
        let width = 78usize;
        let our_count = word_wrapped_line_count(line, width);
        let ratatui_count = ratatui_line_count(line, width as u16);
        assert_eq!(
            our_count, ratatui_count,
            "leading-whitespace wrap mismatch: our={our_count}, ratatui={ratatui_count}\nline: {line:?}"
        );
    }

    /// 36.6.1-01-05: A 25-line assistant message — `transcript_line_count(78)`
    /// must equal the non-blank row count ratatui actually renders via `ui()`.
    #[test]
    fn transcript_line_count_matches_ratatui() {
        use crate::tui_rata::ui::ui;

        let body: &'static str = Box::leak(
            (1..=25)
                .map(|i| format!("Line {i}: this is content"))
                .collect::<Vec<_>>()
                .join("\n")
                .into_boxed_str(),
        );
        let app = App::new_test_with_messages(vec![("assistant", body)]);
        let inner_width = 78usize; // 80-col terminal minus 2 border cols
        let our_count = app.transcript_line_count(inner_width);

        // Render to a tall TestBackend and count non-blank rows in the transcript
        // content area. We exclude:
        // - Col 0 and col 79: block border characters
        // - Col 78: scrollbar track (always non-blank in every interior row)
        // So we check cols 1..78 (exclusive of 78) for actual text content.
        // Rows start at 1 (below top border) and stop at the first blank row
        // after content has been seen.
        let backend = TestBackend::new(80, 200);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| ui(f, &app)).unwrap();
        let buf = terminal.backend().buffer();
        let mut ratatui_count = 0usize;
        for row in 1u16..199 {
            let row_blank = (1u16..78).all(|col| {
                buf.cell((col, row))
                    .map(|c| c.symbol() == " ")
                    .unwrap_or(true)
            });
            if row_blank && ratatui_count > 0 {
                break;
            }
            if !row_blank {
                ratatui_count += 1;
            }
        }
        assert_eq!(
            our_count, ratatui_count,
            "transcript_line_count mismatch: our={our_count}, ratatui={ratatui_count}"
        );
    }
}

// ── Phase 46.7 Plan 06 tests: TUI attachments (D-18/D-20/D-12) ──────────────

#[cfg(all(test, feature = "test-support"))]
mod tui_attach_at_path {
    use super::*;
    use std::sync::{Mutex as StdMutex, OnceLock};

    static ENV_LOCK: OnceLock<StdMutex<()>> = OnceLock::new();

    /// SAFETY: `std::env::set_var` mutates process-global state. This project's
    /// gate runs `cargo nextest` (process-per-test), but the lock is kept as a
    /// defense-in-depth mirror of the `setup.rs`/`chat_capture.rs` convention
    /// in case these tests are ever run under plain multi-threaded `cargo test`.
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.get_or_init(|| StdMutex::new(())).lock().unwrap()
    }

    /// Redirects `IRONHERMES_HOME` to a fresh tempdir (so `session_attachments_dir`
    /// resolves under an isolated root) and wires a fresh on-disk `StateStore`
    /// (Plan 01 schema v11 `chat_attachments`) into `app.state_store`.
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

    #[test]
    fn extract_attach_candidates_strips_at_path_token() {
        let (remaining, candidates) = extract_attach_candidates("look at @notes.txt please");
        assert_eq!(candidates, vec!["notes.txt".to_string()]);
        assert_eq!(remaining, "look at please");
    }

    #[test]
    fn extract_attach_candidates_ignores_nonexistent_bare_path() {
        let (remaining, candidates) =
            extract_attach_candidates("/definitely/not/a/real/path.png hello");
        assert!(
            candidates.is_empty(),
            "a bare path that doesn't resolve to a real file must not auto-attach"
        );
        assert_eq!(remaining, "/definitely/not/a/real/path.png hello");
    }

    #[test]
    fn extract_attach_candidates_detects_dropped_absolute_path() {
        let _g = lock();
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("dropped.png");
        std::fs::write(&file_path, b"fake png bytes").unwrap();

        let text = file_path.to_string_lossy().to_string();
        let (remaining, candidates) = extract_attach_candidates(&text);
        assert_eq!(candidates, vec![text.clone()]);
        assert_eq!(remaining, "");
    }

    #[test]
    fn copy_local_path_into_store_round_trips_and_inserts_row() {
        let _g = lock();
        let (app, _home_dir) = app_with_store();
        let src_dir = tempfile::tempdir().unwrap();
        let src_path = src_dir.path().join("report.txt");
        std::fs::write(&src_path, b"hello world").unwrap();

        let pending = app
            .copy_local_path_into_store(src_path.to_str().unwrap())
            .expect("attach must succeed");
        assert_eq!(pending.filename, "report.txt");

        let dest = ironhermes_core::session_attachments_dir(&app.session_id)
            .join(&pending.stored_rel_path);
        assert!(
            dest.is_file(),
            "bytes must be copied into the session attachment store"
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"hello world");

        // D-20: a chat_attachments row must exist — the SAME store web uploads use.
        let store = app.state_store.clone().unwrap();
        let rows = store
            .lock()
            .unwrap()
            .list_chat_attachments(&app.session_id)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].filename, "report.txt");
        assert_eq!(rows[0].stored_rel_path, pending.stored_rel_path);
    }

    #[test]
    fn copy_local_path_into_store_rejects_missing_file() {
        let _g = lock();
        let (app, _home_dir) = app_with_store();
        let result = app.copy_local_path_into_store("/definitely/does/not/exist-46-7.txt");
        assert_eq!(result.unwrap_err().1, "read error");
    }

    #[test]
    fn copy_local_path_into_store_rejects_oversize_nonimage() {
        let _g = lock();
        let (app, _home_dir) = app_with_store();
        let src_dir = tempfile::tempdir().unwrap();
        let src_path = src_dir.path().join("huge.txt");
        let big = vec![b'a'; ironhermes_gateway::multimodal::NONIMAGE_MAX_BYTES + 1];
        std::fs::write(&src_path, &big).unwrap();

        let result = app.copy_local_path_into_store(src_path.to_str().unwrap());
        assert_eq!(result.unwrap_err().1, "file too large");
    }

    /// T-46.7-18: a path whose `Path::file_name()` component is empty (e.g.
    /// resolving to `..`) must be rejected by `safe_attachment_leaf`'s
    /// empty-string guard BEFORE any filesystem write.
    #[test]
    fn traversal_filename_is_rejected_before_copy() {
        let _g = lock();
        let (app, _home_dir) = app_with_store();
        let result = app.copy_local_path_into_store("/tmp/..");
        assert_eq!(result.unwrap_err().1, "unsupported file type");
    }

    #[test]
    fn submit_with_at_path_attaches_copies_into_store_and_drains_queue() {
        let _g = lock();
        let (mut app, _home_dir) = app_with_store();
        let src_dir = tempfile::tempdir().unwrap();
        let src_path = src_dir.path().join("notes.txt");
        std::fs::write(&src_path, b"attachment body").unwrap();

        let text = format!("please review @{}", src_path.to_string_lossy());
        app.textarea.insert_str(&text);
        app.submit();

        assert!(
            app.pending_attachments.is_empty(),
            "pending_attachments must drain on submit"
        );
        let last = app
            .history
            .last()
            .expect("a user message must have been pushed");
        assert_eq!(last.role, Role::User);
        match &last.content {
            Some(MessageContent::Text(body)) => {
                assert!(
                    body.contains("attachment body"),
                    "inline text attachment body must be present via build_chat_user_message: {body}"
                );
                assert!(
                    !body.contains('@'),
                    "the @path directive token must be stripped from the model-visible text: {body}"
                );
            }
            other => panic!("expected MessageContent::Text (no images attached), got {other:?}"),
        }

        // D-20: the attachment must be discoverable in the shared store.
        let store = app.state_store.clone().unwrap();
        let rows = store
            .lock()
            .unwrap()
            .list_chat_attachments(&app.session_id)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].filename, "notes.txt");
    }

    #[test]
    fn submit_without_attachments_is_unchanged() {
        let _g = lock();
        let (mut app, _home_dir) = app_with_store();
        app.textarea.insert_str("hello world");
        app.submit();

        assert!(app.pending_attachments.is_empty());
        let last = app.history.last().unwrap();
        assert_eq!(last.role, Role::User);
        match &last.content {
            Some(MessageContent::Text(body)) => assert_eq!(body, "hello world"),
            other => panic!("expected plain MessageContent::Text, got {other:?}"),
        }
    }
}

// ── Phase 46.7 Plan 07 tests: transcript chip rendering + hit-test map ─────

#[cfg(all(test, feature = "test-support"))]
mod transcript_chip_tests {
    use super::*;

    fn plain(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>()
    }

    fn area(width: u16, height: u16) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    fn artifact(id: &str, title: &str) -> ironhermes_tools::chat_capture::CapturedArtifact {
        ironhermes_tools::chat_capture::CapturedArtifact {
            artifact_id: id.to_string(),
            title: title.to_string(),
            filename: "index.html".to_string(),
        }
    }

    /// D-19: a sent attachment renders a `[📎 ...]` chip in DarkGray; a
    /// captured artifact renders a `[▤ ...]` chip in Cyan, both appended
    /// after the base transcript lines (attachment chips first, per the
    /// fixed order `rebuild_chip_hit_test` assumes).
    #[test]
    fn transcript_chip_lines_render_expected_glyphs_and_colors() {
        let mut app = App::new_test_empty();
        app.sent_attachment_chips.push(SentAttachmentChip {
            filename: "photo.png".to_string(),
            size_bytes: 2_202_009, // ~2.1 MiB
            history_anchor: app.history.len(),
        });
        app.captured_artifacts
            .lock()
            .unwrap()
            .push(artifact("abc-123", "My Report"));

        let text = app.transcript_render_text();
        assert_eq!(text.lines.len(), 2, "empty base transcript + 2 chip lines");
        let attachment_line = &text.lines[0];
        let artifact_line = &text.lines[1];

        assert_eq!(plain(attachment_line), "[📎 photo.png 2.1 MiB]");
        assert_eq!(attachment_line.spans[0].style.fg, Some(Color::DarkGray));

        assert_eq!(plain(artifact_line), "[▤ My Report]");
        assert_eq!(artifact_line.spans[0].style.fg, Some(Color::Cyan));
    }

    /// Task 1 acceptance: rendering a transcript with one artifact chip
    /// populates exactly one hit-test entry with the artifact URL. A plain
    /// attachment chip must NOT get an entry — only artifact-link chips are
    /// clickable per the UI-SPEC.
    #[test]
    fn transcript_chip_hit_test_populates_one_entry_for_artifact_chip() {
        let mut app = App::new_test_empty();
        app.sent_attachment_chips.push(SentAttachmentChip {
            filename: "notes.txt".to_string(),
            size_bytes: 512,
            history_anchor: app.history.len(),
        });
        app.captured_artifacts
            .lock()
            .unwrap()
            .push(artifact("artifact-xyz", "Dashboard"));

        let a = area(80, 24);
        let measurement = app.transcript_measurement(inner_transcript_width(a));
        app.rebuild_chip_hit_test(a, &measurement);

        let hits = app.chip_hit_test.lock().unwrap();
        assert_eq!(
            hits.len(),
            1,
            "exactly one hit-test entry — only the artifact chip is \
             clickable, the attachment chip must not add one"
        );
        match &hits[0].1 {
            ChipAction::OpenArtifactUrl(url) => {
                assert!(
                    url.contains("artifact-xyz"),
                    "hit-test URL must be derived from the artifact id: {url}"
                );
                assert!(
                    url.contains("/artifacts/"),
                    "URL must hit the 46.6 viewer route: {url}"
                );
            }
            other => panic!("expected ChipAction::OpenArtifactUrl, got: {other:?}"),
        }
    }

    // ── Phase 36.6.4 Plan 05 Task 1 `<behavior>` tests (D-12/D-13, TUI-IMG-01) ──

    fn photo_media_ref(path: &str) -> MediaRef {
        MediaRef {
            source: MediaSource::Path(PathBuf::from(path)),
            kind: MediaKind::Photo,
            original_tag_text: format!("<MEDIA: {path}>"),
        }
    }

    /// Test 1: the chip line carries the frame glyph, `Color::Cyan`, and a
    /// label truncated at 40 DISPLAY cells with a trailing ellipsis.
    #[test]
    fn image_chip_renders_frame_glyph_cyan_truncated_at_40_cells() {
        let long_label = "x".repeat(60);
        let chip = ImageChip {
            label: long_label,
            source: photo_media_ref("/tmp/x.png"),
            history_anchor: 0,
        };
        let line = image_chip_line(&chip);
        let text = plain(&line);

        assert!(
            text.starts_with("[🖼 "),
            "chip must carry the frame-with-picture glyph: {text}"
        );
        assert!(text.ends_with("…]"), "over-long label must end with an ellipsis: {text}");
        let label_part = text
            .trim_start_matches("[🖼 ")
            .trim_end_matches(']');
        assert_eq!(
            UnicodeWidthStr::width(label_part),
            40,
            "truncated label must be exactly 40 DISPLAY cells wide: {label_part}"
        );
        assert_eq!(line.spans[0].style.fg, Some(Color::Cyan));
    }

    /// Test 2: a completed assistant turn containing a media tag renders a
    /// chip and the raw tag literal does not appear in the transcript.
    #[test]
    fn media_tag_in_assistant_turn_yields_chip_not_raw_tag_text() {
        let mut app = App::new_test_empty();
        app.assistant_buffer =
            Some("Here is your image: <MEDIA: /tmp/gen.png>".to_string());
        app.commit_assistant_buffer();

        assert_eq!(
            app.image_chips.len(),
            1,
            "one Photo MediaRef must yield exactly one image chip"
        );
        assert_eq!(app.image_chips[0].label, "gen.png");

        for msg in &app.history {
            if let Some(MessageContent::Text(t)) = &msg.content {
                assert!(
                    !t.contains("<MEDIA:"),
                    "raw tag literal must never reach the transcript: {t}"
                );
            }
        }
    }

    /// Test 5: after scrolling, an image chip's stored hit-test rect
    /// matches the row it is drawn on (Plan 01's Pitfall-2 guard, extended
    /// to the new chip family).
    #[test]
    fn image_chip_hit_test_participates_after_scroll() {
        let body = (1..=3).map(|i| format!("ln{i}")).collect::<Vec<_>>().join("\n");
        let mut app =
            App::new_test_with_messages(vec![("assistant", Box::leak(body.into_boxed_str()))]);
        app.image_chips.push(ImageChip {
            label: "target.png".to_string(),
            source: photo_media_ref("/tmp/target.png"),
            history_anchor: app.history.len(),
        });
        // Padding AFTER the target (chips are always the LAST content rows
        // — without trailing padding, a chip is only ever visible at
        // exactly max_scroll, making "scroll and it's still visible, just
        // at a different row" impossible to construct).
        for i in 0..15 {
            let history_anchor = app.history.len();
            app.image_chips.push(ImageChip {
                label: format!("pad{i}.png"),
                source: photo_media_ref(&format!("/tmp/pad{i}.png")),
                history_anchor,
            });
        }

        let a = area(80, 15);
        let measurement = app.transcript_measurement(inner_transcript_width(a));
        app.rebuild_chip_hit_test(a, &measurement);
        let pre_hits = app.chip_hit_test_snapshot();
        let pre_rect = pre_hits
            .iter()
            .find(|(_, act)| {
                matches!(act, ChipAction::OpenImage { label, .. } if label == "target.png")
            })
            .map(|(r, _)| *r)
            .expect("target image chip must be visible at scroll=0");

        app.scroll_down(3);
        let measurement = app.transcript_measurement(inner_transcript_width(a));
        app.rebuild_chip_hit_test(a, &measurement);
        let post_hits = app.chip_hit_test_snapshot();
        let post_rect = post_hits
            .iter()
            .find(|(_, act)| {
                matches!(act, ChipAction::OpenImage { label, .. } if label == "target.png")
            })
            .map(|(r, _)| *r)
            .expect("target image chip must still be visible after a 3-row scroll");

        assert_eq!(
            post_rect.y,
            pre_rect.y - 3,
            "image chip hit-test rect must move UP by exactly the scroll delta, \
             re-derived from scroll_view_state"
        );
    }

    /// The chip rect width math MUST reuse `inner_transcript_width` — the
    /// SAME shared helper `transcript_max_scroll`/the ui.rs scrollbar call —
    /// not a fresh `area.width` computation. The rect must start inside the
    /// left border and never exceed the inner (border-excluded) width.
    #[test]
    fn transcript_chip_hit_test_rect_uses_inner_width() {
        let app = App::new_test_empty();
        app.captured_artifacts
            .lock()
            .unwrap()
            .push(artifact("id1", "hi"));
        let a = area(80, 24);
        let measurement = app.transcript_measurement(inner_transcript_width(a));
        app.rebuild_chip_hit_test(a, &measurement);
        let hits = app.chip_hit_test.lock().unwrap();
        let (rect, _) = &hits[0];
        assert_eq!(
            rect.x,
            a.x + 1,
            "chip rect must start inside the left border"
        );
        assert!(
            rect.width as usize <= inner_transcript_width(a),
            "chip rect width must not exceed the inner (border-excluded) width"
        );
    }

    /// A chip fully scrolled outside the visible viewport must not get a
    /// hit-test entry (bounded per-frame — T-46.7-22 accepted disposition).
    #[test]
    fn transcript_chip_hit_test_scrolled_out_of_view_yields_no_entry() {
        let mut app = App::new_test_empty();
        app.captured_artifacts
            .lock()
            .unwrap()
            .push(artifact("id1", "hi"));
        // Base transcript is empty (row 0 is the chip's only row); a tiny
        // 1-row-tall viewport scrolled past row 0 pushes the chip fully
        // out of view.
        app.set_transcript_scroll(5);
        let a = area(80, 3); // height 3 → visible_rows = 1
        let measurement = app.transcript_measurement(inner_transcript_width(a));
        app.rebuild_chip_hit_test(a, &measurement);
        let hits = app.chip_hit_test.lock().unwrap();
        assert!(
            hits.is_empty(),
            "a chip scrolled out of the visible viewport must not get a hit-test entry"
        );
    }
}

// ── Phase 46.7 Plan 07 tests: scoped Down(Left) handle_mouse arm ───────────

#[cfg(all(test, feature = "test-support"))]
mod handle_mouse_chip_tests {
    use super::*;
    use std::sync::{Arc as StdArc, Mutex as StdMutex};

    fn area(width: u16, height: u16) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    fn left_click(column: u16, row: u16) -> crossterm::event::MouseEvent {
        crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }
    }

    #[test]
    fn chip_action_at_finds_action_inside_rect_and_none_outside() {
        let rect = Rect {
            x: 5,
            y: 2,
            width: 10,
            height: 1,
        };
        let hits = vec![(
            rect,
            ChipAction::OpenArtifactUrl("http://x/artifacts/1".to_string()),
        )];

        assert_eq!(
            chip_action_at(&hits, 7, 2),
            Some(ChipAction::OpenArtifactUrl(
                "http://x/artifacts/1".to_string()
            ))
        );
        assert_eq!(chip_action_at(&hits, 100, 100), None);
    }

    /// Task 2 acceptance: Down(Left) inside a seeded chip rect triggers the
    /// open action. Uses a swapped-in no-op `opener` so this NEVER launches
    /// the real OS browser during test execution.
    #[test]
    fn down_left_inside_chip_rect_opens_artifact_url() {
        let mut app = App::new_test_empty();
        let opened: StdArc<StdMutex<Vec<String>>> = StdArc::new(StdMutex::new(Vec::new()));
        let opened_clone = opened.clone();
        app.opener = Box::new(move |url: &str| {
            opened_clone.lock().unwrap().push(url.to_string());
            Ok(())
        });

        let a = area(80, 24);
        let rect = Rect {
            x: a.x + 1,
            y: a.y + 1,
            width: 20,
            height: 1,
        };
        *app.chip_hit_test.lock().unwrap() = vec![(
            rect,
            ChipAction::OpenArtifactUrl("http://127.0.0.1:8080/artifacts/id1".to_string()),
        )];

        app.handle_mouse(left_click(rect.x + 2, rect.y), a);

        assert_eq!(
            opened.lock().unwrap().as_slice(),
            ["http://127.0.0.1:8080/artifacts/id1".to_string()]
        );
    }

    /// Task 2 acceptance: Down(Left) outside any chip rect is a no-op — no
    /// open, no scroll. The click still lands inside the transcript `area`
    /// (so the pre-existing bounds check passes) but not on the seeded rect.
    #[test]
    fn down_left_outside_chip_rect_is_noop() {
        let mut app = App::new_test_empty();
        let opened: StdArc<StdMutex<Vec<String>>> = StdArc::new(StdMutex::new(Vec::new()));
        let opened_clone = opened.clone();
        app.opener = Box::new(move |url: &str| {
            opened_clone.lock().unwrap().push(url.to_string());
            Ok(())
        });

        let a = area(80, 24);
        let rect = Rect {
            x: a.x + 1,
            y: a.y + 1,
            width: 20,
            height: 1,
        };
        *app.chip_hit_test.lock().unwrap() = vec![(
            rect,
            ChipAction::OpenArtifactUrl("http://127.0.0.1:8080/artifacts/id1".to_string()),
        )];

        let scroll_before = app.transcript_scroll();
        app.handle_mouse(left_click(rect.x + 50, rect.y), a);

        assert!(
            opened.lock().unwrap().is_empty(),
            "click outside any chip rect must not open a URL"
        );
        assert_eq!(
            app.transcript_scroll(), scroll_before,
            "Down(Left) must never scroll"
        );
    }

    /// D-17 regression: the pre-existing ScrollUp/ScrollDown arms and the
    /// within-bounds early return are byte-for-byte unchanged by the new
    /// Down(Left) arm — mirrors `handle_mouse_outside_area_noop` plus a
    /// live in-bounds ScrollDown check.
    #[test]
    fn handle_mouse_scroll_arm_unchanged_by_chip_addition() {
        let mut app = App::new_test_empty();
        let scroll_before = app.transcript_scroll();
        let auto_before = app.auto_follow;
        let a = area(80, 24);
        let scroll_event = crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::ScrollDown,
            column: a.x + 1,
            row: a.y + 1,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };
        app.handle_mouse(scroll_event, a);
        assert_eq!(app.transcript_scroll(), scroll_before.saturating_add(3));
        assert_eq!(app.auto_follow, auto_before);
    }
}

// ── Phase 46.7 UAT test 7 regression: hit map vs the REAL rendered frame ──────
//
// The Plan 07 hit-test tests all validate `rebuild_chip_hit_test` against its
// own math. A wrong shared assumption therefore passes every one of them while
// the live click misses. This module instead renders the REAL `ui()` frame,
// finds where the chip glyph is actually drawn on screen, and clicks there.
#[cfg(all(test, feature = "test-support"))]
mod chip_click_vs_rendered_frame_tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    /// Screen coords of the first `▤` (artifact-chip glyph) in the rendered buffer.
    fn find_chip_glyph(buf: &ratatui::buffer::Buffer, w: u16, h: u16) -> Option<(u16, u16)> {
        for row in 0..h {
            for col in 0..w {
                if buf.cell((col, row)).map(|c| c.symbol()) == Some("\u{25a4}") {
                    return Some((col, row));
                }
            }
        }
        None
    }

    /// Click the cell where the artifact chip is ACTUALLY drawn — the hit map
    /// must resolve it to an open action. This is the live UAT gesture.
    ///
    /// The transcript is NON-EMPTY and wraps: the chip's row is derived from
    /// `transcript_line_count`, so an empty transcript (base row count 0) can
    /// never expose drift between that count and what ratatui really draws.
    #[test]
    fn clicking_the_rendered_chip_glyph_resolves_to_an_open_action() {
        let (w, h) = (80u16, 24u16);
        let body: &'static str = Box::leak(
            (1..=3)
                .map(|i| {
                    format!(
                        "Line {i}: a deliberately long assistant line that must wrap at eighty columns."
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
                .into_boxed_str(),
        );
        let app =
            App::new_test_with_messages(vec![("user", "make me a dashboard"), ("assistant", body)]);
        app.captured_artifacts.lock().unwrap().push(
            super::transcript_chip_tests_support::artifact_for("artifact-xyz", "Dashboard"),
        );

        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        // The real draw path — this is what populates chip_hit_test in prod.
        terminal.draw(|f| crate::tui_rata::ui::ui(f, &app)).unwrap();

        let (col, row) = {
            let buf = terminal.backend().buffer();
            find_chip_glyph(buf, w, h).expect("artifact chip glyph must be rendered on screen")
        };

        let hits = app.chip_hit_test.lock().unwrap();
        assert!(
            !hits.is_empty(),
            "the real render pass must populate the chip hit-test map"
        );
        let action = chip_action_at(&hits, col, row);
        assert!(
            action.is_some(),
            "clicking the cell where the chip is ACTUALLY drawn ({col},{row}) must resolve \
             to an open action, but the hit map holds {:?} — the hit rect does not line up \
             with the rendered glyph",
            hits.iter().map(|(r, _)| *r).collect::<Vec<_>>()
        );
    }

    // — Phase 36.6.2 Plan 02: thinking panel state / toggle / buffering ─────

    fn ctrl_t_key() -> crossterm::event::KeyEvent {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
        KeyEvent {
            code: KeyCode::Char('t'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    #[test]
    fn ctrl_t_toggles_thinking_expanded() {
        let mut app = App::new_test_empty();
        assert!(!app.thinking_expanded, "default must be collapsed");

        app.handle_key(ctrl_t_key());
        assert!(app.thinking_expanded, "first Ctrl+T must expand");

        app.handle_key(ctrl_t_key());
        assert!(
            !app.thinking_expanded,
            "double-toggle must return to collapsed (idempotent)"
        );
    }

    #[test]
    fn activity_events_buffer_while_collapsed() {
        let mut app = App::new_test_empty();
        assert!(!app.thinking_expanded, "test setup: must start collapsed");
        assert!(app.thinking_lines.is_empty());

        app.handle_stream_event(StreamEvent::ToolCall {
            name: "bash".to_string(),
        });

        assert!(
            !app.thinking_lines.is_empty(),
            "ToolCall must buffer into thinking_lines even while collapsed"
        );
        assert!(app.thinking_lines[0].contains("bash"));
    }

    #[test]
    fn thinking_lines_cleared_per_turn() {
        let mut app = App::new_test_empty();
        app.handle_stream_event(StreamEvent::ToolCall {
            name: "bash".to_string(),
        });
        assert!(!app.thinking_lines.is_empty(), "test setup: must buffer first");

        // New turn: type non-slash text and submit — mirrors real dispatch.
        app.textarea.insert_str("hello there");
        app.submit();

        assert!(
            app.thinking_lines.is_empty(),
            "starting a new turn must clear the previous turn's activity buffer"
        );
    }

    // — Phase 36.6.2 Plan 03: approval/secret/sudo key semantics (TUI-02) ─────

    fn ap_key(code: crossterm::event::KeyCode) -> crossterm::event::KeyEvent {
        use crossterm::event::{KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn ap_ctrl(code: crossterm::event::KeyCode) -> crossterm::event::KeyEvent {
        use crossterm::event::{KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        KeyEvent {
            code,
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn approval_overlay(cache_key: &str) -> OverlayKind {
        OverlayKind::Approval {
            cache_key: cache_key.to_string(),
            label: cache_key.to_string(),
            detail: "detail".to_string(),
        }
    }

    /// The single highest-value test in the phase (RESEARCH Pitfall 2): drive a
    /// REAL `TuiApprovalGate` channel round-trip — a spawned task awaits the
    /// oneshot, the request is surfaced via the real `surface_approval_request`
    /// path, Esc is pressed, and the AWAITING request must resolve to `Denied`
    /// (fail-closed) — NOT Approved, NOT a textarea-clear.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn esc_denies_pending_approval_fail_closed() {
        use crate::tui_rata::approval_gate_tui::TuiApprovalGate;
        use ironhermes_core::ApprovalGate; // bring the trait into scope for request_approval

        let mut app = App::new_test_empty();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = TuiApprovalGate::new(tx, app.approvals_store.clone(), Arc::new(AtomicBool::new(false)));

        // Spawn the blocked tool call awaiting the gate decision (real gate).
        let handle = tokio::spawn(async move {
            gate.request_approval(
                "sess",
                "terminal",
                "rm -rf /",
                &serde_json::json!({ "command": "rm -rf /" }),
            )
            .await
        });

        // Drain + surface the request exactly as the run_app_inner select! arm does.
        let req = rx.recv().await.expect("gate must emit an ApprovalRequest");
        app.surface_approval_request(req);
        assert!(
            matches!(app.active_overlay, Some(OverlayKind::Approval { .. })),
            "the surfaced request must become the active approval overlay"
        );

        // Press Esc — fail-closed deny.
        app.handle_key(ap_key(crossterm::event::KeyCode::Esc));

        let outcome = handle.await.unwrap();
        assert_eq!(
            outcome,
            ApprovalOutcome::Denied,
            "Esc on a REAL pending approval must resolve the awaiting request to Denied"
        );
        assert!(app.active_overlay.is_none(), "overlay must close after Esc");
    }

    #[test]
    fn y_approves_n_denies() {
        // [y] → Approved
        let mut app = App::new_test_empty();
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        app.active_overlay = Some(approval_overlay("cmd"));
        app.pending_approval_resolve = Some(tx);
        app.handle_key(ap_key(crossterm::event::KeyCode::Char('y')));
        assert_eq!(rx.try_recv().unwrap(), ApprovalOutcome::Approved);
        assert!(app.active_overlay.is_none());

        // [n] → Denied
        let mut app2 = App::new_test_empty();
        let (tx2, mut rx2) = tokio::sync::oneshot::channel();
        app2.active_overlay = Some(approval_overlay("cmd"));
        app2.pending_approval_resolve = Some(tx2);
        app2.handle_key(ap_key(crossterm::event::KeyCode::Char('n')));
        assert_eq!(rx2.try_recv().unwrap(), ApprovalOutcome::Denied);
        assert!(app2.active_overlay.is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn s_grants_session_via_store() {
        let mut app = App::new_test_empty();
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        app.active_overlay = Some(approval_overlay("echo hi"));
        app.pending_approval_resolve = Some(tx);

        app.handle_key(ap_key(crossterm::event::KeyCode::Char('s')));

        // [s] resolves Approved AND grants the session on the SHARED store.
        assert_eq!(rx.try_recv().unwrap(), ApprovalOutcome::Approved);

        // The grant is spawned (fire-and-forget) — poll the shared store until it
        // lands (same store, same cache_key scope: no parallel store).
        let mut granted = false;
        for _ in 0..50 {
            if app.approvals_store.is_session_approved("echo hi").await {
                granted = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            granted,
            "[s] must call approve_session on the shared ApprovalsStore (same cache_key scope)"
        );
    }

    #[test]
    fn secret_enter_submits_esc_cancels() {
        // Typing mutates the masked buffer; Enter submits (Approved).
        let mut app = App::new_test_empty();
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        app.active_overlay = Some(OverlayKind::Secret {
            prompt: "token".to_string(),
            masked_input: crate::tui_rata::overlay::RedactedSecret::default(),
        });
        app.pending_approval_resolve = Some(tx);
        app.handle_key(ap_key(crossterm::event::KeyCode::Char('a')));
        app.handle_key(ap_key(crossterm::event::KeyCode::Char('b')));
        if let Some(OverlayKind::Secret { masked_input, .. }) = &app.active_overlay {
            assert_eq!(masked_input.display_len(), 2, "typed chars must grow the buffer");
        } else {
            panic!("secret overlay must still be active while typing");
        }
        app.handle_key(ap_key(crossterm::event::KeyCode::Enter));
        assert_eq!(rx.try_recv().unwrap(), ApprovalOutcome::Approved);
        assert!(app.active_overlay.is_none());

        // Esc cancels (Denied) without leaking the buffer.
        let mut app2 = App::new_test_empty();
        let (tx2, mut rx2) = tokio::sync::oneshot::channel();
        app2.active_overlay = Some(OverlayKind::Secret {
            prompt: "token".to_string(),
            masked_input: crate::tui_rata::overlay::RedactedSecret::default(),
        });
        app2.pending_approval_resolve = Some(tx2);
        app2.handle_key(ap_key(crossterm::event::KeyCode::Char('x')));
        app2.handle_key(ap_key(crossterm::event::KeyCode::Esc));
        assert_eq!(rx2.try_recv().unwrap(), ApprovalOutcome::Denied);
        assert!(app2.active_overlay.is_none());
    }

    #[test]
    fn overlay_exclusivity_blocks_toggles() {
        let mut app = App::new_test_empty();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        app.active_overlay = Some(approval_overlay("cmd"));
        app.pending_approval_resolve = Some(tx);

        let before = app.thinking_expanded;
        app.handle_key(ap_ctrl(crossterm::event::KeyCode::Char('t')));
        assert_eq!(
            app.thinking_expanded, before,
            "Ctrl+T must be a no-op while an approval overlay is active"
        );
        assert!(
            matches!(app.active_overlay, Some(OverlayKind::Approval { .. })),
            "approval overlay must stay active after Ctrl+T"
        );

        app.handle_key(ap_ctrl(crossterm::event::KeyCode::Char('k')));
        assert!(
            matches!(app.active_overlay, Some(OverlayKind::Approval { .. })),
            "Ctrl+K must NOT switch to the Skills Hub while an approval overlay is active"
        );
    }

    #[test]
    fn queue_pops_next_after_resolution() {
        let mut app = App::new_test_empty();
        let (tx1, mut rx1) = tokio::sync::oneshot::channel();
        app.active_overlay = Some(approval_overlay("c1"));
        app.pending_approval_resolve = Some(tx1);

        // A second request queued behind the active one.
        let (tx2, _rx2) = tokio::sync::oneshot::channel();
        app.approval_queue.push(ApprovalRequest {
            session_id: "s".to_string(),
            tool_name: "terminal".to_string(),
            reason: "r".to_string(),
            command: "c2".to_string(),
            cache_key: "c2".to_string(),
            resolve: tx2,
        });

        // Resolve the front ([n] → Denied); the queued next must surface.
        app.handle_key(ap_key(crossterm::event::KeyCode::Char('n')));
        assert_eq!(rx1.try_recv().unwrap(), ApprovalOutcome::Denied);
        match &app.active_overlay {
            Some(OverlayKind::Approval { cache_key, .. }) => {
                assert_eq!(cache_key, "c2", "the next queued request must surface")
            }
            other => panic!("expected the next queued approval, got {other:?}"),
        }
        assert!(app.approval_queue.is_empty(), "queue must have popped the next");
    }

    /// CR-01 regression (BLOCKER): a REAL `TuiApprovalGate` channel round-trip
    /// proving a queued approval is never stranded when a NON-approval overlay
    /// (Skills Hub) closes. Before the fix, `active_overlay = None` on the Esc
    /// arm cleared the overlay WITHOUT draining `approval_queue`, leaving the
    /// request's `oneshot::Sender` orphaned forever — the awaiting
    /// `request_approval` call (and the gated tool call/turn) would hang.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queued_approval_resurfaces_after_skills_hub_closes() {
        use crate::tui_rata::approval_gate_tui::TuiApprovalGate;
        use ironhermes_core::ApprovalGate; // bring request_approval into scope

        let mut app = App::new_test_empty();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = TuiApprovalGate::new(tx, app.approvals_store.clone(), Arc::new(AtomicBool::new(false)));

        // Open the Skills Hub via the real Ctrl+K path — an unrelated overlay
        // is active when the approval request arrives.
        app.handle_key(ap_ctrl(crossterm::event::KeyCode::Char('k')));
        assert!(
            matches!(app.active_overlay, Some(OverlayKind::SkillsHub)),
            "test setup: Skills Hub must be open"
        );

        // Spawn the blocked tool call awaiting the gate decision (real gate,
        // real oneshot channel — not a hand-built ApprovalRequest).
        let handle = tokio::spawn(async move {
            gate.request_approval(
                "sess",
                "terminal",
                "rm -rf /tmp/x",
                &serde_json::json!({ "command": "rm -rf /tmp/x" }),
            )
            .await
        });

        // Drain the request exactly as the run_app_inner select! arm does.
        // Since active_overlay is Some(SkillsHub), this must ENQUEUE, not
        // activate.
        let req = rx.recv().await.expect("gate must emit an ApprovalRequest");
        app.surface_approval_request(req);
        assert!(
            matches!(app.active_overlay, Some(OverlayKind::SkillsHub)),
            "Skills Hub must remain active — the request is queued, not surfaced"
        );
        assert_eq!(
            app.approval_queue.len(),
            1,
            "the approval request must land in the queue while Skills Hub is open"
        );

        // Close the Skills Hub with Esc — CR-01's fix must re-surface the
        // queued request instead of stranding its oneshot::Sender.
        app.handle_key(ap_key(crossterm::event::KeyCode::Esc));
        assert!(
            matches!(app.active_overlay, Some(OverlayKind::Approval { .. })),
            "the queued approval must re-surface once the Skills Hub closes"
        );
        assert!(
            app.approval_queue.is_empty(),
            "the queue must be drained once the request re-surfaces"
        );

        // Resolve it — [n] denies — and prove the ORIGINAL awaiting
        // request_approval call actually resolves (never orphaned/hung).
        app.handle_key(ap_key(crossterm::event::KeyCode::Char('n')));
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), handle)
            .await
            .expect("request_approval must resolve, not hang forever (CR-01)")
            .unwrap();
        assert_eq!(
            outcome,
            ApprovalOutcome::Denied,
            "the re-surfaced request must resolve via the normal [n] fail-closed path"
        );
        assert!(app.active_overlay.is_none(), "overlay must close after resolving");
    }

    /// CR-01 second path (Ctrl+K toggle-off): same invariant as the Skills Hub
    /// Esc path above, but closing via the Ctrl+K toggle instead of Esc.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queued_approval_resurfaces_after_ctrl_k_toggle_off() {
        use crate::tui_rata::approval_gate_tui::TuiApprovalGate;
        use ironhermes_core::ApprovalGate;

        let mut app = App::new_test_empty();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = TuiApprovalGate::new(tx, app.approvals_store.clone(), Arc::new(AtomicBool::new(false)));

        app.handle_key(ap_ctrl(crossterm::event::KeyCode::Char('k')));
        assert!(matches!(app.active_overlay, Some(OverlayKind::SkillsHub)));

        let handle = tokio::spawn(async move {
            gate.request_approval(
                "sess",
                "terminal",
                "echo hi",
                &serde_json::json!({ "command": "echo hi" }),
            )
            .await
        });

        let req = rx.recv().await.expect("gate must emit an ApprovalRequest");
        app.surface_approval_request(req);
        assert_eq!(app.approval_queue.len(), 1);

        // Toggle the Skills Hub OFF via Ctrl+K (not Esc).
        app.handle_key(ap_ctrl(crossterm::event::KeyCode::Char('k')));
        assert!(
            matches!(app.active_overlay, Some(OverlayKind::Approval { .. })),
            "the queued approval must re-surface once Ctrl+K closes the Skills Hub"
        );
        assert!(app.approval_queue.is_empty());

        app.handle_key(ap_key(crossterm::event::KeyCode::Char('n')));
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), handle)
            .await
            .expect("request_approval must resolve, not hang forever (CR-01)")
            .unwrap();
        assert_eq!(outcome, ApprovalOutcome::Denied);
    }

    // — Phase 36.6.2 Plan 04: Help overlay discoverability (TUI-02, D-08/D-09) —

    fn q_key() -> crossterm::event::KeyEvent {
        use crossterm::event::{KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        KeyEvent {
            code: crossterm::event::KeyCode::Char('?'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn question_mark_opens_help_only_when_textarea_empty() {
        // Empty textarea: `?` opens Help.
        let mut app = App::new_test_empty();
        assert!(app.textarea.is_empty(), "test setup: textarea must start empty");
        app.handle_key(q_key());
        assert!(
            matches!(app.active_overlay, Some(OverlayKind::Help)),
            "`?` with an empty textarea must open the Help overlay"
        );

        // Non-empty textarea: `?` types the literal character, never opens Help.
        let mut app2 = App::new_test_empty();
        app2.textarea.insert_str("hello");
        app2.handle_key(q_key());
        assert!(
            app2.active_overlay.is_none(),
            "`?` with a non-empty textarea must NOT open Help"
        );
        assert_eq!(
            app2.textarea.lines().join(""),
            "hello?",
            "`?` with a non-empty textarea must type the literal character"
        );
    }

    #[test]
    fn help_scroll_clamps_at_end() {
        let mut app = App::new_test_empty();
        app.active_overlay = Some(OverlayKind::Help);
        let entry_count = crate::tui_rata::overlay::help_entry_count();
        assert!(entry_count > 1, "test setup: expects a multi-entry registry");

        // Drive PageDown far past the end of the entry list.
        for _ in 0..50 {
            app.handle_key(ap_key(crossterm::event::KeyCode::PageDown));
        }

        assert_eq!(
            app.help_scroll,
            (entry_count as u16).saturating_sub(1),
            "PageDown must clamp help_scroll at the last entry, never scroll into blank space"
        );

        // Esc closes with no side effect (no approval-style resolve fired).
        app.handle_key(crossterm::event::KeyEvent {
            code: crossterm::event::KeyCode::Esc,
            modifiers: crossterm::event::KeyModifiers::NONE,
            kind: crossterm::event::KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        });
        assert!(app.active_overlay.is_none(), "Esc must close the Help overlay");
    }

    #[test]
    fn ctrl_t_and_ctrl_k_are_conflict_free_with_existing_bindings() {
        // Ctrl+T toggles thinking; Ctrl+B (an existing, unrelated binding)
        // still behaves as before and is not shadowed by the new bindings.
        let mut app = App::new_test_empty();
        assert!(!app.thinking_expanded, "test setup: must start collapsed");

        app.handle_key(ap_ctrl(crossterm::event::KeyCode::Char('t')));
        assert!(app.thinking_expanded, "Ctrl+T must toggle thinking_expanded");

        // Ctrl+K opens the Skills Hub — confirm it dispatches to the Skills
        // Hub action, not to thinking-toggle or any other existing arm.
        let mut app2 = App::new_test_empty();
        assert!(app2.active_overlay.is_none());
        app2.handle_key(ap_ctrl(crossterm::event::KeyCode::Char('k')));
        assert!(
            matches!(app2.active_overlay, Some(OverlayKind::SkillsHub)),
            "Ctrl+K must open the Skills Hub"
        );

        // Ctrl+C (pre-existing binding) must still behave as before — it must
        // NOT have been silently shadowed by the Ctrl+T/Ctrl+K/`?` additions.
        let mut app3 = App::new_test_empty();
        app3.handle_key(ap_ctrl(crossterm::event::KeyCode::Char('c')));
        assert_eq!(
            app3.status.hint, "Ctrl+C again to quit",
            "Ctrl+C must still dispatch to handle_ctrl_c_key (unshadowed by new bindings)"
        );
    }
}

#[cfg(all(test, feature = "test-support"))]
mod transcript_chip_tests_support {
    pub fn artifact_for(id: &str, title: &str) -> ironhermes_tools::chat_capture::CapturedArtifact {
        ironhermes_tools::chat_capture::CapturedArtifact {
            artifact_id: id.to_string(),
            title: title.to_string(),
            filename: "index.html".to_string(),
        }
    }
}

// ── Phase 36.6.4 Plan 02 Task 1: vim-style visual mode (D-05) ──────────────
#[cfg(all(test, feature = "test-support"))]
mod visual_mode_tests {
    use super::*;

    fn key(code: crossterm::event::KeyCode) -> crossterm::event::KeyEvent {
        use crossterm::event::{KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn ctrl_key(code: crossterm::event::KeyCode) -> crossterm::event::KeyEvent {
        use crossterm::event::{KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        KeyEvent {
            code,
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    /// Seeds a generous 30-line transcript and a wide cached
    /// `transcript_area` so visual-mode row movement has real bounds to
    /// clamp against (an empty transcript's `max_row` is 0, which would
    /// make every `j`/Down a no-op and defeat the test's purpose).
    fn app_with_wide_area() -> App {
        let body = (1..=30).map(|i| format!("ln{i}")).collect::<Vec<_>>().join("\n");
        let app = App::new_test_with_messages(vec![("assistant", Box::leak(body.into_boxed_str()))]);
        *app.transcript_area.lock().unwrap() = Rect::new(0, 0, 80, 24);
        app
    }

    #[test]
    fn visual_mode_v_starts_selection_only_when_textarea_empty() {
        let mut app = App::new_test_empty();
        assert!(app.textarea.is_empty(), "test setup: textarea must start empty");

        app.handle_key(key(crossterm::event::KeyCode::Char('v')));
        assert_eq!(
            app.selection_mode,
            selection::SelectionMode::Visual,
            "v with an empty textarea must enter visual mode"
        );
        assert!(
            app.textarea.is_empty(),
            "v that enters visual mode must not insert a literal character"
        );
        assert!(app.selection.is_some(), "entering visual mode must anchor a selection");

        // With content already typed, v must be a literal character.
        let mut app2 = App::new_test_empty();
        app2.textarea.insert_str("hello");
        app2.handle_key(key(crossterm::event::KeyCode::Char('v')));
        assert_eq!(
            app2.selection_mode,
            selection::SelectionMode::Idle,
            "v with a non-empty textarea must NOT enter visual mode"
        );
        assert_eq!(
            app2.textarea.lines().join("\n"),
            "hellov",
            "v with a non-empty textarea must type the literal character"
        );
    }

    #[test]
    fn visual_mode_hjkl_and_arrows_extend_the_cursor() {
        let mut app = app_with_wide_area();
        app.selection_mode = selection::SelectionMode::Visual;
        app.selection = Some(Selection::new_at(selection::ContentPos::new(5, 5)));

        app.handle_key(key(crossterm::event::KeyCode::Char('j')));
        assert_eq!(
            app.selection.unwrap().cursor,
            selection::ContentPos::new(6, 5),
            "j must advance the cursor row by one"
        );
        assert_eq!(
            app.selection.unwrap().anchor,
            selection::ContentPos::new(5, 5),
            "anchor must stay fixed"
        );

        app.handle_key(key(crossterm::event::KeyCode::Down));
        assert_eq!(
            app.selection.unwrap().cursor,
            selection::ContentPos::new(7, 5),
            "Down must also advance the cursor row by one"
        );

        app.handle_key(key(crossterm::event::KeyCode::Char('h')));
        assert_eq!(
            app.selection.unwrap().cursor,
            selection::ContentPos::new(7, 4),
            "h must retreat the cursor column"
        );

        app.handle_key(key(crossterm::event::KeyCode::Left));
        assert_eq!(
            app.selection.unwrap().cursor,
            selection::ContentPos::new(7, 3),
            "Left must also retreat the cursor column"
        );
    }

    #[test]
    fn visual_mode_y_yanks_and_exits() {
        let mut app = app_with_wide_area();
        // Re-pointed at an explicit `Supported` capability (Plan 08): the
        // real `selection::yank` reads real process environment and, on
        // macOS, attempts a real `pbcopy` — neither is a stable input to
        // assert an exact toast string against in CI (see `clipboard_yank`
        // doc). This asserts the unchanged working-case wording, not the
        // OLD accidental unconditional one.
        app.clipboard_yank = Box::new(|text: &str| {
            let n = text.chars().count();
            selection::ClipboardOutcome::Written(
                selection::CopyReport { copied: n, total: n, native_clipboard: selection::NativeClipboardOutcome::NotAttempted },
                selection::TerminalClipboardCaps {
                    support: selection::Osc52Support::Supported,
                    display_name: "iTerm2",
                },
            )
        });
        app.selection_mode = selection::SelectionMode::Visual;
        app.selection = Some(Selection {
            anchor: selection::ContentPos::new(0, 0),
            cursor: selection::ContentPos::new(0, 2),
        });

        app.handle_key(key(crossterm::event::KeyCode::Char('y')));

        assert_eq!(
            app.selection_mode,
            selection::SelectionMode::Idle,
            "y must exit visual mode"
        );
        assert!(
            app.copy_confirmation_text().is_some_and(|t| t.starts_with("Copied")),
            "y must have driven a real yank attempt with the unchanged working-case wording; got {:?}",
            app.copy_confirmation_text()
        );
    }

    #[test]
    fn visual_mode_esc_cancels_without_writing() {
        let mut app = app_with_wide_area();
        app.selection_mode = selection::SelectionMode::Visual;
        app.selection = Some(Selection {
            anchor: selection::ContentPos::new(0, 0),
            cursor: selection::ContentPos::new(0, 2),
        });
        let hint_before = app.status.hint.clone();

        app.handle_key(key(crossterm::event::KeyCode::Esc));

        assert_eq!(
            app.selection_mode,
            selection::SelectionMode::Idle,
            "Esc must exit visual mode"
        );
        assert!(app.selection.is_none(), "Esc must clear the selection");
        assert_eq!(
            app.status.hint, hint_before,
            "Esc must never write to the clipboard (hint must be unchanged)"
        );
    }

    #[test]
    fn ctrl_y_yanks_a_mouse_drag_selection() {
        let mut app = App::new_test_with_messages(vec![("assistant", "hello world")]);
        // Re-pointed at an explicit `Supported` capability (Plan 08) — see
        // the doc comment on `visual_mode_y_yanks_and_exits`'s override.
        app.clipboard_yank = Box::new(|text: &str| {
            let n = text.chars().count();
            selection::ClipboardOutcome::Written(
                selection::CopyReport { copied: n, total: n, native_clipboard: selection::NativeClipboardOutcome::NotAttempted },
                selection::TerminalClipboardCaps {
                    support: selection::Osc52Support::Supported,
                    display_name: "iTerm2",
                },
            )
        });
        let area = Rect::new(0, 0, 80, 19);
        // Seed the transcript_area cache the same way a real render would,
        // via the already-tested rebuild_chip_hit_test call site.
        let measurement = app.transcript_measurement(inner_transcript_width(area));
        app.rebuild_chip_hit_test(area, &measurement);

        // A mouse drag establishes a selection — never entering visual mode.
        app.selection = Some(Selection {
            anchor: selection::ContentPos::new(0, 8),
            cursor: selection::ContentPos::new(0, 13),
        });
        assert_eq!(
            app.selection_mode,
            selection::SelectionMode::Idle,
            "test setup: a mouse-drag selection must not touch selection_mode"
        );

        app.handle_key(ctrl_key(crossterm::event::KeyCode::Char('y')));

        assert_eq!(
            app.selection_mode,
            selection::SelectionMode::Idle,
            "Ctrl+Y from Idle mode must not enter visual mode"
        );
        assert!(
            app.copy_confirmation_text().is_some_and(|t| t.starts_with("Copied")),
            "Ctrl+Y must yank the mouse-drag selection; got toast {:?}",
            app.copy_confirmation_text()
        );
    }

    /// Regression guard: the pre-existing `handle_key` family (the `?`
    /// guard, history recall, approval/skills-hub routing) is unaffected by
    /// the new v/hjkl/y/Ctrl+Y arms.
    #[test]
    fn handle_key_up_down_history_recall_unregressed_outside_visual_mode() {
        let mut app = App::new_test_empty();
        assert_eq!(app.selection_mode, selection::SelectionMode::Idle);
        // No history entries — Up/Down must be no-ops, not panics, and must
        // not touch selection_mode/selection.
        app.handle_key(key(crossterm::event::KeyCode::Up));
        app.handle_key(key(crossterm::event::KeyCode::Down));
        assert_eq!(app.selection_mode, selection::SelectionMode::Idle);
        assert!(app.selection.is_none());
    }
}

// ── Phase 36.6.4 Plan 02 Task 2: double/triple-click granularity (D-07) ────
#[cfg(all(test, feature = "test-support"))]
mod click_count_tests {
    use super::*;

    fn left_down(column: u16, row: u16) -> crossterm::event::MouseEvent {
        crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }
    }

    fn left_up(column: u16, row: u16) -> crossterm::event::MouseEvent {
        crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
            column,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }
    }

    /// Matches `chunks[0]` in an 80x24 frame — the same convention every
    /// other selection test in this crate uses.
    fn area() -> Rect {
        Rect::new(0, 0, 80, 19)
    }

    #[test]
    fn double_click_selects_word_under_cursor() {
        let mut app = App::new_test_with_messages(vec![("assistant", "hello world")]);
        let a = area();
        // "Hermes: hello world" — content col 9 ('e' of hello) -> viewport
        // col = area.x(0) + 1(border) + 9 = 10.
        app.handle_mouse(left_down(10, 1), a);
        assert!(
            app.selection.unwrap().is_empty(),
            "test setup: a single click is a char-range (empty) anchor"
        );
        app.handle_mouse(left_up(10, 1), a);

        app.handle_mouse(left_down(10, 1), a);
        let sel = app.selection.expect("second click at the same cell must produce a selection");
        assert_eq!(sel.anchor, selection::ContentPos::new(0, 8));
        assert_eq!(sel.cursor, selection::ContentPos::new(0, 13));

        let rows = app.transcript_rendered_plain_rows(inner_transcript_width(a));
        assert_eq!(selection::selected_text(&rows, &sel), "hello");
    }

    #[test]
    fn triple_click_selects_full_displayed_line() {
        let mut app = App::new_test_with_messages(vec![("assistant", "hello world")]);
        let a = area();
        app.handle_mouse(left_down(10, 1), a);
        app.handle_mouse(left_up(10, 1), a);
        app.handle_mouse(left_down(10, 1), a);
        app.handle_mouse(left_up(10, 1), a);
        app.handle_mouse(left_down(10, 1), a);
        let sel = app.selection.expect("third click at the same cell must produce a selection");
        assert_eq!(sel.anchor, selection::ContentPos::new(0, 0));

        let rows = app.transcript_rendered_plain_rows(inner_transcript_width(a));
        assert_eq!(
            sel.cursor,
            selection::ContentPos::new(0, rows[0].chars().count()),
            "triple-click must select the FULL wrapped display row"
        );
        let text = selection::selected_text(&rows, &sel);
        assert!(
            text.trim_end().ends_with("hello world"),
            "the full displayed line's actual content must be included; got {text:?}"
        );
    }

    #[test]
    fn click_count_resets_after_window_or_cell_change() {
        let mut app = App::new_test_with_messages(vec![("assistant", "hello world")]);
        let a = area();

        // Two escalating clicks at the SAME cell reach Word granularity.
        app.handle_mouse(left_down(10, 1), a);
        app.handle_mouse(left_up(10, 1), a);
        app.handle_mouse(left_down(10, 1), a);
        assert!(!app.selection.unwrap().is_empty(), "test setup: second click must be Word");
        app.handle_mouse(left_up(10, 1), a);

        // A third click in a DIFFERENT cell must reset to count 1 (Char),
        // never escalate to Line.
        app.handle_mouse(left_down(30, 1), a);
        assert!(
            app.selection.unwrap().is_empty(),
            "a press in a different cell must reset to a char anchor, not escalate to Line"
        );

        // Directly simulate an elapsed double-click window (same cell, but
        // stale timestamp) — must also reset to Char, not escalate.
        app.last_press = Some((
            selection::ContentPos::new(0, 9),
            Instant::now() - Duration::from_millis(600),
            2,
        ));
        app.handle_mouse(left_down(10, 1), a);
        assert!(
            app.selection.unwrap().is_empty(),
            "a press outside the double-click window must reset to a char anchor, not escalate"
        );
    }

    #[test]
    fn word_selection_respects_grapheme_boundaries() {
        let mut app = App::new_test_with_messages(vec![("assistant", "a\u{1F600}b cd")]);
        let a = area();
        // "Hermes: a😀b cd" — content col 9 ('😀') -> viewport col 1+9=10.
        app.handle_mouse(left_down(10, 1), a);
        app.handle_mouse(left_up(10, 1), a);
        app.handle_mouse(left_down(10, 1), a);
        let sel = app.selection.expect("second click must select the word");

        let rows = app.transcript_rendered_plain_rows(inner_transcript_width(a));
        let text = selection::selected_text(&rows, &sel);
        assert_eq!(
            text, "a\u{1F600}b",
            "the wide glyph must be selected whole within its word, never split mid-codepoint"
        );
    }

    /// Regression guard: chip-click priority (Phase 46.7 Plan 07 / Plan 01)
    /// is unaffected by click-count granularity — a chip press still opens
    /// the chip and does not escalate/participate in the click-count.
    #[test]
    fn chip_click_priority_unregressed_by_click_count_granularity() {
        use std::sync::{Arc as StdArc, Mutex as StdMutex};

        let mut app = App::new_test_empty();
        let opened: StdArc<StdMutex<Vec<String>>> = StdArc::new(StdMutex::new(Vec::new()));
        let opened_clone = opened.clone();
        app.opener = Box::new(move |url: &str| {
            opened_clone.lock().unwrap().push(url.to_string());
            Ok(())
        });

        let a = area();
        let rect = Rect {
            x: a.x + 1,
            y: a.y + 1,
            width: 20,
            height: 1,
        };
        *app.chip_hit_test.lock().unwrap() = vec![(
            rect,
            ChipAction::OpenArtifactUrl("http://127.0.0.1:8080/artifacts/id1".to_string()),
        )];

        app.handle_mouse(left_down(rect.x + 2, rect.y), a);
        app.handle_mouse(left_up(rect.x + 2, rect.y), a);
        app.handle_mouse(left_down(rect.x + 2, rect.y), a);

        assert_eq!(
            opened.lock().unwrap().len(),
            2,
            "both presses on the chip rect must open the URL every time"
        );
        assert!(
            app.selection.unwrap().is_empty(),
            "a second press on the SAME chip rect must still be a plain char anchor \
             (Char granularity), never escalate to Word/Line"
        );
        assert!(
            app.last_press.is_none(),
            "chip presses must never seed last_press (they don't participate in click-count)"
        );
    }
}

// ── Phase 36.6.4 Plan 02 Task 3: honest copy-confirmation feedback (D-04) ──
#[cfg(all(test, feature = "test-support"))]
mod copy_confirmation_tests {
    use super::*;

    fn supported_written(copied: usize, total: usize) -> selection::ClipboardOutcome {
        selection::ClipboardOutcome::Written(
            selection::CopyReport { copied, total, native_clipboard: selection::NativeClipboardOutcome::NotAttempted },
            selection::TerminalClipboardCaps {
                support: selection::Osc52Support::Supported,
                display_name: "iTerm2",
            },
        )
    }

    #[test]
    fn copy_toast_reports_char_count_without_receipt_language() {
        let mut app = App::new_test_empty();
        app.apply_clipboard_outcome(supported_written(5, 5));

        let toast = app
            .copy_confirmation_text()
            .expect("a successful write must set a confirmation toast");
        assert_eq!(toast, "Copied 5 chars");
        assert!(!toast.contains('✓'), "must carry no checkmark: {toast:?}");
        assert!(
            !toast.to_lowercase().contains("clipboard"),
            "an UNtruncated confirmation must not mention 'clipboard' at all \
             (that word is reserved for the truncated variant): {toast:?}"
        );
    }

    #[test]
    fn copy_toast_reverts_after_window() {
        let mut app = App::new_test_empty();
        app.apply_clipboard_outcome(supported_written(5, 5));
        assert!(app.copy_confirmation_text().is_some(), "test setup: toast must be active");

        for _ in 0..App::COPY_CONFIRMATION_WINDOW_TICKS - 1 {
            app.on_tick();
        }
        assert!(
            app.copy_confirmation_text().is_some(),
            "the toast must still be showing one tick before its window elapses"
        );

        app.on_tick();
        assert!(
            app.copy_confirmation_text().is_none(),
            "the toast must revert to the normal hint once its window elapses"
        );
    }

    #[test]
    fn copy_truncation_message_carries_both_counts() {
        let mut app = App::new_test_empty();
        app.apply_clipboard_outcome(supported_written(74_000, 74_500));

        let toast = app.copy_confirmation_text().expect("truncated write must set a toast");
        assert!(toast.contains("74000") || toast.contains("74,000"), "got: {toast:?}");
        assert!(toast.contains("74500") || toast.contains("74,500"), "got: {toast:?}");
        assert!(
            toast.contains("terminal clipboard limit — truncated"),
            "must name the terminal-clipboard-limit reason: {toast:?}"
        );
    }

    #[test]
    fn copy_write_failure_renders_system_transcript_line_not_toast() {
        let mut app = App::new_test_empty();
        assert!(app.history.is_empty(), "test setup: no pre-existing history");
        let hint_before = app.status.hint.clone();

        app.apply_clipboard_outcome(selection::ClipboardOutcome::WriteFailed(std::io::Error::other(
            "broken pipe",
        )));

        assert!(
            app.copy_confirmation_text().is_none(),
            "a write failure must NEVER set a status-line toast"
        );
        assert_eq!(app.status.hint, hint_before, "the hint slot must be untouched on failure");
        let last = app.history.last().expect("a write failure must push a transcript line");
        assert_eq!(last.role, Role::System);
        match &last.content {
            Some(MessageContent::Text(t)) => {
                assert!(
                    t.starts_with("Could not copy selection: broken pipe"),
                    "got: {t:?}"
                );
            }
            other => panic!("expected Text content, got {other:?}"),
        }
    }

    #[test]
    fn copy_empty_selection_result_is_completely_silent() {
        let mut app = App::new_test_empty();
        let hint_before = app.status.hint.clone();

        app.apply_clipboard_outcome(selection::ClipboardOutcome::Empty);

        assert!(app.copy_confirmation_text().is_none());
        assert_eq!(app.status.hint, hint_before);
        assert!(app.history.is_empty(), "an empty-selection yank must never touch history");
    }

    // — Plan 08: honest wording for the Unsupported/Unknown OSC52 states,
    // and the native-outcome precedence rule (2026-08-17 amendment) ──────

    #[test]
    fn unsupported_terminal_toast_via_app_does_not_claim_a_copy() {
        let mut app = App::new_test_empty();
        app.apply_clipboard_outcome(selection::ClipboardOutcome::Written(
            selection::CopyReport { copied: 5, total: 5, native_clipboard: selection::NativeClipboardOutcome::NotAttempted },
            selection::TerminalClipboardCaps {
                support: selection::Osc52Support::Unsupported,
                display_name: "Terminal.app",
            },
        ));
        let toast = app.copy_confirmation_text().expect("must set a toast");
        assert!(!toast.starts_with("Copied"), "got {toast:?}");
        assert!(toast.contains("Terminal.app"), "got {toast:?}");
    }

    #[test]
    fn confirmed_native_write_via_app_reports_a_copy_even_when_osc52_is_unsupported() {
        let mut app = App::new_test_empty();
        app.apply_clipboard_outcome(selection::ClipboardOutcome::Written(
            selection::CopyReport { copied: 5, total: 5, native_clipboard: selection::NativeClipboardOutcome::Confirmed },
            selection::TerminalClipboardCaps {
                support: selection::Osc52Support::Unsupported,
                display_name: "Terminal.app",
            },
        ));
        let toast = app.copy_confirmation_text().expect("must set a toast");
        assert!(toast.starts_with("Copied"), "the observed native write must win: got {toast:?}");
    }
}

// ── Phase 36.6.4 Plan 03 (D-09..D-11, TUI-BANG-01): `!` dispatch + render ──

#[cfg(all(test, feature = "test-support"))]
mod shell_bang_dispatch_tests {
    use super::*;

    #[test]
    fn shell_bang_output_enters_app_history_matching_transcript() {
        let mut app = App::new_test_empty();
        let outcome = shell_bang::ShellOutcome {
            command: "echo hi".to_string(),
            stdout: "hi".to_string(),
            stderr: String::new(),
            result: shell_bang::ShellResult::Exited(0),
            truncation: None,
        };
        app.apply_shell_outcome(outcome);

        assert_eq!(app.history.len(), 1, "exactly one History entry per shell run");
        assert_eq!(app.history[0].role, Role::System);
        let body = render_message_body(&app.history[0]);

        // T-36.6.4-SHELL-02 / TUI-BANG-01 prohibition: the model-facing text
        // and the operator-visible transcript render must be the SAME
        // content, byte-identical by construction (both derive from
        // shell_bang::shell_block_plain — never re-derived independently).
        assert_eq!(app.shell_runs.len(), 1);
        let plain = shell_bang::shell_block_plain(&app.shell_runs[0]).join("\n");
        assert_eq!(body, plain, "App.history content must match the transcript render exactly");
        assert_eq!(body, "$ echo hi\nhi\n[exit 0]");

        // Double-render guard: the hidden-indices set must mark this
        // message so `transcript_text`'s normal per-message loop skips it
        // (it renders exclusively via `shell_runs`/`shell_block_lines`).
        assert!(app.shell_history_hidden_indices.contains(&0));
        let normal_render = app.transcript_text();
        assert!(
            normal_render.lines.is_empty(),
            "the shell-run System message must NOT also render via the normal \
             per-message loop: {normal_render:?}"
        );

        // The custom-styled render DOES show it exactly once.
        let full_render = app.transcript_render_text();
        let rendered_plain: Vec<String> = full_render
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect::<String>())
            .collect();
        assert_eq!(rendered_plain, vec!["$ echo hi".to_string(), "hi".to_string(), "[exit 0]".to_string()]);
    }

    #[test]
    fn shell_bang_refusal_renders_single_system_line_and_no_history_entry_for_the_command_text() {
        let mut app = App::new_test_empty();
        app.textarea.insert_str("!vim file.txt");
        app.dispatch_or_submit();

        assert_eq!(
            app.history.len(),
            1,
            "REFUSED renders exactly one System line — no Running…/exit-code lifecycle"
        );
        let msg = &app.history[0];
        assert_eq!(msg.role, Role::System, "refusal must be Role::System, never Role::User");
        let body = render_message_body(msg);
        assert!(body.contains("refused"), "got: {body:?}");

        // No shell_runs entry — refusal never spawns, never enters the
        // Running…/exit-code lifecycle.
        assert!(app.shell_runs.is_empty());

        // The raw "!vim file.txt" line itself never entered app.history as
        // its own bubble (the only entry is the constructed refusal
        // message, not an echoed User-role command line).
        assert!(
            app.history.iter().all(|m| m.role != Role::User),
            "no Role::User entry for the raw command text must exist"
        );
    }
}

// ── Phase 36.6.4 Plan 03 (D-16, TUI-HIST-01): slash/`!` recall history ────

#[cfg(all(test, feature = "test-support"))]
mod history_store_tests {
    use super::*;

    #[test]
    fn history_store_recall_includes_slash_commands() {
        let mut app = App::new_test_empty();
        app.textarea.insert_str("/help");
        app.dispatch_or_submit();

        assert!(app.textarea.is_empty(), "textarea must clear after dispatch");
        assert_eq!(
            app.history_store.prev(),
            Some("/help"),
            "Up-arrow must recall the submitted slash command"
        );
    }

    #[test]
    fn history_store_recall_includes_bang_commands() {
        let mut app = App::new_test_empty();
        app.textarea.insert_str("!echo hi");
        app.dispatch_or_submit();

        assert_eq!(
            app.history_store.prev(),
            Some("!echo hi"),
            "Up-arrow must recall the submitted `!` command"
        );
    }

    #[test]
    fn history_store_recall_never_puts_slash_or_bang_in_app_history() {
        let mut app = App::new_test_empty();
        app.textarea.insert_str("/help");
        app.dispatch_or_submit();
        app.textarea.insert_str("!echo hi");
        app.dispatch_or_submit();

        for msg in &app.history {
            let body = render_message_body(msg);
            assert_ne!(body, "/help", "raw slash line must never enter app.history verbatim");
            assert_ne!(body, "!echo hi", "raw `!` line must never enter app.history verbatim");
        }
    }

    #[test]
    fn history_store_push_is_idempotent_for_consecutive_duplicates() {
        let mut app = App::new_test_empty();
        app.textarea.insert_str("/help");
        app.dispatch_or_submit();
        app.textarea.insert_str("/help");
        app.dispatch_or_submit();

        assert_eq!(
            app.history_store.len(),
            1,
            "ReplHistory::push's existing consecutive-duplicate suppression must collapse \
             the second identical /help submission to a single entry"
        );
    }
}

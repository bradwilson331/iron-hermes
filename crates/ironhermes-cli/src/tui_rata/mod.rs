//! Ratatui-backed REPL module (Phase 22.4).
//!
//! Side-by-side with classic `tui/` per D-02. The classic module stays
//! compilable and callable until a follow-up phase deletes it.
//!
//! Submodules are added incrementally by Wave 1–3 plans; this file starts
//! with the two pure-core lifts (`knight_rider`, `double_ctrl_c`) and grows
//! as subsequent plans land `keybindings`, `status_line`, `history`,
//! `stream_events`, `app`, `event_loop`, `ui`.

pub mod app;
pub mod approval_gate_tui; // Phase 36.6.2 Plan 03 — channel-based TuiApprovalGate
pub mod clarify_dispatcher_tui; // Phase 41.1 Plan 10 (G-41.1-1) — channel-based TuiClarifyDispatcher
pub mod commands;
pub mod double_ctrl_c;
pub mod event_loop;
pub mod history;
pub mod keybindings;
pub mod knight_rider;
pub mod overlay; // Phase 36.6.2 Plan 01 — shared overlay/modal rendering
pub mod palette; // Phase 36.6.3 Plan 01 — command palette / slash menu (TUI-INPUT-01)
pub mod status_line;
pub mod stream_events;
pub mod ui;
pub mod voice_reply; // Phase 36.17.8 — spoken agent replies (auto_tts)
pub mod voice_state; // Phase 36.17.8

pub use app::App;
pub use event_loop::run_chat_ratatui;
pub use stream_events::StreamEvent;

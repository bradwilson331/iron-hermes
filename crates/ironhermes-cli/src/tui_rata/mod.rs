//! Ratatui-backed REPL module (Phase 22.4).
//!
//! Side-by-side with classic `tui/` per D-02. The classic module stays
//! compilable and callable until a follow-up phase deletes it.
//!
//! Submodules are added incrementally by Wave 1–3 plans; this file starts
//! with the two pure-core lifts (`knight_rider`, `double_ctrl_c`) and grows
//! as subsequent plans land `keybindings`, `status_line`, `history`,
//! `stream_events`, `app`, `event_loop`, `ui`.

pub mod double_ctrl_c;
pub mod history;
pub mod keybindings;
pub mod knight_rider;
pub mod status_line;
pub mod stream_events;

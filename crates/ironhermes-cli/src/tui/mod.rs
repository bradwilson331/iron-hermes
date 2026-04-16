//! Phase 21: CLI TUI polish — status line, knight-rider scanner, double-ctrl-c state machine.
//!
//! This module is intentionally split into pure-function submodules (`pills`, `knight_rider`,
//! `double_ctrl_c`, `status_line`) with I/O isolated to the (future) render task in `mod.rs`.
//! Per D-15/D-18: crossterm + colored primitives only — no new dependencies.

pub mod activity;
pub mod double_ctrl_c;
pub mod knight_rider;
pub mod pills;
pub mod status_line;

// Re-exports consumed by Plan 21-03 (run_chat integration).
pub use activity::ActivityState;
pub use double_ctrl_c::{CtrlCDecision, DoubleCtrlCState};
pub use status_line::StatusLineState;

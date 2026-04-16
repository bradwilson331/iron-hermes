//! Phase 21: CLI TUI polish — status line, knight-rider scanner, double-ctrl-c state machine.
//!
//! This module is intentionally split into pure-function submodules (`pills`, `knight_rider`,
//! `double_ctrl_c`, `status_line`) with I/O isolated to the render task in `render.rs`.
//! Per D-15/D-18: crossterm + colored primitives only — no new dependencies.
//!
//! Items in this module are wired into run_chat in Plan 21-03 (Wave 3). The allow attributes
//! below suppress dead-code warnings until that wiring is in place.
#![allow(dead_code, unused_imports)]

pub mod activity;
pub mod double_ctrl_c;
pub mod knight_rider;
pub mod pills;
pub mod render;
pub mod status_line;

// Re-exports consumed by Plan 21-03 (run_chat integration).
pub use activity::ActivityState;
pub use double_ctrl_c::{CtrlCDecision, DoubleCtrlCState};
pub use render::TuiHandle;
pub use status_line::StatusLineState;

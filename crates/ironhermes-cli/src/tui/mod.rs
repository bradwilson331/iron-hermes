//! Phase 21: CLI TUI polish — status line, knight-rider scanner, double-ctrl-c state machine.
//! Phase 22.1: TUI extension hooks — TuiExtension trait, Widget, KeybindingRegistry, CommandRegistry.
//!
//! This module is intentionally split into pure-function submodules (`pills`, `knight_rider`,
//! `double_ctrl_c`, `status_line`) with I/O isolated to the render task in `render.rs`.
//! Per D-15/D-18: crossterm + colored primitives only — no new dependencies.
//!
//! Items in this module are wired into run_chat in Plan 21-03 (Wave 3).
//! Extension contracts are wired into the render loop and REPL in Plan 22.1-02.

pub mod activity;
pub mod commands;
pub mod double_ctrl_c;
pub mod extension;
pub mod keybindings;
pub mod knight_rider;
pub mod pills;
pub mod render;
pub mod status_line;

// Re-exports consumed by Plan 21-03 (run_chat integration).
pub use activity::ActivityState;
#[allow(unused_imports)] // Used in Task 2 (ctrl-c state machine wiring)
pub use double_ctrl_c::{CtrlCDecision, DoubleCtrlCState};
pub use render::{
    TuiHandle, finish_prompt, finish_prompt_with_reserve, prepare_prompt,
    prepare_prompt_with_reserve, prompt_position_ansi, reset_terminal_visual,
    write_into_scroll_region,
};
pub use status_line::StatusLineState;

// Re-exports for Phase 22.1 extension system (consumed by Plan 22.1-02).
pub use commands::dispatch_command;
pub use extension::{
    CommandResult, KeyContext, Keybinding, LayoutSlot, StyleOverrides, TuiEvent, TuiExtension,
    Widget,
};
pub use keybindings::KeybindingRegistry;

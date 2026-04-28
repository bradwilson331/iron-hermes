//! IronHermes CLI library — exposes modules for integration test access.
//!
//! Plans 04/08/09 integration tests import modules via this lib entry
//! (e.g. `use ironhermes_cli::status_cmd::StatusReport;`). `main.rs`
//! remains the binary entry; modules that tests need are re-exported here.
//!
//! Keep the public surface narrow — only modules downstream tests
//! genuinely need. Do NOT `pub use` internals that shouldn't become
//! API.

pub mod memory_cmd;
pub mod setup;
pub mod skills_cmd;

// Phase 21.7 Wave 0 (ISS-08) — re-exports for Wave 1 Plan 04 + Wave 3 Plans 08/09:
pub mod status_cmd; // Plan 04 Task 4-01 replaces body; placeholder only in Wave 0.
pub mod tui; // Existing module (render_status_line etc.) — re-exported for Plan 07 tests.

// Phase 22.4 Plan 22.4-01: tui_rata module — ratatui-backed REPL (D-02 side-by-side).
// `tests/tui_rata_snapshots.rs` uses `use ironhermes_cli::tui_rata::{App, ui, StreamEvent}`.
pub mod tui_rata;

// `memory_setup` is intentionally NOT re-exported from the library crate.
// It references `crate::Cli` which lives in `main.rs` (the binary crate),
// so the module is compiled as part of the binary only. The integration
// tests exercise the factory + MemoryManager path directly rather than
// re-entering the binary's Cli surface.

// Phase 21.7 Plan 08 (ISS-06 / ISS-07 / ISS-08): yolo + io_gate + cli_args
// re-exports for integration tests and lib-consumers. `main.rs` imports
// from these same modules so production + test code share one code path.
pub mod cli_args;
pub mod io_gate;
pub mod yolo;

// Phase 21.7 Plan 11 (GAP-21.7-01): concurrent rustyline input channel.
// Hosts the blocking DefaultEditor on a dedicated OS thread so `run_chat`
// can poll for user input from a `tokio::select!` arm alongside the
// in-flight agent turn future (mid-turn `/agents list|kill|logs` dispatch).
pub mod repl_input;

pub use io_gate::{can_prompt, is_terminal_stdin};
pub use repl_input::{ExternalPrinterHandle, PromptRequest, ReplInputChannel, ReplLine};
pub use yolo::{maybe_print_yolo_banner, print_yolo_banner_to_stderr, resolve_yolo};

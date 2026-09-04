//! IronHermes CLI library — exposes modules for integration test access.
//!
//! Plans 04/08/09 integration tests import modules via this lib entry
//! (e.g. `use ironhermes_cli::status_cmd::StatusReport;`). `main.rs`
//! remains the binary entry; modules that tests need are re-exported here.
//!
//! Keep the public surface narrow — only modules downstream tests
//! genuinely need. Do NOT `pub use` internals that shouldn't become
//! API.

/// Process-global env lock for tests, mirroring `main.rs`'s.
///
/// `setup.rs` is compiled into BOTH this lib target and the `ironhermes` bin
/// target, so `crate::test_env_lock()` must resolve in each crate root. Each test
/// binary is its own process, so one lock per crate root is exactly one lock per
/// process — which is the point: independent per-MODULE mutexes do not serialise
/// against each other, so tests in different modules that mutate the same
/// process-global var (`OPENROUTER_API_KEY`, `IRONHERMES_HOME`, …) stomp each
/// other and flake. Every env-mutating test in this crate must take THIS lock.
#[cfg(test)]
pub(crate) fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

pub mod approval_gate;
// Phase 49.6 Plan 03: `/blueprint save`'s CLI-only BlueprintSaverImpl.
// Compiled into BOTH this lib target and the `ironhermes` bin target
// (`main.rs` declares it too, mirroring `setup.rs`'s dual-declaration
// pattern above) — `tui_rata::commands::build_command_context` wires it via
// `crate::blueprint_save`, and `tui_rata` itself lives in this lib crate.
pub mod blueprint_save;
pub mod kanban;
pub mod memory_cmd;
/// Phase 42 EXEC-01: Quick Command dispatch (guard → approval → TerminalTool, LLM-free).
pub mod quick_command;
pub mod setup;
pub mod skills_cmd;

// Phase 25 Plan 04: toolset subcommand — exported so integration tests and
// the lib consumer can call validate_toolset_name / cmd_toolset_enable without
// going through the binary subprocess for unit tests.
pub mod toolset_cmd;

// Phase 25.3 Plan 11: session subcommand — `hermes session export <id>` and
// `hermes session export-all [--since YYYY-MM-DD]` (D-F-1 / D-F-2). Exposed
// from lib.rs so unit tests can exercise resolve_output_dir + the chrono
// `--since` parser without spawning the binary.
pub mod session_cmd;

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

// Phase 36.2 Plan 09: `hermes pricing list|refresh` subcommand. Exported
// from lib.rs so the integration test (tests/pricing_cli.rs) can drive
// `cmd_list_to_string` + `cmd_refresh_from_url_with_path` without spawning
// the binary — matches the toolset_cmd / session_cmd / status_cmd pattern.
pub mod pricing_cmd;

/// WR-03 (phase 47.4): detect profiles possibly exfiltrated during the CR-03 window.
pub mod profile_audit;

pub use io_gate::{can_prompt, is_terminal_stdin};
pub use repl_input::{ExternalPrinterHandle, PromptRequest, ReplInputChannel, ReplLine};
pub use yolo::{maybe_print_yolo_banner, print_yolo_banner_to_stderr, resolve_yolo};

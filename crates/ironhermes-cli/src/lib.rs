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
pub mod skills_cmd;

// Phase 21.7 Wave 0 (ISS-08) — re-exports for Wave 1 Plan 04 + Wave 3 Plans 08/09:
pub mod status_cmd; // Plan 04 Task 4-01 replaces body; placeholder only in Wave 0.
pub mod tui; // Existing module (render_status_line etc.) — re-exported for Plan 07 tests.

// `memory_setup` is intentionally NOT re-exported from the library crate.
// It references `crate::Cli` which lives in `main.rs` (the binary crate),
// so the module is compiled as part of the binary only. The integration
// tests exercise the factory + MemoryManager path directly rather than
// re-entering the binary's Cli surface.

// Intentional: do NOT re-export main-only internals
// (resolve_yolo, maybe_print_yolo_banner, io_gate, etc.). Plan 08
// promotes those to `pub` AFTER this file exists and will add them
// here (or via a `pub mod yolo;` wrapper) at that time.

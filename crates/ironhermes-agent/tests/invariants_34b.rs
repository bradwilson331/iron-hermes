//! Phase 34b: cross-surface wiring invariant suite.
//!
//! This integration-test file is the Wave 0 scaffold (Plan 00). Waves 1 and 2
//! replace the placeholder with real grep-gate tests that assert:
//!
//!   - The 3-surface `preprocess_context_references_async` wiring (Wave 1)
//!   - The `run_turn`-centralization source guard (Wave 2)
//!
//! Source-guard convention: load source text at compile time with `include_str!`
//! so invariants run offline without I/O and catch refactor regressions at
//! `cargo test` time (same pattern as `invariants_33.rs`).
//!
//! Example source anchors Wave 1/2 will use:
//! ```rust
//! const HANDLER_SOURCE: &str = include_str!("../../iron_hermes_ui/src/server/handler.rs");
//! const STATE_SOURCE: &str = include_str!("../../iron_hermes_ui/src/server/state.rs");
//! const MAIN_SOURCE: &str = include_str!("../../ironhermes-cli/src/main.rs");
//! ```

/// Placeholder: Wave 1/2 replace this with the 3-surface preprocess
/// wiring grep-gate and the run_turn-centralization source guard.
#[test]
#[ignore]
fn placeholder_34b_wiring() {
    // Wave 1/2 replace this with the 3-surface preprocess
    // wiring grep-gate and the run_turn-centralization source guard.
}

//! Phase 34b: cross-surface wiring invariant suite (Plan 01).
//!
//! Source-guard convention: load source text at compile time with `include_str!`
//! so invariants run offline without I/O and catch refactor regressions at
//! `cargo test` time (same pattern as `invariants_33.rs`).
//!
//! This file replaces the Wave-0 placeholder with two concrete guards:
//!
//!   (a) `preprocess_context_references_async` appears in `agent_runtime.rs`
//!       BEFORE the `attach_context_engine(` call (centralization-before-engine invariant).
//!
//!   (b) `preprocess_context_references_async` does NOT appear in the three
//!       surface files (handler.rs, state.rs, main.rs) — centralization invariant.

const RUNTIME_SOURCE: &str =
    include_str!("../src/agent_runtime.rs");

// handler.rs lives in ironhermes-gateway (not iron_hermes_ui) per project layout.
const HANDLER_SOURCE: &str =
    include_str!("../../ironhermes-gateway/src/handler.rs");

const STATE_SOURCE: &str =
    include_str!("../../iron_hermes_ui/src/server/state.rs");

const MAIN_SOURCE: &str =
    include_str!("../../ironhermes-cli/src/main.rs");

/// (a) Centralization-before-engine invariant:
/// The byte offset of `preprocess_context_references_async` in agent_runtime.rs
/// must be less than the byte offset of `attach_context_engine(`.
#[test]
fn preprocess_before_attach_context_engine_in_run_turn() {
    let preprocess_pos = RUNTIME_SOURCE
        .find("preprocess_context_references_async")
        .expect("preprocess_context_references_async must appear in agent_runtime.rs");

    let attach_pos = RUNTIME_SOURCE
        .find("attach_context_engine(")
        .expect("attach_context_engine( must appear in agent_runtime.rs");

    assert!(
        preprocess_pos < attach_pos,
        "preprocess_context_references_async (offset {}) must appear BEFORE \
         attach_context_engine( (offset {}) in agent_runtime.rs",
        preprocess_pos,
        attach_pos
    );
}

/// (b) Centralization invariant: `preprocess_context_references_async` must NOT
/// appear in any of the three surface files (handler.rs, state.rs, main.rs).
#[test]
fn preprocess_not_called_in_surfaces() {
    let token = "preprocess_context_references_async";

    assert!(
        !HANDLER_SOURCE.contains(token),
        "handler.rs must not call {} (centralization violated)",
        token
    );
    assert!(
        !STATE_SOURCE.contains(token),
        "state.rs must not call {} (centralization violated)",
        token
    );
    assert!(
        !MAIN_SOURCE.contains(token),
        "main.rs must not call {} (centralization violated)",
        token
    );
}

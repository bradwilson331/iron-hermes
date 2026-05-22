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

// ── Phase 34b Plan 02 (Task 3): per-turn hook loci + CLI session reset ───────

/// (c) `update_model` is wired in run_turn this phase (D-07, no hedge).
#[test]
fn update_model_present_in_run_turn() {
    assert!(
        RUNTIME_SOURCE.contains("update_model"),
        "agent_runtime.rs must call engine.update_model (D-07 per-turn model hook)"
    );
}

/// (d) `update_from_response` is the POST-run per-turn usage locus: its byte
/// offset MUST be greater than `agent.run(` so the hook sees the turn's usage.
#[test]
fn update_from_response_after_agent_run_in_run_turn() {
    let run_pos = RUNTIME_SOURCE
        .find("agent.run(")
        .expect("agent.run( must appear in agent_runtime.rs");

    // The post-run hook call site: search after the run position.
    let update_pos = RUNTIME_SOURCE
        .match_indices("update_from_response")
        .map(|(i, _)| i)
        .find(|&i| i > run_pos)
        .expect("update_from_response must appear AFTER agent.run( in agent_runtime.rs");

    assert!(
        update_pos > run_pos,
        "update_from_response (offset {}) must appear AFTER agent.run( (offset {}) \
         so the post-run usage hook fires on the turn's total_usage",
        update_pos,
        run_pos
    );
}

/// (e) CLI `/new` (ClearSession arm) resets the durable per-session
/// compression_count Arc<AtomicUsize> to 0 (D-09/D-10 surface reset locus).
#[test]
fn cli_clear_session_resets_compression_count() {
    // The ClearSession arm must contain a `compression_count.store(0` reset.
    let clear_pos = MAIN_SOURCE
        .find("ClearSession(output)")
        .expect("ClearSession(output) arm must appear in main.rs");

    let tail = &MAIN_SOURCE[clear_pos..];
    // Bound the search to a window after the arm to keep it local to /new.
    let window_end = tail.len().min(800);
    let window = &tail[..window_end];

    assert!(
        window.contains("compression_count.store(0"),
        "CLI ClearSession (/new) arm must reset compression_count.store(0, ...) \
         (Phase 34b D-09/D-10 surface session-reset locus)"
    );
}

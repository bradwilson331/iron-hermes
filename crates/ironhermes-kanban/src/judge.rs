//! Judge kernel for Phase 36.3.7.12 Goal Mode.
//!
//! Exposes the [`JudgeFn`] injection-point typedef plus the
//! [`JudgeRequest`] / [`JudgeOutput`] / [`JudgeVerdict`] data shapes that
//! the goal-mode worker loop (Plan 04) and the LLM-call site (Plan 03) both
//! consume.
//!
//! **No LLM call lives here.** This file is the contract surface only —
//! the production `build_runtime_judge_fn` that wires a real provider client
//! lives in `ironhermes-cli/src/kanban/commands.rs` per the locked
//! crate-isolation fence (CONTEXT.md §canonical_refs / Pitfall 2 in
//! RESEARCH.md). The pattern mirrors [`crate::decomposer::DecomposeFn`].
//!
//! # Why mirror DecomposeFn exactly
//!
//! Closure-injection over plugin loader (PROJECT.md `Key Decisions` +
//! `project_plugin_loader_rejected` memory). Phase 36.3.7.10 established the
//! shape for `DecomposeFn` at `decomposer.rs:141-142`; this file copies that
//! shape one-for-one so that the goal-loop wrapper can store a
//! `JudgeFn` next to a `DecomposeFn` without surprise lifetime or `Send`
//! mismatches.

use std::sync::Arc;

use crate::error::Result;

// ---------------------------------------------------------------------------
// BoxFuture alias — mirrors decomposer.rs:58 exactly.
// ---------------------------------------------------------------------------

type BoxFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>;

// ---------------------------------------------------------------------------
// Public data structures
// ---------------------------------------------------------------------------

/// Input payload passed to the injected [`JudgeFn`] closure.
///
/// All fields are owned (`String` / `u32`) so the closure can be invoked
/// without holding any store mutex — matches the
/// [`crate::decomposer::DecomposeRequest`] convention.
#[derive(Debug, Clone)]
pub struct JudgeRequest {
    /// DB id of the task being evaluated (e.g. `t_abc1234`).
    pub task_id: String,
    /// Card title — the first half of the literal acceptance criteria pair
    /// passed to the judge system prompt (D-01).
    pub title: String,
    /// Card body — the literal acceptance criteria the judge evaluates
    /// the worker's output against (CONTEXT.md D-01: body is load-bearing,
    /// not advisory).
    pub body: String,
    /// The worker's most recent turn output (textual response + any tool
    /// calls' visible footprint) the judge weighs against `title + body`.
    pub worker_turn_output: String,
    /// 1-indexed turn number within the current goal-mode session
    /// (i.e. the `turn` value of the iteration that just completed).
    /// Used for logging and `judge_verdict` event payloads.
    pub turn: u32,
}

/// Boolean verdict emitted by the judge.
///
/// Plan 03's `build_runtime_judge_fn` parses the LLM's JSON response shape
/// `{verdict: "met" | "not_met", reason: "..."}` into this enum + the
/// [`JudgeOutput::reason`] string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JudgeVerdict {
    /// Acceptance criteria are satisfied → loop exits cleanly; worker
    /// proceeds to call `kanban_complete` (D-02).
    Met,
    /// Acceptance criteria not yet met → worker continues to the next
    /// iteration until budget exhausted (D-02 / D-03).
    NotMet,
}

/// Structured output returned by the [`JudgeFn`] closure.
#[derive(Debug, Clone)]
pub struct JudgeOutput {
    pub verdict: JudgeVerdict,
    /// Free-form human-readable rationale string. Propagated into the next
    /// turn's synthetic user message ("Judge verdict: not met. Reason: …")
    /// when verdict is `NotMet`, and into the `judge_verdict` event payload
    /// per D-04.
    pub reason: String,
}

// ---------------------------------------------------------------------------
// JudgeFn typedef — mirrors DecomposeFn at decomposer.rs:141-142 exactly.
// ---------------------------------------------------------------------------

/// Injectable LLM judge call.
///
/// Production binding: configured by Plan 03's `build_runtime_judge_fn` in
/// `ironhermes-cli/src/kanban/commands.rs`, three-tier provider cascade
/// `kanban.judge_model` → `auxiliary.kanban_judge` → main provider.
/// Tests: a fixture closure returning a fixed [`JudgeOutput`].
///
/// The closure takes an **owned** [`JudgeRequest`] (no borrowed args) so no
/// lifetime annotation is needed. The `BoxFuture` alias mirrors
/// [`crate::decomposer::DecomposeFn`] precisely; do not introduce a
/// different `Send` / lifetime shape here without updating the goal-loop
/// wrapper in lockstep.
pub type JudgeFn = Arc<dyn Fn(JudgeRequest) -> BoxFuture<Result<JudgeOutput>> + Send + Sync>;

// ---------------------------------------------------------------------------
// Test anchor
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-anchor: a refactor that breaks the JudgeFn typedef must fail
    /// compilation here, not at the more remote Plan 03 / Plan 04 callsites.
    #[test]
    fn judge_fn_typedef_compiles() {
        let _: Option<JudgeFn> = None;
    }

    /// JudgeVerdict carries the two locked variants per CONTEXT.md D-04.
    #[test]
    fn judge_verdict_variants() {
        assert_ne!(JudgeVerdict::Met, JudgeVerdict::NotMet);
        // Round-trip via the derived Clone + PartialEq.
        let v = JudgeVerdict::Met;
        assert_eq!(v.clone(), JudgeVerdict::Met);
    }
}

//! Judge kernel for goal-mode evaluation loops.
//!
//! Exposes the [`JudgeFn`] injection-point typedef plus the
//! [`JudgeRequest`] / [`JudgeOutput`] / [`JudgeVerdict`] data shapes that
//! any goal-mode loop consumes to decide whether a worker turn satisfied
//! its acceptance criteria.
//!
//! **No LLM call lives here.** This file is the contract surface only —
//! the production judge-builder that wires a real provider client lives in
//! `ironhermes-agent/src/judge_builder.rs`. The pattern mirrors
//! `DecomposeFn`'s closure-injection shape (closure-injection over plugin
//! loader, per `project_plugin_loader_rejected`).
//!
//! # Why this module lives in `ironhermes-core`
//!
//! Phase 49.7 D-03: moved out of `ironhermes-kanban` so `ironhermes-agent`
//! (which has no `ironhermes-kanban` dependency) can build and consume a
//! `JudgeFn` for the session-level goal loop. `ironhermes-kanban`
//! re-exports these types from its own `judge` module (a thin shim) so its
//! existing call sites keep compiling unchanged.

use std::sync::Arc;

use thiserror::Error;

// ---------------------------------------------------------------------------
// BoxFuture alias — mirrors the move source exactly.
// ---------------------------------------------------------------------------

type BoxFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>;

// ---------------------------------------------------------------------------
// JudgeError (D-04) — the sole substantive change the moved contract
// receives. `ironhermes-core` must never depend on SQLite, so this cannot
// be `ironhermes-kanban::KanbanError` (which carries `rusqlite::Error`).
// ---------------------------------------------------------------------------

/// Errors produced while building or invoking a [`JudgeFn`].
///
/// `ironhermes-kanban` converts this into its own `KanbanError::Judge`
/// variant via `#[from]` so kanban-side call sites keep returning
/// `KanbanError` (D-04).
#[derive(Debug, Error)]
pub enum JudgeError {
    /// The underlying LLM/provider call failed (transport error, non-2xx
    /// response, etc.) before a response body was available to parse.
    #[error("judge provider call failed: {0}")]
    Provider(String),

    /// The judge's response body could not be parsed as JSON.
    #[error("judge response not JSON: {source} (raw: {preview})")]
    ResponseNotJson {
        /// The underlying JSON parse error.
        #[source]
        source: serde_json::Error,
        /// Bounded (200-char, char-boundary-safe) preview of the raw
        /// response body — never the full body (information-disclosure
        /// mitigation: a provider-controlled value must never propagate
        /// unbounded into an error that is later formatted into a kanban
        /// event payload or a chat reply).
        preview: String,
    },

    /// The judge's JSON response was missing the required `verdict` field.
    #[error("judge response missing 'verdict' field (raw: {preview})")]
    MissingVerdict {
        /// Bounded preview of the raw response body.
        preview: String,
    },

    /// The judge's `verdict` field held a value other than `"met"` or
    /// `"not_met"`.
    #[error("judge verdict not in {{met,not_met}}: {literal} (raw: {preview})")]
    UnrecognizedVerdict {
        /// The literal string the judge returned in the `verdict` field.
        literal: String,
        /// Bounded preview of the raw response body.
        preview: String,
    },

    /// Catch-all for ad-hoc errors a caller wants to convert directly.
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

/// Convenience alias local to this module — mirrors the `Result<T, E =
/// KanbanError>` idiom at `ironhermes-kanban/src/error.rs:80`.
pub type Result<T, E = JudgeError> = std::result::Result<T, E>;

// ---------------------------------------------------------------------------
// Public data structures
// ---------------------------------------------------------------------------

/// Input payload passed to the injected [`JudgeFn`] closure.
///
/// All fields are owned (`String` / `u32`) so the closure can be invoked
/// without holding any store mutex.
#[derive(Debug, Clone)]
pub struct JudgeRequest {
    /// Identifier of the task/session turn being evaluated.
    pub task_id: String,
    /// Title — the first half of the literal acceptance criteria pair
    /// passed to the judge system prompt.
    pub title: String,
    /// Body — the literal acceptance criteria the judge evaluates the
    /// worker's output against.
    pub body: String,
    /// The worker's most recent turn output (textual response + any tool
    /// calls' visible footprint) the judge weighs against `title + body`.
    pub worker_turn_output: String,
    /// 1-indexed turn number within the current goal-mode loop (i.e. the
    /// `turn` value of the iteration that just completed). Used for
    /// logging and verdict event payloads.
    pub turn: u32,
}

/// Boolean verdict emitted by the judge.
///
/// A production judge builder parses the LLM's JSON response shape
/// `{verdict: "met" | "not_met", reason: "..."}` into this enum + the
/// [`JudgeOutput::reason`] string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JudgeVerdict {
    /// Acceptance criteria are satisfied — the loop exits cleanly.
    Met,
    /// Acceptance criteria not yet met — the loop continues to the next
    /// iteration until budget exhausted.
    NotMet,
}

/// Structured output returned by the [`JudgeFn`] closure.
#[derive(Debug, Clone)]
pub struct JudgeOutput {
    pub verdict: JudgeVerdict,
    /// Free-form human-readable rationale string. Propagated into the next
    /// turn's synthetic user message ("Judge verdict: not met. Reason: …")
    /// when verdict is `NotMet`, and into the loop's verdict event/log.
    pub reason: String,
}

// ---------------------------------------------------------------------------
// JudgeFn typedef — mirrors the move source's shape exactly, only the
// `Result` alias's error type changed (D-04).
// ---------------------------------------------------------------------------

/// Injectable LLM judge call.
///
/// The closure takes an **owned** [`JudgeRequest`] (no borrowed args) so no
/// lifetime annotation is needed.
///
/// D-05: both goal loops that construct a `JudgeFn` — the kanban card-level
/// loop in `ironhermes-cli/src/kanban/goal_loop.rs` and the session loop in
/// `ironhermes-agent/src/goal_session_loop.rs` — must reference
/// [`GOAL_DEFAULT_MAX_TURNS`] and [`GOAL_JUDGE_ERROR_STRIKES`] rather than
/// re-deriving a literal, so the two loops cannot silently disagree on
/// shared budget/retry semantics.
pub type JudgeFn = Arc<dyn Fn(JudgeRequest) -> BoxFuture<Result<JudgeOutput>> + Send + Sync>;

// ---------------------------------------------------------------------------
// D-05 shared constants — named so both goal loops agree by construction
// rather than by discipline.
// ---------------------------------------------------------------------------

/// Default maximum number of turns a goal-mode loop runs before budget
/// exhaustion (D-05). Both the kanban card-level loop
/// (`ironhermes-cli/src/kanban/goal_loop.rs`) and the session loop
/// (`ironhermes-agent/src/goal_session_loop.rs`) must reference this
/// constant instead of a bare literal.
pub const GOAL_DEFAULT_MAX_TURNS: u32 = 20;

/// Number of consecutive judge failures a goal-mode loop tolerates before
/// giving up and reporting failure to the caller (D-05). Same cross-loop
/// agreement requirement as [`GOAL_DEFAULT_MAX_TURNS`].
pub const GOAL_JUDGE_ERROR_STRIKES: u32 = 2;

// ---------------------------------------------------------------------------
// Test anchors
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-anchor: a refactor that breaks the JudgeFn typedef must fail
    /// compilation here, not at a more remote consumer callsite.
    #[test]
    fn judge_fn_typedef_compiles() {
        let _: Option<JudgeFn> = None;
    }

    /// JudgeVerdict carries the two locked variants and derives Debug,
    /// Clone, PartialEq, Eq.
    #[test]
    fn judge_verdict_variants() {
        assert_ne!(JudgeVerdict::Met, JudgeVerdict::NotMet);
        // Round-trip via the derived Clone + PartialEq.
        let v = JudgeVerdict::Met;
        assert_eq!(v.clone(), JudgeVerdict::Met);
    }

    /// `JudgeError` implements `std::error::Error` + `Display`, and
    /// formatting a `ResponseNotJson` value with the alternate flag still
    /// yields a string containing the preview it was constructed with.
    #[test]
    fn judge_error_response_not_json_display_contains_preview() {
        let source = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        let err: JudgeError = JudgeError::ResponseNotJson {
            source,
            preview: "not json".to_string(),
        };
        // Confirm the std::error::Error impl exists (compiles) and Display
        // renders the preview.
        let _: &dyn std::error::Error = &err;
        let rendered = format!("{err:#}");
        assert!(
            rendered.contains("not json"),
            "expected preview in rendered error, got: {rendered}"
        );
    }

    /// D-05: the two shared constants hold their locked values.
    #[test]
    fn goal_constants_have_expected_values() {
        assert_eq!(GOAL_DEFAULT_MAX_TURNS, 20);
        assert_eq!(GOAL_JUDGE_ERROR_STRIKES, 2);
    }
}

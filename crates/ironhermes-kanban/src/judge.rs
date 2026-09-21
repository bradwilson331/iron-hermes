//! Judge kernel re-export shim.
//!
//! Phase 49.7 D-03: the judge contract (`JudgeFn`, `JudgeRequest`,
//! `JudgeVerdict`, `JudgeOutput`, `JudgeError`) moved to `ironhermes-core`
//! so `ironhermes-agent` — which has no `ironhermes-kanban` dependency —
//! could reach it for the session-level goal loop, without either crate
//! depending on the other. This shim exists so the crate-root `pub use` at
//! `lib.rs:70` and its downstream consumers need no edit: `judge` is still
//! a module here, it just re-exports the types instead of defining them.

pub use ironhermes_core::judge::{
    GOAL_DEFAULT_MAX_TURNS, GOAL_JUDGE_ERROR_STRIKES, JudgeError, JudgeFn, JudgeOutput,
    JudgeRequest, JudgeVerdict,
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-anchor: `ironhermes_kanban::JudgeFn` and
    /// `ironhermes_core::judge::JudgeFn` must be the SAME type, not two
    /// structurally similar aliases — this only compiles if a value
    /// produced as one can be returned typed as the other with no
    /// conversion.
    #[test]
    fn kanban_judge_fn_is_identical_to_core_judge_fn() {
        fn identity(f: ironhermes_core::judge::JudgeFn) -> JudgeFn {
            f
        }
        let _ = identity as fn(ironhermes_core::judge::JudgeFn) -> JudgeFn;
    }
}

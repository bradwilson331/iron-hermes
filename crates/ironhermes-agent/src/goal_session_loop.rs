//! Phase 49.7 Plan 05 (D-02) — session-level `/goal` loop.
//!
//! `/goal <text> [--budget N]` gets its **own** loop here, reusing only the
//! judge contract moved into `ironhermes_core::judge` (Plan 02) and the
//! lifted judge builder (Plan 03). `ironhermes-cli/src/kanban/goal_loop.rs`
//! — the kanban card-level goal loop — is a reference only. D-02 forbids
//! modifying, refactoring, or generalizing that file; this module
//! reimplements the subset of its behavior that has meaning in a chat
//! session and drops the rest.
//!
//! ## Why a second loop implementation (D-02)
//!
//! `run_goal_loop_if_enabled` (`goal_loop.rs:180`) takes `task_id`, `run_id`,
//! `claim_lock` and `Arc<TokioMutex<KanbanStore>>`, and two of its five
//! store touchpoints have no meaning in a chat session:
//! - `bump_goal_turn_counter`'s CAS claim/reclaim — there are no competing
//!   workers in a chat session.
//! - Self-termination via externally-mutated `task.status` — nobody else
//!   moves a chat session to "done".
//!
//! This module therefore reimplements exactly **three** of the kanban
//! loop's five behavioral paths: judge-met, budget exhaustion, and
//! two-strike judge error. The dropped paths (non-goal passthrough,
//! self-termination) have no session analog: every `/goal` invocation
//! enters this loop (there is no `GOAL_MODE` env gate to bypass), and a
//! chat session has no `task.status` another actor can move.
//!
//! ## D-05 — shared budget/retry constants
//!
//! Both this loop and `goal_loop.rs` reference
//! [`ironhermes_core::judge::GOAL_DEFAULT_MAX_TURNS`] and
//! [`ironhermes_core::judge::GOAL_JUDGE_ERROR_STRIKES`] rather than
//! re-deriving a bare literal, so the two loops cannot silently disagree on
//! shared budget/retry semantics. `crates/ironhermes-cli/tests/goal_constants_pin.rs`
//! mechanically guards both files.
//!
//! ## RAII terminal-event guarantee
//!
//! [`BudgetSentinel`] mirrors `goal_loop.rs`'s `BudgetSentinel` RAII
//! drop-guard, but against the [`GoalProgress`] sink instead of a kanban
//! store: it emits exactly one `GoalProgress::Finished` on scope exit
//! unless the loop already emitted its own terminal event and set
//! `done = true` first. This guarantees "exactly one Finished per terminal
//! path" even for an early `?`-propagated error.

use std::pin::Pin;

use anyhow::Context;
use ironhermes_core::judge::{
    GOAL_DEFAULT_MAX_TURNS, GOAL_JUDGE_ERROR_STRIKES, JudgeFn, JudgeOutput, JudgeRequest,
    JudgeVerdict,
};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

/// Boxed, pinned future alias — mirrors `goal_loop.rs::GoalLoopFuture` and the
/// kanban `TurnRunner`'s narrow-closure contract exactly.
pub type GoalSessionFuture<T> = Pin<Box<dyn std::future::Future<Output = T> + Send + 'static>>;

/// Per-turn agent runner for the session-level goal loop.
///
/// Deliberately narrow (`Vec<String> -> String`), mirroring `goal_loop.rs`'s
/// `TurnRunner`, so `ChatMessage`/`AgentResult` never leak into this
/// signature — tests need no agent state, and production callers (Plan 05
/// Task 3) wrap the surface's own per-message `TurnRequest` construction.
///
/// # The `Vec<String>` contract (six points — cross-AI review, codex HIGH)
///
/// 1. Every element is USER-role text. There are no assistant entries and
///    no role tags; the vector never carries the model's own output back in.
/// 2. Element 0 is the objective, exactly as `cmd_goal` parsed it, with the
///    `--budget` flag text already removed.
/// 3. Each subsequent element is one synthetic continuation this loop
///    appended after a `NotMet` verdict, in judge order.
/// 4. **The runner sends only the LAST element as the new user message for
///    this turn.** Prior elements are history the surface's session
///    already holds — `AgentRuntime::run_turn` appends to the running
///    session, exactly as `goal_loop.rs`'s module header records for the
///    kanban loop. Re-sending the whole vector every iteration would
///    duplicate the objective into the context once per turn and grow the
///    prompt quadratically.
/// 5. Assistant outputs are retained by the session, not by this vector.
///    The loop keeps only the most recent output, in
///    [`GoalLoopOutcome::last_output`], for the judge and the closing
///    message.
/// 6. The vector is passed by clone each iteration, so the runner may
///    consume it freely.
pub type GoalTurnRunner =
    Box<dyn FnMut(Vec<String>) -> GoalSessionFuture<anyhow::Result<String>> + Send>;

/// Why a `/goal` session loop stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalStopReason {
    /// The judge returned `Met`.
    JudgeMet { reason: String },
    /// The loop ran `max_turns` iterations without a `Met` verdict.
    BudgetExhausted,
    /// The judge returned `Err` [`GOAL_JUDGE_ERROR_STRIKES`] times in a row.
    JudgeErrorStrikes { last_error: String },
    /// The shared `CancellationToken` was observed cancelled between
    /// iterations (e.g. a `/stop` reached this session's registered turns).
    Cancelled,
}

/// Terminal result of a session-level `/goal` loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalLoopOutcome {
    pub reason: GoalStopReason,
    /// Number of turn-runner invocations that actually ran before the loop
    /// stopped.
    pub turns_used: u32,
    /// The most recent turn-runner output, retained for the judge and the
    /// closing message a surface renders (the `Vec<String>` contract does
    /// NOT carry assistant output — see point 5 on [`GoalTurnRunner`]).
    pub last_output: String,
}

/// Per-iteration events a surface renders live as the loop progresses.
///
/// The sink is a `tokio::sync::mpsc::UnboundedSender` specifically so
/// [`BudgetSentinel`]'s `Drop` can send without blocking, which the RAII
/// pattern below requires (Drop must never `.await`).
#[derive(Debug)]
pub enum GoalProgress {
    /// A new turn is about to run.
    TurnStarted { turn: u32 },
    /// The turn runner returned its output for this turn.
    TurnCompleted { turn: u32, output: String },
    /// The judge rendered a verdict for this turn.
    JudgeVerdict { turn: u32, met: bool, reason: String },
    /// The judge call itself failed (bounded message — never the raw,
    /// unbounded provider body; mirrors `JudgeError`'s own 200-char
    /// preview bound).
    JudgeError { turn: u32, message: String },
    /// Terminal event. Emitted exactly once per loop invocation — either by
    /// the loop body itself (judge-met, judge-error-strikes, cancelled) or,
    /// for natural budget exhaustion, by [`BudgetSentinel`]'s `Drop`.
    Finished(GoalStopReason),
}

// ---------------------------------------------------------------------------
// BudgetSentinel — RAII guard for the "exactly one Finished" invariant.
// ---------------------------------------------------------------------------

/// Reimplements `goal_loop.rs`'s `BudgetSentinel` RAII drop-guard against
/// the [`GoalProgress`] sink instead of a kanban store handle.
///
/// Happy paths (judge-met, judge-error-strikes, cancelled) set `done = true`
/// and send their own `Finished` event before returning. Natural budget
/// exhaustion (the loop runs out its `max_turns` without ever setting
/// `done`) lets `Drop` fire instead — this is the mechanism that makes
/// "exactly one Finished on every terminal path" true even for an early
/// `?`-propagated error, which is why the mutation check below matters:
/// remove a `done = true` assignment and a HAPPY path would ALSO get a
/// second, spurious `Finished(BudgetExhausted)` from `Drop`.
struct BudgetSentinel {
    sink: UnboundedSender<GoalProgress>,
    done: bool,
}

impl Drop for BudgetSentinel {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        // Rust's do-not-panic-from-Drop rule: swallow every send error.
        let _ = self
            .sink
            .send(GoalProgress::Finished(GoalStopReason::BudgetExhausted));
    }
}

// ---------------------------------------------------------------------------
// title_from_objective — JudgeRequest.title derivation.
// ---------------------------------------------------------------------------

/// Derives `JudgeRequest.title` from the objective's first line, bounded to
/// 200 chars via a char-boundary-safe `.chars().take(200)` so a very long
/// single-line objective cannot make the title unboundedly large (mirrors
/// the 200-char preview bound `JudgeError` already uses elsewhere in this
/// contract).
fn title_from_objective(objective: &str) -> String {
    let first_line = objective.lines().next().unwrap_or_default();
    first_line.chars().take(200).collect()
}

// ---------------------------------------------------------------------------
// run_goal_session_loop — the load-bearing entry point.
// ---------------------------------------------------------------------------

/// Run a budget-bounded, judge-evaluated multi-turn session loop.
///
/// `max_turns: None` resolves to [`GOAL_DEFAULT_MAX_TURNS`] — the surface
/// passes `budget` (the `/goal --budget N` value) straight through here
/// rather than pre-resolving it, so this file (not the surface) is the
/// single place that references the D-05 default constant.
///
/// See [`GoalTurnRunner`]'s doc comment for the full six-point `Vec<String>`
/// contract this loop maintains: element 0 is the objective, every element
/// is user-role text, only the last element is a NEW synthetic
/// continuation, assistant output is retained in
/// [`GoalLoopOutcome::last_output`] rather than in the vector, and the
/// vector is cloned per iteration.
pub async fn run_goal_session_loop(
    mut turn_runner: GoalTurnRunner,
    objective: String,
    session_id: String,
    max_turns: Option<u32>,
    judge_fn: &JudgeFn,
    cancel_token: CancellationToken,
    progress: UnboundedSender<GoalProgress>,
) -> anyhow::Result<GoalLoopOutcome> {
    let max_turns = max_turns.unwrap_or(GOAL_DEFAULT_MAX_TURNS);
    let title = title_from_objective(&objective);

    let mut sentinel = BudgetSentinel {
        sink: progress.clone(),
        done: false,
    };

    // Point 2: element 0 is the objective, exactly as parsed.
    let mut messages: Vec<String> = vec![objective.clone()];
    let mut consecutive_judge_errors: u32 = 0;
    let mut last_output = String::new();

    for turn in 0..max_turns {
        // Checked FIRST, every iteration, before the turn runner is called
        // (per-iteration guard) — this is what makes a token cancelled
        // during turn 1 stop the loop before turn 2 starts.
        if cancel_token.is_cancelled() {
            let outcome = GoalLoopOutcome {
                reason: GoalStopReason::Cancelled,
                turns_used: turn,
                last_output: last_output.clone(),
            };
            let _ = progress.send(GoalProgress::Finished(outcome.reason.clone()));
            sentinel.done = true;
            return Ok(outcome);
        }

        let turn_number = turn + 1;
        let _ = progress.send(GoalProgress::TurnStarted { turn: turn_number });

        // Point 6: cloned per iteration so the runner may consume freely.
        let worker_output = turn_runner(messages.clone()).await.with_context(|| {
            format!("goal_session_loop turn {turn_number}: turn runner failed")
        })?;
        last_output = worker_output.clone();
        let _ = progress.send(GoalProgress::TurnCompleted {
            turn: turn_number,
            output: worker_output.clone(),
        });

        let req = JudgeRequest {
            task_id: session_id.clone(),
            title: title.clone(),
            body: objective.clone(),
            worker_turn_output: worker_output,
            turn: turn_number,
        };

        match judge_fn(req).await {
            Ok(JudgeOutput {
                verdict: JudgeVerdict::Met,
                reason,
            }) => {
                let _ = progress.send(GoalProgress::JudgeVerdict {
                    turn: turn_number,
                    met: true,
                    reason: reason.clone(),
                });
                let outcome = GoalLoopOutcome {
                    reason: GoalStopReason::JudgeMet { reason },
                    turns_used: turn_number,
                    last_output: last_output.clone(),
                };
                let _ = progress.send(GoalProgress::Finished(outcome.reason.clone()));
                sentinel.done = true;
                return Ok(outcome);
            }
            Ok(JudgeOutput {
                verdict: JudgeVerdict::NotMet,
                reason,
            }) => {
                let _ = progress.send(GoalProgress::JudgeVerdict {
                    turn: turn_number,
                    met: false,
                    reason: reason.clone(),
                });
                // A single error between successes resets the counter
                // (strike-reset behavior).
                consecutive_judge_errors = 0;
                // Point 3: ONE synthetic continuation per NotMet verdict.
                // Wording copied from goal_loop.rs:339-346 so both loops
                // read alike to an LLM.
                messages.push(format!(
                    "Judge verdict: not met. Reason: {}\nContinue working toward the acceptance criteria.",
                    reason,
                ));
            }
            Err(e) => {
                // 200-char, char-boundary-safe preview — never the
                // unbounded provider body (mirrors JudgeError's own bound).
                let snippet: String = format!("{:#}", e).chars().take(200).collect();
                let _ = progress.send(GoalProgress::JudgeError {
                    turn: turn_number,
                    message: snippet.clone(),
                });
                consecutive_judge_errors += 1;
                // The worker never sees judge errors as text (no push onto
                // `messages` on this branch).
                if consecutive_judge_errors >= GOAL_JUDGE_ERROR_STRIKES {
                    let outcome = GoalLoopOutcome {
                        reason: GoalStopReason::JudgeErrorStrikes { last_error: snippet },
                        turns_used: turn_number,
                        last_output: last_output.clone(),
                    };
                    let _ = progress.send(GoalProgress::Finished(outcome.reason.clone()));
                    sentinel.done = true;
                    return Ok(outcome);
                }
            }
        }
    }

    // Loop exited naturally: budget exhausted. `sentinel` is NOT marked
    // done here — its Drop (fired as this function returns) emits the
    // terminal Finished(BudgetExhausted) event, mirroring goal_loop.rs's
    // own "the Drop is the load-bearing emission site" comment.
    let outcome = GoalLoopOutcome {
        reason: GoalStopReason::BudgetExhausted,
        turns_used: max_turns,
        last_output,
    };
    Ok(outcome)
}

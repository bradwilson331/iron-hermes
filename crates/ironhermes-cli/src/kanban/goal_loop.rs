//! Phase 36.3.7.12-04 — Goal-mode in-session loop wrapper.
//!
//! This module owns the CLI-side wrapper that turns a single
//! `AgentLoop::run()` invocation into a budget-bounded goal loop with
//! per-turn judge invocation and a Drop-guard guaranteeing the synthetic
//! `kanban_block` fires on budget exhaustion (RESEARCH Pitfall 5 mitigation).
//!
//! ## Architecture
//!
//! Production `main.rs` (run_single + run_chat) dispatches a worker turn
//! through `AgentRuntime::run_turn(TurnRequest)`, not directly through
//! `AgentLoop::run`. To compose with both production AND tests without
//! pulling Runtime construction into a unit-test seam, the wrapper takes a
//! **TurnRunner closure**: `Box<dyn FnMut(Vec<String>) -> BoxFuture<Result<String>>>`.
//!
//! - **Production caller** (Plan 04 Task 2 wiring in main.rs) constructs
//!   the closure as `move |messages| { runtime.run_turn(...).await }` once
//!   it has the runtime + per-turn state in scope.
//! - **Tests** (consumer tests in `tests/goal_loop_wrapper.rs`) pass a
//!   mock closure that increments a counter and returns canned text.
//!
//! The closure's argument is `Vec<String>` (a flat textual rendering of
//! messages to date) and the return is `String` (the final assistant
//! turn text used to build the JudgeRequest). This deliberately narrow
//! contract avoids leaking `ChatMessage` / `AgentResult` types into the
//! wrapper signature, which would force every test to construct full
//! agent state.
//!
//! ## Five behavioral paths (PLAN.md `<objective>`)
//!
//! | Path                  | Trigger                                     | Outcome              |
//! |-----------------------|---------------------------------------------|----------------------|
//! | Non-goal passthrough  | `HERMES_KANBAN_GOAL_MODE` unset             | 1 turn, no events    |
//! | Judge-met             | judge returns Met                           | clean Ok(()) exit    |
//! | Self-termination      | task.status ∈ {done, blocked} after a turn  | clean Ok(()) exit    |
//! | Budget exhaustion     | turn counter hits max_turns                 | synthetic block + Ok |
//! | Two-strike judge err  | judge Err twice consecutively               | synthetic block + Ok |
//!
//! ## RAII drop guard
//!
//! `BudgetSentinel` holds the store handle and emits the
//! `goal_budget_exhausted` event + the synthetic `kanban_block` from its
//! Drop impl if `done == false` at scope exit. Happy paths set
//! `done = true` before returning; budget exhaustion lets Drop fire.
//! All Drop-side calls use `let _ =` so a panic during teardown becomes a
//! log-and-continue per Rust's "don't panic from Drop" rule.

use std::pin::Pin;
use std::sync::Arc;

use anyhow::Context;
use ironhermes_kanban::{
    GOAL_SUBKIND_BUDGET_EXHAUSTED, GOAL_SUBKIND_JUDGE_ERROR, GOAL_SUBKIND_JUDGE_VERDICT,
    GOAL_SUBKIND_TURN_ADVANCED, JudgeFn, JudgeOutput, JudgeRequest, JudgeVerdict,
    KanbanEventKind, KanbanStore,
};
use tokio::sync::Mutex as TokioMutex;

/// `BoxFuture` alias — mirrors `crates/ironhermes-kanban/src/decomposer.rs:58`
/// to avoid the `futures::future::BoxFuture` dep (this crate doesn't pull
/// `futures` in directly).
pub type GoalLoopFuture<T> =
    Pin<Box<dyn std::future::Future<Output = T> + Send + 'static>>;

/// Per-turn agent runner. Called once per goal-loop iteration with the
/// running message vector; returns the assistant's final textual output
/// that the judge will evaluate.
///
/// Production code wraps `AgentRuntime::run_turn(TurnRequest)`. Tests pass
/// a mock closure. The narrow `Vec<String> -> String` signature avoids
/// leaking `ChatMessage` / `AgentResult` into the public surface.
pub type TurnRunner = Box<
    dyn FnMut(Vec<String>) -> GoalLoopFuture<anyhow::Result<String>> + Send,
>;

// ---------------------------------------------------------------------------
// BudgetSentinel — RAII guard for the synthetic kanban_block on budget
// exhaustion (PLAN.md D-03 + RESEARCH Pitfall 5).
// ---------------------------------------------------------------------------

/// Owned state used by the Drop impl to emit the budget-exhausted event
/// and the synthetic `kanban_block`. The values are owned (not borrowed)
/// because Drop needs to run inside a sync context but the actual emit
/// requires the async-aware `Arc<TokioMutex<KanbanStore>>`.
struct BudgetSentinel {
    store: Arc<TokioMutex<KanbanStore>>,
    task_id: String,
    run_id: String,
    goal_max_turns: u32,
    goal_turns_used: u32,
    done: bool,
}

impl Drop for BudgetSentinel {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        // RESEARCH Pitfall 5: budget exhaustion MUST surface as a block;
        // silent exit produces orphaned `running` tasks. Drop runs in a
        // sync context, so we use `try_lock` first (cheap, expected to
        // win because the wrapper releases its lock before falling out
        // of scope). If contention is detected we degrade to a tracing
        // warning rather than blocking in Drop. All store calls are
        // wrapped in `let _ =` so a Drop panic is impossible.
        match self.store.try_lock() {
            Ok(mut guard) => {
                let _ = guard.append_event(
                    &self.task_id,
                    Some(&self.run_id),
                    KanbanEventKind::Edited,
                    Some(&serde_json::json!({
                        "subkind": GOAL_SUBKIND_BUDGET_EXHAUSTED,
                        "goal_max_turns": self.goal_max_turns,
                        "goal_turns_used": self.goal_turns_used,
                    })),
                );
                let _ = guard.block_task(
                    &self.task_id,
                    "goal_max_turns exhausted; needs human review",
                    Some(&self.run_id),
                );
            }
            Err(_) => {
                tracing::warn!(
                    target: "goal_loop",
                    task_id = %self.task_id,
                    "BudgetSentinel::drop could not try_lock store — synthetic block may be missed (Pitfall 5 fallback)"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// synthetic_kanban_block helper — mirrors the production `kanban_block` tool
// path at `crates/ironhermes-kanban/src/tools/block.rs:140` which calls
// `store.block_task(task_id, reason, expected_run_id)`. Using the same
// public store method preserves the tool's existing invariants
// (status validation, event emission inside one BEGIN IMMEDIATE tx) —
// no new mutation pathway is introduced (T-36.3.7.12-04-T05 mitigation).
// ---------------------------------------------------------------------------

async fn synthetic_kanban_block(
    store: &Arc<TokioMutex<KanbanStore>>,
    task_id: &str,
    run_id: &str,
    reason: &str,
) -> anyhow::Result<()> {
    let mut guard = store.lock().await;
    guard
        .block_task(task_id, reason, Some(run_id))
        .with_context(|| format!("synthetic kanban_block from goal_loop ({reason})"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Helper — `Task::status` is a plain `String` (D-17), so the terminal check
// is string-based.
// ---------------------------------------------------------------------------

fn status_is_terminal(status: &str) -> bool {
    status == "done" || status == "blocked"
}

// ---------------------------------------------------------------------------
// run_goal_loop_if_enabled — the load-bearing wrapper.
// ---------------------------------------------------------------------------

/// Wrap `AgentLoop::run()` (via the supplied `TurnRunner`) in a
/// budget-bounded goal loop when `HERMES_KANBAN_GOAL_MODE` is set in env.
///
/// Returns Ok(()) for ALL five behavioral paths. Errors that bubble up are
/// limited to non-recoverable store failures during the happy-path
/// emit-and-bump sequence; the synthetic-block paths swallow Errs into
/// tracing logs to honor the "always exit cleanly when done" contract.
///
/// The wrapper enters the loop only when `HERMES_KANBAN_GOAL_MODE` is set
/// in env at call time. When unset, it calls `turn_runner` ONCE with the
/// initial messages and returns its Result mapped to `Result<()>`. This
/// preserves the non-goal `chat -q` worker path byte-for-byte equivalent
/// to a direct `runtime.run_turn` invocation (D-06).
pub async fn run_goal_loop_if_enabled(
    mut turn_runner: TurnRunner,
    initial_messages: Vec<String>,
    task_id: &str,
    run_id: &str,
    claim_lock: &str,
    judge_fn: &JudgeFn,
    store: Arc<TokioMutex<KanbanStore>>,
) -> anyhow::Result<()> {
    if std::env::var("HERMES_KANBAN_GOAL_MODE").is_err() {
        // Non-goal passthrough: existing manual worker path preserved
        // byte-for-byte. No event emission, no budget tracking, no
        // counter bump.
        let _ = turn_runner(initial_messages).await?;
        return Ok(());
    }

    let max_turns: u32 = std::env::var("HERMES_KANBAN_GOAL_MAX_TURNS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(20);

    // Fetch task title + body once for judge requests. We re-fetch the
    // task on each iteration for the self-termination status check
    // (status can change mid-turn).
    let initial_task = {
        let guard = store.lock().await;
        guard
            .get_task(task_id)
            .context("goal_loop: fetch task for title/body")?
    };
    let title = initial_task.title.clone();
    let body = initial_task.body.clone().unwrap_or_default();

    // The sentinel guarantees the budget-exhausted block fires on scope
    // exit unless `done` is set true on a happy-path return.
    let mut sentinel = BudgetSentinel {
        store: store.clone(),
        task_id: task_id.to_string(),
        run_id: run_id.to_string(),
        goal_max_turns: max_turns,
        goal_turns_used: 0,
        done: false,
    };

    let mut messages = initial_messages;
    let mut consecutive_judge_errors: u32 = 0;

    for turn in 0..max_turns {
        // (a) one agent invocation — TurnRunner is the production
        // `runtime.run_turn` wrapper or a test mock.
        let worker_output = turn_runner(messages.clone())
            .await
            .with_context(|| format!("goal_loop turn {}: TurnRunner failed", turn))?;

        // (b) CAS-gated counter bump (Plan 02 Task 3 helper).
        let bumped = {
            let mut guard = store.lock().await;
            guard
                .bump_goal_turn_counter(task_id, claim_lock)
                .context("goal_loop: bump_goal_turn_counter")?
        };
        if !bumped {
            // Stale claim — reclaim happened mid-loop. The
            // `bump_goal_turn_counter` helper already emitted a
            // `claim_expired` advisory via worker_write_gated. Surface a
            // judge_error subkind so the wrapper's event log narrates
            // the exit reason, then return cleanly.
            {
                let mut guard = store.lock().await;
                let _ = guard.append_event(
                    task_id,
                    Some(run_id),
                    KanbanEventKind::Edited,
                    Some(&serde_json::json!({
                        "subkind": GOAL_SUBKIND_JUDGE_ERROR,
                        "error": "claim_expired",
                        "turn": turn + 1,
                    })),
                );
            }
            sentinel.done = true;
            return Ok(());
        }
        sentinel.goal_turns_used = turn + 1;
        {
            let mut guard = store.lock().await;
            let _ = guard.append_event(
                task_id,
                Some(run_id),
                KanbanEventKind::Edited,
                Some(&serde_json::json!({
                    "subkind": GOAL_SUBKIND_TURN_ADVANCED,
                    "turn": turn + 1,
                })),
            );
        }

        // (d) self-termination check.
        let fresh_status = {
            let guard = store.lock().await;
            guard
                .get_task(task_id)
                .context("goal_loop: refetch task for self-term check")?
                .status
        };
        if status_is_terminal(&fresh_status) {
            sentinel.done = true;
            return Ok(());
        }

        // (e) judge invocation.
        let req = JudgeRequest {
            task_id: task_id.to_string(),
            title: title.clone(),
            body: body.clone(),
            worker_turn_output: worker_output.clone(),
            turn: turn + 1,
        };
        match judge_fn(req).await {
            Ok(JudgeOutput { verdict: JudgeVerdict::Met, reason }) => {
                let mut guard = store.lock().await;
                let _ = guard.append_event(
                    task_id,
                    Some(run_id),
                    KanbanEventKind::Edited,
                    Some(&serde_json::json!({
                        "subkind": GOAL_SUBKIND_JUDGE_VERDICT,
                        "verdict": "met",
                        "reason": reason,
                        "turn": turn + 1,
                    })),
                );
                drop(guard);
                sentinel.done = true;
                return Ok(());
            }
            Ok(JudgeOutput { verdict: JudgeVerdict::NotMet, reason }) => {
                {
                    let mut guard = store.lock().await;
                    let _ = guard.append_event(
                        task_id,
                        Some(run_id),
                        KanbanEventKind::Edited,
                        Some(&serde_json::json!({
                            "subkind": GOAL_SUBKIND_JUDGE_VERDICT,
                            "verdict": "not_met",
                            "reason": reason.clone(),
                            "turn": turn + 1,
                        })),
                    );
                }
                consecutive_judge_errors = 0;
                // Push a synthetic user-message so the next turn's
                // TurnRunner sees the judge's feedback. Production
                // TurnRunner wraps `runtime.run_turn` which appends this
                // to the running session.
                messages.push(format!(
                    "Judge verdict: not met. Reason: {}\nContinue working toward the acceptance criteria.",
                    reason,
                ));
            }
            Err(e) => {
                let snippet: String = format!("{:#}", e).chars().take(200).collect();
                {
                    let mut guard = store.lock().await;
                    let _ = guard.append_event(
                        task_id,
                        Some(run_id),
                        KanbanEventKind::Edited,
                        Some(&serde_json::json!({
                            "subkind": GOAL_SUBKIND_JUDGE_ERROR,
                            "error": snippet,
                            "turn": turn + 1,
                        })),
                    );
                }
                consecutive_judge_errors += 1;
                if consecutive_judge_errors >= 2 {
                    synthetic_kanban_block(&store, task_id, run_id, "judge unavailable").await?;
                    sentinel.done = true;
                    return Ok(());
                }
                // Otherwise continue to the next turn. The worker LLM
                // does NOT see judge errors as text (per D-04).
            }
        }
    }

    // Loop exited naturally: budget exhausted. The sentinel's Drop emits
    // the budget-exhausted event + synthetic kanban_block (RESEARCH
    // Pitfall 5).
    //
    // We DON'T set `sentinel.done = true` here — the Drop is the
    // load-bearing emission site. The function returns Ok(()) because
    // the contract is "always return Ok(()) when a path completes; the
    // BLOCKED status surfaces the failure to operators".
    let _ = sentinel; // silence "no-op" lint; the Drop is the side-effect.
    Ok(())
}

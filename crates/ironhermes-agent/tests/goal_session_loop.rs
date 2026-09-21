//! Phase 49.7 Plan 05 (D-02) — behavioral tests for the session-level
//! `/goal` loop at `crates/ironhermes-agent/src/goal_session_loop.rs`.
//!
//! Ten behavior bullets from PLAN.md, one named test each (plus the
//! message-contract and "exactly one Finished" cross-cutting tests):
//! 1. `judge_met_stops_after_exactly_one_turn`
//! 2. `budget_exhaustion_runs_exactly_max_turns`
//! 3. `two_strike_judge_error_stops_after_exactly_two_turns`
//! 4. `strike_reset_does_not_stop_early`
//! 5. `not_met_continuation_grows_message_vector_by_one_with_reason`
//! 6. `judge_error_does_not_grow_message_vector`
//! 7. `cancellation_between_turns_stops_after_exactly_one_turn`
//! 8. `message_contract_element_zero_stable_and_length_grows_by_one`
//! 9. `finished_progress_emitted_exactly_once_on_every_terminal_path`
//! 10. `default_max_turns_and_strike_threshold_come_from_shared_constants`

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use ironhermes_agent::goal_session_loop::{
    GoalLoopOutcome, GoalProgress, GoalSessionFuture, GoalStopReason, GoalTurnRunner,
    run_goal_session_loop,
};
use ironhermes_core::judge::{
    GOAL_DEFAULT_MAX_TURNS, GOAL_JUDGE_ERROR_STRIKES, JudgeError, JudgeFn, JudgeOutput,
    JudgeRequest, JudgeVerdict,
};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio_util::sync::CancellationToken;

/// A turn runner that just counts invocations and returns a canned string.
/// Also records every message vector it was handed, for the message-contract
/// test.
fn counting_turn_runner(
    counter: Arc<AtomicU32>,
    recorded: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
) -> GoalTurnRunner {
    Box::new(move |messages: Vec<String>| -> GoalSessionFuture<anyhow::Result<String>> {
        counter.fetch_add(1, Ordering::SeqCst);
        recorded.lock().unwrap().push(messages);
        Box::pin(async move { Ok("worker output".to_string()) })
    })
}

/// A turn runner that cancels the shared token at the end of the FIRST
/// invocation, then behaves like `counting_turn_runner` for any further
/// (unexpected) calls.
fn cancel_after_first_turn_runner(
    counter: Arc<AtomicU32>,
    token: CancellationToken,
) -> GoalTurnRunner {
    Box::new(move |_messages: Vec<String>| -> GoalSessionFuture<anyhow::Result<String>> {
        let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
        if n == 1 {
            token.cancel();
        }
        Box::pin(async move { Ok("worker output".to_string()) })
    })
}

/// Judge that always returns `Met`.
fn always_met_judge() -> JudgeFn {
    Arc::new(|_req: JudgeRequest| {
        Box::pin(async move {
            Ok(JudgeOutput {
                verdict: JudgeVerdict::Met,
                reason: "criteria satisfied".to_string(),
            })
        })
    })
}

/// Judge that always returns `NotMet`.
fn always_not_met_judge() -> JudgeFn {
    Arc::new(|_req: JudgeRequest| {
        Box::pin(async move {
            Ok(JudgeOutput {
                verdict: JudgeVerdict::NotMet,
                reason: "keep going".to_string(),
            })
        })
    })
}

/// Judge that always returns `Err`.
fn always_err_judge() -> JudgeFn {
    Arc::new(|_req: JudgeRequest| {
        Box::pin(async move { Err(JudgeError::Provider("provider unavailable".to_string())) })
    })
}

/// Judge scripted with a fixed sequence of verdicts (by index, 0-based
/// turn), Erring beyond the scripted length so a test can assert the loop
/// never runs past what it expects.
fn scripted_judge(script: Vec<Result<(JudgeVerdict, &'static str), &'static str>>) -> JudgeFn {
    let calls = Arc::new(std::sync::Mutex::new(0usize));
    Arc::new(move |_req: JudgeRequest| {
        let idx = {
            let mut c = calls.lock().unwrap();
            let i = *c;
            *c += 1;
            i
        };
        let script = script.clone();
        Box::pin(async move {
            match script.get(idx) {
                Some(Ok((verdict, reason))) => Ok(JudgeOutput {
                    verdict: verdict.clone(),
                    reason: reason.to_string(),
                }),
                Some(Err(msg)) => Err(JudgeError::Provider(msg.to_string())),
                None => panic!("scripted_judge called more times than scripted ({idx})"),
            }
        })
    })
}

/// Drains every `GoalProgress` currently buffered on the channel (loop body
/// has already returned by the time tests call this, so `try_recv` is safe
/// — no more producers can be writing).
fn drain_progress(rx: &mut UnboundedReceiver<GoalProgress>) -> Vec<GoalProgress> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push(ev);
    }
    out
}

fn count_finished(events: &[GoalProgress]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, GoalProgress::Finished(_)))
        .count()
}

// ---------------------------------------------------------------------------
// 1. Judge-met
// ---------------------------------------------------------------------------

#[tokio::test]
async fn judge_met_stops_after_exactly_one_turn() {
    let counter = Arc::new(AtomicU32::new(0));
    let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
    let runner = counting_turn_runner(counter.clone(), recorded);
    let judge = always_met_judge();
    let (tx, mut rx) = unbounded_channel();

    let outcome = run_goal_session_loop(
        runner,
        "ship the docs update".to_string(),
        "session-1".to_string(),
        Some(20),
        &judge,
        CancellationToken::new(),
        tx,
    )
    .await
    .expect("loop must succeed");

    assert_eq!(counter.load(Ordering::SeqCst), 1, "exactly one turn-run");
    assert_eq!(outcome.turns_used, 1);
    assert!(matches!(outcome.reason, GoalStopReason::JudgeMet { .. }));

    let events = drain_progress(&mut rx);
    assert_eq!(count_finished(&events), 1, "exactly one Finished event");
}

// ---------------------------------------------------------------------------
// 2. Budget exhaustion
// ---------------------------------------------------------------------------

#[tokio::test]
async fn budget_exhaustion_runs_exactly_max_turns() {
    let counter = Arc::new(AtomicU32::new(0));
    let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
    let runner = counting_turn_runner(counter.clone(), recorded);
    let judge = always_not_met_judge();
    let (tx, mut rx) = unbounded_channel();

    let outcome = run_goal_session_loop(
        runner,
        "objective".to_string(),
        "session-2".to_string(),
        Some(3),
        &judge,
        CancellationToken::new(),
        tx,
    )
    .await
    .expect("loop must succeed");

    assert_eq!(counter.load(Ordering::SeqCst), 3, "exactly 3 turn-runs");
    assert_eq!(outcome.turns_used, 3);
    assert_eq!(outcome.reason, GoalStopReason::BudgetExhausted);

    let events = drain_progress(&mut rx);
    assert_eq!(
        count_finished(&events),
        1,
        "exactly one Finished event (from the BudgetSentinel Drop)"
    );
}

// ---------------------------------------------------------------------------
// 3. Two-strike judge error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn two_strike_judge_error_stops_after_exactly_two_turns() {
    let counter = Arc::new(AtomicU32::new(0));
    let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
    let runner = counting_turn_runner(counter.clone(), recorded);
    let judge = always_err_judge();
    let (tx, mut rx) = unbounded_channel();

    let outcome = run_goal_session_loop(
        runner,
        "objective".to_string(),
        "session-3".to_string(),
        Some(20),
        &judge,
        CancellationToken::new(),
        tx,
    )
    .await
    .expect("loop must succeed");

    assert_eq!(counter.load(Ordering::SeqCst), 2, "exactly 2 turn-runs");
    assert_eq!(outcome.turns_used, 2);
    assert!(matches!(
        outcome.reason,
        GoalStopReason::JudgeErrorStrikes { .. }
    ));

    let events = drain_progress(&mut rx);
    assert_eq!(count_finished(&events), 1);
}

// ---------------------------------------------------------------------------
// 4. Strike reset
// ---------------------------------------------------------------------------

#[tokio::test]
async fn strike_reset_does_not_stop_early() {
    let counter = Arc::new(AtomicU32::new(0));
    let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
    let runner = counting_turn_runner(counter.clone(), recorded);
    // Err, NotMet, Err, NotMet, then NotMet again (turn 5) — max_turns=5, so
    // the loop must reach budget exhaustion, never JudgeErrorStrikes,
    // because a single error between successes resets the counter.
    let judge = scripted_judge(vec![
        Err("boom-1"),
        Ok((JudgeVerdict::NotMet, "keep going 1")),
        Err("boom-2"),
        Ok((JudgeVerdict::NotMet, "keep going 2")),
        Ok((JudgeVerdict::NotMet, "keep going 3")),
    ]);
    let (tx, mut rx) = unbounded_channel();

    let outcome = run_goal_session_loop(
        runner,
        "objective".to_string(),
        "session-4".to_string(),
        Some(5),
        &judge,
        CancellationToken::new(),
        tx,
    )
    .await
    .expect("loop must succeed");

    assert_eq!(
        counter.load(Ordering::SeqCst),
        5,
        "the loop must reach budget exhaustion, not stop early on strikes"
    );
    assert_eq!(outcome.turns_used, 5);
    assert_eq!(outcome.reason, GoalStopReason::BudgetExhausted);

    let events = drain_progress(&mut rx);
    assert_eq!(count_finished(&events), 1);
}

// ---------------------------------------------------------------------------
// 5. Not-met continuation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn not_met_continuation_grows_message_vector_by_one_with_reason() {
    let counter = Arc::new(AtomicU32::new(0));
    let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
    let runner = counting_turn_runner(counter.clone(), recorded.clone());
    // NotMet on turn 1, Met on turn 2 — checks the vector handed to turn 2.
    let judge = scripted_judge(vec![
        Ok((JudgeVerdict::NotMet, "needs more detail")),
        Ok((JudgeVerdict::Met, "done")),
    ]);
    let (tx, mut rx) = unbounded_channel();

    let _ = run_goal_session_loop(
        runner,
        "objective".to_string(),
        "session-5".to_string(),
        Some(20),
        &judge,
        CancellationToken::new(),
        tx,
    )
    .await
    .expect("loop must succeed");
    let _ = drain_progress(&mut rx);

    let calls = recorded.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].len(), 1, "turn 1 sees only the objective");
    assert_eq!(
        calls[1].len(),
        2,
        "turn 2's vector grew by exactly one entry"
    );
    assert!(
        calls[1][1].contains("needs more detail"),
        "the new entry must carry the judge's reason text, got: {}",
        calls[1][1]
    );
}

// ---------------------------------------------------------------------------
// 6. Judge errors are not fed to the turn runner
// ---------------------------------------------------------------------------

#[tokio::test]
async fn judge_error_does_not_grow_message_vector() {
    let counter = Arc::new(AtomicU32::new(0));
    let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
    let runner = counting_turn_runner(counter.clone(), recorded.clone());
    // Err on turn 1, Err on turn 2 (two-strike stop) — checks the vector
    // handed to turn 2 is unchanged in length from turn 1's.
    let judge = always_err_judge();
    let (tx, mut rx) = unbounded_channel();

    let _ = run_goal_session_loop(
        runner,
        "objective".to_string(),
        "session-6".to_string(),
        Some(20),
        &judge,
        CancellationToken::new(),
        tx,
    )
    .await
    .expect("loop must succeed");
    let _ = drain_progress(&mut rx);

    let calls = recorded.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[0].len(),
        calls[1].len(),
        "a judge Err must not grow the message vector"
    );
}

// ---------------------------------------------------------------------------
// 7. Cancellation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cancellation_between_turns_stops_after_exactly_one_turn() {
    let counter = Arc::new(AtomicU32::new(0));
    let token = CancellationToken::new();
    let runner = cancel_after_first_turn_runner(counter.clone(), token.clone());
    let judge = always_not_met_judge();
    let (tx, mut rx) = unbounded_channel();

    let outcome = run_goal_session_loop(
        runner,
        "objective".to_string(),
        "session-7".to_string(),
        Some(20),
        &judge,
        token,
        tx,
    )
    .await
    .expect("loop must succeed");

    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "turn 2 must never run once the token is cancelled between iterations"
    );
    assert_eq!(outcome.turns_used, 1);
    assert_eq!(outcome.reason, GoalStopReason::Cancelled);

    let events = drain_progress(&mut rx);
    assert_eq!(count_finished(&events), 1);
}

// ---------------------------------------------------------------------------
// 8. Message contract
// ---------------------------------------------------------------------------

#[tokio::test]
async fn message_contract_element_zero_stable_and_length_grows_by_one() {
    let counter = Arc::new(AtomicU32::new(0));
    let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
    let runner = counting_turn_runner(counter.clone(), recorded.clone());
    // NotMet, NotMet, Met.
    let judge = scripted_judge(vec![
        Ok((JudgeVerdict::NotMet, "reason one")),
        Ok((JudgeVerdict::NotMet, "reason two")),
        Ok((JudgeVerdict::Met, "done")),
    ]);
    let (tx, mut rx) = unbounded_channel();

    let objective = "the objective text".to_string();
    let _ = run_goal_session_loop(
        runner,
        objective.clone(),
        "session-8".to_string(),
        Some(20),
        &judge,
        CancellationToken::new(),
        tx,
    )
    .await
    .expect("loop must succeed");
    let _ = drain_progress(&mut rx);

    let calls = recorded.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0].len(), 1);
    assert_eq!(calls[1].len(), 2);
    assert_eq!(calls[2].len(), 3);
    for (i, vec) in calls.iter().enumerate() {
        assert_eq!(
            vec[0], objective,
            "element 0 must be byte-identical to the objective on iteration {i}"
        );
    }
}

// ---------------------------------------------------------------------------
// 9. Progress — exactly one Finished on every terminal path, including
//    mutation-checked judge-met path.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn finished_progress_emitted_exactly_once_on_every_terminal_path() {
    // judge-met
    {
        let counter = Arc::new(AtomicU32::new(0));
        let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
        let runner = counting_turn_runner(counter, recorded);
        let judge = always_met_judge();
        let (tx, mut rx) = unbounded_channel();
        let _ = run_goal_session_loop(
            runner,
            "o".to_string(),
            "s-9a".to_string(),
            Some(20),
            &judge,
            CancellationToken::new(),
            tx,
        )
        .await
        .unwrap();
        assert_eq!(count_finished(&drain_progress(&mut rx)), 1, "judge-met");
    }
    // budget exhaustion
    {
        let counter = Arc::new(AtomicU32::new(0));
        let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
        let runner = counting_turn_runner(counter, recorded);
        let judge = always_not_met_judge();
        let (tx, mut rx) = unbounded_channel();
        let _ = run_goal_session_loop(
            runner,
            "o".to_string(),
            "s-9b".to_string(),
            Some(2),
            &judge,
            CancellationToken::new(),
            tx,
        )
        .await
        .unwrap();
        assert_eq!(
            count_finished(&drain_progress(&mut rx)),
            1,
            "budget exhaustion"
        );
    }
    // two-strike judge error
    {
        let counter = Arc::new(AtomicU32::new(0));
        let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
        let runner = counting_turn_runner(counter, recorded);
        let judge = always_err_judge();
        let (tx, mut rx) = unbounded_channel();
        let _ = run_goal_session_loop(
            runner,
            "o".to_string(),
            "s-9c".to_string(),
            Some(20),
            &judge,
            CancellationToken::new(),
            tx,
        )
        .await
        .unwrap();
        assert_eq!(
            count_finished(&drain_progress(&mut rx)),
            1,
            "two-strike judge error"
        );
    }
    // cancelled
    {
        let counter = Arc::new(AtomicU32::new(0));
        let token = CancellationToken::new();
        let runner = cancel_after_first_turn_runner(counter, token.clone());
        let judge = always_not_met_judge();
        let (tx, mut rx) = unbounded_channel();
        let _ = run_goal_session_loop(
            runner,
            "o".to_string(),
            "s-9d".to_string(),
            Some(20),
            &judge,
            token,
            tx,
        )
        .await
        .unwrap();
        assert_eq!(count_finished(&drain_progress(&mut rx)), 1, "cancelled");
    }
}

// ---------------------------------------------------------------------------
// 10. Constants
// ---------------------------------------------------------------------------

#[tokio::test]
async fn default_max_turns_and_strike_threshold_come_from_shared_constants() {
    // max_turns unset (None) must resolve to GOAL_DEFAULT_MAX_TURNS (20) —
    // proven by running a fast NotMet judge to natural budget exhaustion and
    // counting turn-runs.
    let counter = Arc::new(AtomicU32::new(0));
    let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
    let runner = counting_turn_runner(counter.clone(), recorded);
    let judge = always_not_met_judge();
    let (tx, mut rx) = unbounded_channel();

    let outcome = run_goal_session_loop(
        runner,
        "objective".to_string(),
        "session-10".to_string(),
        None,
        &judge,
        CancellationToken::new(),
        tx,
    )
    .await
    .expect("loop must succeed");
    let _ = drain_progress(&mut rx);

    assert_eq!(counter.load(Ordering::SeqCst), GOAL_DEFAULT_MAX_TURNS);
    assert_eq!(outcome.turns_used, GOAL_DEFAULT_MAX_TURNS);
    assert_eq!(GOAL_DEFAULT_MAX_TURNS, 20);
    assert_eq!(GOAL_JUDGE_ERROR_STRIKES, 2);

    // The strike threshold is exercised by the two-strike test above
    // (`two_strike_judge_error_stops_after_exactly_two_turns`), which stops
    // at exactly GOAL_JUDGE_ERROR_STRIKES (2) consecutive errors.
    let _: GoalLoopOutcome = outcome;
}

//! Phase 36.3.7.12-04 Plan 04 Task 2 — behavioral consumer tests for the
//! goal-loop wrapper at `crates/ironhermes-cli/src/kanban/goal_loop.rs`.
//!
//! Five behavioral paths from PLAN.md `<objective>`:
//! 1. `non_goal_passthrough_calls_run_once` — env unset → exactly one turn,
//!    zero goal-subkind events.
//! 2. `judge_met_exits_cleanly` — judge returns Met after turn 1 → loop
//!    returns Ok(()) after exactly one turn-run + one judge call.
//! 3. `self_termination_exits_cleanly` — task status becomes Done mid-loop
//!    (worker self-terminates via kanban_complete) → loop exits without
//!    calling judge again.
//! 4. `budget_exhaustion_synthesizes_block` — judge always NotMet,
//!    max_turns=3 → exactly 3 turn-runs, 1 budget-exhausted event, 1
//!    synthetic block; task is BLOCKED with the human-review reason.
//! 5. `two_consecutive_judge_errors_block` — judge Err on turn 1+2 → 2
//!    turn-runs, 2 judge_error events, 1 synthetic block with reason
//!    "judge unavailable"; NO budget_exhausted event.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use ironhermes_kanban::{
    CreateTaskOptions, JudgeFn, JudgeOutput, JudgeRequest, JudgeVerdict, KanbanStore,
    cas::{DEFAULT_CLAIM_TTL_SECONDS, atomic_claim, build_claim_lock},
};
use rusqlite::Connection;
use serde_json::Value;
use tempfile::TempDir;

// Test-local mutex serializing env-var manipulation across the five tests
// in this binary (parallel tests share the process env).
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

fn open_conn(path: &Path) -> Connection {
    let conn = Connection::open(path).expect("open connection");
    conn.busy_timeout(std::time::Duration::from_millis(5000))
        .unwrap();
    conn
}

fn create_goal_task(db_path: &Path, max_turns: u32) -> String {
    let mut store = KanbanStore::new(db_path).expect("open store");
    let opts = CreateTaskOptions {
        goal_mode: true,
        goal_max_turns: max_turns,
        ..Default::default()
    };
    store
        .create_task("test goal task", "tester", opts)
        .expect("create_task")
        .id
}

fn create_non_goal_task(db_path: &Path) -> String {
    let mut store = KanbanStore::new(db_path).expect("open store");
    let opts = CreateTaskOptions::default();
    store
        .create_task("non-goal task", "tester", opts)
        .expect("create_task")
        .id
}

fn claim_task(db_path: &Path, task_id: &str) -> (String, String) {
    let claim_lock = build_claim_lock("test-host", 4242);
    let run_id = format!("r_{}", uuid::Uuid::new_v4().simple());
    let mut conn = open_conn(db_path);
    let won = atomic_claim(
        &mut conn,
        task_id,
        &claim_lock,
        4242,
        DEFAULT_CLAIM_TTL_SECONDS,
        now(),
        &run_id,
    )
    .expect("atomic_claim");
    assert!(won, "atomic_claim must succeed on a fresh task");
    (claim_lock, run_id)
}

fn count_subkind_events(db_path: &Path, task_id: &str, subkind: &str) -> usize {
    let store = KanbanStore::new(db_path).expect("reopen for read");
    let events = store.get_events(task_id).expect("get events");
    events
        .into_iter()
        .filter(|e| {
            e.kind == "edited"
                && e.payload
                    .as_ref()
                    .and_then(|p| serde_json::from_str::<Value>(p).ok())
                    .and_then(|v| v.get("subkind").and_then(|s| s.as_str().map(String::from)))
                    .as_deref()
                    == Some(subkind)
        })
        .count()
}

fn task_status(db_path: &Path, task_id: &str) -> String {
    let store = KanbanStore::new(db_path).expect("reopen for read");
    store.get_task(task_id).expect("get task").status
}

fn task_goal_turns_used(db_path: &Path, task_id: &str) -> u32 {
    let store = KanbanStore::new(db_path).expect("reopen for read");
    store.get_task(task_id).expect("get task").goal_turns_used
}

// ---------------------------------------------------------------------------
// Test 1: non-goal passthrough — env unset, single turn-run, no events.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn non_goal_passthrough_calls_run_once() {
    let _g = ENV_LOCK.lock().await;
    // SAFETY: tests serialize via ENV_LOCK; production code only reads env.
    unsafe {
        std::env::remove_var("IRONHERMES_KANBAN_GOAL_MODE");
        std::env::remove_var("IRONHERMES_KANBAN_GOAL_MAX_TURNS");
    }

    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("kanban.db");
    let task_id = create_non_goal_task(&db_path);
    let (claim_lock, run_id) = claim_task(&db_path, &task_id);

    let turn_runs = Arc::new(AtomicU32::new(0));
    let judge_calls = Arc::new(AtomicU32::new(0));

    let turn_runs_for_runner = turn_runs.clone();
    let turn_runner: ironhermes_cli::kanban::goal_loop::TurnRunner =
        Box::new(move |_msgs: Vec<String>| {
            let c = turn_runs_for_runner.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok("(non-goal output)".to_string())
            })
        });

    let judge_calls_for_fn = judge_calls.clone();
    let judge_fn: JudgeFn = Arc::new(move |_req: JudgeRequest| {
        let c = judge_calls_for_fn.clone();
        Box::pin(async move {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(JudgeOutput {
                verdict: JudgeVerdict::Met,
                reason: "(should not be called)".into(),
            })
        })
    });

    let store = KanbanStore::new(&db_path).expect("open store for wrapper");
    let store_arc = Arc::new(tokio::sync::Mutex::new(store));
    let result = ironhermes_cli::kanban::goal_loop::run_goal_loop_if_enabled(
        turn_runner,
        Vec::new(),
        &task_id,
        &run_id,
        &claim_lock,
        &judge_fn,
        store_arc.clone(),
    )
    .await;
    drop(store_arc);
    assert!(result.is_ok(), "wrapper returned Err: {:?}", result.err());

    assert_eq!(
        turn_runs.load(Ordering::SeqCst),
        1,
        "expected exactly one turn-run"
    );
    assert_eq!(
        judge_calls.load(Ordering::SeqCst),
        0,
        "judge must not fire in non-goal path"
    );

    assert_eq!(count_subkind_events(&db_path, &task_id, "judge_verdict"), 0);
    assert_eq!(
        count_subkind_events(&db_path, &task_id, "goal_turn_advanced"),
        0
    );
    assert_eq!(
        count_subkind_events(&db_path, &task_id, "goal_budget_exhausted"),
        0
    );
    assert_eq!(task_goal_turns_used(&db_path, &task_id), 0);
}

// ---------------------------------------------------------------------------
// Test 2: judge-met → clean exit after 1 turn.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn judge_met_exits_cleanly() {
    let _g = ENV_LOCK.lock().await;
    unsafe {
        std::env::set_var("IRONHERMES_KANBAN_GOAL_MODE", "1");
        std::env::set_var("IRONHERMES_KANBAN_GOAL_MAX_TURNS", "10");
    }

    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("kanban.db");
    let task_id = create_goal_task(&db_path, 10);
    let (claim_lock, run_id) = claim_task(&db_path, &task_id);

    let turn_runs = Arc::new(AtomicU32::new(0));
    let turn_runs_for_runner = turn_runs.clone();
    let turn_runner: ironhermes_cli::kanban::goal_loop::TurnRunner =
        Box::new(move |_msgs: Vec<String>| {
            let c = turn_runs_for_runner.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok("done — acceptance met".to_string())
            })
        });

    let judge_fn: JudgeFn = Arc::new(move |_req: JudgeRequest| {
        Box::pin(async move {
            Ok(JudgeOutput {
                verdict: JudgeVerdict::Met,
                reason: "all criteria satisfied".into(),
            })
        })
    });

    let store = KanbanStore::new(&db_path).expect("open store");
    let store_arc = Arc::new(tokio::sync::Mutex::new(store));
    let result = ironhermes_cli::kanban::goal_loop::run_goal_loop_if_enabled(
        turn_runner,
        Vec::new(),
        &task_id,
        &run_id,
        &claim_lock,
        &judge_fn,
        store_arc.clone(),
    )
    .await;
    drop(store_arc);

    unsafe {
        std::env::remove_var("IRONHERMES_KANBAN_GOAL_MODE");
        std::env::remove_var("IRONHERMES_KANBAN_GOAL_MAX_TURNS");
    }

    assert!(result.is_ok(), "wrapper returned Err: {:?}", result.err());
    assert_eq!(turn_runs.load(Ordering::SeqCst), 1);
    assert_eq!(count_subkind_events(&db_path, &task_id, "judge_verdict"), 1);
    assert_eq!(
        count_subkind_events(&db_path, &task_id, "goal_budget_exhausted"),
        0
    );
    assert_eq!(
        task_status(&db_path, &task_id),
        "running",
        "judge-met does NOT change status — worker LLM owns kanban_complete"
    );
}

// ---------------------------------------------------------------------------
// Test 3: self-termination — task becomes Done via worker tool call.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn self_termination_exits_cleanly() {
    let _g = ENV_LOCK.lock().await;
    unsafe {
        std::env::set_var("IRONHERMES_KANBAN_GOAL_MODE", "1");
        std::env::set_var("IRONHERMES_KANBAN_GOAL_MAX_TURNS", "10");
    }

    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("kanban.db");
    let task_id = create_goal_task(&db_path, 10);
    let (claim_lock, run_id) = claim_task(&db_path, &task_id);

    // The turn-runner directly mutates task.status to "done" via an
    // independent connection — simulating the worker LLM calling
    // kanban_complete from inside the chat session. The wrapper's
    // self-termination check happens AFTER the turn-run AND AFTER the
    // counter bump, so this should short-circuit before judge fires.
    let db_path_for_runner = db_path.clone();
    let task_id_for_runner = task_id.clone();
    let turn_runs = Arc::new(AtomicU32::new(0));
    let turn_runs_for_runner = turn_runs.clone();
    let turn_runner: ironhermes_cli::kanban::goal_loop::TurnRunner =
        Box::new(move |_msgs: Vec<String>| {
            let counter = turn_runs_for_runner.clone();
            let path = db_path_for_runner.clone();
            let tid = task_id_for_runner.clone();
            Box::pin(async move {
                counter.fetch_add(1, Ordering::SeqCst);
                // Worker self-terminates: status → done.
                let conn = open_conn(&path);
                conn.execute(
                    "UPDATE tasks SET status='done' WHERE id=?1",
                    rusqlite::params![tid],
                )
                .expect("self-terminate");
                Ok("self-terminating now".to_string())
            })
        });

    let judge_calls = Arc::new(AtomicU32::new(0));
    let judge_calls_for_fn = judge_calls.clone();
    let judge_fn: JudgeFn = Arc::new(move |_req: JudgeRequest| {
        let c = judge_calls_for_fn.clone();
        Box::pin(async move {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(JudgeOutput {
                verdict: JudgeVerdict::NotMet,
                reason: "(should not be called)".into(),
            })
        })
    });

    let store = KanbanStore::new(&db_path).expect("open store");
    let store_arc = Arc::new(tokio::sync::Mutex::new(store));
    let result = ironhermes_cli::kanban::goal_loop::run_goal_loop_if_enabled(
        turn_runner,
        Vec::new(),
        &task_id,
        &run_id,
        &claim_lock,
        &judge_fn,
        store_arc.clone(),
    )
    .await;
    drop(store_arc);

    unsafe {
        std::env::remove_var("IRONHERMES_KANBAN_GOAL_MODE");
        std::env::remove_var("IRONHERMES_KANBAN_GOAL_MAX_TURNS");
    }

    assert!(result.is_ok(), "wrapper returned Err: {:?}", result.err());
    assert_eq!(turn_runs.load(Ordering::SeqCst), 1);
    assert_eq!(
        judge_calls.load(Ordering::SeqCst),
        0,
        "judge must not fire after self-term"
    );
    assert_eq!(
        count_subkind_events(&db_path, &task_id, "goal_budget_exhausted"),
        0
    );
    assert_eq!(task_status(&db_path, &task_id), "done");
}

// ---------------------------------------------------------------------------
// Test 4: budget exhaustion → synthetic block.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn budget_exhaustion_synthesizes_block() {
    let _g = ENV_LOCK.lock().await;
    unsafe {
        std::env::set_var("IRONHERMES_KANBAN_GOAL_MODE", "1");
        std::env::set_var("IRONHERMES_KANBAN_GOAL_MAX_TURNS", "3");
    }

    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("kanban.db");
    let task_id = create_goal_task(&db_path, 3);
    let (claim_lock, run_id) = claim_task(&db_path, &task_id);

    let turn_runs = Arc::new(AtomicU32::new(0));
    let turn_runs_for_runner = turn_runs.clone();
    let turn_runner: ironhermes_cli::kanban::goal_loop::TurnRunner =
        Box::new(move |_msgs: Vec<String>| {
            let c = turn_runs_for_runner.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok("(another not-met turn)".to_string())
            })
        });

    let judge_fn: JudgeFn = Arc::new(move |_req: JudgeRequest| {
        Box::pin(async move {
            Ok(JudgeOutput {
                verdict: JudgeVerdict::NotMet,
                reason: "not yet".into(),
            })
        })
    });

    let store = KanbanStore::new(&db_path).expect("open store");
    let store_arc = Arc::new(tokio::sync::Mutex::new(store));
    let result = ironhermes_cli::kanban::goal_loop::run_goal_loop_if_enabled(
        turn_runner,
        Vec::new(),
        &task_id,
        &run_id,
        &claim_lock,
        &judge_fn,
        store_arc.clone(),
    )
    .await;
    drop(store_arc);

    unsafe {
        std::env::remove_var("IRONHERMES_KANBAN_GOAL_MODE");
        std::env::remove_var("IRONHERMES_KANBAN_GOAL_MAX_TURNS");
    }

    assert!(result.is_ok(), "wrapper returned Err: {:?}", result.err());
    assert_eq!(
        turn_runs.load(Ordering::SeqCst),
        3,
        "expected 3 turn-runs for max_turns=3"
    );
    assert_eq!(count_subkind_events(&db_path, &task_id, "judge_verdict"), 3);
    assert_eq!(
        count_subkind_events(&db_path, &task_id, "goal_budget_exhausted"),
        1
    );
    assert_eq!(
        count_subkind_events(&db_path, &task_id, "goal_turn_advanced"),
        3
    );
    assert_eq!(
        task_status(&db_path, &task_id),
        "blocked",
        "task must be BLOCKED on budget exhaustion (D-03)"
    );
}

// ---------------------------------------------------------------------------
// Test 5: two consecutive judge errors → block.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn two_consecutive_judge_errors_block() {
    let _g = ENV_LOCK.lock().await;
    unsafe {
        std::env::set_var("IRONHERMES_KANBAN_GOAL_MODE", "1");
        std::env::set_var("IRONHERMES_KANBAN_GOAL_MAX_TURNS", "10");
    }

    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("kanban.db");
    let task_id = create_goal_task(&db_path, 10);
    let (claim_lock, run_id) = claim_task(&db_path, &task_id);

    let turn_runs = Arc::new(AtomicU32::new(0));
    let turn_runs_for_runner = turn_runs.clone();
    let turn_runner: ironhermes_cli::kanban::goal_loop::TurnRunner =
        Box::new(move |_msgs: Vec<String>| {
            let c = turn_runs_for_runner.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok("(turn output)".to_string())
            })
        });

    let judge_fn: JudgeFn = Arc::new(move |_req: JudgeRequest| {
        Box::pin(async move {
            Err(ironhermes_kanban::KanbanError::Other(anyhow::anyhow!(
                "simulated provider failure: rate-limited"
            )))
        })
    });

    let store = KanbanStore::new(&db_path).expect("open store");
    let store_arc = Arc::new(tokio::sync::Mutex::new(store));
    let result = ironhermes_cli::kanban::goal_loop::run_goal_loop_if_enabled(
        turn_runner,
        Vec::new(),
        &task_id,
        &run_id,
        &claim_lock,
        &judge_fn,
        store_arc.clone(),
    )
    .await;
    drop(store_arc);

    unsafe {
        std::env::remove_var("IRONHERMES_KANBAN_GOAL_MODE");
        std::env::remove_var("IRONHERMES_KANBAN_GOAL_MAX_TURNS");
    }

    assert!(result.is_ok(), "wrapper returned Err: {:?}", result.err());
    assert_eq!(
        turn_runs.load(Ordering::SeqCst),
        2,
        "expected 2 turn-runs before the 2-strike judge-error block"
    );
    assert_eq!(count_subkind_events(&db_path, &task_id, "judge_error"), 2);
    assert_eq!(
        count_subkind_events(&db_path, &task_id, "goal_budget_exhausted"),
        0,
        "2-strike block is NOT the budget path"
    );
    assert_eq!(
        task_status(&db_path, &task_id),
        "blocked",
        "task must be BLOCKED after 2-strike judge errors"
    );
}

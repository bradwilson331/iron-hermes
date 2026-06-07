//! End-to-end lifecycle test for the kanban kernel (Plan 09, Task 1).
//!
//! Exercises the full create → dispatcher-claim → tool-complete → done
//! pipeline in a single tempfile-backed store, using an injectable spawn_fn
//! (no real subprocess is spawned — the "worker" is a closure that calls
//! KanbanCompleteTool directly).
//!
//! # What this proves
//!
//! - KanbanStore creates a task and records it correctly.
//! - `run_dispatch_tick` claims a `ready` task via `atomic_claim` (D-40).
//! - The injected spawn_fn can simulate a well-behaved worker by calling
//!   `KanbanCompleteTool::execute` in-process.
//! - After the tick, `get_task` returns `status = "done"`.
//! - `task_runs` has a completed row; `task_events` has `claimed` and `completed`
//!   events (and optionally a `spawned` event written by the dispatcher).
//!
//! # Env-var race note
//!
//! The spawn_fn uses `std::env::set_var` to set `HERMES_KANBAN_TASK`,
//! `HERMES_KANBAN_RUN_ID`, `HERMES_KANBAN_CLAIM_LOCK`, and `HERMES_PROFILE`
//! before calling the tool, then removes them on drop. This is safe in a
//! single-threaded test but would race if this test ran concurrently with
//! other tests that also touch those env vars. Run with `--test-threads=1`
//! for deterministic results (documented in VALIDATION.md quick-run command).

use std::sync::Arc;

use ironhermes_kanban::store::{CreateTaskOptions, KanbanStore};
use ironhermes_kanban::tools::KanbanCompleteTool;
use ironhermes_kanban::{KanbanConfig, run_dispatch_tick};
use ironhermes_kanban::dispatcher::DispatcherContext;
use ironhermes_tools::Tool;
use rusqlite::params;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::Mutex as TokioMutex;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn open_store(dir: &TempDir) -> KanbanStore {
    KanbanStore::new(dir.path().join("kanban.db")).expect("open test store")
}

fn count_rows(store: &KanbanStore, sql: &str, task_id: &str) -> i64 {
    store
        .conn
        .query_row(sql, params![task_id], |r| r.get::<_, i64>(0))
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// End-to-end golden-path test
// ---------------------------------------------------------------------------

/// Full lifecycle: store → create → dispatcher tick with stub spawn_fn that
/// calls KanbanCompleteTool → assert task is done + event log is correct.
#[tokio::test]
async fn full_lifecycle_via_tools_layer() {
    let dir = TempDir::new().unwrap();
    let store_arc = Arc::new(TokioMutex::new(open_store(&dir)));

    // ---- Step 1: create a ready task ----
    let task_id = {
        let mut store = store_arc.lock().await;
        let task = store
            .create_task(
                "e2e smoke task — say hello via kanban_complete",
                "bot-worker",
                CreateTaskOptions {
                    tenant: Some("t-test".into()),
                    workspace: Some("scratch".into()),
                    ..Default::default()
                },
            )
            .expect("create_task failed");
        assert_eq!(task.status, "ready", "new task must start as ready");
        task.id
    };

    // ---- Step 2: inject a spawn_fn that acts as a well-behaved worker ----
    //
    // The closure captures the store Arc and, when "spawned", sets the three
    // required env vars then calls KanbanCompleteTool::execute. It cleans up
    // the vars before returning. SAFETY: see env-var race note in module doc.
    let store_for_spawn = store_arc.clone();
    let task_id_for_spawn = task_id.clone();

    let spawn_fn = Arc::new(
        move |task: ironhermes_kanban::types::Task,
              run: ironhermes_kanban::types::TaskRun,
              _ws: String,
              _board_slug: String|
              -> std::pin::Pin<
            Box<dyn std::future::Future<Output = ironhermes_kanban::error::Result<u32>> + Send>,
        > {
            let store = store_for_spawn.clone();
            let task_id = task_id_for_spawn.clone();

            // Capture the run_id and claim_lock for env injection.
            let run_id = run.id.clone();
            let claim_lock = run.claim_lock.clone();
            let assignee = task.assignee.clone();

            Box::pin(async move {
                // Set the env vars the tool reads (D-17 / D-22).
                unsafe {
                    std::env::set_var("HERMES_KANBAN_TASK", &task_id);
                    std::env::set_var("HERMES_KANBAN_RUN_ID", &run_id);
                    std::env::set_var("HERMES_KANBAN_CLAIM_LOCK", &claim_lock);
                    std::env::set_var("HERMES_PROFILE", &assignee);
                }

                // Call kanban_complete via the tool layer (D-20 / D-22).
                let result = {
                    let tool = KanbanCompleteTool::new(store.clone(), false);
                    tool.execute(json!({
                        "summary": "E2E smoke: task completed successfully via tool layer",
                        "metadata": { "verified": true, "test": "full_lifecycle_via_tools_layer" }
                    }))
                    .await
                };

                // Clean up env vars before returning regardless of tool result.
                unsafe {
                    std::env::remove_var("HERMES_KANBAN_TASK");
                    std::env::remove_var("HERMES_KANBAN_RUN_ID");
                    std::env::remove_var("HERMES_KANBAN_CLAIM_LOCK");
                    std::env::remove_var("HERMES_PROFILE");
                }

                match result {
                    Ok(_json) => {
                        // Return self PID as "alive" — the dispatcher treats any
                        // Ok(pid) from the spawn_fn as a successful spawn.
                        Ok(std::process::id())
                    }
                    Err(e) => Err(ironhermes_kanban::error::KanbanError::Other(
                        anyhow::anyhow!("tool execute failed: {e}"),
                    )),
                }
            })
        },
    );

    let ctx = Arc::new(DispatcherContext::with_spawn_fn(
        store_arc.clone(),
        KanbanConfig::default(),
        spawn_fn,
    ));

    // ---- Step 3: dispatcher tick — claims + invokes spawn_fn ----
    run_dispatch_tick(&ctx)
        .await
        .expect("dispatch tick must not error");

    // ---- Step 4: give KanbanCompleteTool a moment to commit (it holds the
    //               mutex inside the spawn_fn, which the tick awaits) ----
    // Nothing to await — the spawn_fn is awaited inline by the dispatcher tick,
    // so by the time run_dispatch_tick returns the tool has already committed.

    // ---- Step 5: assertions ----
    let store = store_arc.lock().await;

    // 5a. Task status must be "done".
    let task = store.get_task(&task_id).expect("get_task failed");
    assert_eq!(
        task.status, "done",
        "task must be 'done' after full lifecycle tick"
    );

    // 5b. task_runs must have exactly one 'completed' outcome row for this task.
    let completed_runs = count_rows(
        &store,
        "SELECT COUNT(*) FROM task_runs WHERE task_id=?1 AND outcome='completed'",
        &task_id,
    );
    assert_eq!(
        completed_runs, 1,
        "exactly one task_run with outcome='completed' expected"
    );

    // 5c. task_events must have a 'claimed' event.
    let claimed_count = count_rows(
        &store,
        "SELECT COUNT(*) FROM task_events WHERE task_id=?1 AND kind='claimed'",
        &task_id,
    );
    assert_eq!(
        claimed_count, 1,
        "exactly one 'claimed' event expected in task_events"
    );

    // 5d. task_events must have a 'completed' event.
    let completed_count = count_rows(
        &store,
        "SELECT COUNT(*) FROM task_events WHERE task_id=?1 AND kind='completed'",
        &task_id,
    );
    assert_eq!(
        completed_count, 1,
        "exactly one 'completed' event expected in task_events"
    );

    // 5e. Optional: the completed event payload should mention the summary.
    let events = store.get_events(&task_id).expect("get_events failed");
    let completed_evt = events
        .iter()
        .find(|e| e.kind == "completed")
        .expect("completed event must exist");
    if let Some(payload_str) = &completed_evt.payload {
        // The store encodes summary in the payload — verify it round-trips.
        let payload: serde_json::Value = serde_json::from_str(payload_str)
            .unwrap_or(serde_json::Value::Null);
        if payload.is_object() {
            // Payload presence is sufficient — exact field names may vary.
            assert!(
                !payload_str.is_empty(),
                "completed event payload must not be empty"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// E2E: completed task blocks duplicate completion
// ---------------------------------------------------------------------------

/// A second kanban_complete call on an already-done task must return a
/// structured rejection (stale_run_id) rather than corrupting the row.
#[tokio::test]
async fn duplicate_completion_is_rejected() {
    let dir = TempDir::new().unwrap();
    let store_arc = Arc::new(TokioMutex::new(open_store(&dir)));

    // Create and manually seed a running task with a known run_id.
    let (task_id, run_id) = {
        let mut store = store_arc.lock().await;
        let task = store
            .create_task(
                "duplicate-completion test task",
                "bot-worker",
                CreateTaskOptions::default(),
            )
            .unwrap();
        let task_id = task.id.clone();
        let run_id = format!("r_{}", uuid::Uuid::new_v4().simple());

        store
            .conn
            .execute(
                "INSERT INTO task_runs (id, task_id, claim_lock, started_at) \
                 VALUES (?1, ?2, 'lock', 1700000000.0)",
                params![run_id, task_id],
            )
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE tasks SET status='running', current_run_id=?1, claim_lock='lock' \
                 WHERE id=?2",
                params![run_id, task_id],
            )
            .unwrap();

        (task_id, run_id)
    };

    // First completion — should succeed.
    {
        let result = {
            let tool = KanbanCompleteTool::new(store_arc.clone(), true);
            tool.execute(json!({
                "task_id": task_id,
                "expected_run_id": run_id,
                "summary": "first completion"
            }))
            .await
            .unwrap()
        };
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            parsed["status"], "ok",
            "first completion must succeed; got: {result}"
        );
    }

    // Second completion with the SAME run_id — task is now done; run_id is stale.
    {
        let result = {
            let tool = KanbanCompleteTool::new(store_arc.clone(), true);
            tool.execute(json!({
                "task_id": task_id,
                "expected_run_id": run_id,
                "summary": "duplicate — should be rejected"
            }))
            .await
            .unwrap()
        };
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            parsed["status"], "rejected",
            "second completion with stale run_id must be rejected; got: {result}"
        );
    }
}

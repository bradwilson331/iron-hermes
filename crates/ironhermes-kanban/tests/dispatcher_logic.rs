//! Dispatcher 8-step tick loop integration tests (Plan 03, Task 2).
//!
//! Each test seeds a synthetic Task/TaskRun into a tempfile DB and asserts
//! the post-tick state. The dispatcher spawn_fn is injected via
//! `DispatcherContext::with_spawn_fn` so tests never exec `ironhermes`.
//!
//! Test map (D-10 steps covered):
//! - `tick_no_op_when_no_ready_tasks` — step 6: empty DB is a no-op.
//! - `tick_promotes_todo_when_parents_done` — step 5: todo → ready promotion.
//! - `tick_respects_max_in_progress` — step 6: cap enforcement (D-11).
//! - `respawn_guard_blocker_auth` — step 7: 429/auth guard.
//! - `respawn_guard_recent_success` — step 7: recent_success guard.
//! - `respawn_guard_active_pr` — step 7: active_pr guard.
//! - `circuit_breaker_after_failure_limit` — step 8: gave_up + auto-block.
//! - `live_pid_extends_when_alive` — step 2: alive PID extends claim.
//! - `dead_pid_triggers_reclaim` — step 1/3: dead PID → reclaim.
//! - `stranded_task_diagnostic_severity_escalation` — D-14 diagnose_stranded.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::params;
use tempfile::TempDir;
use tokio::sync::Mutex as TokioMutex;

use ironhermes_kanban::{
    KanbanConfig, KanbanStore, StrandedSeverity, diagnose_stranded, run_dispatch_tick,
};
use ironhermes_kanban::dispatcher::DispatcherContext;
use ironhermes_kanban::store::{CreateTaskOptions, ListFilters};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn open_store(dir: &TempDir) -> KanbanStore {
    KanbanStore::new(dir.path().join("kanban.db")).expect("open test store")
}

fn make_ctx_failing_spawn(
    store: Arc<TokioMutex<KanbanStore>>,
    config: KanbanConfig,
) -> Arc<DispatcherContext> {
    let spawn_fn = Arc::new(
        |_task: ironhermes_kanban::types::Task,
         _run: ironhermes_kanban::types::TaskRun,
         _ws: String|
         -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = ironhermes_kanban::error::Result<u32>>
                    + Send,
            >,
        > {
            Box::pin(async move {
                Err(ironhermes_kanban::error::KanbanError::Other(
                    anyhow::anyhow!("spawn_worker stubbed for test — not exec'ing ironhermes"),
                ))
            })
        },
    );
    Arc::new(DispatcherContext::with_spawn_fn(store, config, spawn_fn))
}

fn make_ctx_ok_spawn(
    store: Arc<TokioMutex<KanbanStore>>,
    config: KanbanConfig,
    fake_pid: u32,
) -> Arc<DispatcherContext> {
    let spawn_fn = Arc::new(
        move |_task: ironhermes_kanban::types::Task,
              _run: ironhermes_kanban::types::TaskRun,
              _ws: String|
              -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = ironhermes_kanban::error::Result<u32>>
                    + Send,
            >,
        > { Box::pin(async move { Ok(fake_pid) }) },
    );
    Arc::new(DispatcherContext::with_spawn_fn(store, config, spawn_fn))
}

// Seed a task into running state directly via SQL (bypasses atomic_claim which
// needs &mut Connection that conflicts with TokioMutex borrow patterns).
fn seed_running(
    store: &mut KanbanStore,
    task_id: &str,
    claim_lock: &str,
    claim_pid: i64,
    claim_expires: f64,
    run_id: &str,
    now: f64,
) {
    store
        .conn
        .execute(
            "UPDATE tasks SET status='running', claim_lock=?1, claim_expires=?2, \
             started_at=COALESCE(started_at, ?3), current_run_id=?4 \
             WHERE id=?5",
            params![claim_lock, claim_expires, now, run_id, task_id],
        )
        .unwrap();
    store
        .conn
        .execute(
            "INSERT INTO task_runs (id, task_id, claim_lock, claim_pid, started_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![run_id, task_id, claim_lock, claim_pid, now],
        )
        .unwrap();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Step 6: An empty DB tick is a no-op — zero events, zero spawns.
#[tokio::test]
async fn tick_no_op_when_no_ready_tasks() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(TokioMutex::new(open_store(&dir)));
    let ctx = make_ctx_failing_spawn(store.clone(), KanbanConfig::default());

    run_dispatch_tick(&ctx)
        .await
        .expect("tick should not error on empty DB");

    let s = store.lock().await;
    let tasks = s.list_tasks(ListFilters::default()).unwrap();
    assert!(tasks.is_empty(), "no tasks should exist after no-op tick");
}

/// Step 5: A `todo` task whose only parent is `done` → `ready` after tick,
/// `promoted` event appended.
#[tokio::test]
async fn tick_promotes_todo_when_parents_done() {
    let dir = TempDir::new().unwrap();
    let store_arc = Arc::new(TokioMutex::new(open_store(&dir)));

    let (parent_id, child_id) = {
        let mut store = store_arc.lock().await;
        let parent = store
            .create_task("parent task", "alice", CreateTaskOptions::default())
            .unwrap();

        let child = store
            .create_task(
                "child task",
                "alice",
                CreateTaskOptions {
                    parents: vec![parent.id.clone()],
                    ..Default::default()
                },
            )
            .unwrap();

        // Manually complete the parent (mark as done).
        store
            .complete_task(&parent.id, None, None, None, None, None, "alice")
            .unwrap();

        (parent.id, child.id)
    };

    // Child should start as `todo` (has parents).
    {
        let store = store_arc.lock().await;
        let child = store.get_task(&child_id).unwrap();
        assert_eq!(child.status, "todo", "child must start as todo");
    }

    // Tick with a failing spawn (we only care about promotion, not spawn).
    let ctx = make_ctx_failing_spawn(store_arc.clone(), KanbanConfig::default());
    run_dispatch_tick(&ctx).await.expect("tick failed");

    // After tick: child should be ready, promoted event appended.
    {
        let store = store_arc.lock().await;
        let child = store.get_task(&child_id).unwrap();
        assert_eq!(
            child.status, "ready",
            "child must be ready after tick promotes it"
        );

        let events = store.get_events(&child_id).unwrap();
        let has_promoted = events.iter().any(|e| e.kind == "promoted");
        assert!(has_promoted, "promoted event must be appended to child task");
    }
    let _ = parent_id; // suppress unused warning
}

/// Step 6: max_in_progress cap prevents new claims when running count >= cap.
#[tokio::test]
async fn tick_respects_max_in_progress() {
    let dir = TempDir::new().unwrap();
    let store_arc = Arc::new(TokioMutex::new(open_store(&dir)));

    // Insert 1 already-running task and 3 ready tasks.
    let running_task_id = {
        let mut store = store_arc.lock().await;
        let t = store
            .create_task("running task", "alice", CreateTaskOptions::default())
            .unwrap();

        let now = now_secs();
        let run_id = format!("r_{}", uuid::Uuid::new_v4().simple());
        let claim_lock = format!("host:99:{}", uuid::Uuid::new_v4().simple());
        seed_running(
            &mut store,
            &t.id,
            &claim_lock,
            99,
            now + 900.0,
            &run_id,
            now,
        );
        t.id
    };

    let ready_ids: Vec<String> = {
        let mut store = store_arc.lock().await;
        (0..3)
            .map(|i| {
                store
                    .create_task(
                        &format!("ready task {i}"),
                        "alice",
                        CreateTaskOptions::default(),
                    )
                    .unwrap()
                    .id
            })
            .collect()
    };

    // Config: cap of 1 (already at cap with running_task).
    let config = KanbanConfig {
        max_in_progress: Some(1),
        ..Default::default()
    };

    let ctx = make_ctx_ok_spawn(store_arc.clone(), config, 12345);
    run_dispatch_tick(&ctx).await.expect("tick failed");

    // No ready tasks should have been claimed.
    {
        let store = store_arc.lock().await;
        for rid in &ready_ids {
            let t = store.get_task(rid).unwrap();
            assert_eq!(
                t.status, "ready",
                "task {rid} should still be ready (max_in_progress cap)"
            );
        }
    }
    let _ = running_task_id;
}

/// Step 7: `blocker_auth` respawn-guard skips a ready task whose last error
/// contains "HTTP 429 rate limit".
#[tokio::test]
async fn respawn_guard_blocker_auth() {
    let dir = TempDir::new().unwrap();
    let store_arc = Arc::new(TokioMutex::new(open_store(&dir)));

    let task_id = {
        let mut store = store_arc.lock().await;
        let task = store
            .create_task("auth-blocked task", "alice", CreateTaskOptions::default())
            .unwrap();

        // Seed a closed task_run with a 429 error.
        let now = now_secs();
        let run_id = format!("r_{}", uuid::Uuid::new_v4().simple());
        store
            .conn
            .execute(
                "INSERT INTO task_runs \
                 (id, task_id, claim_lock, started_at, ended_at, outcome, error) \
                 VALUES (?1, ?2, 'lock:1:uuid', ?3, ?3, 'spawn_failed', ?4)",
                params![run_id, task.id, now - 600.0, "HTTP 429 rate limit exceeded"],
            )
            .unwrap();

        task.id
    };

    let ctx = make_ctx_ok_spawn(store_arc.clone(), KanbanConfig::default(), 99999);
    run_dispatch_tick(&ctx).await.expect("tick failed");

    // Task should remain ready (not spawned) and have respawn_guarded event.
    {
        let store = store_arc.lock().await;
        let task = store.get_task(&task_id).unwrap();
        assert_eq!(
            task.status, "ready",
            "task must remain ready — blocker_auth guard"
        );

        let events = store.get_events(&task_id).unwrap();
        let guard_evt = events.iter().find(|e| e.kind == "respawn_guarded");
        assert!(guard_evt.is_some(), "respawn_guarded event must be appended");

        let payload: serde_json::Value =
            serde_json::from_str(guard_evt.unwrap().payload.as_deref().unwrap_or("{}")).unwrap();
        assert_eq!(
            payload["reason"].as_str(),
            Some("blocker_auth"),
            "respawn_guarded reason must be blocker_auth"
        );
    }
}

/// Step 7: `recent_success` respawn-guard skips a task whose last run
/// completed within 3600 seconds.
#[tokio::test]
async fn respawn_guard_recent_success() {
    let dir = TempDir::new().unwrap();
    let store_arc = Arc::new(TokioMutex::new(open_store(&dir)));

    let task_id = {
        let mut store = store_arc.lock().await;
        let task = store
            .create_task(
                "recently-succeeded task",
                "alice",
                CreateTaskOptions::default(),
            )
            .unwrap();

        // Seed a closed task_run that completed 600 seconds ago (< 3600).
        let now = now_secs();
        let run_id = format!("r_{}", uuid::Uuid::new_v4().simple());
        store
            .conn
            .execute(
                "INSERT INTO task_runs \
                 (id, task_id, claim_lock, started_at, ended_at, outcome) \
                 VALUES (?1, ?2, 'lock:2:uuid', ?3, ?4, 'completed')",
                params![run_id, task.id, now - 700.0, now - 600.0],
            )
            .unwrap();

        task.id
    };

    let ctx = make_ctx_ok_spawn(store_arc.clone(), KanbanConfig::default(), 99999);
    run_dispatch_tick(&ctx).await.expect("tick failed");

    {
        let store = store_arc.lock().await;
        let task = store.get_task(&task_id).unwrap();
        assert_eq!(
            task.status, "ready",
            "task must remain ready — recent_success guard"
        );

        let events = store.get_events(&task_id).unwrap();
        let guard_evt = events.iter().find(|e| e.kind == "respawn_guarded");
        assert!(guard_evt.is_some(), "respawn_guarded event must be appended");

        let payload: serde_json::Value =
            serde_json::from_str(guard_evt.unwrap().payload.as_deref().unwrap_or("{}")).unwrap();
        assert_eq!(
            payload["reason"].as_str(),
            Some("recent_success"),
            "respawn_guarded reason must be recent_success"
        );
    }
}

/// Step 7: `active_pr` respawn-guard skips a task that has a recent comment
/// containing a GitHub PR URL.
#[tokio::test]
async fn respawn_guard_active_pr() {
    let dir = TempDir::new().unwrap();
    let store_arc = Arc::new(TokioMutex::new(open_store(&dir)));

    let task_id = {
        let mut store = store_arc.lock().await;
        let task = store
            .create_task(
                "task with active PR",
                "alice",
                CreateTaskOptions::default(),
            )
            .unwrap();

        // Add a comment with a GitHub PR URL (within last 7 days).
        store
            .add_comment(
                &task.id,
                "alice",
                "Review in progress: see https://github.com/foo/bar/pull/42",
            )
            .unwrap();

        task.id
    };

    let ctx = make_ctx_ok_spawn(store_arc.clone(), KanbanConfig::default(), 99999);
    run_dispatch_tick(&ctx).await.expect("tick failed");

    {
        let store = store_arc.lock().await;
        let task = store.get_task(&task_id).unwrap();
        assert_eq!(
            task.status, "ready",
            "task must remain ready — active_pr guard"
        );

        let events = store.get_events(&task_id).unwrap();
        let guard_evt = events.iter().find(|e| e.kind == "respawn_guarded");
        assert!(guard_evt.is_some(), "respawn_guarded event must be appended");

        let payload: serde_json::Value =
            serde_json::from_str(guard_evt.unwrap().payload.as_deref().unwrap_or("{}")).unwrap();
        assert_eq!(
            payload["reason"].as_str(),
            Some("active_pr"),
            "respawn_guarded reason must be active_pr"
        );
    }
}

/// D-12 circuit breaker: after failure_limit consecutive spawn failures,
/// the task is blocked with a `gave_up` event.
#[tokio::test]
async fn circuit_breaker_after_failure_limit() {
    let dir = TempDir::new().unwrap();
    let store_arc = Arc::new(TokioMutex::new(open_store(&dir)));

    let task_id = {
        let mut store = store_arc.lock().await;
        let task = store
            .create_task(
                "circuit-breaker task",
                "alice",
                CreateTaskOptions::default(),
            )
            .unwrap();

        // Pre-seed consecutive_failures = failure_limit - 1 = 1 (default limit is 2).
        // After one more spawn failure the circuit breaker will trip.
        store
            .conn
            .execute(
                "UPDATE tasks SET consecutive_failures = 1 WHERE id = ?1",
                params![task.id],
            )
            .unwrap();

        task.id
    };

    // Config: failure_limit = 2.
    let config = KanbanConfig {
        failure_limit: 2,
        ..Default::default()
    };

    // Failing spawn: one more failure → consecutive_failures reaches 2 → gave_up.
    let ctx = make_ctx_failing_spawn(store_arc.clone(), config);
    run_dispatch_tick(&ctx).await.expect("tick failed");

    {
        let store = store_arc.lock().await;
        let task = store.get_task(&task_id).unwrap();
        assert_eq!(
            task.status, "blocked",
            "task must be blocked after circuit breaker trips"
        );

        let events = store.get_events(&task_id).unwrap();
        let gave_up = events.iter().find(|e| e.kind == "gave_up");
        assert!(gave_up.is_some(), "gave_up event must be appended");
    }
}

/// D-12 circuit breaker (crashed-detection path, BUG-36.3.7-03):
/// A task with `consecutive_failures = failure_limit - 1 = 1` and a dead PID
/// reaches exactly `failure_limit = 2` via `detect_crashed_workers`, which
/// must invoke `apply_circuit_breaker` on the same tick → task blocked,
/// `gave_up` event emitted.
#[tokio::test]
async fn circuit_breaker_trips_on_crashed_detection_path() {
    let dir = TempDir::new().unwrap();
    let store_arc = Arc::new(TokioMutex::new(open_store(&dir)));

    let dead_pid: u32 = 999_999_999; // guaranteed dead
    let task_id;

    {
        let mut store = store_arc.lock().await;
        let task = store
            .create_task(
                "crashed-breaker trip task",
                "alice",
                CreateTaskOptions::default(),
            )
            .unwrap();

        let now = now_secs();
        let rid = format!("r_{}", uuid::Uuid::new_v4().simple());
        let claim_lock = format!("host:{}:uuid_cb_trip", dead_pid);

        // Seed as running with a dead PID.
        seed_running(
            &mut store,
            &task.id,
            &claim_lock,
            dead_pid as i64,
            now + 900.0,
            &rid,
            now,
        );

        // Pre-seed consecutive_failures = failure_limit - 1 = 1.
        // detect_crashed_workers will bump to 2 (== failure_limit), tripping the breaker.
        store
            .conn
            .execute(
                "UPDATE tasks SET consecutive_failures = 1 WHERE id = ?1",
                params![task.id],
            )
            .unwrap();

        task_id = task.id;
    }

    // Config: failure_limit = 2 (default).
    let config = KanbanConfig {
        failure_limit: 2,
        ..Default::default()
    };
    // Use ok_spawn so that the spawner doesn't interfere — the task should be
    // blocked by the circuit breaker before any new spawn happens.
    let ctx = make_ctx_ok_spawn(store_arc.clone(), config, 12345);
    run_dispatch_tick(&ctx).await.expect("tick failed");

    {
        let store = store_arc.lock().await;
        let task = store.get_task(&task_id).unwrap();
        assert_eq!(
            task.status, "blocked",
            "task must be blocked when crashed-detection path hits failure_limit"
        );

        let events = store.get_events(&task_id).unwrap();
        let crashed = events.iter().find(|e| e.kind == "crashed");
        assert!(crashed.is_some(), "crashed event must be appended");
        let gave_up = events.iter().find(|e| e.kind == "gave_up");
        assert!(
            gave_up.is_some(),
            "gave_up event must be appended on the same tick the limit is reached"
        );
    }
}

/// D-12 circuit breaker (crashed-detection path, BUG-36.3.7-03):
/// A task with `consecutive_failures = 0` and a dead PID bumps to 1 via
/// `detect_crashed_workers` — below `failure_limit = 2` — so the circuit
/// breaker must NOT fire. Task must be `ready` (claim released), `crashed`
/// event present, and NO `gave_up` event emitted.
#[tokio::test]
async fn circuit_breaker_does_not_trip_below_limit_on_crashed_path() {
    let dir = TempDir::new().unwrap();
    let store_arc = Arc::new(TokioMutex::new(open_store(&dir)));

    let dead_pid: u32 = 999_999_998; // guaranteed dead (different PID from sibling test)
    let task_id;

    {
        let mut store = store_arc.lock().await;
        let task = store
            .create_task(
                "crashed-breaker no-trip task",
                "alice",
                CreateTaskOptions::default(),
            )
            .unwrap();

        let now = now_secs();
        let rid = format!("r_{}", uuid::Uuid::new_v4().simple());
        let claim_lock = format!("host:{}:uuid_cb_notrip", dead_pid);

        // Seed as running with a dead PID.
        seed_running(
            &mut store,
            &task.id,
            &claim_lock,
            dead_pid as i64,
            now + 900.0,
            &rid,
            now,
        );

        // consecutive_failures starts at 0 (default). detect_crashed_workers bumps to 1,
        // which is below failure_limit = 2, so the breaker must NOT fire.

        // Set scheduled_at to a future time so the dispatcher does NOT re-claim the
        // task in the same tick (prevents the spawn-failure path from also bumping
        // consecutive_failures and tripping the breaker at 2).
        store
            .conn
            .execute(
                "UPDATE tasks SET scheduled_at = ?1 WHERE id = ?2",
                params![now_secs() + 3600.0, task.id],
            )
            .unwrap();

        task_id = task.id;
    }

    // Config: failure_limit = 2 (default).
    let config = KanbanConfig {
        failure_limit: 2,
        ..Default::default()
    };
    let ctx = make_ctx_failing_spawn(store_arc.clone(), config);
    run_dispatch_tick(&ctx).await.expect("tick failed");

    {
        let store = store_arc.lock().await;
        let task = store.get_task(&task_id).unwrap();
        assert_eq!(
            task.status, "ready",
            "task must be ready (claim released, not blocked) when below failure_limit"
        );
        assert_eq!(
            task.consecutive_failures, 1,
            "consecutive_failures must be 1 after one crash (below limit)"
        );

        let events = store.get_events(&task_id).unwrap();
        let crashed = events.iter().find(|e| e.kind == "crashed");
        assert!(crashed.is_some(), "crashed event must be appended");
        let gave_up = events.iter().any(|e| e.kind == "gave_up");
        assert!(
            !gave_up,
            "gave_up event must NOT be emitted when consecutive_failures < failure_limit"
        );
    }
}

/// Step 2: A running task with alive PID and expired claim → `claim_extended`
/// event, claim_expires updated, task still running.
#[tokio::test]
async fn live_pid_extends_when_alive() {
    let dir = TempDir::new().unwrap();
    let store_arc = Arc::new(TokioMutex::new(open_store(&dir)));

    let self_pid = std::process::id();
    let task_id;

    {
        let mut store = store_arc.lock().await;
        let task = store
            .create_task("live-pid task", "alice", CreateTaskOptions::default())
            .unwrap();

        let now = now_secs();
        let rid = format!("r_{}", uuid::Uuid::new_v4().simple());
        let claim_lock = format!("host:{}:uuid_live", self_pid);
        let past_expires = now - 10.0; // expired 10s ago

        seed_running(
            &mut store,
            &task.id,
            &claim_lock,
            self_pid as i64,
            now + 900.0,
            &rid,
            now,
        );

        // Override claim_expires to be in the past.
        store
            .conn
            .execute(
                "UPDATE tasks SET claim_expires = ?1 WHERE id = ?2",
                params![past_expires, task.id],
            )
            .unwrap();

        task_id = task.id;
    }

    let ctx = make_ctx_ok_spawn(store_arc.clone(), KanbanConfig::default(), 12345);
    run_dispatch_tick(&ctx).await.expect("tick failed");

    {
        let store = store_arc.lock().await;
        let task = store.get_task(&task_id).unwrap();

        // Task must still be running.
        assert_eq!(
            task.status, "running",
            "task must remain running after live-PID extension"
        );

        // claim_expires must have been extended.
        let new_expires = task.claim_expires.expect("claim_expires must be set");
        assert!(
            new_expires > now_secs() - 5.0,
            "claim_expires must be extended beyond the old expired value"
        );

        // claim_extended event must be present.
        let events = store.get_events(&task_id).unwrap();
        let extended = events.iter().find(|e| e.kind == "claim_extended");
        assert!(extended.is_some(), "claim_extended event must be appended");
    }
}

/// Step 1/3: A running task with dead PID and expired claim → reclaimed,
/// status reset to `ready`.
#[tokio::test]
async fn dead_pid_triggers_reclaim() {
    let dir = TempDir::new().unwrap();
    let store_arc = Arc::new(TokioMutex::new(open_store(&dir)));

    let dead_pid: u32 = 99999999; // guaranteed dead
    let task_id;

    {
        let mut store = store_arc.lock().await;
        let task = store
            .create_task("dead-pid task", "alice", CreateTaskOptions::default())
            .unwrap();

        let now = now_secs();
        let rid = format!("r_{}", uuid::Uuid::new_v4().simple());
        let claim_lock = format!("host:{}:uuid_dead", dead_pid);
        let past_expires = now - 10.0;

        seed_running(
            &mut store,
            &task.id,
            &claim_lock,
            dead_pid as i64,
            now + 900.0,
            &rid,
            now,
        );

        // Override claim_expires to past.
        store
            .conn
            .execute(
                "UPDATE tasks SET claim_expires = ?1 WHERE id = ?2",
                params![past_expires, task.id],
            )
            .unwrap();

        task_id = task.id;
    }

    let ctx = make_ctx_ok_spawn(store_arc.clone(), KanbanConfig::default(), 12345);
    run_dispatch_tick(&ctx).await.expect("tick failed");

    {
        let store = store_arc.lock().await;
        let task = store.get_task(&task_id).unwrap();
        // After detect_crashed or reclaim_stale, task should be ready (or running if
        // re-claimed in the same tick).
        assert!(
            task.status == "ready" || task.status == "running",
            "task must be ready or reclaimed after dead PID detected; got {}",
            task.status
        );
        // At minimum, consecutive_failures should have been bumped.
        assert!(
            task.consecutive_failures > 0,
            "consecutive_failures must be incremented after dead PID"
        );
    }
}

/// D-14: `diagnose_stranded` returns severity-escalated reports at 1x/2x/6x
/// threshold. Non-stranded tasks must NOT appear in the result.
/// This is VALIDATION.md critical invariant #6.
#[test]
fn stranded_task_diagnostic_severity_escalation() {
    let dir = TempDir::new().unwrap();
    let mut store = open_store(&dir);

    let threshold_secs: u64 = 300; // 5 min threshold for the test
    let now = now_secs();

    // Task 1: Warn band — age = threshold + 1s
    let warn_task = store
        .create_task("warn task", "alice", CreateTaskOptions::default())
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE tasks SET created_at = ?1 WHERE id = ?2",
            params![now - (threshold_secs as f64 + 1.0), warn_task.id],
        )
        .unwrap();

    // Task 2: Error band — age = 2*threshold + 1s
    let error_task = store
        .create_task("error task", "alice", CreateTaskOptions::default())
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE tasks SET created_at = ?1 WHERE id = ?2",
            params![now - (2.0 * threshold_secs as f64 + 1.0), error_task.id],
        )
        .unwrap();

    // Task 3: Critical band — age = 6*threshold + 1s
    let critical_task = store
        .create_task("critical task", "alice", CreateTaskOptions::default())
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE tasks SET created_at = ?1 WHERE id = ?2",
            params![now - (6.0 * threshold_secs as f64 + 1.0), critical_task.id],
        )
        .unwrap();

    // Task 4: NOT stranded — age = 10s (well under threshold).
    let fresh_task = store
        .create_task("fresh task", "alice", CreateTaskOptions::default())
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE tasks SET created_at = ?1 WHERE id = ?2",
            params![now - 10.0, fresh_task.id],
        )
        .unwrap();

    let reports = diagnose_stranded(&store, threshold_secs).unwrap();

    // Fresh task must NOT appear.
    assert!(
        !reports.iter().any(|r| r.task_id == fresh_task.id),
        "fresh task (under threshold) must NOT appear in stranded report"
    );

    // Warn band task.
    let warn_report = reports
        .iter()
        .find(|r| r.task_id == warn_task.id)
        .expect("warn-band task must appear in stranded report");
    assert_eq!(
        warn_report.severity,
        StrandedSeverity::Warn,
        "warn-band task must have Warn severity"
    );

    // Error band task.
    let error_report = reports
        .iter()
        .find(|r| r.task_id == error_task.id)
        .expect("error-band task must appear in stranded report");
    assert_eq!(
        error_report.severity,
        StrandedSeverity::Error,
        "error-band task must have Error severity"
    );

    // Critical band task.
    let critical_report = reports
        .iter()
        .find(|r| r.task_id == critical_task.id)
        .expect("critical-band task must appear in stranded report");
    assert_eq!(
        critical_report.severity,
        StrandedSeverity::Critical,
        "critical-band task must have Critical severity"
    );
}

/// D-12 circuit breaker (reclaim-stale-claims path, BUG-36.3.7.1-01):
/// A task with `consecutive_failures = failure_limit - 1 = 1`, an expired
/// `claim_expires`, and a NULL `claim_pid` reaches exactly `failure_limit = 2`
/// via `reclaim_stale_claims`, which must invoke `apply_circuit_breaker` on
/// the same tick → task blocked, `gave_up` event emitted.
///
/// Mirrors `circuit_breaker_trips_on_crashed_detection_path` (the 36.3.7.0-03
/// canonical receiver-end shape) with the path-specific seed differences
/// required to drive the reclaim branch rather than the crashed branch.
///
/// Producer-targeting design (Rule-1 deviation, see 36.3.7.1-01-SUMMARY.md):
/// `detect_crashed_workers` (Step 1) catches dead-but-non-null PIDs first and
/// would invoke ITS OWN breaker call at dispatcher.rs:305, masking the NEW
/// line-481 reclaim-path breaker. To prove the NEW breaker actually fires
/// (and not the line-305 site), we NULL out `claim_pid` after `seed_running`
/// — Step 1 skips tasks with `run.claim_pid == None` (dispatcher.rs:247),
/// so Step 3's `reclaim_stale_claims` is the only path that can bump
/// consecutive_failures and tip the limit. The plan's error_msg ("claim TTL
/// expired with dead or null PID") explicitly names this null-PID case.
#[tokio::test]
async fn circuit_breaker_trips_on_reclaim_path() {
    let dir = TempDir::new().unwrap();
    let store_arc = Arc::new(TokioMutex::new(open_store(&dir)));

    // Use a distinct dead PID from the two crashed-path tests (999_999_999,
    // 999_999_998) to avoid accidental coupling — even though we NULL it
    // below, the value is still seeded so any escape is traceable.
    let dead_pid: u32 = 999_999_997;
    let task_id;

    {
        let mut store = store_arc.lock().await;
        let task = store
            .create_task(
                "reclaim-breaker trip task",
                "alice",
                CreateTaskOptions::default(),
            )
            .unwrap();

        let now = now_secs();
        let rid = format!("r_{}", uuid::Uuid::new_v4().simple());
        let claim_lock = format!("host:{}:uuid_cb_reclaim_trip", dead_pid);

        // Seed as running with an EXPIRED claim (claim_expires strictly < now)
        // so `reclaim_stale_claims` (Step 3) actually fires the reclaim branch
        // — unlike the crashed-path tests which use now + 900.0 (future).
        seed_running(
            &mut store,
            &task.id,
            &claim_lock,
            dead_pid as i64,
            now - 1.0,
            &rid,
            now,
        );

        // NULL claim_pid on the run row so Step 1 (detect_crashed_workers)
        // skips this task (its filter requires `run.claim_pid.is_some()` —
        // dispatcher.rs:247). This isolates the new reclaim-path breaker at
        // dispatcher.rs:481 as the sole producer for this assertion. The
        // reclaim path treats null PID as "not alive" (dispatcher.rs:427-431).
        store
            .conn
            .execute(
                "UPDATE task_runs SET claim_pid = NULL WHERE id = ?1",
                params![rid],
            )
            .unwrap();

        // Pre-seed consecutive_failures = failure_limit - 1 = 1.
        // reclaim_stale_claims will bump to 2 (== failure_limit), tripping the
        // new line-481 breaker on the same tick.
        store
            .conn
            .execute(
                "UPDATE tasks SET consecutive_failures = 1 WHERE id = ?1",
                params![task.id],
            )
            .unwrap();

        task_id = task.id;
    }

    // Config: failure_limit = 2 (default).
    let config = KanbanConfig {
        failure_limit: 2,
        ..Default::default()
    };
    // Use ok_spawn so that the spawner doesn't interfere — the task should be
    // blocked by the circuit breaker before any new spawn happens. Mirrors
    // the crashed-path trip test's choice.
    let ctx = make_ctx_ok_spawn(store_arc.clone(), config, 12345);
    run_dispatch_tick(&ctx).await.expect("tick failed");

    {
        let store = store_arc.lock().await;
        let task = store.get_task(&task_id).unwrap();
        assert_eq!(
            task.status, "blocked",
            "reclaim_stale_claims path: breaker did not fire at failure_limit (BUG-36.3.7.1-01)"
        );
        assert_eq!(
            task.consecutive_failures, 2,
            "reclaim_stale_claims path: consecutive_failures must be 2 after bump at failure_limit (BUG-36.3.7.1-01)"
        );

        let events = store.get_events(&task_id).unwrap();
        let gave_up = events.iter().find(|e| e.kind == "gave_up");
        assert!(
            gave_up.is_some(),
            "reclaim_stale_claims path: gave_up event must be appended on the same tick the limit is reached (BUG-36.3.7.1-01)"
        );
    }
}

/// D-12 circuit breaker (reclaim-stale-claims path, BUG-36.3.7.1-01):
/// A task with `consecutive_failures = 0`, an expired `claim_expires`, and a
/// NULL `claim_pid` bumps to 1 via `reclaim_stale_claims` — below
/// `failure_limit = 2` — so the circuit breaker must NOT fire. Task must be
/// `ready` (claim released), `consecutive_failures = 1`, and NO `gave_up`
/// event emitted.
///
/// Mirrors `circuit_breaker_does_not_trip_below_limit_on_crashed_path` with
/// the path-specific seed differences. Same null-PID isolation rationale as
/// the trip test's docstring: NULL `claim_pid` so Step 1 skips, ensuring the
/// new line-481 reclaim-path breaker is the ONLY potential producer (so this
/// negative test actually locks the new producer's no-fire behavior, not
/// Step 1's). The `scheduled_at = now + 3600.0` guard prevents step-6
/// re-claim from tipping the spawn-failure breaker (same workaround as the
/// crashed-path no-trip test at lines 608-617).
///
/// Latent-finding lock (CONTEXT.md, 36.3.7.1-01-SUMMARY.md):
/// `reclaim_stale_claims` currently emits NO `Reclaimed` event — only a
/// `tracing::info!(event = "reclaimed", ...)` at dispatcher.rs:437-441.
/// dispatcher.rs:19's doc comment says the function "appends a `reclaimed`
/// event" but no `store.append_event(KanbanEventKind::Reclaimed)` call
/// exists. This is intentionally OUT OF SCOPE for Plan 01 — `apply_circuit_breaker`
/// does NOT depend on the event. We document the absence by asserting NO
/// `gave_up` event (the only event this path would emit, via the breaker).
#[tokio::test]
async fn circuit_breaker_does_not_trip_below_limit_on_reclaim_path() {
    let dir = TempDir::new().unwrap();
    let store_arc = Arc::new(TokioMutex::new(open_store(&dir)));

    // Distinct dead PID from the trip test and the two crashed-path tests
    // (NULLed below — value retained for traceability only).
    let dead_pid: u32 = 999_999_996;
    let task_id;

    {
        let mut store = store_arc.lock().await;
        let task = store
            .create_task(
                "reclaim-breaker no-trip task",
                "alice",
                CreateTaskOptions::default(),
            )
            .unwrap();

        let now = now_secs();
        let rid = format!("r_{}", uuid::Uuid::new_v4().simple());
        let claim_lock = format!("host:{}:uuid_cb_reclaim_notrip", dead_pid);

        // Seed as running with an EXPIRED claim.
        seed_running(
            &mut store,
            &task.id,
            &claim_lock,
            dead_pid as i64,
            now - 1.0,
            &rid,
            now,
        );

        // NULL claim_pid (same producer-isolation rationale as the trip test).
        store
            .conn
            .execute(
                "UPDATE task_runs SET claim_pid = NULL WHERE id = ?1",
                params![rid],
            )
            .unwrap();

        // consecutive_failures starts at 0 (default). reclaim_stale_claims bumps
        // to 1, which is below failure_limit = 2, so the new line-481 breaker
        // must NOT fire.

        // Set scheduled_at to a future time so the dispatcher does NOT re-claim
        // the task in the same tick (prevents the spawn-failure path from also
        // bumping consecutive_failures and tripping the breaker at 2). Same
        // workaround as `circuit_breaker_does_not_trip_below_limit_on_crashed_path`.
        store
            .conn
            .execute(
                "UPDATE tasks SET scheduled_at = ?1 WHERE id = ?2",
                params![now_secs() + 3600.0, task.id],
            )
            .unwrap();

        task_id = task.id;
    }

    // Config: failure_limit = 2 (default).
    let config = KanbanConfig {
        failure_limit: 2,
        ..Default::default()
    };
    let ctx = make_ctx_failing_spawn(store_arc.clone(), config);
    run_dispatch_tick(&ctx).await.expect("tick failed");

    {
        let store = store_arc.lock().await;
        let task = store.get_task(&task_id).unwrap();
        assert_eq!(
            task.status, "ready",
            "reclaim_stale_claims path: breaker fired below failure_limit (BUG-36.3.7.1-01 over-fire)"
        );
        assert_eq!(
            task.consecutive_failures, 1,
            "reclaim_stale_claims path: consecutive_failures must be 1 after one reclaim (below limit)"
        );

        let events = store.get_events(&task_id).unwrap();
        let gave_up = events.iter().any(|e| e.kind == "gave_up");
        assert!(
            !gave_up,
            "reclaim_stale_claims path: gave_up event must NOT be emitted when consecutive_failures < failure_limit"
        );
    }
}

/// D-12 circuit breaker (max-runtime-exceeded path, BUG-36.3.7.1-02):
/// A task whose worker exceeded `max_runtime_seconds` AND whose
/// `consecutive_failures = failure_limit - 1 = 1` reaches exactly
/// `failure_limit = 2` via `enforce_max_runtime`, which must invoke
/// `apply_circuit_breaker` on the same tick → task blocked,
/// `timed_out` event emitted, `gave_up` event emitted.
#[tokio::test]
async fn circuit_breaker_trips_on_max_runtime_path() {
    // Note: this test takes ~6 seconds wall-clock due to the SIGTERM grace
    // sleep at dispatcher.rs (tokio::time::sleep(Duration::from_secs(5))).
    let dir = TempDir::new().unwrap();
    let store_arc = Arc::new(TokioMutex::new(open_store(&dir)));

    // Spawn a real `sleep 120` child so the PID is alive. Plan's dead-PID
    // approach is incompatible: step 1 (detect_crashed_workers) gates on
    // dead-PID and would intercept the task BEFORE step 4 (enforce_max_runtime)
    // sees it. SIGTERM/SIGKILL from the dispatcher safely targets our owned
    // child, not the test runner. See SUMMARY's deviation section.
    let mut child = std::process::Command::new("sleep")
        .arg("120")
        .spawn()
        .expect("spawn sleep child for max_runtime trip test");
    let live_pid: u32 = child.id();
    let task_id;

    {
        let mut store = store_arc.lock().await;
        let task = store
            .create_task(
                "max-runtime-breaker trip task",
                "alice",
                CreateTaskOptions {
                    max_runtime_seconds: Some(60),
                    ..Default::default()
                },
            )
            .unwrap();

        let now = now_secs();
        let rid = format!("r_{}", uuid::Uuid::new_v4().simple());
        let claim_lock = format!("host:{}:uuid_mr_trip", live_pid);

        // Seed as running with the live child PID; claim_expires FUTURE so
        // reclaim_stale_claims and extend_live_pid_claims do NOT fire (both
        // gate on claim_expires < now). detect_crashed_workers gates on
        // dead PID — alive PID skips it.
        seed_running(
            &mut store,
            &task.id,
            &claim_lock,
            live_pid as i64,
            now + 900.0,
            &rid,
            now,
        );

        // Overwrite started_at to 2 hours ago so elapsed = 7200 > 60 = max_runtime,
        // triggering the enforce_max_runtime branch. seed_running uses COALESCE so
        // we must overwrite via direct SQL UPDATE.
        store
            .conn
            .execute(
                "UPDATE tasks SET started_at = ?1 WHERE id = ?2",
                params![now - 7200.0, task.id],
            )
            .unwrap();

        // Pre-seed consecutive_failures = failure_limit - 1 = 1.
        // enforce_max_runtime will bump to 2 (== failure_limit), tripping the breaker.
        store
            .conn
            .execute(
                "UPDATE tasks SET consecutive_failures = 1 WHERE id = ?1",
                params![task.id],
            )
            .unwrap();

        task_id = task.id;
    }

    // Config: failure_limit = 2 (default).
    let config = KanbanConfig {
        failure_limit: 2,
        ..Default::default()
    };
    // Use ok_spawn so that the spawner doesn't interfere — the task should be
    // blocked by the circuit breaker before any new spawn happens.
    let ctx = make_ctx_ok_spawn(store_arc.clone(), config, 12345);
    run_dispatch_tick(&ctx).await.expect("tick failed");

    // Reap the child (dispatcher SIGTERM'd it during enforce_max_runtime).
    // Avoids leaving a zombie even if SIGKILL also fired.
    let _ = child.wait();

    {
        let store = store_arc.lock().await;
        let task = store.get_task(&task_id).unwrap();
        assert_eq!(
            task.status, "blocked",
            "enforce_max_runtime path: breaker did not fire at failure_limit (BUG-36.3.7.1-02)"
        );

        let events = store.get_events(&task_id).unwrap();
        let timed_out = events.iter().find(|e| e.kind == "timed_out");
        assert!(
            timed_out.is_some(),
            "enforce_max_runtime path: timed_out event must be appended (BUG-36.3.7.1-02)"
        );
        let gave_up = events.iter().find(|e| e.kind == "gave_up");
        assert!(
            gave_up.is_some(),
            "enforce_max_runtime path: gave_up event must be appended on the same tick the limit is reached (BUG-36.3.7.1-02)"
        );
    }
}

/// D-12 circuit breaker (max-runtime-exceeded path, BUG-36.3.7.1-02):
/// A task whose worker exceeded `max_runtime_seconds` AND whose
/// `consecutive_failures = 0` bumps to 1 via `enforce_max_runtime` — below
/// `failure_limit = 2` — so the circuit breaker must NOT fire. Task must be
/// `ready` (claim released), `timed_out` event present, and NO `gave_up`
/// event emitted.
#[tokio::test]
async fn circuit_breaker_does_not_trip_below_limit_on_max_runtime_path() {
    // Note: this test takes ~6 seconds wall-clock due to the SIGTERM grace
    // sleep at dispatcher.rs (tokio::time::sleep(Duration::from_secs(5))).
    let dir = TempDir::new().unwrap();
    let store_arc = Arc::new(TokioMutex::new(open_store(&dir)));

    // Spawn a real `sleep 120` child so the PID is alive — same rationale as
    // the at-limit test. See SUMMARY's deviation section.
    let mut child = std::process::Command::new("sleep")
        .arg("120")
        .spawn()
        .expect("spawn sleep child for max_runtime no-trip test");
    let live_pid: u32 = child.id();
    let task_id;

    {
        let mut store = store_arc.lock().await;
        let task = store
            .create_task(
                "max-runtime-breaker no-trip task",
                "alice",
                CreateTaskOptions {
                    max_runtime_seconds: Some(60),
                    ..Default::default()
                },
            )
            .unwrap();

        let now = now_secs();
        let rid = format!("r_{}", uuid::Uuid::new_v4().simple());
        let claim_lock = format!("host:{}:uuid_mr_notrip", live_pid);

        // Seed as running with the live child PID; claim_expires FUTURE so
        // reclaim_stale_claims and extend_live_pid_claims do NOT fire.
        seed_running(
            &mut store,
            &task.id,
            &claim_lock,
            live_pid as i64,
            now + 900.0,
            &rid,
            now,
        );

        // Overwrite started_at to 2 hours ago so elapsed = 7200 > 60 = max_runtime,
        // triggering the enforce_max_runtime branch.
        store
            .conn
            .execute(
                "UPDATE tasks SET started_at = ?1 WHERE id = ?2",
                params![now - 7200.0, task.id],
            )
            .unwrap();

        // consecutive_failures starts at 0 (default). enforce_max_runtime bumps to 1,
        // which is below failure_limit = 2, so the breaker must NOT fire.

        // Set scheduled_at to a future time so the dispatcher does NOT re-claim the
        // task in the same tick (prevents the spawn-failure path from also bumping
        // consecutive_failures and tripping the breaker at 2). Same workaround as
        // circuit_breaker_does_not_trip_below_limit_on_crashed_path.
        store
            .conn
            .execute(
                "UPDATE tasks SET scheduled_at = ?1 WHERE id = ?2",
                params![now_secs() + 3600.0, task.id],
            )
            .unwrap();

        task_id = task.id;
    }

    // Config: failure_limit = 2 (default).
    let config = KanbanConfig {
        failure_limit: 2,
        ..Default::default()
    };
    let ctx = make_ctx_failing_spawn(store_arc.clone(), config);
    run_dispatch_tick(&ctx).await.expect("tick failed");

    // Reap the child (dispatcher SIGTERM'd it during enforce_max_runtime).
    let _ = child.wait();

    {
        let store = store_arc.lock().await;
        let task = store.get_task(&task_id).unwrap();
        assert_eq!(
            task.status, "ready",
            "enforce_max_runtime path: breaker fired below failure_limit (BUG-36.3.7.1-02 over-fire)"
        );
        assert_eq!(
            task.consecutive_failures, 1,
            "enforce_max_runtime path: consecutive_failures must be 1 after one timeout (below limit)"
        );

        let events = store.get_events(&task_id).unwrap();
        let timed_out = events.iter().find(|e| e.kind == "timed_out");
        assert!(
            timed_out.is_some(),
            "enforce_max_runtime path: timed_out event must be appended (BUG-36.3.7.1-02)"
        );
        let gave_up = events.iter().any(|e| e.kind == "gave_up");
        assert!(
            !gave_up,
            "enforce_max_runtime path: gave_up event must NOT be emitted when consecutive_failures < failure_limit (BUG-36.3.7.1-02)"
        );
    }
}

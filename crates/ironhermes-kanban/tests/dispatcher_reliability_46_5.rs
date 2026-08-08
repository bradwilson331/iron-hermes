//! Phase 46.5 Plan 03 — dispatcher/worker-spawn reliability regression tests.
//!
//! Covers D-04 (enriched crash diagnostic + stderr tail), D-05 (scratch-only
//! workspace re-init on retry), and D-03 (a worker that exits without a
//! terminal event stays `blocked`, never auto-completed).
//!
//! Test map:
//! - `diagnostic_includes_stderr_tail` — D-04: the block diagnostic carries a
//!   human reason + non-empty stderr tail, replacing the bare crash string.
//! - `diagnostic_missing_log_no_panic` — D-04: a missing/unreadable stderr
//!   log yields the reason with an empty tail, no panic.
//! - `reinit_scratch_wipes_on_retry` — D-05: a scratch task retry
//!   (consecutive_failures > 0) wipes pre-seeded stale leftovers.
//! - `reinit_scratch_noop_first_spawn` — D-05: a scratch task's first spawn
//!   (consecutive_failures == 0) leaves existing contents untouched.
//! - `dir_project_workspace_never_wiped` — D-05 prohibition guard: `dir:`/
//!   `project:` workspaces are never acted on by the helper, regardless of
//!   `consecutive_failures`.
//! - `clean_exit_no_terminator_blocks_never_completes` — D-03: across
//!   `failure_limit` real dispatcher ticks driven by dead-PID detection
//!   (the v1 heuristic's only observable shape for "exited without calling
//!   kanban_complete/kanban_block" — see `tests/protocol_violation.rs` module
//!   doc), the task_runs.outcome sequence is crashed×N → blocked; the task
//!   is NEVER auto-completed and NEVER reaches `done`.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::params;
use tempfile::TempDir;
use tokio::sync::Mutex as TokioMutex;

use ironhermes_kanban::dispatcher::DispatcherContext;
use ironhermes_kanban::store::CreateTaskOptions;
use ironhermes_kanban::types::Task;
use ironhermes_kanban::{KanbanConfig, KanbanStore, run_dispatch_tick};

// ---------------------------------------------------------------------------
// Env-mutation guard (IRONHERMES_HOME) — this file's tests that touch
// profile-scoped log/workspace paths MUST hold this lock. Mirrors the
// per-file ENV_LOCK pattern established in worker_bin_resolution.rs /
// cycle_detection_per_board.rs (each integration-test file is its own
// process under nextest, so a file-local lock only needs to serialise
// *this file's* tests against each other).
// ---------------------------------------------------------------------------

// Async-aware mutex — several tests in this file hold the guard across
// `.await` points (dispatcher ticks), which `std::sync::Mutex` forbids
// (clippy::await_holding_lock). Mirrors the `tokio::sync::Mutex` ENV_LOCK
// pattern in `tests/tools_smoke.rs` / `tests/notifier_multi_board.rs`.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// RAII guard that sets an env var and restores the previous value on drop.
struct ScopedEnv {
    key: String,
    prev: Option<String>,
}

impl ScopedEnv {
    fn set(key: &str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        // Safety: test-only; callers hold ENV_LOCK for the duration.
        unsafe { std::env::set_var(key, value) };
        Self {
            key: key.to_string(),
            prev,
        }
    }
}

impl Drop for ScopedEnv {
    fn drop(&mut self) {
        // Safety: see `set` above.
        match &self.prev {
            Some(v) => unsafe { std::env::set_var(&self.key, v) },
            None => unsafe { std::env::remove_var(&self.key) },
        }
    }
}

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

/// A bare-minimum synthetic `Task`, for tests that exercise
/// `reinit_scratch_workspace_if_retry` directly without going through the
/// store (mirrors `worker_spawn.rs`'s own in-crate `fake_task` helper).
fn fake_task(id: &str) -> Task {
    Task {
        id: id.to_string(),
        title: "test task".to_string(),
        body: None,
        assignee: "alice".to_string(),
        status: "ready".to_string(),
        priority: 0,
        tenant: None,
        workspace: None,
        skills: None,
        idempotency_key: None,
        claim_lock: None,
        claim_expires: None,
        current_run_id: None,
        consecutive_failures: 0,
        max_retries: None,
        max_runtime_seconds: None,
        scheduled_at: None,
        workflow_template_id: None,
        current_step_key: None,
        created_by: None,
        created_at: 1_700_000_000.0,
        started_at: None,
        ended_at: None,
        goal_mode: false,
        goal_max_turns: 20,
        goal_turns_used: 0,
        goal_toolset: None,
        output_path: None,
    }
}

/// Seed a task directly into `running` state via SQL (bypasses atomic_claim,
/// which needs `&mut Connection` that conflicts with the `TokioMutex` borrow
/// patterns used elsewhere in these tests). Mirrors `dispatcher_logic.rs`.
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

fn make_ctx_ok_spawn(
    store: Arc<TokioMutex<KanbanStore>>,
    config: KanbanConfig,
    fake_pid: u32,
) -> Arc<DispatcherContext> {
    let spawn_fn = Arc::new(
        move |_task: ironhermes_kanban::types::Task,
              _run: ironhermes_kanban::types::TaskRun,
              _ws: String,
              _board_slug: String|
              -> std::pin::Pin<
            Box<dyn std::future::Future<Output = ironhermes_kanban::error::Result<u32>> + Send>,
        > { Box::pin(async move { Ok(fake_pid) }) },
    );
    Arc::new(DispatcherContext::with_spawn_fn(store, config, spawn_fn).with_gate_fn(allow_all_gate()))
}

// ---------------------------------------------------------------------------
// D-04: enriched crash diagnostic + stderr tail
// ---------------------------------------------------------------------------

/// D-04: when a worker exits without a terminal event and the circuit
/// breaker trips, the block diagnostic (task_runs.error / the `blocked`
/// event payload) carries BOTH a human reason AND a non-empty tail of the
/// worker's profile-scoped stderr — replacing the old bare
/// "worker process crashed (pid=…)" string.
#[tokio::test]
async fn diagnostic_includes_stderr_tail() {
    let _env_guard = ENV_LOCK.lock().await;
    let home_dir = TempDir::new().unwrap();
    let _home_guard = ScopedEnv::set("IRONHERMES_HOME", home_dir.path().to_str().unwrap());

    let dir = TempDir::new().unwrap();
    let store_arc = Arc::new(TokioMutex::new(open_store(&dir)));

    let dead_pid: u32 = 999_977_001; // guaranteed dead
    let task_id;
    let assignee = "alice";

    {
        let mut store = store_arc.lock().await;
        let task = store
            .create_task("stderr-tail task", assignee, CreateTaskOptions::default())
            .unwrap();

        let now = now_secs();
        let rid = format!("r_{}", uuid::Uuid::new_v4().simple());
        let claim_lock = format!("host:{}:uuid_stderr_tail", dead_pid);
        seed_running(
            &mut store,
            &task.id,
            &claim_lock,
            dead_pid as i64,
            now + 900.0,
            &rid,
            now,
        );

        // Pre-seed consecutive_failures = failure_limit - 1 = 1 so this
        // crash trips the breaker on this tick and the enriched error_msg
        // actually gets persisted via block_task / the `blocked` event.
        store
            .conn
            .execute(
                "UPDATE tasks SET consecutive_failures = 1 WHERE id = ?1",
                params![task.id],
            )
            .unwrap();

        task_id = task.id.clone();

        // Write a fixture stderr log at the exact profile-scoped path the
        // crash path reads (kanban_log_stderr_for).
        let stderr_path = ironhermes_kanban::kanban_log_stderr_for(assignee, &task.id);
        std::fs::create_dir_all(stderr_path.parent().unwrap()).unwrap();
        std::fs::write(
            &stderr_path,
            "thread 'main' panicked at src/worker.rs:42\nsomething exploded during the run\n",
        )
        .unwrap();
    }

    let config = KanbanConfig {
        failure_limit: 2,
        ..Default::default()
    };
    let ctx = make_ctx_ok_spawn(store_arc.clone(), config, 12345);
    run_dispatch_tick(&ctx).await.expect("tick failed");

    let store = store_arc.lock().await;
    let task = store.get_task(&task_id).unwrap();
    assert_eq!(
        task.status, "blocked",
        "task must be blocked after the circuit breaker trips"
    );

    let events = store.get_events(&task_id).unwrap();
    let blocked_evt = events
        .iter()
        .find(|e| e.kind == "blocked")
        .expect("blocked event must be appended");
    let payload = blocked_evt.payload.as_deref().unwrap_or("");

    assert!(
        payload.contains("no terminal event recorded"),
        "enriched diagnostic must carry the human reason; payload was: {payload}"
    );
    assert!(
        payload.contains("something exploded during the run"),
        "enriched diagnostic must carry the stderr tail; payload was: {payload}"
    );
    assert!(
        !payload.contains("worker process crashed (pid="),
        "the old bare crash string must be replaced; payload was: {payload}"
    );
}

/// D-04: a missing/unreadable stderr log yields the reason with an empty
/// tail — `read_stderr_tail` is non-fatal, this must not panic.
#[tokio::test]
async fn diagnostic_missing_log_no_panic() {
    let _env_guard = ENV_LOCK.lock().await;
    let home_dir = TempDir::new().unwrap();
    let _home_guard = ScopedEnv::set("IRONHERMES_HOME", home_dir.path().to_str().unwrap());

    let dir = TempDir::new().unwrap();
    let store_arc = Arc::new(TokioMutex::new(open_store(&dir)));

    let dead_pid: u32 = 999_977_002;
    let task_id;

    {
        let mut store = store_arc.lock().await;
        let task = store
            .create_task("missing-log task", "alice", CreateTaskOptions::default())
            .unwrap();

        let now = now_secs();
        let rid = format!("r_{}", uuid::Uuid::new_v4().simple());
        let claim_lock = format!("host:{}:uuid_missing_log", dead_pid);
        seed_running(
            &mut store,
            &task.id,
            &claim_lock,
            dead_pid as i64,
            now + 900.0,
            &rid,
            now,
        );

        store
            .conn
            .execute(
                "UPDATE tasks SET consecutive_failures = 1 WHERE id = ?1",
                params![task.id],
            )
            .unwrap();

        task_id = task.id.clone();
        // Deliberately do NOT write a stderr log file — IRONHERMES_HOME
        // points at a fresh empty tempdir, so the profile-scoped log path
        // does not exist.
    }

    let config = KanbanConfig {
        failure_limit: 2,
        ..Default::default()
    };
    let ctx = make_ctx_ok_spawn(store_arc.clone(), config, 12345);

    // Must not panic.
    run_dispatch_tick(&ctx)
        .await
        .expect("tick must not error even when the stderr log is missing");

    let store = store_arc.lock().await;
    let task = store.get_task(&task_id).unwrap();
    assert_eq!(task.status, "blocked");

    let events = store.get_events(&task_id).unwrap();
    let blocked_evt = events
        .iter()
        .find(|e| e.kind == "blocked")
        .expect("blocked event must be appended");
    let payload = blocked_evt.payload.as_deref().unwrap_or("");
    assert!(
        payload.contains("no terminal event recorded"),
        "reason must still be present with an empty tail; payload was: {payload}"
    );
    assert!(
        !payload.contains("--- stderr tail ---"),
        "an empty tail must not append the stderr-tail section; payload was: {payload}"
    );
}

// ---------------------------------------------------------------------------
// D-05: scratch-only workspace re-init on retry
// ---------------------------------------------------------------------------

/// D-05: a scratch task's retry (consecutive_failures > 0) wipes a
/// pre-seeded stale leftover from a prior crashed run.
#[tokio::test]
async fn reinit_scratch_wipes_on_retry() {
    let _env_guard = ENV_LOCK.lock().await;
    let home_dir = TempDir::new().unwrap();
    let _home_guard = ScopedEnv::set("IRONHERMES_HOME", home_dir.path().to_str().unwrap());

    let task_id = "t_reinit_wipe_test";
    let scratch_dir = ironhermes_kanban::kanban_workspace_for(task_id);
    std::fs::create_dir_all(&scratch_dir).unwrap();
    let stale_file = scratch_dir.join("stale.txt");
    std::fs::write(&stale_file, b"leftover from a crashed prior run").unwrap();
    assert!(stale_file.exists(), "fixture must be written before wipe");

    let mut task = fake_task(task_id);
    task.workspace = None; // scratch
    task.consecutive_failures = 1; // a retry, not the first spawn

    ironhermes_kanban::worker_spawn::reinit_scratch_workspace_if_retry(&task)
        .expect("reinit should succeed for a scratch retry");

    assert!(
        !stale_file.exists(),
        "stale leftover must be wiped on a scratch retry (D-05)"
    );
}

/// D-05: a scratch task's first spawn (consecutive_failures == 0) must NOT
/// wipe anything — the helper is a no-op.
#[tokio::test]
async fn reinit_scratch_noop_first_spawn() {
    let _env_guard = ENV_LOCK.lock().await;
    let home_dir = TempDir::new().unwrap();
    let _home_guard = ScopedEnv::set("IRONHERMES_HOME", home_dir.path().to_str().unwrap());

    let task_id = "t_reinit_noop_test";
    let scratch_dir = ironhermes_kanban::kanban_workspace_for(task_id);
    std::fs::create_dir_all(&scratch_dir).unwrap();
    let existing_file = scratch_dir.join("keep-me.txt");
    std::fs::write(&existing_file, b"first spawn - nothing to wipe yet").unwrap();

    let mut task = fake_task(task_id);
    task.workspace = None; // scratch
    task.consecutive_failures = 0; // first spawn, NOT a retry

    ironhermes_kanban::worker_spawn::reinit_scratch_workspace_if_retry(&task)
        .expect("reinit should succeed (as a no-op) on a first spawn");

    assert!(
        existing_file.exists(),
        "first-spawn (consecutive_failures == 0) must NOT wipe existing scratch contents"
    );
}

/// D-05 prohibition guard: a `dir:`/`project:` workspace is NEVER acted on
/// by `reinit_scratch_workspace_if_retry`, regardless of
/// `consecutive_failures` — these are the persistent handoff worktrees
/// (46.4 D-07/D-09) that D-05 explicitly forbids touching. Does not require
/// IRONHERMES_HOME since a `Some(..)` workspace short-circuits before any
/// path derived from `get_hermes_home()` is even computed.
#[test]
fn dir_project_workspace_never_wiped() {
    let worktree_dir = TempDir::new().unwrap(); // NOT under kanban_workspaces_root()
    let marker = worktree_dir.path().join("worktree-marker.txt");
    std::fs::write(&marker, b"persistent handoff trail - must survive retries").unwrap();

    // dir: workspace.
    let mut dir_task = fake_task("t_dir_workspace_test");
    dir_task.workspace = Some(format!("dir:{}", worktree_dir.path().display()));
    dir_task.consecutive_failures = 5; // many retries — still must not wipe

    ironhermes_kanban::worker_spawn::reinit_scratch_workspace_if_retry(&dir_task)
        .expect("guard must no-op (Ok) for a dir: workspace");
    assert!(
        marker.exists(),
        "dir: workspace must NEVER be wiped by the retry helper (D-05 prohibition)"
    );

    // project: workspace — same guard, same marker file.
    let mut project_task = fake_task("t_project_workspace_test");
    project_task.workspace = Some(format!("project:{}", worktree_dir.path().display()));
    project_task.consecutive_failures = 5;

    ironhermes_kanban::worker_spawn::reinit_scratch_workspace_if_retry(&project_task)
        .expect("guard must no-op (Ok) for a project: workspace");
    assert!(
        marker.exists(),
        "project: workspace must NEVER be wiped by the retry helper (D-05 prohibition)"
    );
}

// ---------------------------------------------------------------------------
// D-03: clean exit without a terminator stays blocked, never completed
// ---------------------------------------------------------------------------

/// D-03 regression: a worker whose process exits without ever calling
/// `kanban_complete`/`kanban_block` must end `blocked` after `failure_limit`
/// retries — NEVER auto-completed, NEVER `done`.
///
/// # Why this drives the existing crashed-detection path
///
/// Per `tests/protocol_violation.rs`'s module doc, the v1 dispatcher heuristic
/// cannot distinguish "exited 0 without calling a terminator" from "crashed
/// (signal-killed / non-zero exit)" — both appear identically as "dead PID
/// with task still running" at the next tick, and both are labeled `crashed`.
/// This is the ONLY observable shape the no-terminator case can take without
/// a `dispatcher_state.json` exit-code reconciliation (out of scope, not
/// built in v1 — see `protocol_violation_distinguished_from_crashed_requires_state_file`).
///
/// So this test simulates the incident case (worker process gone, task still
/// `running`, no `kanban_complete`/`kanban_block` ever called) via the same
/// dead-PID detection path, driven across TWO REAL dispatcher ticks (not a
/// single pre-seeded-failures tick) to observe the actual crashed×N →
/// blocked sequence as it accrues run-by-run — mirroring a real dispatcher
/// respawning the task after each crash and it dying again before completing
/// its protocol.
#[tokio::test]
async fn clean_exit_no_terminator_blocks_never_completes() {
    let dir = TempDir::new().unwrap();
    let store_arc = Arc::new(TokioMutex::new(open_store(&dir)));

    let config = KanbanConfig {
        failure_limit: 2,
        ..Default::default()
    };
    let failure_limit = config.failure_limit;

    let task_id = {
        let mut store = store_arc.lock().await;
        store
            .create_task(
                "clean-exit-no-terminator task",
                "alice",
                CreateTaskOptions::default(),
            )
            .unwrap()
            .id
    };

    // Drive `failure_limit` attempts. Each attempt: seed the task as
    // `running` with a fresh dead PID (simulating a worker whose process is
    // gone, having never called kanban_complete/kanban_block), then run one
    // real dispatcher tick. `scheduled_at` is pushed far into the future
    // before each tick so step 6 (claim_ready_tasks) cannot re-claim the
    // task in the SAME tick it was just released by step 1 — isolating each
    // tick to exactly one crash detection (same workaround pattern as
    // `dispatcher_logic.rs`'s below-limit tests).
    for i in 0..failure_limit {
        let dead_pid: u32 = 999_966_100 + i;
        {
            let mut store = store_arc.lock().await;
            let now = now_secs();
            let rid = format!("r_{}", uuid::Uuid::new_v4().simple());
            let claim_lock = format!("host:{}:uuid_no_term_{}", dead_pid, i);
            seed_running(
                &mut store,
                &task_id,
                &claim_lock,
                dead_pid as i64,
                now + 900.0,
                &rid,
                now,
            );
            store
                .conn
                .execute(
                    "UPDATE tasks SET scheduled_at = ?1 WHERE id = ?2",
                    params![now + 3600.0, task_id],
                )
                .unwrap();
        }

        let ctx = make_ctx_ok_spawn(store_arc.clone(), config.clone(), 424242);
        run_dispatch_tick(&ctx).await.expect("tick failed");

        let store = store_arc.lock().await;
        let task = store.get_task(&task_id).unwrap();
        let expect_blocked = i + 1 == failure_limit;
        assert_eq!(
            task.status,
            if expect_blocked { "blocked" } else { "ready" },
            "unexpected status after attempt {} (dead pid, no terminator)",
            i + 1
        );
        assert_eq!(
            task.consecutive_failures,
            (i + 1) as i64,
            "consecutive_failures must increment by exactly 1 per crashed attempt"
        );
        // NEVER completed, NEVER done — at any point in the sequence.
        assert_ne!(task.status, "done", "task must never reach done");

        // Clear scheduled_at so the next iteration's seed_running call finds
        // a clean slate (the loop body re-sets it before every tick anyway,
        // but this keeps intermediate reads honest about task state).
        if !expect_blocked {
            store
                .conn
                .execute(
                    "UPDATE tasks SET scheduled_at = NULL WHERE id = ?1",
                    params![task_id],
                )
                .unwrap();
        }
    }

    // Final assertions: blocked, never completed, outcome sequence is
    // crashed×N → blocked.
    let store = store_arc.lock().await;
    let task = store.get_task(&task_id).unwrap();
    assert_eq!(
        task.status, "blocked",
        "task must end blocked after failure_limit no-terminator attempts (D-03)"
    );
    assert_eq!(task.consecutive_failures, failure_limit as i64);

    let runs = store.get_runs(&task_id).unwrap();
    let crashed_count = runs
        .iter()
        .filter(|r| r.outcome.as_deref() == Some("crashed"))
        .count();
    let blocked_count = runs
        .iter()
        .filter(|r| r.outcome.as_deref() == Some("blocked"))
        .count();
    let completed_count = runs
        .iter()
        .filter(|r| r.outcome.as_deref() == Some("completed"))
        .count();

    assert_eq!(
        crashed_count, failure_limit as usize,
        "task_runs.outcome must record exactly {failure_limit} 'crashed' rows; runs: {runs:?}"
    );
    assert_eq!(
        blocked_count, 1,
        "task_runs.outcome must record exactly one 'blocked' row (the circuit-breaker trip); runs: {runs:?}"
    );
    assert_eq!(
        completed_count, 0,
        "NO run may ever be 'completed' for a no-terminator clean exit (D-03 prohibition); runs: {runs:?}"
    );

    let events = store.get_events(&task_id).unwrap();
    let crashed_events = events.iter().filter(|e| e.kind == "crashed").count();
    let gave_up_events = events.iter().filter(|e| e.kind == "gave_up").count();
    let completed_events = events.iter().filter(|e| e.kind == "completed").count();
    assert_eq!(
        crashed_events, failure_limit as usize,
        "must observe exactly {failure_limit} crashed events; events: {events:?}"
    );
    assert_eq!(
        gave_up_events, 1,
        "must observe exactly one gave_up event (the breaker trip); events: {events:?}"
    );
    assert_eq!(
        completed_events, 0,
        "must NEVER observe a completed event for a no-terminator clean exit (D-03 prohibition)"
    );
}

/// Allow-all dispatch gate for tests exercising dispatcher steps OTHER than
/// the Phase 47.4 pre-spawn gate. The production default is the real
/// fail-closed predicate, which resolves `$IRONHERMES_HOME/profiles/<assignee>/`;
/// these tests use synthetic assignees with no profile directory. The gate is
/// covered against the REAL predicate in `tests/dispatch_gate_loop.rs`.
fn allow_all_gate() -> ironhermes_kanban::dispatcher::DispatchGateFn {
    std::sync::Arc::new(|_assignee: &str| ironhermes_core::dispatch_gate::DispatchDecision::Allow)
}

//! Concurrency tests for the atomic CAS claim primitive (Plan 02 Task 2).
//!
//! These tests prove that `BEGIN IMMEDIATE` correctly serializes concurrent
//! claim attempts — exactly one caller wins, the store stays consistent, and
//! `worker_write_gated` correctly blocks stale-lock writes.
//!
//! Invariant tested here:
//! - INV-36.3.7-01 (via behavior): only one winner per trial, over N=20 trials.
//! - INV-36.3.7-04 (via behavior): exactly one `task_runs` row after a
//!   successful claim.
//! - D-41 worker_write_gated: stale lock → Ok(false) + claim_expired event.

use rusqlite::Connection;
use tempfile::TempDir;

use ironhermes_kanban::{
    cas::{atomic_claim, build_claim_lock, worker_write_gated, DEFAULT_CLAIM_TTL_SECONDS},
    store::{CreateTaskOptions, KanbanStore},
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

fn open_conn(path: &std::path::Path) -> Connection {
    let conn = Connection::open(path).expect("open connection");
    conn.busy_timeout(std::time::Duration::from_millis(5000))
        .unwrap();
    conn
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Core CAS race property: over N=20 trials (each trial = 2 threads racing
/// on the same task), exactly one thread wins per trial.
///
/// This test catches the DEFERRED-vs-IMMEDIATE landmine (Pitfall 1): if
/// `transaction()` (DEFERRED) is used instead of `transaction_with_behavior
/// (Immediate)`, both threads may win the UPDATE on the same task row.
#[test]
fn concurrent_claims_exactly_one_winner() {
    use std::sync::{Arc, Mutex};
    use std::thread;

    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("kanban.db");

    // Seed the DB with ready tasks using a KanbanStore.
    let task_ids: Vec<String> = {
        let mut store = KanbanStore::new(&db_path).expect("open store");
        (0..20)
            .map(|_| {
                store
                    .create_task("race-task", "alice", CreateTaskOptions::default())
                    .expect("create task")
                    .id
            })
            .collect()
    };

    // For each trial: two threads race to claim the same task.
    for task_id in &task_ids {
        let path = db_path.clone();
        let tid = task_id.clone();

        let results: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));

        let results_a = Arc::clone(&results);
        let path_a = path.clone();
        let tid_a = tid.clone();
        let thread_a = thread::spawn(move || {
            let mut conn = open_conn(&path_a);
            let run_id = format!("r_{}", uuid::Uuid::new_v4().simple());
            let lock = build_claim_lock("host-a", 1001);
            let won = atomic_claim(
                &mut conn,
                &tid_a,
                &lock,
                1001,
                DEFAULT_CLAIM_TTL_SECONDS,
                now(),
                &run_id,
            )
            .expect("atomic_claim thread_a");
            results_a.lock().unwrap().push(won);
        });

        let results_b = Arc::clone(&results);
        let path_b = path.clone();
        let tid_b = tid.clone();
        let thread_b = thread::spawn(move || {
            let mut conn = open_conn(&path_b);
            let run_id = format!("r_{}", uuid::Uuid::new_v4().simple());
            let lock = build_claim_lock("host-b", 1002);
            let won = atomic_claim(
                &mut conn,
                &tid_b,
                &lock,
                1002,
                DEFAULT_CLAIM_TTL_SECONDS,
                now(),
                &run_id,
            )
            .expect("atomic_claim thread_b");
            results_b.lock().unwrap().push(won);
        });

        thread_a.join().expect("thread_a panicked");
        thread_b.join().expect("thread_b panicked");

        let r = results.lock().unwrap();
        let winners: usize = r.iter().filter(|&&w| w).count();
        assert_eq!(
            winners, 1,
            "task {tid}: expected exactly 1 winner, got {winners} (results: {r:?})"
        );

        // Verify exactly 1 task_runs row.
        let mut store = KanbanStore::new(&db_path).expect("reopen store");
        let runs = store.get_runs(task_id).expect("get_runs");
        assert_eq!(
            runs.len(),
            1,
            "task {tid}: expected exactly 1 task_runs row, got {}",
            runs.len()
        );

        // Verify task is in running state.
        let task = store.get_task(task_id).expect("get_task");
        assert_eq!(
            task.status, "running",
            "task {tid}: expected status='running' after claim"
        );
    }
}

/// `worker_write_gated` with a mismatched lock → returns Ok(false) and emits
/// a `claim_expired` event (D-41).
#[test]
fn claim_lock_gates_writes() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("kanban.db");

    // Create and claim a task with lock A.
    let task_id = {
        let mut store = KanbanStore::new(&db_path).expect("open store");
        store
            .create_task("gated-task", "alice", CreateTaskOptions::default())
            .expect("create")
            .id
    };

    let lock_a = build_claim_lock("host", 42);
    let run_id = format!("r_{}", uuid::Uuid::new_v4().simple());
    {
        let mut conn = open_conn(&db_path);
        atomic_claim(
            &mut conn,
            &task_id,
            &lock_a,
            42,
            DEFAULT_CLAIM_TTL_SECONDS,
            now(),
            &run_id,
        )
        .expect("claim with lock A");
    }

    // Attempt a write using lock B (stale / wrong lock).
    let lock_b = build_claim_lock("host", 99);
    {
        let mut conn = open_conn(&db_path);
        let result = worker_write_gated(&mut conn, &task_id, &lock_b, |_tx| Ok(()))
            .expect("worker_write_gated must not error");
        assert!(
            !result,
            "worker_write_gated with stale lock must return Ok(false)"
        );
    }

    // Verify a claim_expired event was written.
    let mut store = KanbanStore::new(&db_path).expect("reopen");
    let events = store.get_events(&task_id).expect("get_events");
    let has_claim_expired = events.iter().any(|e| e.kind == "claim_expired");
    assert!(
        has_claim_expired,
        "worker_write_gated with stale lock must emit 'claim_expired' event (D-41)"
    );
}

//! Multi-board dispatcher integration tests (Phase 36.3.7.9, Plan 06, Task 2).
//!
//! Verifies that `run_dispatch_tick` sweeps every board per tick:
//! - "default" is always processed (synthesized first, never skipped when absent from disk)
//! - Named boards under `boards/<slug>/` are also processed
//! - Corrupt boards (non-SQLite bytes) are logged + skipped; other boards continue
//!   (INV-36.3.7-08-05 extension / T-36.3.7.9-06-01)
//! - The resolved board slug is propagated into the worker env via spawn_fn
//!   (T-36.3.7.9-06-02)
//!
//! # Tracing assertion strategy
//!
//! The `tracing-test` crate is NOT a current workspace dependency. Rather than
//! adding it, the warn-emission for corrupt boards is verified indirectly:
//! `open_for_board` returns `Err` on a corrupt DB, which causes the dispatcher
//! to skip that board. We assert the SKIP by confirming that tasks seeded on the
//! corrupt board's store were NOT processed while tasks on healthy boards WERE.
//! For the slug-propagation test we use an `Arc<Mutex<Vec<String>>>` accumulator
//! injected via `DispatcherContext::with_spawn_fn` to capture the board_slug arg.
//!
//! # Test serialization
//!
//! These tests mutate `IRONHERMES_HOME` which is a process-global env var.
//! A `static SERIAL_MUTEX` serializes all tests in this file so they cannot
//! interfere with each other even under `cargo test`'s default parallel runner.

use std::sync::{Arc, Mutex};

use ironhermes_kanban::{
    DispatcherContext, KanbanConfig, KanbanStore,
    dispatcher::run_dispatch_tick,
};
use tokio::sync::Mutex as TokioMutex;

// Serialize all tests that mutate IRONHERMES_HOME to avoid cross-test
// contamination (process-global env var; `std::env::set_var` is not thread-safe).
static SERIAL_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_config() -> KanbanConfig {
    KanbanConfig {
        dispatch_in_gateway: true,
        dispatch_interval_seconds: 30,
        max_in_progress: Some(16),
        failure_limit: 3,
        ..KanbanConfig::default()
    }
}

/// Build a DispatcherContext with an injectable spawn function that records
/// (board_slug, task_id) pairs into `captured`.
fn make_ctx_capturing_spawn(
    store: Arc<TokioMutex<KanbanStore>>,
    captured: Arc<Mutex<Vec<(String, String)>>>,
) -> Arc<DispatcherContext> {
    let spawn_fn = Arc::new(
        move |task: ironhermes_kanban::types::Task,
              _run: ironhermes_kanban::types::TaskRun,
              _ws: String,
              board_slug: String|
              -> std::pin::Pin<
            Box<dyn std::future::Future<Output = ironhermes_kanban::error::Result<u32>> + Send>,
        > {
            let captured = captured.clone();
            let tid = task.id.clone();
            Box::pin(async move {
                captured.lock().unwrap().push((board_slug, tid));
                Ok(std::process::id())
            })
        },
    );
    Arc::new(DispatcherContext::with_spawn_fn(store, default_config(), spawn_fn))
}

/// Seed a `ready` task directly into a store via SQL (bypasses atomic_claim).
fn seed_ready_task(store: &mut KanbanStore, task_id: &str, assignee: &str) {
    store.conn.execute(
        "INSERT INTO tasks (id, title, assignee, status, priority, consecutive_failures, created_at) \
         VALUES (?1, ?2, ?3, 'ready', 0, 0, 1700000000.0)",
        rusqlite::params![task_id, format!("Task {}", task_id), assignee],
    ).expect("seed_ready_task failed");
}

// ---------------------------------------------------------------------------
// Test 1: default-only environment — default board tasks are processed
// ---------------------------------------------------------------------------

/// `dispatcher_tick_sweeps_default_only_when_no_named_boards`:
///
/// When no named boards exist on disk (`boards/` dir absent or empty), the tick
/// should process the default board's ready tasks. The per-board open for
/// "default" must succeed and at least one claim+spawn must occur.
#[tokio::test]
async fn dispatcher_tick_sweeps_default_only_when_no_named_boards() {
    let _guard = SERIAL_MUTEX.lock().unwrap();
    let dir = tempfile::tempdir().expect("tempdir");
    // Point IRONHERMES_HOME at a fresh dir with no boards/ subdirectory.
    let _env = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());

    // Open the default board and seed a ready task.
    let mut default_store = KanbanStore::open_default().expect("open_default");
    seed_ready_task(&mut default_store, "t_default_only_001", "alice");
    let store_arc = Arc::new(TokioMutex::new(default_store));

    let captured: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(vec![]));
    let ctx = make_ctx_capturing_spawn(store_arc.clone(), captured.clone());

    run_dispatch_tick(&ctx).await.expect("tick should succeed");

    let spawned = captured.lock().unwrap();
    assert_eq!(
        spawned.len(),
        1,
        "exactly one spawn expected for single default-board ready task, got: {:?}",
        *spawned
    );
    let (slug, tid) = &spawned[0];
    assert_eq!(slug, "default", "spawn must receive board_slug='default'");
    assert_eq!(tid, "t_default_only_001");
}

// ---------------------------------------------------------------------------
// Test 2: default + named board — both boards' tasks processed in same tick
// ---------------------------------------------------------------------------

/// `dispatcher_tick_sweeps_default_plus_named`:
///
/// When one named board exists (`boards/alpha/`), the tick should process
/// both the default board's tasks AND the alpha board's tasks in the same tick.
#[tokio::test]
async fn dispatcher_tick_sweeps_default_plus_named() {
    let _guard = SERIAL_MUTEX.lock().unwrap();
    let dir = tempfile::tempdir().expect("tempdir");
    let _env = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());

    // Create boards/alpha/
    let alpha_dir = dir.path().join("kanban").join("boards").join("alpha");
    std::fs::create_dir_all(&alpha_dir).expect("create alpha dir");

    // Seed default board.
    let mut default_store = KanbanStore::open_default().expect("open_default");
    seed_ready_task(&mut default_store, "t_default_multi_001", "alice");
    let default_arc = Arc::new(TokioMutex::new(default_store));

    // Seed alpha board.
    let mut alpha_store = KanbanStore::open_for_board("alpha").expect("open alpha");
    seed_ready_task(&mut alpha_store, "t_alpha_multi_001", "bob");
    drop(alpha_store); // release connection before the tick opens it

    let captured: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(vec![]));
    let ctx = make_ctx_capturing_spawn(default_arc, captured.clone());

    run_dispatch_tick(&ctx).await.expect("tick should succeed");

    let spawned = captured.lock().unwrap();
    assert_eq!(
        spawned.len(),
        2,
        "expected 2 spawns (one per board), got: {:?}",
        *spawned
    );

    // Collect slugs to verify both boards were processed.
    let slugs: Vec<&str> = spawned.iter().map(|(s, _)| s.as_str()).collect();
    assert!(slugs.contains(&"default"), "default board must be processed");
    assert!(slugs.contains(&"alpha"), "alpha board must be processed");
}

// ---------------------------------------------------------------------------
// Test 3: corrupt board is skipped; healthy boards continue
// ---------------------------------------------------------------------------

/// `dispatcher_tick_skips_corrupt_board_continues_sweeping`:
///
/// When a `boards/broken/kanban.db` exists but contains non-SQLite bytes,
/// `open_for_board("broken")` returns Err. The dispatcher must log + skip it
/// (INV-36.3.7-08-05 extension) and continue processing default and alpha.
///
/// The warn emission is tested indirectly: we assert that default and alpha
/// tasks WERE processed while no task from "broken" appears in spawned list.
#[tokio::test]
async fn dispatcher_tick_skips_corrupt_board_continues_sweeping() {
    let _guard = SERIAL_MUTEX.lock().unwrap();
    let dir = tempfile::tempdir().expect("tempdir");
    let _env = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());

    // Create boards/alpha/ with a valid DB + ready task.
    let alpha_dir = dir.path().join("kanban").join("boards").join("alpha");
    std::fs::create_dir_all(&alpha_dir).expect("create alpha dir");
    let mut alpha_store = KanbanStore::open_for_board("alpha").expect("open alpha");
    seed_ready_task(&mut alpha_store, "t_alpha_corrupt_test_001", "carol");
    drop(alpha_store);

    // Create boards/broken/ AFTER alpha so list_boards() sorts: alpha, broken.
    let broken_dir = dir.path().join("kanban").join("boards").join("broken");
    std::fs::create_dir_all(&broken_dir).expect("create broken dir");
    std::fs::write(broken_dir.join("kanban.db"), b"THIS IS NOT SQLITE\n")
        .expect("write corrupt db");

    // Default board with a ready task.
    let mut default_store = KanbanStore::open_default().expect("open_default");
    seed_ready_task(&mut default_store, "t_default_corrupt_test_001", "dave");
    let default_arc = Arc::new(TokioMutex::new(default_store));

    let captured: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(vec![]));
    let ctx = make_ctx_capturing_spawn(default_arc, captured.clone());

    // Tick must not panic or return Err even though "broken" is in the board list.
    run_dispatch_tick(&ctx).await.expect("tick must not error on corrupt board");

    let spawned = captured.lock().unwrap();
    // default + alpha tasks processed; broken skipped.
    assert_eq!(
        spawned.len(),
        2,
        "expected 2 spawns (default + alpha, broken skipped), got: {:?}",
        *spawned
    );

    let slugs: Vec<&str> = spawned.iter().map(|(s, _)| s.as_str()).collect();
    assert!(slugs.contains(&"default"), "default must be processed");
    assert!(slugs.contains(&"alpha"), "alpha must be processed");
    assert!(
        !slugs.contains(&"broken"),
        "broken board must NOT appear in spawned list"
    );
}

// ---------------------------------------------------------------------------
// Test 4: resolved slug propagated to worker env
// ---------------------------------------------------------------------------

/// `dispatcher_tick_passes_resolved_slug_to_worker_env`:
///
/// When a named board "alpha" has a ready task, the dispatcher must call
/// `spawn_fn` with `board_slug == "alpha"`. This locks T-36.3.7.9-06-02
/// (worker cannot accidentally write to a different board's DB).
#[tokio::test]
async fn dispatcher_tick_passes_resolved_slug_to_worker_env() {
    let _guard = SERIAL_MUTEX.lock().unwrap();
    let dir = tempfile::tempdir().expect("tempdir");
    let _env = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());

    // Create boards/alpha/ with a ready task.
    let alpha_dir = dir.path().join("kanban").join("boards").join("alpha");
    std::fs::create_dir_all(&alpha_dir).expect("create alpha dir");
    let mut alpha_store = KanbanStore::open_for_board("alpha").expect("open alpha");
    seed_ready_task(&mut alpha_store, "t_alpha_slug_001", "eve");
    drop(alpha_store);

    // Open default board (no tasks — we only want to see alpha processed).
    let default_store = KanbanStore::open_default().expect("open_default");
    let default_arc = Arc::new(TokioMutex::new(default_store));

    let captured: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(vec![]));
    let ctx = make_ctx_capturing_spawn(default_arc, captured.clone());

    run_dispatch_tick(&ctx).await.expect("tick should succeed");

    let spawned = captured.lock().unwrap();
    // Find the alpha spawn entry.
    let alpha_entry = spawned.iter().find(|(slug, _)| slug == "alpha");
    assert!(
        alpha_entry.is_some(),
        "expected a spawn with board_slug='alpha', got: {:?}",
        *spawned
    );
    let (slug, tid) = alpha_entry.unwrap();
    assert_eq!(slug, "alpha");
    assert_eq!(tid, "t_alpha_slug_001");
}

// ---------------------------------------------------------------------------
// ScopedEnv RAII helper
// ---------------------------------------------------------------------------

struct ScopedEnv {
    key: String,
    prev: Option<String>,
}

impl ScopedEnv {
    fn set(key: &str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: guarded by SERIAL_MUTEX which serializes all callers in this
        // test binary; no concurrent env access within this file.
        unsafe { std::env::set_var(key, value) };
        Self { key: key.to_string(), prev }
    }
}

impl Drop for ScopedEnv {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => unsafe { std::env::set_var(&self.key, v) },
            None => unsafe { std::env::remove_var(&self.key) },
        }
    }
}

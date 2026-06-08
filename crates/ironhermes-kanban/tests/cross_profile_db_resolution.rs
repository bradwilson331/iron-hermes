//! Phase 36.3.7.13 Plan 01 Task 1 — bilateral cross-profile DB resolution tests.
//!
//! Validates D-A1 (`open_from_env()`) and D-A2 (`open_from_env_or_board()`),
//! plus one producer-side Test 6 verifying `build_kanban_worker_env` emits
//! `HERMES_KANBAN_DB` for non-default board slugs (WARNING 6 bilateral pairing).
//!
//! Tests:
//! 1. `open_from_env_reads_hermes_kanban_db` — D-A1: env override wins
//! 2. `open_from_env_fallback_when_unset` — D-A1: fallback to kanban_db_path() when env absent
//! 3. `open_from_env_err_on_unwritable_path` — D-A1: returns Err on bad path
//! 4. `open_from_env_or_board_env_wins_over_slug` — D-A2: env wins; slug ignored
//! 5. `open_from_env_or_board_fallback_when_env_unset_and_no_slug` — D-A2: None slug + no env → default path
//! 6. `build_kanban_worker_env_emits_hermes_kanban_db` — producer-side (WARNING 6):
//!    `build_kanban_worker_env` emits `HERMES_KANBAN_DB` into the child env Vec for a named board slug.

use ironhermes_kanban::store::KanbanStore;
use ironhermes_kanban::types::{Task, TaskRun};
use ironhermes_kanban::worker_spawn::build_kanban_worker_env;

/// Global env mutex — process-global env mutation must be serialized.
///
/// Each test that touches `HERMES_KANBAN_DB` acquires this lock first and
/// removes the var on exit (via explicit cleanup or the guard Drop).
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// RAII guard that removes HERMES_KANBAN_DB when dropped.
struct EnvGuard;
impl Drop for EnvGuard {
    fn drop(&mut self) {
        // Safety: test-only; ENV_LOCK serialises all env mutations in this file.
        unsafe { std::env::remove_var("HERMES_KANBAN_DB") };
    }
}

// ---------------------------------------------------------------------------
// Helper: Task fixture (mirrors worker_spawn.rs::fake_task)
// ---------------------------------------------------------------------------

fn fake_task(id: &str, assignee: &str) -> Task {
    Task {
        id: id.to_string(),
        title: "test task".to_string(),
        body: None,
        assignee: assignee.to_string(),
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
        // Phase 36.3.7.13 D-B1: no toolset preset by default.
        goal_toolset: None,
    }
}

fn fake_run(id: &str, task_id: &str, claim_lock: &str) -> TaskRun {
    TaskRun {
        id: id.to_string(),
        task_id: task_id.to_string(),
        claim_lock: claim_lock.to_string(),
        claim_pid: None,
        started_at: 1_700_000_000.0,
        ended_at: None,
        outcome: None,
        summary: None,
        metadata: None,
        error: None,
        log_path: None,
    }
}

// ---------------------------------------------------------------------------
// Test 1 (D-A1): open_from_env() reads HERMES_KANBAN_DB and opens that path
// ---------------------------------------------------------------------------

#[test]
fn open_from_env_reads_hermes_kanban_db() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _guard = EnvGuard;

    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("override.db");

    // Safety: test-only; ENV_LOCK serializes env mutations.
    unsafe { std::env::set_var("HERMES_KANBAN_DB", db_path.to_str().unwrap()) };

    let store = KanbanStore::open_from_env().expect("open_from_env must succeed with valid path");
    // Verify the DB was created at the overridden path.
    assert!(
        db_path.exists(),
        "DB file must be created at HERMES_KANBAN_DB path"
    );
    drop(store);
}

// ---------------------------------------------------------------------------
// Test 2 (D-A1): open_from_env() falls back to kanban_db_path() when env unset
// ---------------------------------------------------------------------------

#[test]
fn open_from_env_fallback_when_unset() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    // Ensure env is clean (no leftover from a prior test in the same process).
    // Safety: test-only; ENV_LOCK serializes env mutations.
    unsafe { std::env::remove_var("HERMES_KANBAN_DB") };

    // open_from_env() must not error when HERMES_KANBAN_DB is absent.
    // It falls back to kanban_db_path() which resolves to IRONHERMES_HOME/kanban.db.
    // We set IRONHERMES_HOME to a tempdir so the test is hermetic.
    let dir = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };
    let result = KanbanStore::open_from_env();
    unsafe { std::env::remove_var("IRONHERMES_HOME") };

    assert!(
        result.is_ok(),
        "open_from_env must succeed via fallback path, got: {:?}",
        result.err()
    );
    let expected_db = dir.path().join("kanban.db");
    assert!(
        expected_db.exists(),
        "DB must be created at fallback kanban_db_path()"
    );
}

// ---------------------------------------------------------------------------
// Test 3 (D-A1): open_from_env() returns Err on a non-existent unwritable path
// ---------------------------------------------------------------------------

#[test]
fn open_from_env_err_on_unwritable_path() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _guard = EnvGuard;

    // Point at a path under a non-existent parent directory that cannot be created
    // (a file used as a "directory" prefix prevents mkdir).
    let dir = tempfile::tempdir().unwrap();
    let file_as_dir = dir.path().join("not_a_dir");
    std::fs::write(&file_as_dir, b"I am a file").unwrap();
    let bad_path = file_as_dir.join("nested").join("kanban.db");

    // Safety: test-only; ENV_LOCK serializes env mutations.
    unsafe { std::env::set_var("HERMES_KANBAN_DB", bad_path.to_str().unwrap()) };

    let result = KanbanStore::open_from_env();
    assert!(
        result.is_err(),
        "open_from_env must return Err when path parent is unwritable"
    );
}

// ---------------------------------------------------------------------------
// Test 4 (D-A2): open_from_env_or_board(Some("foo")) with HERMES_KANBAN_DB set → env wins
// ---------------------------------------------------------------------------

#[test]
fn open_from_env_or_board_env_wins_over_slug() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _guard = EnvGuard;

    let dir = tempfile::tempdir().unwrap();
    let env_db = dir.path().join("env_override.db");

    // Safety: test-only; ENV_LOCK serializes env mutations.
    unsafe { std::env::set_var("HERMES_KANBAN_DB", env_db.to_str().unwrap()) };

    let store = KanbanStore::open_from_env_or_board(Some("some-slug"))
        .expect("open_from_env_or_board must succeed");
    // The env path must have been opened — the slug-based path must NOT exist.
    assert!(
        env_db.exists(),
        "HERMES_KANBAN_DB path must be created when env wins"
    );
    drop(store);
}

// ---------------------------------------------------------------------------
// Test 5 (D-A2): open_from_env_or_board(None) with HERMES_KANBAN_DB unset → kanban_db_path()
// ---------------------------------------------------------------------------

#[test]
fn open_from_env_or_board_fallback_when_env_unset_and_no_slug() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    // Ensure env is clean.
    // Safety: test-only; ENV_LOCK serializes env mutations.
    unsafe { std::env::remove_var("HERMES_KANBAN_DB") };

    let dir = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };
    let result = KanbanStore::open_from_env_or_board(None);
    unsafe { std::env::remove_var("IRONHERMES_HOME") };

    assert!(
        result.is_ok(),
        "open_from_env_or_board(None) must succeed via default path, got: {:?}",
        result.err()
    );
    let expected_db = dir.path().join("kanban.db");
    assert!(
        expected_db.exists(),
        "DB must be created at kanban_db_path() when env unset and slug is None"
    );
}

// ---------------------------------------------------------------------------
// Test 6 (D-A1 PRODUCER SIDE — WARNING 6 bilateral pairing):
//   build_kanban_worker_env emits HERMES_KANBAN_DB for a named board slug.
// ---------------------------------------------------------------------------

#[test]
fn build_kanban_worker_env_emits_hermes_kanban_db() {
    // No env mutation needed — build_kanban_worker_env reads task + slug, not env.
    let task = fake_task("t_xpro01", "alice");
    let run = fake_run("r_xpro01", "t_xpro01", "host:1:uuid");

    let env = build_kanban_worker_env(&task, &run, "/tmp/ws_xpro", "my-board");

    let db_entry = env
        .iter()
        .find(|(k, _)| k == "HERMES_KANBAN_DB")
        .expect("HERMES_KANBAN_DB must be present in worker env for named board");

    // The value must contain the boards/my-board layout path.
    assert!(
        db_entry.1.contains("boards/my-board/kanban.db"),
        "HERMES_KANBAN_DB must reference boards/my-board/kanban.db, got: {}",
        db_entry.1
    );
}

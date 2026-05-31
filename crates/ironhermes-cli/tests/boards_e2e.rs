//! Phase 36.3.7.9 Plan 09 — end-to-end multi-board pipeline integration tests.
//!
//! Exercises the full pipeline that Plans 01-08 built in a single user-shaped flow:
//!   paths (Plan 01) → store.open_for_board (Plan 02) → boards CLI commands (Plan 03)
//!   → --board flag / open_store_for_board (Plan 04) → board resolver (Plan 01/02)
//!   → LLM tool board-aware execute (Plan 07) → tool envelope assertions (Plan 07)
//!
//! Three scenarios:
//!
//! **Scenario A** — "create board alpha, switch, create task, list via tool, verify envelope"
//!   Locks: D-08 (file-source tier in tool envelope), D-01 (board isolation)
//!
//! **Scenario B** — "--board flag overrides env and file (4-tier precedence)"
//!   Locks: D-02 (flag > env > file > default)
//!
//! **Scenario C** — "archive then verify board dir is gone and archived entry exists"
//!   Locks: D-07 archive semantics + recoverability
//!
//! NOTE: These tests use `tokio::runtime::Runtime::new().unwrap().block_on(...)` to
//! drive async cmd_boards_* fns from a synchronous test body. This matches the
//! existing pattern in boards_lifecycle.rs. The tool `execute()` calls are driven
//! from `#[tokio::test]` functions.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::Arc;

use ironhermes_cli::kanban::boards;
use ironhermes_kanban::store::{CreateTaskOptions, KanbanStore};
use ironhermes_kanban::tools::{KanbanListTool, KanbanShowTool};
use ironhermes_tools::Tool;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::Mutex as TokioMutex;

/// Serialise all tests that mutate IRONHERMES_HOME / HERMES_KANBAN_BOARD.
static ENV_LOCK: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// RAII guard: sets IRONHERMES_HOME to a fresh tempdir for the lifetime of the guard.
struct ScopedHome {
    _dir: TempDir,
    pub root: PathBuf,
}

impl ScopedHome {
    fn new() -> Self {
        let dir = TempDir::new().expect("create tempdir");
        let root = dir.path().to_path_buf();
        unsafe { std::env::set_var("IRONHERMES_HOME", &root) };
        unsafe { std::env::remove_var("HERMES_KANBAN_BOARD") };
        ScopedHome { _dir: dir, root }
    }
}

impl Drop for ScopedHome {
    fn drop(&mut self) {
        unsafe { std::env::remove_var("IRONHERMES_HOME") };
        unsafe { std::env::remove_var("HERMES_KANBAN_BOARD") };
    }
}

/// Build a dummy Arc<TokioMutex<KanbanStore>> (tools' legacy constructor arg — not
/// used for actual DB access in per-board mode but required by the constructor signature).
fn make_dummy_store() -> Arc<TokioMutex<KanbanStore>> {
    let dir = TempDir::new().unwrap();
    let store = KanbanStore::new(dir.path().join("dummy.db")).unwrap();
    // Intentionally forget the dir so the path stays alive for the store's lifetime.
    std::mem::forget(dir);
    Arc::new(TokioMutex::new(store))
}

/// Parse JSON and assert both board envelope fields are present with expected values.
fn assert_envelope_board(json_str: &str, expected_board: &str, expected_source: &str) {
    let v: Value = serde_json::from_str(json_str)
        .unwrap_or_else(|e| panic!("response is not valid JSON: {e}\nraw: {json_str}"));
    assert_eq!(
        v["board"].as_str().unwrap_or("<missing>"),
        expected_board,
        "envelope 'board' mismatch in: {json_str}"
    );
    assert_eq!(
        v["board_source"].as_str().unwrap_or("<missing>"),
        expected_source,
        "envelope 'board_source' mismatch in: {json_str}"
    );
}

// ---------------------------------------------------------------------------
// Scenario A: create board alpha → switch → task → list via tool → envelope check
//
// Locks: D-08 (file-source tier), D-01 (per-board isolation)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn scenario_a_create_switch_task_list_envelope() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let home = ScopedHome::new();

    // Step 1: create board "alpha" with --switch.
    let code = boards::cmd_boards_create("alpha".into(), None, None, true, false)
        .await
        .expect("cmd_boards_create should not Err");
    assert_eq!(code, 0, "boards create alpha --switch should exit 0");

    // Step 2: assert board_dir exists and current file contains "alpha".
    let board_dir = home
        .root
        .join("kanban")
        .join("boards")
        .join("alpha");
    assert!(
        board_dir.exists(),
        "board dir should exist at {:?}",
        board_dir
    );
    let current_file = home.root.join("kanban").join("current");
    let current_content = std::fs::read_to_string(&current_file)
        .unwrap_or_else(|e| panic!("current file missing: {e}"));
    assert_eq!(
        current_content.trim(),
        "alpha",
        "current file should contain 'alpha' after --switch"
    );

    // Step 3: insert a task directly into the alpha board's store.
    let task_id = {
        let mut alpha_store =
            KanbanStore::open_for_board("alpha").expect("open_for_board('alpha')");
        alpha_store
            .create_task("alpha-e2e-task", "test-agent", CreateTaskOptions::default())
            .expect("create_task on alpha")
            .id
    };

    // Verify the default board's DB does NOT contain this task (isolation check).
    let default_db_path = ironhermes_kanban::paths::kanban_db_path();
    if default_db_path.exists() {
        let default_store = KanbanStore::open_default().expect("open_default");
        let filters = ironhermes_kanban::store::ListFilters::default();
        let tasks = default_store.list_tasks(filters).expect("list default tasks");
        let found_in_default = tasks.iter().any(|t| t.id == task_id);
        assert!(
            !found_in_default,
            "task {task_id} must NOT appear in the default board — per-board isolation (D-01)"
        );
    }

    // Step 4: invoke KanbanListTool without board arg (resolver reads current file → "alpha").
    let list_tool = KanbanListTool::new(make_dummy_store(), true);
    let list_result = list_tool.execute(json!({})).await.unwrap();

    // Envelope must carry board=alpha, board_source=file (D-08).
    assert_envelope_board(&list_result, "alpha", "file");

    // The newly created task must appear in the listing.
    let list_v: Value = serde_json::from_str(&list_result).unwrap();
    let tasks_arr = list_v["tasks"].as_array().unwrap_or_else(|| {
        panic!("envelope 'tasks' field missing or not array: {list_result}")
    });
    let found = tasks_arr.iter().any(|t| {
        t["id"].as_str() == Some(&task_id) || t["title"].as_str() == Some("alpha-e2e-task")
    });
    assert!(
        found,
        "alpha-e2e-task must appear in list envelope; tasks: {tasks_arr:?}"
    );

    // Step 5: invoke KanbanListTool with explicit board="default".
    // Envelope must carry board=default, board_source=flag (flag overrides file tier).
    let list_default_result = list_tool
        .execute(json!({"board": "default"}))
        .await
        .unwrap();
    assert_envelope_board(&list_default_result, "default", "flag");

    // The alpha task must NOT appear in the default board's listing.
    let list_default_v: Value = serde_json::from_str(&list_default_result).unwrap();
    let default_tasks = list_default_v["tasks"].as_array().cloned().unwrap_or_default();
    let alpha_in_default = default_tasks
        .iter()
        .any(|t| t["id"].as_str() == Some(&task_id));
    assert!(
        !alpha_in_default,
        "alpha task must NOT appear when listing the default board"
    );
}

// ---------------------------------------------------------------------------
// Scenario B: --board flag overrides env and current file (D-02 4-tier precedence)
//
// Locks: D-02 (flag > env > file > default)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn scenario_b_board_flag_overrides_env_and_file() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _home = ScopedHome::new();

    // Create boards alpha, beta, gamma.
    for slug in &["alpha", "beta", "gamma"] {
        let code = boards::cmd_boards_create((*slug).to_string(), None, None, false, false)
            .await
            .unwrap();
        assert_eq!(code, 0, "create {slug} should succeed");
    }

    // Set current file to "alpha" via switch.
    let code = boards::cmd_boards_switch("alpha".into()).await.unwrap();
    assert_eq!(code, 0, "switch to alpha should succeed");

    // Set HERMES_KANBAN_BOARD env to "beta" (env tier wins over file tier).
    unsafe { std::env::set_var("HERMES_KANBAN_BOARD", "beta") };

    // Seed a task on the gamma board to use in KanbanShowTool.
    let gamma_task_id = {
        let mut gamma_store =
            KanbanStore::open_for_board("gamma").expect("open_for_board('gamma')");
        gamma_store
            .create_task("gamma-task", "test-agent", CreateTaskOptions::default())
            .expect("create task on gamma")
            .id
    };

    // Scenario B.1: explicit board="gamma" flag on KanbanShowTool.
    // Flag tier must win over env ("beta") and file ("alpha").
    let show_tool = KanbanShowTool::new(make_dummy_store(), true);
    let show_result = show_tool
        .execute(json!({"board": "gamma", "task_id": &gamma_task_id}))
        .await
        .unwrap();
    assert_envelope_board(&show_result, "gamma", "flag");

    // Scenario B.2: no board arg → env tier ("beta") wins over file ("alpha").
    // The show tool will try to find gamma_task_id on beta — it doesn't exist there,
    // so we get a rejection envelope. But the envelope MUST still carry board=beta,
    // board_source=env (T-5: rejection envelopes always include board context, D-08).
    let show_rejection = show_tool
        .execute(json!({"task_id": &gamma_task_id}))
        .await
        .unwrap();
    let rejection_v: Value = serde_json::from_str(&show_rejection)
        .unwrap_or_else(|e| panic!("rejection not valid JSON: {e}\nraw: {show_rejection}"));

    // Board context must be beta (env tier) not alpha (file tier) — D-02 ordering.
    assert_eq!(
        rejection_v["board"].as_str().unwrap_or("<missing>"),
        "beta",
        "rejection envelope must carry board=beta (env tier wins over file tier); \
         raw: {show_rejection}"
    );
    assert_eq!(
        rejection_v["board_source"].as_str().unwrap_or("<missing>"),
        "env",
        "rejection envelope must carry board_source=env; raw: {show_rejection}"
    );

    // The rejection reason must NOT be board-related — it's a task-lookup miss
    // because gamma_task_id doesn't exist in the beta DB.
    let status = rejection_v["status"].as_str().unwrap_or("");
    assert!(
        status == "rejected" || status == "error",
        "expected a rejected/error status for cross-board lookup miss; got: {show_rejection}"
    );

    unsafe { std::env::remove_var("HERMES_KANBAN_BOARD") };
}

// ---------------------------------------------------------------------------
// Scenario C: archive board, verify dir gone, archived entry intact
//
// Locks: D-07 archive semantics + recoverability
// ---------------------------------------------------------------------------

#[test]
fn scenario_c_archive_board_dir_gone_and_archived_entry_intact() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let home = ScopedHome::new();
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Step 1: create board "archive-me" and insert one task.
    rt.block_on(boards::cmd_boards_create(
        "archive-me".into(),
        None,
        None,
        false,
        false,
    ))
    .unwrap();

    let task_id = {
        let mut store =
            KanbanStore::open_for_board("archive-me").expect("open_for_board('archive-me')");
        store
            .create_task("persisted-task", "test-agent", CreateTaskOptions::default())
            .expect("create task")
            .id
    };

    // Verify board dir exists.
    let board_dir = home
        .root
        .join("kanban")
        .join("boards")
        .join("archive-me");
    assert!(board_dir.exists(), "board dir should exist before rm");

    // Step 2: archive (default rm, no --delete).
    let code = rt
        .block_on(boards::cmd_boards_rm("archive-me".into(), false))
        .expect("cmd_boards_rm should not Err");
    assert_eq!(code, 0, "boards rm (archive) should exit 0");

    // Step 3: board dir must be gone.
    assert!(
        !board_dir.exists(),
        "board dir must not exist after archive: {:?}",
        board_dir
    );

    // Step 4: archived entry must exist under _archived/.
    let archived_root = home
        .root
        .join("kanban")
        .join("boards")
        .join("_archived");
    assert!(
        archived_root.exists(),
        "_archived dir must exist after rm: {:?}",
        archived_root
    );
    let entries: Vec<_> = std::fs::read_dir(&archived_root)
        .expect("read_dir _archived")
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "_archived must have exactly one entry, found: {:?}",
        entries.iter().map(|e| e.file_name()).collect::<Vec<_>>()
    );
    let archived_entry = &entries[0];
    let entry_name = archived_entry.file_name();
    let entry_name_str = entry_name.to_string_lossy();
    assert!(
        entry_name_str.starts_with("archive-me-"),
        "archived entry must start with 'archive-me-<ts>', got: {entry_name_str}"
    );

    // Step 5: the archived DB must be intact — open it and read the task.
    let archived_db = archived_entry.path().join("kanban.db");
    assert!(
        archived_db.exists(),
        "archived kanban.db must exist at {:?}",
        archived_db
    );
    let archived_store = KanbanStore::open(&archived_db).expect("open archived DB");
    let filters = ironhermes_kanban::store::ListFilters::default();
    let archived_tasks = archived_store
        .list_tasks(filters)
        .expect("list archived tasks");
    let found = archived_tasks.iter().any(|t| t.id == task_id);
    assert!(
        found,
        "archived DB must contain the original task {task_id} (D-07 recoverability)"
    );
}

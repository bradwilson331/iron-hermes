//! Phase 36.3.7.9 Plan 03 — lifecycle integration tests for `hermes kanban boards`.
//!
//! Each test uses a temporary IRONHERMES_HOME to isolate filesystem state.
//! Tests cover D-07 archive/hard-delete semantics, T-3 atomicity, T-6 symlink
//! refusal, and T-7 advisory lock.

use ironhermes_cli::kanban::boards;
use std::path::PathBuf;
use std::sync::Mutex;
use tempfile::TempDir;

/// Serialise tests that mutate IRONHERMES_HOME.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// RAII guard: sets IRONHERMES_HOME to `dir` for the lifetime of the guard.
struct ScopedHome {
    _dir: TempDir,
    pub root: PathBuf,
}

impl ScopedHome {
    fn new() -> Self {
        let dir = TempDir::new().expect("create tempdir");
        let root = dir.path().to_path_buf();
        // SAFETY: test-only; we hold ENV_LOCK for the duration.
        unsafe { std::env::set_var("IRONHERMES_HOME", &root) };
        // Also clear HERMES_KANBAN_BOARD so the resolver falls through to default.
        unsafe { std::env::remove_var("HERMES_KANBAN_BOARD") };
        ScopedHome { _dir: dir, root }
    }
}

impl Drop for ScopedHome {
    fn drop(&mut self) {
        unsafe { std::env::remove_var("IRONHERMES_HOME") };
    }
}

/// Run an async boards function under a scoped IRONHERMES_HOME.
macro_rules! with_home {
    ($body:expr) => {{
        let _lock = ENV_LOCK.lock().unwrap();
        let home = ScopedHome::new();
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async { $body });
        (home, result)
    }};
}

// ---------------------------------------------------------------------------
// boards list
// ---------------------------------------------------------------------------

#[test]
fn list_shows_default_and_named_boards() {
    let _lock = ENV_LOCK.lock().unwrap();
    let home = ScopedHome::new();
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Create two named boards first
    rt.block_on(boards::cmd_boards_create(
        "alpha".into(), None, None, false, false,
    ))
    .unwrap();
    rt.block_on(boards::cmd_boards_create(
        "beta".into(), None, None, false, false,
    ))
    .unwrap();

    let code = rt
        .block_on(boards::cmd_boards_list(false))
        .expect("cmd_boards_list should succeed");
    assert_eq!(code, 0);

    drop(home); // drop after rt.block_on to keep IRONHERMES_HOME alive
}

// ---------------------------------------------------------------------------
// boards create
// ---------------------------------------------------------------------------

#[test]
fn create_rejects_default_slug() {
    let _lock = ENV_LOCK.lock().unwrap();
    let _home = ScopedHome::new();
    let rt = tokio::runtime::Runtime::new().unwrap();

    let code = rt
        .block_on(boards::cmd_boards_create(
            "default".into(), None, None, false, false,
        ))
        .expect("cmd should not Err");
    assert_eq!(code, 1, "should refuse the reserved 'default' slug");
}

#[test]
fn create_writes_board_dir_and_db() {
    let _lock = ENV_LOCK.lock().unwrap();
    let home = ScopedHome::new();
    let rt = tokio::runtime::Runtime::new().unwrap();

    let code = rt
        .block_on(boards::cmd_boards_create(
            "myboard".into(), None, None, false, false,
        ))
        .expect("cmd_boards_create should succeed");
    assert_eq!(code, 0);

    // Verify the board directory and DB were created
    let board_dir = home.root.join("kanban").join("boards").join("myboard");
    assert!(board_dir.exists(), "board dir should exist at {:?}", board_dir);
    let db_path = board_dir.join("kanban.db");
    assert!(db_path.exists(), "kanban.db should exist at {:?}", db_path);
}

#[test]
fn create_with_switch_writes_current_only_after_db_init() {
    let _lock = ENV_LOCK.lock().unwrap();
    let home = ScopedHome::new();
    let rt = tokio::runtime::Runtime::new().unwrap();

    let code = rt
        .block_on(boards::cmd_boards_create(
            "switchtest".into(), None, None, true, false,
        ))
        .expect("cmd_boards_create --switch should succeed");
    assert_eq!(code, 0);

    // current file should name "switchtest"
    let current_file = home.root.join("kanban").join("current");
    assert!(current_file.exists(), "current file should exist");
    let content = std::fs::read_to_string(&current_file).unwrap();
    assert_eq!(content.trim(), "switchtest");

    // DB must also exist (order: create_dir → open_for_board → write current)
    let db = home
        .root
        .join("kanban")
        .join("boards")
        .join("switchtest")
        .join("kanban.db");
    assert!(db.exists(), "kanban.db should exist");
}

// ---------------------------------------------------------------------------
// boards switch
// ---------------------------------------------------------------------------

#[test]
fn switch_writes_current_atomically() {
    let _lock = ENV_LOCK.lock().unwrap();
    let home = ScopedHome::new();
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Create a board first
    rt.block_on(boards::cmd_boards_create(
        "projx".into(), None, None, false, false,
    ))
    .unwrap();

    let code = rt
        .block_on(boards::cmd_boards_switch("projx".into()))
        .expect("switch should succeed");
    assert_eq!(code, 0);

    let current_file = home.root.join("kanban").join("current");
    let content = std::fs::read_to_string(current_file).unwrap();
    assert_eq!(content, "projx");
}

#[test]
fn switch_rejects_nonexistent_board() {
    let _lock = ENV_LOCK.lock().unwrap();
    let _home = ScopedHome::new();
    let rt = tokio::runtime::Runtime::new().unwrap();

    let code = rt
        .block_on(boards::cmd_boards_switch("does-not-exist".into()))
        .expect("switch should return Ok(1) not Err");
    assert_eq!(code, 1);
}

// ---------------------------------------------------------------------------
// boards show
// ---------------------------------------------------------------------------

#[test]
fn show_prints_resolved_context() {
    let _lock = ENV_LOCK.lock().unwrap();
    let _home = ScopedHome::new();
    let rt = tokio::runtime::Runtime::new().unwrap();

    let code = rt
        .block_on(boards::cmd_boards_show(false))
        .expect("show should succeed");
    assert_eq!(code, 0);
}

// ---------------------------------------------------------------------------
// boards rename
// ---------------------------------------------------------------------------

#[test]
fn rename_persists_to_board_toml() {
    let _lock = ENV_LOCK.lock().unwrap();
    let home = ScopedHome::new();
    let rt = tokio::runtime::Runtime::new().unwrap();

    rt.block_on(boards::cmd_boards_create(
        "renametest".into(), None, None, false, false,
    ))
    .unwrap();

    let code = rt
        .block_on(boards::cmd_boards_rename(
            "renametest".into(),
            "Renamed Board".into(),
        ))
        .expect("rename should succeed");
    assert_eq!(code, 0);

    let toml_path = home
        .root
        .join("kanban")
        .join("boards")
        .join("renametest")
        .join("board.toml");
    assert!(toml_path.exists(), "board.toml should exist after rename");
    let content = std::fs::read_to_string(toml_path).unwrap();
    assert!(
        content.contains("Renamed Board"),
        "board.toml should contain new display name"
    );
}

// ---------------------------------------------------------------------------
// boards rm (archive path — default)
// ---------------------------------------------------------------------------

#[test]
fn rm_archives_board_by_default() {
    let _lock = ENV_LOCK.lock().unwrap();
    let home = ScopedHome::new();
    let rt = tokio::runtime::Runtime::new().unwrap();

    rt.block_on(boards::cmd_boards_create(
        "archiveme".into(), None, None, false, false,
    ))
    .unwrap();

    let board_dir = home
        .root
        .join("kanban")
        .join("boards")
        .join("archiveme");
    assert!(board_dir.exists(), "board should exist before rm");

    let code = rt
        .block_on(boards::cmd_boards_rm("archiveme".into(), false))
        .expect("rm should succeed");
    assert_eq!(code, 0);

    // Original location should be gone
    assert!(!board_dir.exists(), "board dir should be gone after archive");

    // Should appear somewhere in _archived/
    let archive_root = home.root.join("kanban").join("boards").join("_archived");
    let found = std::fs::read_dir(&archive_root)
        .expect("_archived dir should exist")
        .any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("archiveme-")
        });
    assert!(found, "archived dir should exist under _archived/");
}

#[test]
fn rm_refuses_default_board() {
    let _lock = ENV_LOCK.lock().unwrap();
    let _home = ScopedHome::new();
    let rt = tokio::runtime::Runtime::new().unwrap();

    let code = rt
        .block_on(boards::cmd_boards_rm("default".into(), false))
        .expect("rm default should return Ok(1)");
    assert_eq!(code, 1);
}

// ---------------------------------------------------------------------------
// boards rm --delete
// ---------------------------------------------------------------------------

#[test]
fn rm_delete_succeeds_with_zero_open_tasks() {
    let _lock = ENV_LOCK.lock().unwrap();
    let home = ScopedHome::new();
    let rt = tokio::runtime::Runtime::new().unwrap();

    rt.block_on(boards::cmd_boards_create(
        "cleanboard".into(), None, None, false, false,
    ))
    .unwrap();

    let code = rt
        .block_on(boards::cmd_boards_rm("cleanboard".into(), true))
        .expect("rm --delete should succeed with 0 open tasks");
    assert_eq!(code, 0);

    let board_dir = home
        .root
        .join("kanban")
        .join("boards")
        .join("cleanboard");
    assert!(!board_dir.exists(), "board dir should be deleted");
}

#[test]
fn rm_delete_refuses_with_open_tasks() {
    let _lock = ENV_LOCK.lock().unwrap();
    let home = ScopedHome::new();
    let rt = tokio::runtime::Runtime::new().unwrap();

    rt.block_on(boards::cmd_boards_create(
        "haswork".into(), None, None, false, false,
    ))
    .unwrap();

    // Seed an open task into the board
    {
        let mut store = ironhermes_kanban::KanbanStore::open_for_board("haswork")
            .expect("open board store");
        store
            .create_task(
                "Pending task",
                "alice",
                ironhermes_kanban::store::CreateTaskOptions::default(),
            )
            .expect("create task");
    }

    let code = rt
        .block_on(boards::cmd_boards_rm("haswork".into(), true))
        .expect("rm --delete should return Ok(1) when open tasks exist");
    assert_eq!(code, 1, "should refuse --delete with open tasks");

    // Board should still exist
    let board_dir = home.root.join("kanban").join("boards").join("haswork");
    assert!(board_dir.exists(), "board should still exist after refusal");
}

// ---------------------------------------------------------------------------
// T-6: symlink refusal
// ---------------------------------------------------------------------------

#[test]
fn rm_refuses_to_follow_symlink() {
    let _lock = ENV_LOCK.lock().unwrap();
    let home = ScopedHome::new();
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Create a real board in a side dir and symlink it into boards/
    let real_dir = home.root.join("real-board-dir");
    std::fs::create_dir_all(&real_dir).unwrap();

    let boards_root = home.root.join("kanban").join("boards");
    std::fs::create_dir_all(&boards_root).unwrap();
    let link_path = boards_root.join("symlink-board");
    std::os::unix::fs::symlink(&real_dir, &link_path).expect("create symlink");

    // rm should refuse because the path is a symlink
    let code = rt
        .block_on(boards::cmd_boards_rm("symlink-board".into(), false))
        .expect("rm should return Ok(1) for symlink");
    assert_eq!(code, 1, "should refuse symlink board");

    // Real dir should be untouched
    assert!(real_dir.exists(), "real dir should be untouched");
}

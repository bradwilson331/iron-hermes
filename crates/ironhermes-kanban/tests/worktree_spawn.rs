//! Integration tests for dispatcher-side `project:` git worktree creation
//! (D-06/D-07) and spawn-time attachment copy-in (D-05) — Phase 46.4 Plan 04.
//!
//! Task 1 tests drive a REAL `run_dispatch_tick` against an injected
//! `spawn_fn` (never exec's `ironhermes`) so the dispatcher-side
//! `create_project_worktree` -> `git worktree add` subprocess runs for real
//! against a throwaway `git init`'d tempdir repo.
//!
//! Task 2 tests cover `worker_spawn::copy_attachments_into` (calling the
//! now-`pub` function directly) and a workspace-persistence assertion after
//! `kanban_complete` (D-09).

use std::process::Command as StdCommand;
use std::sync::{Arc, Mutex};

use tokio::sync::Mutex as AsyncMutex;

use tempfile::TempDir;
use tokio::sync::Mutex as TokioMutex;

use ironhermes_kanban::dispatcher::DispatcherContext;
use ironhermes_kanban::store::CreateTaskOptions;
use ironhermes_kanban::{KanbanConfig, KanbanStore, run_dispatch_tick};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Serializes every `IRONHERMES_HOME`-mutating test in THIS binary (env vars
/// are process-global; `crate::ENV_LOCK` in `lib.rs` is `#[cfg(test)]`-gated
/// and therefore invisible to this external integration-test crate — this
/// is a file-local equivalent for the same purpose). An async-aware
/// `tokio::sync::Mutex` is used (not `std::sync::Mutex`) because the guard
/// must stay held across `.await` points spanning the whole test body.
static TEST_ENV_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());

/// RAII guard that sets an env var and restores the previous value on drop.
/// Mirrors the identical helper used across this crate's other test files
/// (`dispatcher_logic.rs`, `worker_spawn.rs` unit tests) — kept file-local
/// rather than shared to avoid widening any pub surface.
struct ScopedEnv {
    key: String,
    prev: Option<String>,
}

impl ScopedEnv {
    fn set(key: &str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        // Safety: test-only; this binary does not run this env key in
        // parallel from multiple threads.
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

/// `git init` a throwaway repo with one commit, at an absolute path.
fn init_git_repo() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let status = StdCommand::new("git")
            .args(args)
            .current_dir(dir.path())
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .status()
            .expect("git command must spawn");
        assert!(status.success(), "git {args:?} failed");
    };
    run(&["init", "-q"]);
    std::fs::write(dir.path().join("README.md"), "test repo\n").unwrap();
    run(&["add", "README.md"]);
    run(&["commit", "-q", "-m", "initial commit"]);
    dir
}

fn open_store(dir: &TempDir) -> KanbanStore {
    KanbanStore::new(dir.path().join("kanban.db")).expect("open test store")
}

/// Build a `DispatcherContext` whose `spawn_fn` records the workspace string
/// it receives (into `recorded`) and returns a synthetic pid — never exec's
/// `ironhermes`.
fn ctx_with_recording_spawn(
    store: Arc<TokioMutex<KanbanStore>>,
    recorded: Arc<Mutex<Vec<String>>>,
) -> Arc<DispatcherContext> {
    let spawn_fn = Arc::new(
        move |_task: ironhermes_kanban::types::Task,
              _run: ironhermes_kanban::types::TaskRun,
              ws: String,
              _board_slug: String|
              -> std::pin::Pin<
            Box<dyn std::future::Future<Output = ironhermes_kanban::error::Result<u32>> + Send>,
        > {
            let recorded = recorded.clone();
            Box::pin(async move {
                recorded.lock().unwrap().push(ws);
                Ok(4242u32)
            })
        },
    );
    Arc::new(
        DispatcherContext::with_spawn_fn(store, KanbanConfig::default(), spawn_fn)
            .with_gate_fn(allow_all_gate()),
        )
}

/// Build a `DispatcherContext` whose `spawn_fn` panics if invoked — used to
/// assert a worktree-creation failure short-circuits BEFORE `spawn_fn` is
/// ever called.
fn ctx_with_unreachable_spawn(store: Arc<TokioMutex<KanbanStore>>) -> Arc<DispatcherContext> {
    let spawn_fn = Arc::new(
        |_task: ironhermes_kanban::types::Task,
         _run: ironhermes_kanban::types::TaskRun,
         _ws: String,
         _board_slug: String|
         -> std::pin::Pin<
            Box<dyn std::future::Future<Output = ironhermes_kanban::error::Result<u32>> + Send>,
        > {
            Box::pin(async move {
                panic!(
                    "spawn_fn must NOT be called when project: worktree creation fails (D-07 fail-loud)"
                );
            })
        },
    );
    Arc::new(
        DispatcherContext::with_spawn_fn(store, KanbanConfig::default(), spawn_fn)
            .with_gate_fn(allow_all_gate()),
        )
}

// ---------------------------------------------------------------------------
// Task 1: dispatcher-side git worktree creation (D-06/D-07)
// ---------------------------------------------------------------------------

/// A `project:<repo>` task gets a REAL dispatcher-created git worktree under
/// `kanban_worktree_for(task_id)` — outside the referenced repo, distinct
/// from the GSD `.claude/worktrees/` namespace — and the worker receives
/// that absolute path as its workspace.
#[tokio::test]
async fn project_workspace_creates_dispatcher_managed_worktree() {
    // Serialize against every other IRONHERMES_HOME-mutating test in this
    // crate (process-global env var) — mirrors the established pattern in
    // env_compat.rs / notifier_config.rs.
    let _env_lock = TEST_ENV_LOCK.lock().await;
    let home_dir = TempDir::new().unwrap();
    let _home_guard = ScopedEnv::set("IRONHERMES_HOME", home_dir.path().to_str().unwrap());

    let repo_dir = init_git_repo();
    let repo_path = repo_dir.path().to_path_buf();

    let db_dir = TempDir::new().unwrap();
    let store_arc = Arc::new(TokioMutex::new(open_store(&db_dir)));

    let task_id = {
        let mut store = store_arc.lock().await;
        let task = store
            .create_task(
                "project workspace task",
                "alice",
                CreateTaskOptions {
                    workspace: Some(format!("project:{}", repo_path.display())),
                    ..Default::default()
                },
            )
            .unwrap();
        task.id
    };

    let recorded = Arc::new(Mutex::new(Vec::new()));
    let ctx = ctx_with_recording_spawn(store_arc.clone(), recorded.clone());

    run_dispatch_tick(&ctx).await.expect("tick must succeed");

    let expected_worktree = ironhermes_kanban::paths::kanban_worktree_for(&task_id);

    let recorded_ws = recorded.lock().unwrap().clone();
    assert_eq!(
        recorded_ws.len(),
        1,
        "spawn_fn must have been invoked exactly once, got: {recorded_ws:?}"
    );
    let received = std::path::Path::new(&recorded_ws[0]);

    // (a) recorded workspace equals kanban_worktree_for(task_id) as an
    // absolute path.
    assert!(
        received.is_absolute(),
        "worker must receive an absolute worktree path"
    );
    assert_eq!(
        received,
        expected_worktree.as_path(),
        "worker's workspace must equal kanban_worktree_for(task_id)"
    );

    // (b) that directory exists on disk and lives under
    // ~/.ironhermes/kanban/worktrees — NOT inside the repo, NOT under
    // .claude/worktrees.
    assert!(
        received.is_dir(),
        "the worktree directory must exist on disk"
    );
    assert!(
        received.starts_with(home_dir.path().join("kanban").join("worktrees")),
        "worktree must live under ~/.ironhermes/kanban/worktrees, got: {received:?}"
    );
    assert!(
        !received.starts_with(&repo_path),
        "worktree must NOT live inside the referenced repo (D-07), got: {received:?}"
    );
    assert!(
        !received.to_string_lossy().contains(".claude/worktrees"),
        "worktree must NOT collide with the GSD .claude/worktrees/ namespace"
    );

    // (c) `git -C <repo> worktree list --porcelain` reports it as a real
    // git worktree (compare canonicalized paths — macOS /var vs
    // /private/var symlink resolution can differ between the literal path
    // and what git reports).
    let list_output = StdCommand::new("git")
        .arg("-C")
        .arg(&repo_path)
        .arg("worktree")
        .arg("list")
        .arg("--porcelain")
        .output()
        .expect("git worktree list must run");
    let list_str = String::from_utf8_lossy(&list_output.stdout);
    let received_canon = received
        .canonicalize()
        .expect("created worktree dir must canonicalize");
    let listed = list_str
        .lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .any(|p| {
            std::path::Path::new(p).canonicalize().ok().as_deref() == Some(received_canon.as_path())
        });
    assert!(
        listed,
        "git worktree list --porcelain must report the created worktree; got: {list_str}"
    );
}

/// A `project:<repo>` task whose repo path does not exist fails loud into
/// `outcome='spawn_failed'` — and `spawn_fn` is never invoked, proving the
/// worktree-creation guard runs BEFORE any spawn attempt.
#[tokio::test]
async fn project_workspace_missing_repo_yields_spawn_failed() {
    // Serialize against every other IRONHERMES_HOME-mutating test in this
    // crate (process-global env var).
    let _env_lock = TEST_ENV_LOCK.lock().await;
    let home_dir = TempDir::new().unwrap();
    let _home_guard = ScopedEnv::set("IRONHERMES_HOME", home_dir.path().to_str().unwrap());

    let db_dir = TempDir::new().unwrap();
    let store_arc = Arc::new(TokioMutex::new(open_store(&db_dir)));

    // A syntactically absolute path that does not exist on disk.
    let missing_repo = home_dir.path().join("does-not-exist-repo");
    assert!(!missing_repo.exists());

    let task_id = {
        let mut store = store_arc.lock().await;
        let task = store
            .create_task(
                "missing repo task",
                "alice",
                CreateTaskOptions {
                    workspace: Some(format!("project:{}", missing_repo.display())),
                    ..Default::default()
                },
            )
            .unwrap();
        task.id
    };

    let ctx = ctx_with_unreachable_spawn(store_arc.clone());

    run_dispatch_tick(&ctx)
        .await
        .expect("tick itself must not error — worktree-creation failure is handled per-task");

    let store = store_arc.lock().await;
    let runs = store.get_runs(&task_id).unwrap();
    let latest_run = runs
        .last()
        .expect("a task_run row must exist after the claim + worktree-creation attempt");
    assert_eq!(
        latest_run.outcome.as_deref(),
        Some("spawn_failed"),
        "a missing project: repo must be rejected fail-loud into outcome='spawn_failed'"
    );
    assert!(
        !latest_run.error.as_deref().unwrap_or("").is_empty(),
        "spawn_failed run must carry a non-empty, actionable error string"
    );
    assert!(
        !missing_repo.exists(),
        "a missing project: repo must not be created as a side effect"
    );
}

// ---------------------------------------------------------------------------
// Task 2: copy_attachments_into (D-05) + workspace persistence after
// kanban_complete (D-09)
// ---------------------------------------------------------------------------

/// `worker_spawn::copy_attachments_into` is DATABASE-driven (D-05 UAT fix):
/// it reads `KanbanStore::list_attachments` (not the filesystem layout) and
/// copies each attachment into the workspace under its ORIGINAL filename,
/// regardless of on-disk layout. This test drives it through the real store
/// (`add_attachment`) rather than hand-building a filesystem tree, so it
/// exercises the exact same path production code takes.
#[tokio::test]
async fn worker_spawn_copy_attachments_into_copies_files_and_counts_them() {
    let _env_lock = TEST_ENV_LOCK.lock().await;
    let home_dir = TempDir::new().unwrap();
    let _home_guard = ScopedEnv::set("IRONHERMES_HOME", home_dir.path().to_str().unwrap());

    // The "default" board's db lives at kanban_db_path() (== home_dir/kanban.db)
    // — open it directly there so open_from_env_or_board(Some("default"))
    // resolves to the SAME db file inside copy_attachments_into.
    let mut store = KanbanStore::new(ironhermes_kanban::paths::kanban_db_path())
        .expect("open default-board store");

    let task = store
        .create_task("attachment task", "alice", CreateTaskOptions::default())
        .unwrap();

    store
        .add_attachment(&task.id, "hero.png", b"PNGDATA", None, Some("ui"))
        .expect("add_attachment hero.png must succeed");
    store
        .add_attachment(&task.id, "notes.txt", b"hello", None, Some("ui"))
        .expect("add_attachment notes.txt must succeed");

    let workspace_dir = TempDir::new().unwrap();

    let count = ironhermes_kanban::worker_spawn::copy_attachments_into(
        workspace_dir.path(),
        &task.id,
        "default",
    )
    .expect("copy_attachments_into must succeed");

    assert_eq!(count, 2, "both attachments must be copied");
    assert_eq!(
        std::fs::read(workspace_dir.path().join("hero.png")).unwrap(),
        b"PNGDATA",
        "must be copied by its ORIGINAL filename (hero.png), not the on-disk attachment id"
    );
    assert_eq!(
        std::fs::read(workspace_dir.path().join("notes.txt")).unwrap(),
        b"hello",
        "must be copied by its ORIGINAL filename (notes.txt), not the on-disk attachment id"
    );
}

/// A task with no attachments recorded in the store returns `Ok(0)`, never
/// an error.
#[tokio::test]
async fn worker_spawn_copy_attachments_into_empty_or_missing_dir_returns_zero() {
    let _env_lock = TEST_ENV_LOCK.lock().await;
    let home_dir = TempDir::new().unwrap();
    let _home_guard = ScopedEnv::set("IRONHERMES_HOME", home_dir.path().to_str().unwrap());

    let mut store = KanbanStore::new(ironhermes_kanban::paths::kanban_db_path())
        .expect("open default-board store");

    let workspace_dir = TempDir::new().unwrap();

    // A task that exists in the store but has no attachments.
    let task = store
        .create_task("no attachments task", "alice", CreateTaskOptions::default())
        .unwrap();
    let count = ironhermes_kanban::worker_spawn::copy_attachments_into(
        workspace_dir.path(),
        &task.id,
        "default",
    )
    .expect("task with no attachments must not error");
    assert_eq!(count, 0);

    // A task id that does not exist in the store at all — list_attachments
    // returns an empty result set rather than erroring.
    let count2 = ironhermes_kanban::worker_spawn::copy_attachments_into(
        workspace_dir.path(),
        "t_does_not_exist",
        "default",
    )
    .expect("nonexistent task id must not error");
    assert_eq!(count2, 0);
}

/// D-09: completing a task does not remove its (scratch) workspace
/// directory — no code path deletes it on `kanban_complete`.
#[tokio::test]
async fn worker_spawn_workspace_persists_after_complete() {
    let _env_lock = TEST_ENV_LOCK.lock().await;
    let home_dir = TempDir::new().unwrap();
    let _home_guard = ScopedEnv::set("IRONHERMES_HOME", home_dir.path().to_str().unwrap());

    let db_dir = TempDir::new().unwrap();
    let mut store = open_store(&db_dir);

    let task = store
        .create_task(
            "scratch persistence task",
            "alice",
            CreateTaskOptions::default(),
        )
        .unwrap();

    // Simulate the scratch workspace resolve_workspace_dir eagerly creates
    // at spawn time (D-31) — this test targets kanban_complete's contract
    // (does it clean up?), not the spawn path itself.
    let scratch_dir = ironhermes_kanban::paths::kanban_workspace_for(&task.id);
    std::fs::create_dir_all(&scratch_dir).unwrap();
    std::fs::write(scratch_dir.join("work.txt"), b"work product").unwrap();
    assert!(scratch_dir.is_dir());

    store
        .complete_task(
            &task.id,
            Some("done"),
            None,
            None,
            None,
            None,
            "alice",
            None,
        )
        .expect("complete_task must succeed");

    assert!(
        scratch_dir.is_dir(),
        "the scratch workspace must still exist on disk after kanban_complete (D-09)"
    );
    assert_eq!(
        std::fs::read(scratch_dir.join("work.txt")).unwrap(),
        b"work product",
        "workspace contents must be untouched by kanban_complete"
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

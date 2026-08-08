//! Integration smoke tests for the 6 kanban LLM tools (Plan 04).
//!
//! Each test constructs a tempfile-backed KanbanStore inside an
//! Arc<TokioMutex<>>, drives a tool's execute() path, and asserts the
//! resulting JSON matches the expected shape / store state.
//!
//! D-22 gate tests verify stale_run_id and created_cards rejection paths.
//! D-24 idempotency test verifies no duplicate rows are created.
//! D-31 workspace test verifies relative dir: paths are rejected.

use std::sync::Arc;

use ironhermes_kanban::mention::MAX_MENTION_CHAIN_DEPTH;
use ironhermes_kanban::store::{CreateTaskOptions, KanbanStore};
use ironhermes_kanban::store::{MentionChildSpec, MentionPlan};
use ironhermes_kanban::tools::{
    KanbanBlockTool, KanbanCommentTool, KanbanCompleteTool, KanbanCreateTool, KanbanHeartbeatTool,
    KanbanLinkTool, KanbanListTool, KanbanMentionTool, KanbanShowTool, KanbanUnblockTool,
};
use ironhermes_kanban::{KanbanWorkerSpec, SwarmGraphIds, SwarmGraphSpec};
use ironhermes_tools::Tool;
use rusqlite::params;
use serde_json::{Value, json};
use tokio::sync::Mutex as TokioMutex;

// Phase 36.3.7.x test-isolation hardening: `IRONHERMES_KANBAN_TASK` /
// `HERMES_PROFILE` are process-global env vars read by the kanban tools at
// `execute()` time. Tests that manipulate them must serialize via this lock
// or they race when cargo runs them in parallel (the failure shape is
// `task not found: t_<other-test's-id>` because a sibling test overwrites
// `IRONHERMES_KANBAN_TASK` mid-execute). Each affected test takes the lock
// before any `set_var` / `remove_var` / `tool.execute(...)` call.
//
// `unwrap_or_else(|e| e.into_inner())` recovers from poison: if a prior
// test panicked while holding the lock, we still run, the stale env-var
// state from that test would have been remove_var'd before the panic in
// well-formed tests OR is overwritten by the new test's set_var.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a test store backed by a fresh tempdir.
///
/// Phase 36.3.7.9 Plan 07: tools now use `KanbanStore::open_for_board("default")` so
/// they resolve the DB path via `IRONHERMES_HOME`. This helper sets `IRONHERMES_HOME`
/// to the tempdir and opens `KanbanStore::open_default()` so both the seeding code
/// (via the returned Arc) and the tools (via `open_for_board("default")`) hit the
/// same on-disk file. Callers MUST hold `ENV_LOCK` before calling this function to
/// prevent IRONHERMES_HOME from racing between tests.
fn make_store() -> Arc<TokioMutex<KanbanStore>> {
    let dir = tempfile::tempdir().unwrap();
    // Redirect IRONHERMES_HOME so open_for_board("default") inside tools hits this dir.
    unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };
    // Leak tempdir so the DB file survives the test.
    std::mem::forget(dir);
    let store = KanbanStore::open_default().unwrap();
    Arc::new(TokioMutex::new(store))
}

/// Seed a running task + task_run row so we have a valid run_id to test against.
///
/// Returns (task_id, run_id).
fn seed_running_task(
    store: &mut KanbanStore,
    title: &str,
    assignee: &str,
    run_id: &str,
    profile: Option<&str>,
) -> String {
    let opts = CreateTaskOptions {
        created_by: profile.map(String::from),
        ..Default::default()
    };
    let task = store.create_task(title, assignee, opts).unwrap();
    let task_id = task.id.clone();

    // Manually insert a task_run row and set current_run_id / status so the
    // complete/block tools see a real running task.
    let now = 1_700_000_000.0f64;
    store
        .conn
        .execute(
            "INSERT INTO task_runs (id, task_id, claim_lock, started_at) VALUES (?1, ?2, 'lock', ?3)",
            params![run_id, task_id, now],
        )
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE tasks SET status='running', current_run_id=?1, claim_lock='lock' WHERE id=?2",
            params![run_id, task_id],
        )
        .unwrap();

    task_id
}

// ---------------------------------------------------------------------------
// kanban_show tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kanban_show_unavailable_without_env() {
    let _env_guard = ENV_LOCK.lock().await;
    unsafe {
        std::env::remove_var("IRONHERMES_KANBAN_TASK");
    }
    let store = make_store();
    let tool = KanbanShowTool::new(store, false);
    assert!(
        !tool.is_available(),
        "should be unavailable without env or explicit_enable"
    );
}

#[tokio::test]
async fn kanban_show_available_with_explicit_enable() {
    let _env_guard = ENV_LOCK.lock().await;
    unsafe {
        std::env::remove_var("IRONHERMES_KANBAN_TASK");
    }
    let store = make_store();
    let tool = KanbanShowTool::new(store, true);
    assert!(
        tool.is_available(),
        "explicit_enable=true should make tool available"
    );
}

#[tokio::test]
async fn kanban_show_envelope_shape() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();

    // Create parent task, mark it done with a run that has summary+metadata.
    let parent_id = {
        let mut s = store.lock().await;
        let opts = CreateTaskOptions {
            created_by: Some("bot".into()),
            ..Default::default()
        };
        let parent = s.create_task("Parent Task", "bot-worker", opts).unwrap();
        let pid = parent.id.clone();
        // Insert a completed run for the parent.
        s.conn.execute(
            "INSERT INTO task_runs (id, task_id, claim_lock, started_at, ended_at, outcome, summary, metadata) \
             VALUES ('r_parent', ?1, 'lock', 1000.0, 2000.0, 'completed', 'parent done', '{\"key\":\"val\"}')",
            params![pid],
        ).unwrap();
        s.conn
            .execute("UPDATE tasks SET status='done' WHERE id=?1", params![pid])
            .unwrap();
        pid
    };

    // Create child task that depends on parent.
    let child_id = {
        let mut s = store.lock().await;
        let opts = CreateTaskOptions {
            parents: vec![parent_id.clone()],
            created_by: Some("bot".into()),
            ..Default::default()
        };
        let child = s.create_task("Child Task", "bot-worker", opts).unwrap();
        child.id.clone()
    };

    // Add two comments to the child.
    {
        let mut s = store.lock().await;
        s.add_comment(&child_id, "operator", "first comment")
            .unwrap();
        s.add_comment(&child_id, "bot", "second comment").unwrap();
    }

    // Set env so tool is available.
    unsafe {
        std::env::set_var("IRONHERMES_KANBAN_TASK", &child_id);
    }
    let tool = KanbanShowTool::new(store, false);
    let result = tool.execute(json!({})).await.unwrap();
    let v: Value = serde_json::from_str(&result).unwrap();
    unsafe {
        std::env::remove_var("IRONHERMES_KANBAN_TASK");
    }

    // Assert envelope shape.
    assert_eq!(v["task_id"].as_str().unwrap(), child_id);
    assert_eq!(v["title"].as_str().unwrap(), "Child Task");
    assert!(v["parent_handoffs"].is_array());
    assert_eq!(v["parent_handoffs"].as_array().unwrap().len(), 1);
    assert_eq!(
        v["parent_handoffs"][0]["parent_id"].as_str().unwrap(),
        parent_id
    );
    assert_eq!(
        v["parent_handoffs"][0]["summary"].as_str().unwrap(),
        "parent done"
    );
    assert!(v["comments"].is_array());
    assert_eq!(v["comments"].as_array().unwrap().len(), 2);
    assert_eq!(v["comments"][0]["author"].as_str().unwrap(), "operator");
    assert!(v["prior_attempts"].is_array());
}

// ---------------------------------------------------------------------------
// kanban_complete tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kanban_complete_rejects_stale_run_id() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let task_id = {
        let mut s = store.lock().await;
        seed_running_task(&mut s, "Test Task", "bot-worker", "r_real", Some("bot"))
    };

    unsafe {
        std::env::set_var("IRONHERMES_KANBAN_TASK", &task_id);
    }
    unsafe {
        std::env::set_var("HERMES_PROFILE", "bot");
    }
    let tool = KanbanCompleteTool::new(store, false);

    let result = tool
        .execute(json!({
            "summary": "done",
            "expected_run_id": "r_stale"
        }))
        .await
        .unwrap();

    unsafe {
        std::env::remove_var("IRONHERMES_KANBAN_TASK");
    }
    unsafe {
        std::env::remove_var("HERMES_PROFILE");
    }

    let v: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["status"].as_str().unwrap(), "rejected");
    assert_eq!(v["reason"].as_str().unwrap(), "stale_run_id");
}

#[tokio::test]
async fn kanban_complete_rejects_phantom_created_cards() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let task_id = {
        let mut s = store.lock().await;
        seed_running_task(&mut s, "Test Task", "bot-worker", "r_real", Some("bot"))
    };

    unsafe {
        std::env::set_var("IRONHERMES_KANBAN_TASK", &task_id);
    }
    unsafe {
        std::env::set_var("HERMES_PROFILE", "bot");
    }
    let tool = KanbanCompleteTool::new(store.clone(), false);

    let result = tool
        .execute(json!({
            "summary": "x",
            "created_cards": ["t_does_not_exist"]
        }))
        .await
        .unwrap();

    unsafe {
        std::env::remove_var("IRONHERMES_KANBAN_TASK");
    }
    unsafe {
        std::env::remove_var("HERMES_PROFILE");
    }

    let v: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["status"].as_str().unwrap(), "rejected");
    assert_eq!(v["reason"].as_str().unwrap(), "created_cards");
    let phantom = v["phantom_ids"].as_array().unwrap();
    assert!(
        phantom
            .iter()
            .any(|p| p.as_str() == Some("t_does_not_exist")),
        "phantom_ids must contain t_does_not_exist"
    );

    // Verify a completion_rejected event row exists in task_events.
    let s = store.lock().await;
    let count: i64 = s
        .conn
        .query_row(
            "SELECT COUNT(*) FROM task_events WHERE task_id=?1 AND kind='completion_rejected'",
            params![task_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 1,
        "completion_rejected event must be permanently written"
    );
}

#[tokio::test]
async fn kanban_complete_rejects_wrong_profile_card() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();

    // Create a child task owned by "other_profile".
    let child_id = {
        let mut s = store.lock().await;
        let opts = CreateTaskOptions {
            created_by: Some("other_profile".into()),
            ..Default::default()
        };
        s.create_task("Child", "bot-worker", opts).unwrap().id
    };

    // Create parent task running under "bot".
    let task_id = {
        let mut s = store.lock().await;
        seed_running_task(&mut s, "Parent", "bot-worker", "r_real", Some("bot"))
    };

    unsafe {
        std::env::set_var("IRONHERMES_KANBAN_TASK", &task_id);
    }
    unsafe {
        std::env::set_var("HERMES_PROFILE", "bot");
    }
    let tool = KanbanCompleteTool::new(store, false);

    let result = tool
        .execute(json!({
            "summary": "done",
            "created_cards": [child_id]
        }))
        .await
        .unwrap();

    unsafe {
        std::env::remove_var("IRONHERMES_KANBAN_TASK");
    }
    unsafe {
        std::env::remove_var("HERMES_PROFILE");
    }

    let v: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["status"].as_str().unwrap(), "rejected");
    assert_eq!(v["reason"].as_str().unwrap(), "created_cards");
    let wrong = v["wrong_profile_ids"].as_array().unwrap();
    assert!(!wrong.is_empty(), "wrong_profile_ids must be non-empty");
}

#[tokio::test]
async fn kanban_complete_accepts_valid_created_cards() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();

    // Bot creates child task.
    let child_id = {
        let mut s = store.lock().await;
        let opts = CreateTaskOptions {
            created_by: Some("bot".into()),
            ..Default::default()
        };
        s.create_task("Child Task", "bot-worker", opts).unwrap().id
    };

    // Bot creates and runs parent.
    let task_id = {
        let mut s = store.lock().await;
        seed_running_task(&mut s, "Parent Task", "bot-worker", "r_real", Some("bot"))
    };

    unsafe {
        std::env::set_var("IRONHERMES_KANBAN_TASK", &task_id);
    }
    unsafe {
        std::env::set_var("HERMES_PROFILE", "bot");
    }
    let tool = KanbanCompleteTool::new(store, false);

    let result = tool
        .execute(json!({
            "summary": "done",
            "expected_run_id": "r_real",
            "created_cards": [child_id]
        }))
        .await
        .unwrap();

    unsafe {
        std::env::remove_var("IRONHERMES_KANBAN_TASK");
    }
    unsafe {
        std::env::remove_var("HERMES_PROFILE");
    }

    let v: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["status"].as_str().unwrap(), "ok");
}

// ---------------------------------------------------------------------------
// kanban_complete output_path tests (Phase 46.4 Plan 03 — D-10)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kanban_complete_output_path_round_trips_through_get_task() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let task_id = {
        let mut s = store.lock().await;
        seed_running_task(
            &mut s,
            "Output Path Task",
            "bot-worker",
            "r_op1",
            Some("bot"),
        )
    };

    unsafe {
        std::env::set_var("IRONHERMES_KANBAN_TASK", &task_id);
    }
    unsafe {
        std::env::set_var("HERMES_PROFILE", "bot");
    }
    let tool = KanbanCompleteTool::new(store.clone(), false);

    let result = tool
        .execute(json!({
            "summary": "done",
            "output_path": "/tmp/deploy/artifact.tar.gz"
        }))
        .await
        .unwrap();

    unsafe {
        std::env::remove_var("IRONHERMES_KANBAN_TASK");
    }
    unsafe {
        std::env::remove_var("HERMES_PROFILE");
    }

    let v: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["status"].as_str().unwrap(), "ok");

    let s = store.lock().await;
    let task = s.get_task(&task_id).unwrap();
    assert_eq!(
        task.output_path.as_deref(),
        Some("/tmp/deploy/artifact.tar.gz"),
        "output_path must round-trip through get_task after kanban_complete"
    );
}

#[tokio::test]
async fn kanban_complete_without_output_path_stays_none() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let task_id = {
        let mut s = store.lock().await;
        seed_running_task(
            &mut s,
            "No Output Path Task",
            "bot-worker",
            "r_op2",
            Some("bot"),
        )
    };

    unsafe {
        std::env::set_var("IRONHERMES_KANBAN_TASK", &task_id);
    }
    unsafe {
        std::env::set_var("HERMES_PROFILE", "bot");
    }
    let tool = KanbanCompleteTool::new(store.clone(), false);

    let result = tool.execute(json!({ "summary": "done" })).await.unwrap();

    unsafe {
        std::env::remove_var("IRONHERMES_KANBAN_TASK");
    }
    unsafe {
        std::env::remove_var("HERMES_PROFILE");
    }

    let v: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["status"].as_str().unwrap(), "ok");

    let s = store.lock().await;
    let task = s.get_task(&task_id).unwrap();
    assert_eq!(
        task.output_path, None,
        "output_path must stay None when the tool arg is omitted"
    );
}

#[tokio::test]
async fn kanban_complete_second_call_without_output_path_preserves_earlier_value() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let task_id = {
        let mut s = store.lock().await;
        seed_running_task(&mut s, "Preserve Task", "bot-worker", "r_op3", Some("bot"))
    };

    // First complete sets output_path directly via the store (mirrors a
    // worker's kanban_complete call with the arg set).
    {
        let mut s = store.lock().await;
        s.complete_task(
            &task_id,
            Some("first"),
            None,
            None,
            None,
            None,
            "bot",
            Some("/tmp/deploy/v1"),
        )
        .unwrap();
    }

    // Second complete call omits output_path (None) — must NOT clear the
    // earlier value (COALESCE-preserve semantics, D-10).
    {
        let mut s = store.lock().await;
        s.complete_task(
            &task_id,
            Some("second"),
            None,
            None,
            None,
            None,
            "bot",
            None,
        )
        .unwrap();
    }

    let s = store.lock().await;
    let task = s.get_task(&task_id).unwrap();
    assert_eq!(
        task.output_path.as_deref(),
        Some("/tmp/deploy/v1"),
        "a subsequent complete_task call with output_path=None must preserve the earlier value"
    );
}

// ---------------------------------------------------------------------------
// kanban_create tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kanban_create_idempotency_key_dedups() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();

    let tool = KanbanCreateTool::new(store.clone(), true);

    let r1 = tool
        .execute(json!({
            "title": "My Task",
            "assignee": "bot-worker",
            "idempotency_key": "unique-key-123"
        }))
        .await
        .unwrap();
    let v1: Value = serde_json::from_str(&r1).unwrap();
    let id1 = v1["task_id"].as_str().unwrap().to_string();

    let r2 = tool
        .execute(json!({
            "title": "My Task (dup)",
            "assignee": "bot-worker",
            "idempotency_key": "unique-key-123"
        }))
        .await
        .unwrap();
    let v2: Value = serde_json::from_str(&r2).unwrap();
    let id2 = v2["task_id"].as_str().unwrap().to_string();

    assert_eq!(id1, id2, "idempotency_key must return the same task_id");

    // Only one row.
    let s = store.lock().await;
    let count: i64 = s
        .conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE idempotency_key='unique-key-123'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "only one task row should exist");
}

#[tokio::test]
async fn kanban_create_with_parents_starts_in_todo() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();

    // Create a ready parent.
    let parent_id = {
        let mut s = store.lock().await;
        let opts = CreateTaskOptions::default();
        s.create_task("Parent", "bot-worker", opts).unwrap().id
    };

    let tool = KanbanCreateTool::new(store.clone(), true);
    let result = tool
        .execute(json!({
            "title": "Child",
            "assignee": "bot-worker",
            "parents": [parent_id]
        }))
        .await
        .unwrap();

    let v: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(
        v["status"].as_str().unwrap(),
        "todo",
        "child with parents must start in 'todo'"
    );
}

#[tokio::test]
async fn kanban_create_rejects_relative_dir_workspace() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let tool = KanbanCreateTool::new(store, true);

    let result = tool
        .execute(json!({
            "title": "Bad workspace",
            "assignee": "bot-worker",
            "workspace": "dir:../foo"
        }))
        .await;

    assert!(
        result.is_err(),
        "relative dir: workspace must be rejected as Err"
    );
}

#[tokio::test]
async fn kanban_create_max_runtime_parses_human() {
    use ironhermes_kanban::tools::create::parse_max_runtime;

    assert_eq!(parse_max_runtime(&json!("30m")), Some(1800));
    assert_eq!(parse_max_runtime(&json!("2h")), Some(7200));
    assert_eq!(parse_max_runtime(&json!("1d")), Some(86400));
    assert_eq!(parse_max_runtime(&json!(120)), Some(120));
}

// ---------------------------------------------------------------------------
// kanban_block tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kanban_block_with_review_required_prefix_logs_advisory() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let task_id = {
        let mut s = store.lock().await;
        seed_running_task(&mut s, "Review Task", "bot-worker", "r_review", Some("bot"))
    };

    unsafe {
        std::env::set_var("IRONHERMES_KANBAN_TASK", &task_id);
    }
    let tool = KanbanBlockTool::new(store.clone(), false);

    let result = tool
        .execute(json!({
            "reason": "review-required: needs eyes",
            "expected_run_id": "r_review"
        }))
        .await
        .unwrap();

    unsafe {
        std::env::remove_var("IRONHERMES_KANBAN_TASK");
    }

    let v: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["status"].as_str().unwrap(), "ok");

    // Verify the task is now blocked.
    let s = store.lock().await;
    let task = s.get_task(&task_id).unwrap();
    assert_eq!(task.status, "blocked");
}

// ---------------------------------------------------------------------------
// kanban_list test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kanban_list_returns_compact_summaries() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();

    // Seed a few tasks.
    {
        let mut s = store.lock().await;
        s.create_task("Task A", "bot-worker", CreateTaskOptions::default())
            .unwrap();
        s.create_task("Task B", "bot-worker", CreateTaskOptions::default())
            .unwrap();
    }

    let tool = KanbanListTool::new(store, true);
    let result = tool.execute(json!({})).await.unwrap();
    let v: Value = serde_json::from_str(&result).unwrap();

    // Plan 07: list now returns {"tasks": [...], "board": ..., "board_source": ...}.
    let tasks = v["tasks"]
        .as_array()
        .expect("expected tasks array in envelope");
    assert!(tasks.len() >= 2);
    // Compact shape: must have id, title, status — must NOT have body.
    let first = &tasks[0];
    assert!(first.get("id").is_some());
    assert!(first.get("title").is_some());
    assert!(first.get("status").is_some());
    assert!(
        first.get("body").is_none(),
        "list must not include body (D-25)"
    );
}

// ---------------------------------------------------------------------------
// kanban_comment test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kanban_comment_adds_comment_to_task() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();

    let task_id = {
        let mut s = store.lock().await;
        s.create_task("Comment Test", "bot-worker", CreateTaskOptions::default())
            .unwrap()
            .id
    };

    unsafe {
        std::env::set_var("IRONHERMES_KANBAN_TASK", &task_id);
    }
    unsafe {
        std::env::set_var("HERMES_PROFILE", "bot");
    }
    let tool = KanbanCommentTool::new(store.clone(), false);

    let result = tool
        .execute(json!({ "body": "Hello from bot" }))
        .await
        .unwrap();

    unsafe {
        std::env::remove_var("IRONHERMES_KANBAN_TASK");
    }
    unsafe {
        std::env::remove_var("HERMES_PROFILE");
    }

    let v: Value = serde_json::from_str(&result).unwrap();
    assert!(v.get("comment_id").is_some());
    assert_eq!(v["task_id"].as_str().unwrap(), task_id);

    // Verify comment is in the DB.
    let s = store.lock().await;
    let count: i64 = s
        .conn
        .query_row(
            "SELECT COUNT(*) FROM task_comments WHERE task_id=?1",
            params![task_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

// ---------------------------------------------------------------------------
// kanban_heartbeat tests (Phase 36.3.7.6 BUG-36.3.7.6-01 + BUG-36.3.7.6-04)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kanban_heartbeat_appends_event_row() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let task_id = {
        let mut s = store.lock().await;
        seed_running_task(&mut s, "Long Op", "alice", "r_hb", Some("alice"))
    };

    unsafe {
        std::env::set_var("IRONHERMES_KANBAN_TASK", &task_id);
    }
    unsafe {
        std::env::set_var("IRONHERMES_KANBAN_RUN_ID", "r_hb");
    }

    let tool = KanbanHeartbeatTool::new(store.clone(), false);
    let result = tool.execute(json!({"note": "halfway"})).await.unwrap();

    unsafe {
        std::env::remove_var("IRONHERMES_KANBAN_TASK");
    }
    unsafe {
        std::env::remove_var("IRONHERMES_KANBAN_RUN_ID");
    }

    let v: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(
        v["status"].as_str().unwrap(),
        "ok",
        "BUG-36.3.7.6-01: heartbeat happy path"
    );
    assert!(v["event_id"].as_i64().unwrap() > 0);
    assert_eq!(v["task_id"].as_str().unwrap(), task_id);

    // Assert exactly one heartbeat event row exists for this task.
    let s = store.lock().await;
    let count: i64 = s
        .conn
        .query_row(
            "SELECT count(*) FROM task_events WHERE task_id = ?1 AND kind = 'heartbeat'",
            params![&task_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 1,
        "BUG-36.3.7.6-01: exactly one heartbeat row appended"
    );
}

#[tokio::test]
async fn kanban_heartbeat_missing_task_id_errors_without_env() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();

    unsafe {
        std::env::remove_var("IRONHERMES_KANBAN_TASK");
    }
    unsafe {
        std::env::remove_var("IRONHERMES_KANBAN_RUN_ID");
    }

    // explicit_enable=true so is_available() is true even without the env var;
    // we want the execute() path to be the one that rejects.
    let tool = KanbanHeartbeatTool::new(store.clone(), true);
    let result = tool.execute(json!({})).await.unwrap();
    let v: Value = serde_json::from_str(&result).unwrap();

    assert_eq!(
        v["status"].as_str().unwrap(),
        "rejected",
        "BUG-36.3.7.6-01: missing task id rejected"
    );
    assert_eq!(v["reason"].as_str().unwrap(), "missing_task_id");
}

// ---------------------------------------------------------------------------
// kanban_link tests (Phase 36.3.7.6 BUG-36.3.7.6-02 + BUG-36.3.7.6-04)
// ---------------------------------------------------------------------------

/// Helper for non-running tasks (no run_id / claim_lock). Shared by kanban_link
/// and kanban_unblock tests; declared at module scope so both sections can use
/// it without redeclaration. (NIT-2 from plan-checker.)
fn seed_plain_task(store: &mut KanbanStore, title: &str) -> String {
    let opts = CreateTaskOptions::default();
    store.create_task(title, "alice", opts).unwrap().id
}

#[tokio::test]
async fn kanban_link_happy_path_inserts_row() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let (a, b) = {
        let mut s = store.lock().await;
        (seed_plain_task(&mut s, "A"), seed_plain_task(&mut s, "B"))
    };

    let tool = KanbanLinkTool::new(store.clone(), true);
    let result = tool
        .execute(json!({"parent_id": &a, "child_id": &b}))
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&result).unwrap();

    assert_eq!(
        v["status"].as_str().unwrap(),
        "ok",
        "BUG-36.3.7.6-02: link happy path"
    );
    assert_eq!(v["parent_id"].as_str().unwrap(), a);
    assert_eq!(v["child_id"].as_str().unwrap(), b);

    let s = store.lock().await;
    let count: i64 = s
        .conn
        .query_row(
            "SELECT count(*) FROM task_links WHERE parent_id = ?1 AND child_id = ?2",
            params![&a, &b],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "BUG-36.3.7.6-02: one row written on happy path");
}

#[tokio::test]
async fn kanban_link_rejects_cycle() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let (a, b) = {
        let mut s = store.lock().await;
        (seed_plain_task(&mut s, "A"), seed_plain_task(&mut s, "B"))
    };

    let tool = KanbanLinkTool::new(store.clone(), true);

    // Step 1: A → B is fine.
    let r1 = tool
        .execute(json!({"parent_id": &a, "child_id": &b}))
        .await
        .unwrap();
    let v1: Value = serde_json::from_str(&r1).unwrap();
    assert_eq!(v1["status"].as_str().unwrap(), "ok");

    // Step 2: B → A would close the cycle.
    let r2 = tool
        .execute(json!({"parent_id": &b, "child_id": &a}))
        .await
        .unwrap();
    let v2: Value = serde_json::from_str(&r2).unwrap();
    assert_eq!(
        v2["status"].as_str().unwrap(),
        "rejected",
        "BUG-36.3.7.6-02: cycle rejected"
    );
    assert_eq!(v2["reason"].as_str().unwrap(), "link_cycle");
    assert_eq!(v2["parent_id"].as_str().unwrap(), b);
    assert_eq!(v2["child_id"].as_str().unwrap(), a);

    let s = store.lock().await;
    let count: i64 = s
        .conn
        .query_row(
            "SELECT count(*) FROM task_links WHERE parent_id = ?1 AND child_id = ?2",
            params![&b, &a],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "BUG-36.3.7.6-02: no cycle row written");
}

#[tokio::test]
async fn kanban_link_phantom_id_rejected() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let b = {
        let mut s = store.lock().await;
        seed_plain_task(&mut s, "B")
    };

    let tool = KanbanLinkTool::new(store.clone(), true);
    let result = tool
        .execute(json!({"parent_id": "t_phantom", "child_id": &b}))
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&result).unwrap();

    assert_eq!(
        v["status"].as_str().unwrap(),
        "rejected",
        "BUG-36.3.7.6-02: phantom rejected"
    );
    assert_eq!(v["reason"].as_str().unwrap(), "task_not_found");
    assert_eq!(v["id"].as_str().unwrap(), "t_phantom");

    // No row written.
    let s = store.lock().await;
    let count: i64 = s
        .conn
        .query_row(
            "SELECT count(*) FROM task_links WHERE child_id = ?1",
            params![&b],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

// ---------------------------------------------------------------------------
// kanban_unblock tests (Phase 36.3.7.6 BUG-36.3.7.6-03 + BUG-36.3.7.6-04)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kanban_unblock_happy_path_from_blocked() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let task_id = {
        let mut s = store.lock().await;
        let id = seed_plain_task(&mut s, "Blockable");
        // Drop to a known blocked state via direct UPDATE — simpler than
        // running through block_task (which requires a synthetic run).
        s.conn
            .execute(
                "UPDATE tasks SET status = 'blocked' WHERE id = ?1",
                params![&id],
            )
            .unwrap();
        id
    };

    let tool = KanbanUnblockTool::new(store.clone(), true);
    let result = tool.execute(json!({"task_id": &task_id})).await.unwrap();
    let v: Value = serde_json::from_str(&result).unwrap();

    assert_eq!(
        v["status"].as_str().unwrap(),
        "ok",
        "BUG-36.3.7.6-03: unblock happy path"
    );
    assert_eq!(v["task_id"].as_str().unwrap(), task_id);

    // Status now ready; one `unblocked` event row.
    let s = store.lock().await;
    let status: String = s
        .conn
        .query_row(
            "SELECT status FROM tasks WHERE id = ?1",
            params![&task_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "ready");

    let unblocked_count: i64 = s
        .conn
        .query_row(
            "SELECT count(*) FROM task_events WHERE task_id = ?1 AND kind = 'unblocked'",
            params![&task_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        unblocked_count, 1,
        "BUG-36.3.7.6-03: one unblocked event appended"
    );
}

#[tokio::test]
async fn kanban_unblock_rejects_wrong_status() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let task_id = {
        let mut s = store.lock().await;
        let id = seed_plain_task(&mut s, "Done");
        s.conn
            .execute(
                "UPDATE tasks SET status = 'done' WHERE id = ?1",
                params![&id],
            )
            .unwrap();
        id
    };

    let tool = KanbanUnblockTool::new(store.clone(), true);
    let result = tool.execute(json!({"task_id": &task_id})).await.unwrap();
    let v: Value = serde_json::from_str(&result).unwrap();

    assert_eq!(
        v["status"].as_str().unwrap(),
        "rejected",
        "BUG-36.3.7.6-03: wrong status rejected"
    );
    assert_eq!(v["reason"].as_str().unwrap(), "invalid_status");
    assert_eq!(v["task_id"].as_str().unwrap(), task_id);
    assert_eq!(v["current"].as_str().unwrap(), "done");
    assert_eq!(v["expected"].as_str().unwrap(), "blocked");

    // Side effects: status unchanged; NO `unblocked` event row.
    let s = store.lock().await;
    let status: String = s
        .conn
        .query_row(
            "SELECT status FROM tasks WHERE id = ?1",
            params![&task_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "done", "BUG-36.3.7.6-03: status not silently moved");

    let unblocked_count: i64 = s
        .conn
        .query_row(
            "SELECT count(*) FROM task_events WHERE task_id = ?1 AND kind = 'unblocked'",
            params![&task_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        unblocked_count, 0,
        "BUG-36.3.7.6-03: no unblocked event appended"
    );
}

// ---------------------------------------------------------------------------
// kanban_swarm tests (Phase 36.3.7.7 BUG-36.3.7.7-01)
// ---------------------------------------------------------------------------
//
// Receiver-end tests for the new `KanbanStore::create_swarm` sibling primitive
// + the `KanbanSwarmTool` LLM tool registered as the 10th tool. Most tests
// drive `store.create_swarm(spec)` directly to assert DB-level shape (rows in
// tasks/task_links/task_events/task_comments) and atomic rollback on failure.
// The flat-worker auto-title test covers the per-card title format.

/// Helper that builds a minimal SwarmGraphSpec with a flat-form worker set,
/// no verifier, no synth, no blackboard.
fn seed_swarm_graph(store: &mut KanbanStore, goal: &str, workers: &[&str]) -> SwarmGraphIds {
    let spec = SwarmGraphSpec {
        goal: goal.to_string(),
        workers: workers
            .iter()
            .map(|w| KanbanWorkerSpec {
                assignee: (*w).to_string(),
                title: None,
                body: None,
            })
            .collect(),
        verifier: None,
        synthesizer: None,
        blackboard: None,
        workspace: None,
        skills: None,
        tenant: None,
        priority: None,
        max_runtime_seconds: None,
        max_retries: None,
        idempotency_key: None,
        created_by: Some("alice".to_string()),
        body: None,
    };
    store.create_swarm(spec).unwrap()
}

#[tokio::test]
async fn swarm_fan_out_only_creates_root_and_workers() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let mut s = store.lock().await;

    let ids = seed_swarm_graph(&mut s, "fan-out goal", &["bob", "carol", "dave"]);

    assert_eq!(ids.worker_ids.len(), 3);
    assert!(ids.verifier_id.is_none());
    assert!(ids.synthesizer_id.is_none());
    assert!(ids.blackboard_event_id.is_none());

    // Root card: status=done + ended_at=Some + assignee=alice
    let root = s.get_task(&ids.root_id).unwrap();
    assert_eq!(root.status, "done", "root card status must be 'done'");
    assert!(
        root.ended_at.is_some(),
        "root card ended_at must be set (D-root-ended-at)"
    );
    assert_eq!(root.assignee, "alice");

    // Workers: status=todo
    for wid in &ids.worker_ids {
        let w = s.get_task(wid).unwrap();
        assert_eq!(w.status, "todo");
        assert!(w.ended_at.is_none());
    }

    // task_links: exactly 3 rows from root -> each worker.
    let link_count: i64 = s
        .conn
        .query_row(
            "SELECT count(*) FROM task_links WHERE parent_id = ?1",
            params![&ids.root_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(link_count, 3);

    // Created events: 1 root + 3 workers = 4 rows.
    let event_count: i64 = s
        .conn
        .query_row(
            "SELECT count(*) FROM task_events WHERE kind = 'created'",
            params![],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(event_count, 4);

    // No comments seeded.
    let comment_count: i64 = s
        .conn
        .query_row("SELECT count(*) FROM task_comments", params![], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(comment_count, 0);
}

#[tokio::test]
async fn swarm_with_verifier_gates_on_all_workers() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let mut s = store.lock().await;

    let spec = SwarmGraphSpec {
        goal: "verify goal".to_string(),
        workers: ["bob", "carol", "dave"]
            .iter()
            .map(|w| KanbanWorkerSpec {
                assignee: (*w).to_string(),
                title: None,
                body: None,
            })
            .collect(),
        verifier: Some("eve".to_string()),
        synthesizer: None,
        blackboard: None,
        workspace: None,
        skills: None,
        tenant: None,
        priority: None,
        max_runtime_seconds: None,
        max_retries: None,
        idempotency_key: None,
        created_by: Some("alice".to_string()),
        body: None,
    };

    let ids = s.create_swarm(spec).unwrap();
    let vid = ids.verifier_id.clone().expect("verifier_id must be Some");
    assert!(ids.synthesizer_id.is_none());

    let v = s.get_task(&vid).unwrap();
    assert_eq!(v.status, "todo");
    assert_eq!(v.assignee, "eve");

    // 3 worker->verifier links + 3 root->worker links = 6 task_links total.
    let total_links: i64 = s
        .conn
        .query_row("SELECT count(*) FROM task_links", params![], |r| r.get(0))
        .unwrap();
    assert_eq!(total_links, 6);

    let verifier_inbound: i64 = s
        .conn
        .query_row(
            "SELECT count(*) FROM task_links WHERE child_id = ?1",
            params![&vid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(verifier_inbound, 3, "verifier parents = all workers");
}

#[tokio::test]
async fn swarm_full_4_tier_matches_reference_md_664() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let mut s = store.lock().await;

    let spec = SwarmGraphSpec {
        goal: "Design a multi-region failover plan".to_string(),
        workers: ["researcher", "architect", "sre"]
            .iter()
            .map(|w| KanbanWorkerSpec {
                assignee: (*w).to_string(),
                title: None,
                body: None,
            })
            .collect(),
        verifier: Some("reviewer".to_string()),
        synthesizer: Some("writer".to_string()),
        blackboard: None,
        workspace: None,
        skills: None,
        tenant: None,
        priority: None,
        max_runtime_seconds: None,
        max_retries: None,
        idempotency_key: None,
        created_by: Some("alice".to_string()),
        body: None,
    };

    let ids = s.create_swarm(spec).unwrap();
    let vid = ids.verifier_id.clone().unwrap();
    let sid = ids.synthesizer_id.clone().unwrap();

    // 5 cards total: root + 3 workers + verifier + synthesizer.
    let task_count: i64 = s
        .conn
        .query_row("SELECT count(*) FROM tasks", params![], |r| r.get(0))
        .unwrap();
    assert_eq!(task_count, 6);

    // 3 root->worker + 3 worker->verifier + 1 verifier->synth = 7 task_links.
    let link_count: i64 = s
        .conn
        .query_row("SELECT count(*) FROM task_links", params![], |r| r.get(0))
        .unwrap();
    assert_eq!(link_count, 7);

    let synth_inbound: i64 = s
        .conn
        .query_row(
            "SELECT count(*) FROM task_links WHERE child_id = ?1",
            params![&sid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(synth_inbound, 1, "synth's only parent is verifier");

    let synth_parent: String = s
        .conn
        .query_row(
            "SELECT parent_id FROM task_links WHERE child_id = ?1",
            params![&sid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(synth_parent, vid);
}

#[tokio::test]
async fn swarm_with_synth_no_verifier_gates_synth_on_workers() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let mut s = store.lock().await;

    let spec = SwarmGraphSpec {
        goal: "quorum goal".to_string(),
        workers: ["bob", "carol", "dave"]
            .iter()
            .map(|w| KanbanWorkerSpec {
                assignee: (*w).to_string(),
                title: None,
                body: None,
            })
            .collect(),
        verifier: None,
        synthesizer: Some("eve".to_string()),
        blackboard: None,
        workspace: None,
        skills: None,
        tenant: None,
        priority: None,
        max_runtime_seconds: None,
        max_retries: None,
        idempotency_key: None,
        created_by: Some("alice".to_string()),
        body: None,
    };

    let ids = s.create_swarm(spec).unwrap();
    let sid = ids.synthesizer_id.clone().unwrap();
    assert!(ids.verifier_id.is_none());

    let synth_inbound: i64 = s
        .conn
        .query_row(
            "SELECT count(*) FROM task_links WHERE child_id = ?1",
            params![&sid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        synth_inbound, 3,
        "synth's parents are all workers (P3 quorum)"
    );
}

#[tokio::test]
async fn swarm_blackboard_arg_seeds_initial_comment_on_root() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let mut s = store.lock().await;

    let seeded = json!({
        "shared_context": "multi-region failover",
        "nested": { "a": 1, "b": [1, 2, 3] },
        "flag": true
    });

    let spec = SwarmGraphSpec {
        goal: "blackboard goal".to_string(),
        workers: vec![KanbanWorkerSpec {
            assignee: "bob".to_string(),
            title: None,
            body: None,
        }],
        verifier: None,
        synthesizer: None,
        blackboard: Some(seeded.clone()),
        workspace: None,
        skills: None,
        tenant: None,
        priority: None,
        max_runtime_seconds: None,
        max_retries: None,
        idempotency_key: None,
        created_by: Some("alice".to_string()),
        body: None,
    };

    let ids = s.create_swarm(spec).unwrap();
    assert!(ids.blackboard_event_id.is_some());

    // Exactly one task_comments row on the root with author='swarm'.
    let (author, body): (String, String) = s
        .conn
        .query_row(
            "SELECT author, body FROM task_comments WHERE task_id = ?1",
            params![&ids.root_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(author, "swarm");
    let round_trip: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(round_trip, seeded);
}

#[tokio::test]
async fn swarm_idempotency_key_replays_return_same_graph() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let mut s = store.lock().await;

    let build_spec = || SwarmGraphSpec {
        goal: "idempotent goal".to_string(),
        workers: ["bob", "carol"]
            .iter()
            .map(|w| KanbanWorkerSpec {
                assignee: (*w).to_string(),
                title: None,
                body: None,
            })
            .collect(),
        verifier: Some("eve".to_string()),
        synthesizer: None,
        blackboard: None,
        workspace: None,
        skills: None,
        tenant: None,
        priority: None,
        max_runtime_seconds: None,
        max_retries: None,
        idempotency_key: Some("k".to_string()),
        created_by: Some("alice".to_string()),
        body: None,
    };

    let first = s.create_swarm(build_spec()).unwrap();

    let task_count_before: i64 = s
        .conn
        .query_row("SELECT count(*) FROM tasks", params![], |r| r.get(0))
        .unwrap();
    let link_count_before: i64 = s
        .conn
        .query_row("SELECT count(*) FROM task_links", params![], |r| r.get(0))
        .unwrap();
    let event_count_before: i64 = s
        .conn
        .query_row("SELECT count(*) FROM task_events", params![], |r| r.get(0))
        .unwrap();

    let second = s.create_swarm(build_spec()).unwrap();

    assert_eq!(first.root_id, second.root_id);
    assert_eq!(first.worker_ids, second.worker_ids);
    assert_eq!(first.verifier_id, second.verifier_id);
    assert_eq!(first.synthesizer_id, second.synthesizer_id);

    // No new rows after replay.
    let task_count_after: i64 = s
        .conn
        .query_row("SELECT count(*) FROM tasks", params![], |r| r.get(0))
        .unwrap();
    let link_count_after: i64 = s
        .conn
        .query_row("SELECT count(*) FROM task_links", params![], |r| r.get(0))
        .unwrap();
    let event_count_after: i64 = s
        .conn
        .query_row("SELECT count(*) FROM task_events", params![], |r| r.get(0))
        .unwrap();
    assert_eq!(task_count_before, task_count_after);
    assert_eq!(link_count_before, link_count_after);
    assert_eq!(event_count_before, event_count_after);
}

#[tokio::test]
async fn swarm_invalid_assignee_rolls_back_whole_graph() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let mut s = store.lock().await;

    // Snapshot baselines (the empty DB still has 0 rows everywhere).
    let task_before: i64 = s
        .conn
        .query_row("SELECT count(*) FROM tasks", params![], |r| r.get(0))
        .unwrap();
    let link_before: i64 = s
        .conn
        .query_row("SELECT count(*) FROM task_links", params![], |r| r.get(0))
        .unwrap();
    let event_before: i64 = s
        .conn
        .query_row("SELECT count(*) FROM task_events", params![], |r| r.get(0))
        .unwrap();
    let comment_before: i64 = s
        .conn
        .query_row("SELECT count(*) FROM task_comments", params![], |r| {
            r.get(0)
        })
        .unwrap();

    let spec = SwarmGraphSpec {
        goal: "bad goal".to_string(),
        workers: vec![
            KanbanWorkerSpec {
                assignee: "bob".to_string(),
                title: None,
                body: None,
            },
            KanbanWorkerSpec {
                assignee: "bad@@user".to_string(),
                title: None,
                body: None,
            },
        ],
        verifier: None,
        synthesizer: None,
        blackboard: Some(json!({"x": 1})),
        workspace: None,
        skills: None,
        tenant: None,
        priority: None,
        max_runtime_seconds: None,
        max_retries: None,
        idempotency_key: None,
        created_by: Some("alice".to_string()),
        body: None,
    };

    let result = s.create_swarm(spec);
    assert!(
        result.is_err(),
        "invalid assignee must cause create_swarm to error"
    );

    let task_after: i64 = s
        .conn
        .query_row("SELECT count(*) FROM tasks", params![], |r| r.get(0))
        .unwrap();
    let link_after: i64 = s
        .conn
        .query_row("SELECT count(*) FROM task_links", params![], |r| r.get(0))
        .unwrap();
    let event_after: i64 = s
        .conn
        .query_row("SELECT count(*) FROM task_events", params![], |r| r.get(0))
        .unwrap();
    let comment_after: i64 = s
        .conn
        .query_row("SELECT count(*) FROM task_comments", params![], |r| {
            r.get(0)
        })
        .unwrap();

    assert_eq!(task_before, task_after, "no tasks rows on rollback");
    assert_eq!(link_before, link_after, "no task_links rows on rollback");
    assert_eq!(event_before, event_after, "no task_events rows on rollback");
    assert_eq!(
        comment_before, comment_after,
        "no task_comments rows on rollback"
    );
}

#[tokio::test]
async fn swarm_rich_worker_form_accepts_per_card_title_and_body() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let mut s = store.lock().await;

    let spec = SwarmGraphSpec {
        goal: "rich goal".to_string(),
        workers: vec![
            KanbanWorkerSpec {
                assignee: "bob".to_string(),
                title: Some("Research vendor A".to_string()),
                body: Some("Detailed body A".to_string()),
            },
            KanbanWorkerSpec {
                assignee: "carol".to_string(),
                title: Some("Research vendor B".to_string()),
                body: Some("Detailed body B".to_string()),
            },
        ],
        verifier: None,
        synthesizer: None,
        blackboard: None,
        workspace: None,
        skills: None,
        tenant: None,
        priority: None,
        max_runtime_seconds: None,
        max_retries: None,
        idempotency_key: None,
        created_by: Some("alice".to_string()),
        body: None,
    };

    let ids = s.create_swarm(spec).unwrap();
    let w0 = s.get_task(&ids.worker_ids[0]).unwrap();
    let w1 = s.get_task(&ids.worker_ids[1]).unwrap();

    assert_eq!(w0.title, "Research vendor A");
    assert_eq!(w0.body.as_deref(), Some("Detailed body A"));
    assert_eq!(w1.title, "Research vendor B");
    assert_eq!(w1.body.as_deref(), Some("Detailed body B"));
}

#[tokio::test]
async fn swarm_flat_worker_form_auto_titles_each_card() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let mut s = store.lock().await;

    let ids = seed_swarm_graph(&mut s, "flat-title goal", &["bob", "carol", "dave"]);

    let titles: Vec<String> = ids
        .worker_ids
        .iter()
        .map(|wid| s.get_task(wid).unwrap().title)
        .collect();

    assert_eq!(titles[0], "flat-title goal — worker 1 of 3");
    assert_eq!(titles[1], "flat-title goal — worker 2 of 3");
    assert_eq!(titles[2], "flat-title goal — worker 3 of 3");
}

#[tokio::test]
async fn swarm_root_card_has_ended_at_set() {
    // Risk 1 mitigation — explicit invariant lock for D-root-ended-at.
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let mut s = store.lock().await;

    let ids = seed_swarm_graph(&mut s, "ended_at goal", &["bob"]);
    let root = s.get_task(&ids.root_id).unwrap();
    assert!(
        root.ended_at.is_some(),
        "root card MUST have ended_at set (D-root-ended-at; Pitfall 1)"
    );
    assert!(
        root.ended_at.unwrap() >= root.created_at,
        "ended_at must be >= created_at"
    );
}

// ---------------------------------------------------------------------------
// kanban_mention tests (Phase 36.3.7.8 — @mention delegation parser)
// ---------------------------------------------------------------------------
//
// Receiver-end tests for the new KanbanStore::create_mention_children sibling
// primitive + the KanbanMentionTool LLM tool registered as the 11th tool.
// Most tests drive store.create_mention_children(plan) directly to assert
// DB-level shape (rows in tasks/task_links/task_events) and atomic rollback.

/// Seeds a parent task with the given body and assignee; returns (task_id, assignee).
fn seed_mention_parent(s: &mut KanbanStore, body: &str, assignee: &str) -> (String, String) {
    let opts = CreateTaskOptions {
        body: Some(body.to_string()),
        ..Default::default()
    };
    let t = s.create_task("parent task", assignee, opts).unwrap();
    (t.id, assignee.to_string())
}

/// Build a minimal MentionPlan with a single resolved child.
fn make_single_child_plan(
    parent_id: &str,
    handle: &str,
    assignee: &str,
    idempotency_key: Option<String>,
) -> MentionPlan {
    MentionPlan {
        parent_id: parent_id.to_string(),
        children: vec![MentionChildSpec {
            handle: handle.to_string(),
            assignee: assignee.to_string(),
            mention_index: 0,
            title: format!("@{handle} mention from {parent_id}"),
            body: None,
        }],
        idempotency_key,
        workspace: None,
        skills: None,
        tenant: None,
        created_by: Some("alice".to_string()),
    }
}

// ─── Test 1: happy path — single mention creates one child ───────────────────

#[tokio::test]
async fn mention_happy_path_single_mention_creates_one_child() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let mut s = store.lock().await;

    // Seed a known assignee so the resolver can find "reviewer".
    let (parent_id, _) = seed_mention_parent(&mut s, "Please @reviewer look at this", "alice");
    s.create_task("reviewer task", "reviewer", CreateTaskOptions::default())
        .unwrap();

    let plan = make_single_child_plan(&parent_id, "reviewer", "reviewer", None);
    let result = s.create_mention_children(plan).unwrap();

    assert_eq!(result.children.len(), 1, "one child created");
    let child_id = &result.children[0].child_id;
    assert_eq!(result.children[0].assignee, "reviewer");

    // DB-level: child row exists, link row exists, event row exists.
    let task_count: i64 = s
        .conn
        .query_row(
            "SELECT count(*) FROM tasks WHERE id = ?1",
            params![child_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(task_count, 1, "child task row must exist");

    let link_count: i64 = s
        .conn
        .query_row(
            "SELECT count(*) FROM task_links WHERE parent_id = ?1 AND child_id = ?2",
            params![&parent_id, child_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(link_count, 1, "parent→child link must exist");

    let event_count: i64 = s
        .conn
        .query_row(
            "SELECT count(*) FROM task_events WHERE task_id = ?1 AND kind = 'created'",
            params![child_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(event_count, 1, "created event must exist for child");
}

// ─── Test 2: multi-mention body creates N children ────────────────────────────

#[tokio::test]
async fn mention_multi_mention_body_creates_n_children() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let mut s = store.lock().await;

    let (parent_id, _) =
        seed_mention_parent(&mut s, "Assign @bob, @carol, and @dave to this", "alice");
    // Seed known assignees.
    for name in ["bob", "carol", "dave"] {
        s.create_task(&format!("{name} task"), name, CreateTaskOptions::default())
            .unwrap();
    }

    let plan = MentionPlan {
        parent_id: parent_id.clone(),
        children: vec![
            MentionChildSpec {
                handle: "bob".to_string(),
                assignee: "bob".to_string(),
                mention_index: 0,
                title: format!("@bob mention from {parent_id}"),
                body: None,
            },
            MentionChildSpec {
                handle: "carol".to_string(),
                assignee: "carol".to_string(),
                mention_index: 1,
                title: format!("@carol mention from {parent_id}"),
                body: None,
            },
            MentionChildSpec {
                handle: "dave".to_string(),
                assignee: "dave".to_string(),
                mention_index: 2,
                title: format!("@dave mention from {parent_id}"),
                body: None,
            },
        ],
        idempotency_key: None,
        workspace: None,
        skills: None,
        tenant: None,
        created_by: Some("alice".to_string()),
    };

    let result = s.create_mention_children(plan).unwrap();
    assert_eq!(result.children.len(), 3, "three children created");

    let link_count: i64 = s
        .conn
        .query_row(
            "SELECT count(*) FROM task_links WHERE parent_id = ?1",
            params![&parent_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(link_count, 3, "three links from parent");
}

// ─── Test 3: mention inside fenced code block is skipped by parser ────────────

#[tokio::test]
async fn mention_inside_fenced_block_skipped() {
    use ironhermes_kanban::mention::parse_mentions;

    let body = "Normal @reviewer text\n```\n@fenced_handle should be skipped\n```\nBack to @author";
    let spans = parse_mentions(body);

    // Only @reviewer and @author should be parsed; @fenced_handle must be absent.
    let handles: Vec<&str> = spans.iter().map(|s| s.handle.as_str()).collect();
    assert!(
        handles.contains(&"reviewer"),
        "prose @reviewer must be found"
    );
    assert!(handles.contains(&"author"), "prose @author must be found");
    assert!(
        !handles.contains(&"fenced_handle"),
        "@fenced_handle inside fence must be skipped: {handles:?}"
    );
}

// ─── Test 4: mention inside inline code is skipped by parser ─────────────────

#[tokio::test]
async fn mention_inside_inline_code_skipped() {
    use ironhermes_kanban::mention::parse_mentions;

    let body = "Call `@inlinecode` here but @realauthor is outside";
    let spans = parse_mentions(body);

    let handles: Vec<&str> = spans.iter().map(|s| s.handle.as_str()).collect();
    assert!(
        handles.contains(&"realauthor"),
        "prose @realauthor must be found: {handles:?}"
    );
    assert!(
        !handles.contains(&"inlinecode"),
        "@inlinecode inside backtick must be skipped: {handles:?}"
    );
}

// ─── Test 5: mention inside HTML comment is skipped by parser ────────────────

#[tokio::test]
async fn mention_inside_html_comment_skipped() {
    use ironhermes_kanban::mention::parse_mentions;

    let body = "<!-- @hiddenbot should be invisible -->\n@visibleauthor is real";
    let spans = parse_mentions(body);

    let handles: Vec<&str> = spans.iter().map(|s| s.handle.as_str()).collect();
    assert!(
        handles.contains(&"visibleauthor"),
        "prose @visibleauthor must be found: {handles:?}"
    );
    assert!(
        !handles.contains(&"hiddenbot"),
        "@hiddenbot inside HTML comment must be skipped: {handles:?}"
    );
}

// ─── Test 6: mixed prose and fence — only prose mentions extracted ────────────

#[tokio::test]
async fn mention_mixed_prose_and_fence_only_prose_extracted() {
    use ironhermes_kanban::mention::parse_mentions;

    let body = r#"@alice starts it.
```rust
let x = "@bob_inside_fence";
```
Then @carol finishes."#;

    let spans = parse_mentions(body);
    let handles: Vec<&str> = spans.iter().map(|s| s.handle.as_str()).collect();

    assert!(handles.contains(&"alice"), "prose @alice must be found");
    assert!(handles.contains(&"carol"), "prose @carol must be found");
    assert!(
        !handles.contains(&"bob_inside_fence"),
        "@bob_inside_fence must not appear: {handles:?}"
    );
    assert_eq!(handles.len(), 2, "exactly 2 prose mentions: {handles:?}");
}

// ─── Test 7: unknown handle — skip policy ────────────────────────────────────

#[tokio::test]
async fn mention_unknown_handle_skip_policy() {
    use ironhermes_kanban::mention::{FallbackPolicy, Resolution, ResolverCtx, resolve_mention};
    use std::collections::HashSet;

    // known_assignees is empty — "ghostbot" won't be found (valid name, not reserved).
    let known: HashSet<String> = HashSet::new();
    let ctx = ResolverCtx {
        parent_task_id: "t_parent".to_string(),
        parent_assignee: "alice".to_string(),
        known_assignees: &known,
        fallback_policy: FallbackPolicy::Skip,
    };

    let resolution = resolve_mention("ghostbot", &ctx);
    match resolution {
        Resolution::Skipped(_) => {} // correct
        other => panic!("Expected Skipped, got {other:?}"),
    }
}

// ─── Test 8: unknown handle — pending policy ─────────────────────────────────

#[tokio::test]
async fn mention_unknown_handle_pending_policy() {
    use ironhermes_kanban::mention::{FallbackPolicy, Resolution, ResolverCtx, resolve_mention};
    use std::collections::HashSet;

    let known: HashSet<String> = HashSet::new();
    let ctx = ResolverCtx {
        parent_task_id: "t_parent".to_string(),
        parent_assignee: "alice".to_string(),
        known_assignees: &known,
        fallback_policy: FallbackPolicy::Pending,
    };

    // "ghostbot" is a valid, non-reserved name not in known_assignees.
    let resolution = resolve_mention("ghostbot", &ctx);
    match resolution {
        Resolution::Fallback(assignee, _) => {
            assert_eq!(
                assignee, "pending",
                "pending policy must produce assignee='pending'"
            );
        }
        other => panic!("Expected Fallback(pending, ...), got {other:?}"),
    }
}

// ─── Test 9: unknown handle — error policy ────────────────────────────────────

#[tokio::test]
async fn mention_unknown_handle_error_policy() {
    use ironhermes_kanban::mention::{
        FallbackPolicy, Resolution, ResolverCtx, SkipReason, resolve_mention,
    };
    use std::collections::HashSet;

    let known: HashSet<String> = HashSet::new();
    let ctx = ResolverCtx {
        parent_task_id: "t_parent".to_string(),
        parent_assignee: "alice".to_string(),
        known_assignees: &known,
        fallback_policy: FallbackPolicy::Error,
    };

    // With Error policy, resolver still returns Skipped(UnknownHandle) —
    // the tool layer is responsible for converting this to a hard reject.
    // "ghostbot" is a valid, non-reserved name not in known_assignees.
    let resolution = resolve_mention("ghostbot", &ctx);
    match resolution {
        Resolution::Skipped(SkipReason::UnknownHandle) => {} // correct
        other => panic!("Expected Skipped(UnknownHandle), got {other:?}"),
    }
}

// ─── Test 10: self-reference is skipped (REQ-09) ─────────────────────────────

#[tokio::test]
async fn mention_self_reference_skipped() {
    use ironhermes_kanban::mention::{
        FallbackPolicy, Resolution, ResolverCtx, SkipReason, resolve_mention,
    };
    use std::collections::HashSet;

    let mut known: HashSet<String> = HashSet::new();
    known.insert("alice".to_string());

    // alice mentioning herself — should be Skipped(SelfReference).
    let ctx = ResolverCtx {
        parent_task_id: "t_parent".to_string(),
        parent_assignee: "alice".to_string(),
        known_assignees: &known,
        fallback_policy: FallbackPolicy::Skip,
    };

    let resolution = resolve_mention("alice", &ctx);
    match resolution {
        Resolution::Skipped(SkipReason::SelfReference) => {} // correct
        other => panic!("Expected Skipped(SelfReference), got {other:?}"),
    }
}

// ─── Test 11: ancestor-chain cycle skipped (REQ-10 layer 2) ──────────────────
//
// Build chain: t1(alice) → t2(bob) → t3(alice) → t4(bob).
// Then call create_mention_children on t4 (assignee=bob) with @alice —
// alice appears in the ancestor chain, so the store layer should reject.

#[tokio::test]
async fn mention_ancestor_cycle_skipped() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let mut s = store.lock().await;

    // Create 4 tasks building a chain a→b→a→b.
    let t1 = s
        .create_task("t1", "alice", CreateTaskOptions::default())
        .unwrap()
        .id;
    let t2 = s
        .create_task("t2", "bob", CreateTaskOptions::default())
        .unwrap()
        .id;
    let t3 = s
        .create_task("t3", "alice", CreateTaskOptions::default())
        .unwrap()
        .id;
    let t4 = s
        .create_task("t4", "bob", CreateTaskOptions::default())
        .unwrap()
        .id;

    // Wire the chain: t1→t2→t3→t4 (parent→child).
    s.insert_link(&t1, &t2).unwrap();
    s.insert_link(&t2, &t3).unwrap();
    s.insert_link(&t3, &t4).unwrap();

    // Now call create_mention_children on t4 (bob) mentioning @alice —
    // alice appears in the ancestor chain within MAX_MENTION_CHAIN_DEPTH hops.
    // The store must block this (layer-2 cycle detection).
    let _ = MAX_MENTION_CHAIN_DEPTH; // used to confirm the const is accessible

    // Snapshot pre-call counts so we can assert REQ-11 rollback below.
    let task_before: i64 = s
        .conn
        .query_row("SELECT count(*) FROM tasks", params![], |r| r.get(0))
        .unwrap();
    let link_before: i64 = s
        .conn
        .query_row("SELECT count(*) FROM task_links", params![], |r| r.get(0))
        .unwrap();
    let _ = t1;
    let _ = t2;
    let _ = t3;

    let plan = make_single_child_plan(&t4, "alice", "alice", None);
    let result = s.create_mention_children(plan);

    // WR-06 (Phase 36.3.7.8 code review): REQ-10 layer-2 cycle detection MUST
    // fire here. The chain t1(alice)→t2(bob)→t3(alice)→t4(bob) plus a
    // @alice mention under t4 means t4's ancestor t3 carries assignee "alice"
    // — the store-layer WITH RECURSIVE walk hits it inside
    // MAX_MENTION_CHAIN_DEPTH=4 hops and aborts the batch with
    // KanbanError::Other("mention_cycle: ...").
    let e = result.expect_err("layer-2 cycle detection must fire (alice in ancestor chain)");
    let msg = format!("{e}");
    assert!(
        msg.contains("mention_cycle"),
        "error must report mention_cycle, got: {msg}"
    );

    // REQ-11: no new rows on cycle abort.
    let task_after: i64 = s
        .conn
        .query_row("SELECT count(*) FROM tasks", params![], |r| r.get(0))
        .unwrap();
    let link_after: i64 = s
        .conn
        .query_row("SELECT count(*) FROM task_links", params![], |r| r.get(0))
        .unwrap();
    assert_eq!(task_before, task_after, "no tasks rows on cycle abort");
    assert_eq!(link_before, link_after, "no task_links rows on cycle abort");
}

// ─── Test 12: idempotency key replay returns same children (REQ-12) ──────────

#[tokio::test]
async fn mention_idempotency_key_replays_return_same_children() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let mut s = store.lock().await;

    let (parent_id, _) = seed_mention_parent(&mut s, "Hello @reviewer", "alice");
    s.create_task("reviewer task", "reviewer", CreateTaskOptions::default())
        .unwrap();

    let plan1 = make_single_child_plan(&parent_id, "reviewer", "reviewer", Some("k1".to_string()));
    let result1 = s.create_mention_children(plan1).unwrap();
    assert_eq!(result1.children.len(), 1);

    // Snapshot task count after first call.
    let tasks_count_after_1: i64 = s
        .conn
        .query_row("SELECT count(*) FROM tasks", params![], |r| r.get(0))
        .unwrap();

    // Second invocation with same idempotency_key.
    let plan2 = make_single_child_plan(&parent_id, "reviewer", "reviewer", Some("k1".to_string()));
    let result2 = s.create_mention_children(plan2).unwrap();

    // No new rows after replay.
    let tasks_count_after_2: i64 = s
        .conn
        .query_row("SELECT count(*) FROM tasks", params![], |r| r.get(0))
        .unwrap();
    assert_eq!(
        tasks_count_after_1, tasks_count_after_2,
        "idempotency replay must not insert new rows"
    );

    // Results must be structurally identical (same child_id, same assignee).
    assert_eq!(
        result1.children[0].child_id, result2.children[0].child_id,
        "replayed child_id must match first invocation"
    );
    assert_eq!(
        result1.children[0].assignee, result2.children[0].assignee,
        "replayed assignee must match first invocation"
    );

    // Landmine 2: per-child idempotency_key suffix is `:mention:0` (0-indexed).
    let child_id = &result1.children[0].child_id;
    let stored_key: Option<String> = s
        .conn
        .query_row(
            "SELECT idempotency_key FROM tasks WHERE id = ?1",
            params![child_id],
            |r| r.get(0),
        )
        .unwrap();
    if let Some(key) = stored_key {
        assert!(
            key.ends_with(":mention:0"),
            "child idempotency_key must end with ':mention:0', got: {key}"
        );
    }
}

// ─── Test 13: invalid task_id rejected at tool layer (REQ-13) ────────────────

#[tokio::test]
async fn mention_invalid_task_id_rejected() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();

    // No IRONHERMES_KANBAN_TASK set, no task_id arg — should reject.
    unsafe {
        std::env::remove_var("IRONHERMES_KANBAN_TASK");
    }

    let tool = KanbanMentionTool::new(store, true);
    let result = tool.execute(json!({})).await.unwrap();
    let v: Value = serde_json::from_str(&result).unwrap();

    assert_eq!(
        v["status"].as_str().unwrap(),
        "rejected",
        "missing task_id must produce rejected envelope"
    );
    assert_eq!(
        v["reason"].as_str().unwrap(),
        "invalid_task_id",
        "reason must be invalid_task_id"
    );
}

// ─── Test 14: task_id defaults to IRONHERMES_KANBAN_TASK env ─────────────────────

#[tokio::test]
async fn mention_task_id_defaults_to_hermes_kanban_task_env() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();

    // Seed a task so the tool can find it.
    let task_id = {
        let mut s = store.lock().await;
        seed_mention_parent(&mut s, "no mentions here", "alice").0
    };

    unsafe {
        std::env::set_var("IRONHERMES_KANBAN_TASK", &task_id);
    }

    let tool = KanbanMentionTool::new(store, false);
    // Execute with no args — should use IRONHERMES_KANBAN_TASK and return ok (no children).
    let result = tool.execute(json!({})).await.unwrap();

    unsafe {
        std::env::remove_var("IRONHERMES_KANBAN_TASK");
    }

    let v: Value = serde_json::from_str(&result).unwrap();
    // Body "no mentions here" has no @handles, so mentions_parsed=0, children_created=[].
    assert_eq!(
        v["task_id"].as_str().unwrap(),
        task_id,
        "task_id must match IRONHERMES_KANBAN_TASK"
    );
    assert_eq!(
        v["mentions_parsed"].as_i64().unwrap(),
        0,
        "no mentions in body"
    );
}

// ─── Test 15: case-insensitive handle resolution (REQ-04) ────────────────────

#[tokio::test]
async fn mention_case_insensitive_handle_resolution() {
    use ironhermes_kanban::mention::{FallbackPolicy, Resolution, ResolverCtx, resolve_mention};
    use std::collections::HashSet;

    let mut known: HashSet<String> = HashSet::new();
    // Known assignee stored lowercase.
    known.insert("reviewer".to_string());

    let ctx = ResolverCtx {
        parent_task_id: "t_parent".to_string(),
        parent_assignee: "alice".to_string(),
        known_assignees: &known,
        fallback_policy: FallbackPolicy::Skip,
    };

    // @Reviewer (capitalized) must resolve to "reviewer" (lowercase).
    let resolution = resolve_mention("Reviewer", &ctx);
    match resolution {
        Resolution::Resolved(assignee) => {
            assert_eq!(
                assignee, "reviewer",
                "resolved assignee must be lowercased: {assignee}"
            );
        }
        other => panic!("Expected Resolved(reviewer), got {other:?}"),
    }
}

// ─── Test 16 (bonus): invalid assignee rejected BEFORE tx (pre-flight) ────────
//
// WR-02 (Phase 36.3.7.8 code review): this test exercises the PRE-TRANSACTION
// validate_profile_name loop in create_mention_children, NOT the in-loop atomic
// rollback path. No INSERT is ever attempted because the validation failure
// returns before `transaction_with_behavior(Immediate)` opens. The "atomic
// rollback" assertion holds trivially.
//
// The genuine mid-batch rollback path is exercised by
// `mention_unique_collision_rolls_back_first_child` below.

#[tokio::test]
async fn mention_invalid_assignee_rejected_before_tx() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let mut s = store.lock().await;

    let (parent_id, _) = seed_mention_parent(&mut s, "Assign @good and @bad", "alice");

    // Snapshot pre-call row counts (only the parent and seeded assignee tasks exist).
    let task_before: i64 = s
        .conn
        .query_row("SELECT count(*) FROM tasks", params![], |r| r.get(0))
        .unwrap();
    let link_before: i64 = s
        .conn
        .query_row("SELECT count(*) FROM task_links", params![], |r| r.get(0))
        .unwrap();
    let event_before: i64 = s
        .conn
        .query_row("SELECT count(*) FROM task_events", params![], |r| r.get(0))
        .unwrap();

    // Two children: first has valid assignee, second has invalid UPPERCASE_FAIL.
    let plan = MentionPlan {
        parent_id: parent_id.clone(),
        children: vec![
            MentionChildSpec {
                handle: "good".to_string(),
                assignee: "good".to_string(),
                mention_index: 0,
                title: format!("@good mention from {parent_id}"),
                body: None,
            },
            MentionChildSpec {
                handle: "bad".to_string(),
                assignee: "UPPERCASE_FAIL".to_string(), // fails validate_profile_name
                mention_index: 1,
                title: format!("@bad mention from {parent_id}"),
                body: None,
            },
        ],
        idempotency_key: None,
        workspace: None,
        skills: None,
        tenant: None,
        created_by: Some("alice".to_string()),
    };

    let result = s.create_mention_children(plan);
    assert!(
        result.is_err(),
        "invalid assignee must cause create_mention_children to error"
    );

    // Pre-tx validation: no rows changed (vacuous because tx never opened).
    let task_after: i64 = s
        .conn
        .query_row("SELECT count(*) FROM tasks", params![], |r| r.get(0))
        .unwrap();
    let link_after: i64 = s
        .conn
        .query_row("SELECT count(*) FROM task_links", params![], |r| r.get(0))
        .unwrap();
    let event_after: i64 = s
        .conn
        .query_row("SELECT count(*) FROM task_events", params![], |r| r.get(0))
        .unwrap();

    assert_eq!(task_before, task_after, "no tasks rows after pre-tx reject");
    assert_eq!(
        link_before, link_after,
        "no task_links rows after pre-tx reject"
    );
    assert_eq!(
        event_before, event_after,
        "no task_events rows after pre-tx reject"
    );
}

// ─── Test 16b: mid-batch UNIQUE-constraint failure actually triggers rollback ─
//
// WR-02 (Phase 36.3.7.8 code review): this is the real test for REQ-11 atomic
// rollback. Two children share the same `mention_index` AND a non-None
// `idempotency_key`, so the per-child key suffix `:mention:{mention_index}` is
// identical for both. The first INSERT succeeds inside the tx; the second
// fails with a UNIQUE-constraint violation on `tasks.idempotency_key`. Dropping
// the rusqlite Transaction RAII-rolls back the first INSERT — so a regression
// that auto-commits per child would leave the first row behind and this test
// would catch it.

#[tokio::test]
async fn mention_unique_collision_rolls_back_first_child() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let mut s = store.lock().await;

    let (parent_id, _) = seed_mention_parent(&mut s, "Assign @a and @a", "alice");

    let task_before: i64 = s
        .conn
        .query_row("SELECT count(*) FROM tasks", params![], |r| r.get(0))
        .unwrap();
    let link_before: i64 = s
        .conn
        .query_row("SELECT count(*) FROM task_links", params![], |r| r.get(0))
        .unwrap();
    let event_before: i64 = s
        .conn
        .query_row("SELECT count(*) FROM task_events", params![], |r| r.get(0))
        .unwrap();

    // Both children use the same mention_index AND a non-None idempotency_key,
    // forcing identical per-child keys "midbatch_dup:mention:0" and UNIQUE-violation
    // on the second INSERT.
    let plan = MentionPlan {
        parent_id: parent_id.clone(),
        children: vec![
            MentionChildSpec {
                handle: "alpha".to_string(),
                assignee: "alpha".to_string(),
                mention_index: 0,
                title: format!("@alpha mention from {parent_id}"),
                body: None,
            },
            MentionChildSpec {
                handle: "beta".to_string(),
                assignee: "beta".to_string(),
                mention_index: 0, // identical to first → UNIQUE collision
                title: format!("@beta mention from {parent_id}"),
                body: None,
            },
        ],
        idempotency_key: Some("midbatch_dup".to_string()),
        workspace: None,
        skills: None,
        tenant: None,
        created_by: Some("alice".to_string()),
    };

    let result = s.create_mention_children(plan);
    assert!(
        result.is_err(),
        "duplicate per-child idempotency key must surface as an error"
    );

    // REQ-11: the first INSERT must be rolled back — no new rows in any of
    // tasks, task_links, or task_events.
    let task_after: i64 = s
        .conn
        .query_row("SELECT count(*) FROM tasks", params![], |r| r.get(0))
        .unwrap();
    let link_after: i64 = s
        .conn
        .query_row("SELECT count(*) FROM task_links", params![], |r| r.get(0))
        .unwrap();
    let event_after: i64 = s
        .conn
        .query_row("SELECT count(*) FROM task_events", params![], |r| r.get(0))
        .unwrap();

    assert_eq!(
        task_before, task_after,
        "first child INSERT must be rolled back on UNIQUE collision"
    );
    assert_eq!(
        link_before, link_after,
        "first child link must be rolled back on UNIQUE collision"
    );
    assert_eq!(
        event_before, event_after,
        "first child Created event must be rolled back on UNIQUE collision"
    );
}

// ─── Phase 36.3.7.12 Plan 02 Task 1: kanban_create goal_mode arg + schema ────

/// D-01 / D-03: `kanban_create` accepts `goal_mode=true` + `goal_max_turns=5`
/// JSON args, threads them through `CreateTaskOptions`, and the resulting row
/// shows the matching column values.
#[tokio::test]
async fn create_goal_mode_sets_columns() {
    let _env_guard = ENV_LOCK.lock().await;
    let store = make_store();
    let tool = KanbanCreateTool::new(store.clone(), true);

    let result = tool
        .execute(json!({
            "title": "Translate docs",
            "assignee": "alice",
            "body": "Acceptance: every page translated, no English left.",
            "goal_mode": true,
            "goal_max_turns": 5,
        }))
        .await
        .unwrap();

    let v: Value = serde_json::from_str(&result).unwrap();
    let task_id = v["task_id"].as_str().expect("task_id present").to_string();

    let s = store.lock().await;
    let task = s.get_task(&task_id).expect("get_task");
    assert!(task.goal_mode, "goal_mode must be true on the row");
    assert_eq!(
        task.goal_max_turns, 5,
        "goal_max_turns must equal the value passed (5)"
    );
    assert_eq!(
        task.goal_turns_used, 0,
        "goal_turns_used must start at 0 on a fresh card"
    );
}

/// D-01: `kanban_create` JSON schema advertises `goal_mode` (default false)
/// and `goal_max_turns` (default 20) under `properties`.
#[test]
fn schema_contains_goal_properties() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _env_guard = rt.block_on(ENV_LOCK.lock());
    let store = {
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };
        std::mem::forget(dir);
        Arc::new(TokioMutex::new(KanbanStore::open_default().unwrap()))
    };
    let tool = KanbanCreateTool::new(store, true);

    let schema_str = serde_json::to_string(&tool.schema()).unwrap();
    assert!(
        schema_str.contains("\"goal_mode\""),
        "schema missing goal_mode property: {schema_str}"
    );
    assert!(
        schema_str.contains("\"goal_max_turns\""),
        "schema missing goal_max_turns property: {schema_str}"
    );
    // Default value 20 for goal_max_turns must be advertised so the LLM
    // caller knows the budget when it omits the arg.
    assert!(
        schema_str.contains("\"default\":20"),
        "schema missing default:20 for goal_max_turns: {schema_str}"
    );
}

// ─── Test 17 (bonus): kanban_mention registered as 11th tool (REQ-06) ────────

#[tokio::test]
async fn mention_registered_eleventh_tool() {
    let _env_guard = ENV_LOCK.lock().await;
    use ironhermes_kanban::tools::register_kanban_tools;
    use ironhermes_tools::ToolRegistry;

    let store = make_store();
    let mut registry = ToolRegistry::new();
    register_kanban_tools(&mut registry, store, false);

    let names = registry.list_tools();

    assert!(
        names.contains(&"kanban_mention"),
        "kanban_mention must be registered: {names:?}"
    );
    assert_eq!(
        names.iter().filter(|n| n.starts_with("kanban_")).count(),
        13,
        "exactly 13 kanban_* tools must be registered (kanban_decompose + kanban_specify added in Phase 36.3.7.10): {names:?}"
    );
}

// ---------------------------------------------------------------------------
// Dual-read backward-compatibility regression test (260625-vr4)
// ---------------------------------------------------------------------------

/// INV-260625-vr4: legacy-only `HERMES_KANBAN_TASK` must still resolve via
/// the dual-read fallback in `kanban_env("TASK")`.
///
/// Simulates a worker spawned by an old producer that only set `HERMES_KANBAN_TASK`
/// without the IRONHERMES_KANBAN_TASK twin. The dual-read resolver must return
/// Some so the tool gate activates correctly.
#[tokio::test]
async fn legacy_hermes_kanban_task_only_still_gates_tools_on() {
    let _env_guard = ENV_LOCK.lock().await;
    unsafe {
        std::env::remove_var("IRONHERMES_KANBAN_TASK");
        std::env::set_var("HERMES_KANBAN_TASK", "t_legacy_test");
    }
    assert!(
        ironhermes_kanban::kanban_env("TASK").is_some(),
        "legacy HERMES_KANBAN_TASK should still resolve via dual-read fallback"
    );
    unsafe {
        std::env::remove_var("HERMES_KANBAN_TASK");
        std::env::remove_var("IRONHERMES_KANBAN_TASK");
    }
}

/// INV-260625-vr4: IRONHERMES_KANBAN_TASK must win over a simultaneously set
/// legacy HERMES_KANBAN_TASK (primary-wins priority).
#[tokio::test]
async fn primary_ironhermes_kanban_task_wins_over_legacy() {
    let _env_guard = ENV_LOCK.lock().await;
    unsafe {
        std::env::set_var("IRONHERMES_KANBAN_TASK", "t_primary");
        std::env::set_var("HERMES_KANBAN_TASK", "t_legacy");
    }
    let result = ironhermes_kanban::kanban_env("TASK");
    assert_eq!(
        result.as_deref(),
        Some("t_primary"),
        "IRONHERMES_KANBAN_TASK must shadow HERMES_KANBAN_TASK when both are set"
    );
    unsafe {
        std::env::remove_var("IRONHERMES_KANBAN_TASK");
        std::env::remove_var("HERMES_KANBAN_TASK");
    }
}

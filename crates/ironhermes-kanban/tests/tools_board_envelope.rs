//! Integration tests verifying D-08 (T-5 mitigation): every kanban LLM tool
//! injects `board` + `board_source` into EVERY response envelope — both success
//! and rejection paths (Plan 07, Phase 36.3.7.9).
//!
//! Each tool gets at least two tests:
//!   - success path: response JSON contains `"board"` and `"board_source"`
//!   - rejection path: response JSON contains `"board"` and `"board_source"`
//!
//! Tests redirect `IRONHERMES_HOME` to a per-test tempdir so `open_for_board`
//! never touches `~/.ironhermes`. The ENV_LOCK serialises all env-var mutations.

use std::sync::Arc;

use ironhermes_kanban::store::{CreateTaskOptions, KanbanStore};
use ironhermes_kanban::tools::{
    KanbanBlockTool, KanbanCommentTool, KanbanCompleteTool, KanbanCreateTool, KanbanHeartbeatTool,
    KanbanLinkTool, KanbanListTool, KanbanMentionTool, KanbanShowTool, KanbanSwarmTool,
    KanbanUnblockTool,
};
use ironhermes_tools::Tool;
use rusqlite::params;
use serde_json::{Value, json};
use tokio::sync::Mutex as TokioMutex;

// Serialize all tests that touch env vars.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Redirect IRONHERMES_HOME to a fresh tempdir. Returns the tempdir so it
/// stays alive for the duration of the test. The guard must be dropped AFTER
/// the tempdir to avoid UAF of the path string. Caller holds ENV_LOCK.
fn setup_home() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };
    dir
}

/// Make a dummy Arc<TokioMutex<KanbanStore>> (not used by tools in per-board
/// mode but required for construction).
fn make_dummy_store() -> Arc<TokioMutex<KanbanStore>> {
    let dir = tempfile::tempdir().unwrap();
    let store = KanbanStore::new(dir.path().join("dummy.db")).unwrap();
    std::mem::forget(dir);
    Arc::new(TokioMutex::new(store))
}

/// Parse JSON response and assert both board fields are present.
fn assert_board_fields(json_str: &str) {
    let v: Value = serde_json::from_str(json_str)
        .unwrap_or_else(|e| panic!("response is not valid JSON: {e}\nraw: {json_str}"));
    assert!(
        v.get("board").is_some(),
        "missing 'board' field in envelope: {json_str}"
    );
    assert!(
        v.get("board_source").is_some(),
        "missing 'board_source' field in envelope: {json_str}"
    );
}

/// Create a running task in the "default" board (IRONHERMES_HOME must already
/// be set). Returns (task_id, run_id).
fn seed_running_task_in_default(title: &str) -> (String, String) {
    let mut store = KanbanStore::open_default().unwrap();
    let opts = CreateTaskOptions::default();
    let task = store.create_task(title, "agent", opts).unwrap();
    let task_id = task.id.clone();
    let run_id = format!("run_{}", &task_id[..8]);
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
            "UPDATE tasks SET status = 'running', current_run_id = ?1 WHERE id = ?2",
            params![run_id, task_id],
        )
        .unwrap();
    (task_id, run_id)
}

// ===========================================================================
// kanban_show
// ===========================================================================

#[tokio::test]
async fn show_success_has_board_fields() {
    let _guard = ENV_LOCK.lock().await;
    let _home = setup_home();
    let (task_id, _run_id) = seed_running_task_in_default("show-success");
    unsafe { std::env::set_var("HERMES_KANBAN_TASK", &task_id) };
    let tool = KanbanShowTool::new(make_dummy_store(), true);
    let result = tool.execute(json!({})).await.unwrap();
    unsafe { std::env::remove_var("HERMES_KANBAN_TASK") };
    assert_board_fields(&result);
}

#[tokio::test]
async fn show_rejection_missing_task_id_has_board_fields() {
    let _guard = ENV_LOCK.lock().await;
    let _home = setup_home();
    // No task_id arg and HERMES_KANBAN_TASK not set → structured missing_task_id rejection.
    unsafe { std::env::remove_var("HERMES_KANBAN_TASK") };
    let tool = KanbanShowTool::new(make_dummy_store(), true);
    let result = tool.execute(json!({})).await.unwrap();
    assert_board_fields(&result);
    assert!(
        result.contains("missing_task_id") || result.contains("rejected"),
        "unexpected: {result}"
    );
}

// ===========================================================================
// kanban_list
// ===========================================================================

#[tokio::test]
async fn list_success_has_board_fields() {
    let _guard = ENV_LOCK.lock().await;
    let _home = setup_home();
    // Empty board — list still returns success envelope.
    let tool = KanbanListTool::new(make_dummy_store(), true);
    let result = tool.execute(json!({})).await.unwrap();
    assert_board_fields(&result);
}

#[tokio::test]
async fn list_explicit_board_slug_in_envelope() {
    let _guard = ENV_LOCK.lock().await;
    let _home = setup_home();
    let tool = KanbanListTool::new(make_dummy_store(), true);
    let result = tool.execute(json!({"board": "default"})).await.unwrap();
    let v: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["board"], "default", "unexpected: {result}");
    assert_eq!(v["board_source"], "flag", "unexpected: {result}");
}

// ===========================================================================
// kanban_comment
// ===========================================================================

#[tokio::test]
async fn comment_success_has_board_fields() {
    let _guard = ENV_LOCK.lock().await;
    let _home = setup_home();
    let (task_id, _run_id) = seed_running_task_in_default("comment-success");
    unsafe { std::env::set_var("HERMES_KANBAN_TASK", &task_id) };
    let tool = KanbanCommentTool::new(make_dummy_store(), true);
    let result = tool
        .execute(json!({"task_id": task_id, "body": "progress note"}))
        .await
        .unwrap();
    unsafe { std::env::remove_var("HERMES_KANBAN_TASK") };
    assert_board_fields(&result);
}

#[tokio::test]
async fn comment_rejection_missing_task_id_has_board_fields() {
    let _guard = ENV_LOCK.lock().await;
    let _home = setup_home();
    // No task_id arg and HERMES_KANBAN_TASK not set → missing_task_id rejection.
    unsafe { std::env::remove_var("HERMES_KANBAN_TASK") };
    let tool = KanbanCommentTool::new(make_dummy_store(), true);
    let result = tool.execute(json!({"body": "note"})).await.unwrap();
    assert_board_fields(&result);
    assert!(
        result.contains("rejected") || result.contains("missing_task_id"),
        "unexpected: {result}"
    );
}

// ===========================================================================
// kanban_complete
// ===========================================================================

#[tokio::test]
async fn complete_success_has_board_fields() {
    let _guard = ENV_LOCK.lock().await;
    let _home = setup_home();
    let (task_id, run_id) = seed_running_task_in_default("complete-success");
    unsafe { std::env::set_var("HERMES_KANBAN_TASK", &task_id) };
    unsafe { std::env::set_var("HERMES_KANBAN_RUN_ID", &run_id) };
    let tool = KanbanCompleteTool::new(make_dummy_store(), true);
    let result = tool
        .execute(json!({"task_id": task_id, "expected_run_id": run_id, "summary": "done"}))
        .await
        .unwrap();
    unsafe { std::env::remove_var("HERMES_KANBAN_TASK") };
    unsafe { std::env::remove_var("HERMES_KANBAN_RUN_ID") };
    assert_board_fields(&result);
}

#[tokio::test]
async fn complete_rejection_stale_run_id_has_board_fields() {
    let _guard = ENV_LOCK.lock().await;
    let _home = setup_home();
    let (task_id, _run_id) = seed_running_task_in_default("complete-stale");
    unsafe { std::env::set_var("HERMES_KANBAN_TASK", &task_id) };
    let tool = KanbanCompleteTool::new(make_dummy_store(), true);
    // Use expected_run_id (the correct arg name, per D-22).
    let result = tool
        .execute(json!({"task_id": task_id, "expected_run_id": "stale_run_xyz", "summary": "done"}))
        .await
        .unwrap();
    unsafe { std::env::remove_var("HERMES_KANBAN_TASK") };
    assert_board_fields(&result);
    assert!(
        result.contains("stale_run_id") || result.contains("rejected"),
        "unexpected: {result}"
    );
}

// ===========================================================================
// kanban_block
// ===========================================================================

#[tokio::test]
async fn block_success_has_board_fields() {
    let _guard = ENV_LOCK.lock().await;
    let _home = setup_home();
    let (task_id, run_id) = seed_running_task_in_default("block-success");
    unsafe { std::env::set_var("HERMES_KANBAN_TASK", &task_id) };
    unsafe { std::env::set_var("HERMES_KANBAN_RUN_ID", &run_id) };
    let tool = KanbanBlockTool::new(make_dummy_store(), true);
    let result = tool
        .execute(
            json!({"task_id": task_id, "expected_run_id": run_id, "reason": "waiting for dep"}),
        )
        .await
        .unwrap();
    unsafe { std::env::remove_var("HERMES_KANBAN_TASK") };
    unsafe { std::env::remove_var("HERMES_KANBAN_RUN_ID") };
    assert_board_fields(&result);
}

#[tokio::test]
async fn block_rejection_stale_run_id_has_board_fields() {
    let _guard = ENV_LOCK.lock().await;
    let _home = setup_home();
    let (task_id, _run_id) = seed_running_task_in_default("block-stale");
    unsafe { std::env::set_var("HERMES_KANBAN_TASK", &task_id) };
    let tool = KanbanBlockTool::new(make_dummy_store(), true);
    // Use expected_run_id (the correct arg name, per D-22/D-23).
    let result = tool
        .execute(json!({"task_id": task_id, "expected_run_id": "stale_run_xyz", "reason": "dep"}))
        .await
        .unwrap();
    unsafe { std::env::remove_var("HERMES_KANBAN_TASK") };
    assert_board_fields(&result);
    assert!(
        result.contains("stale_run_id") || result.contains("rejected"),
        "unexpected: {result}"
    );
}

// ===========================================================================
// kanban_create
// ===========================================================================

#[tokio::test]
async fn create_success_has_board_fields() {
    let _guard = ENV_LOCK.lock().await;
    let _home = setup_home();
    let tool = KanbanCreateTool::new(make_dummy_store(), true);
    let result = tool
        .execute(json!({"title": "new task", "assignee": "agent"}))
        .await
        .unwrap();
    assert_board_fields(&result);
    assert!(result.contains("task_id"), "unexpected: {result}");
}

#[tokio::test]
async fn create_rejection_missing_title_has_board_fields() {
    let _guard = ENV_LOCK.lock().await;
    let _home = setup_home();
    let tool = KanbanCreateTool::new(make_dummy_store(), true);
    // Missing title → structured missing_required_arg rejection (after Rule-1 fix).
    let result = tool.execute(json!({"assignee": "agent"})).await.unwrap();
    assert_board_fields(&result);
    assert!(
        result.contains("rejected") || result.contains("missing_required_arg"),
        "unexpected: {result}"
    );
}

// ===========================================================================
// kanban_heartbeat
// ===========================================================================

#[tokio::test]
async fn heartbeat_success_has_board_fields() {
    let _guard = ENV_LOCK.lock().await;
    let _home = setup_home();
    let (task_id, run_id) = seed_running_task_in_default("heartbeat-success");
    unsafe { std::env::set_var("HERMES_KANBAN_TASK", &task_id) };
    unsafe { std::env::set_var("HERMES_KANBAN_RUN_ID", &run_id) };
    let tool = KanbanHeartbeatTool::new(make_dummy_store(), true);
    let result = tool
        .execute(json!({"task_id": task_id, "note": "still running"}))
        .await
        .unwrap();
    unsafe { std::env::remove_var("HERMES_KANBAN_TASK") };
    unsafe { std::env::remove_var("HERMES_KANBAN_RUN_ID") };
    assert_board_fields(&result);
    assert!(result.contains("event_id"), "unexpected: {result}");
}

#[tokio::test]
async fn heartbeat_rejection_missing_task_id_has_board_fields() {
    let _guard = ENV_LOCK.lock().await;
    let _home = setup_home();
    // No task_id arg and HERMES_KANBAN_TASK not set → missing_task_id rejection.
    unsafe { std::env::remove_var("HERMES_KANBAN_TASK") };
    let tool = KanbanHeartbeatTool::new(make_dummy_store(), true);
    let result = tool.execute(json!({})).await.unwrap();
    assert_board_fields(&result);
    assert!(
        result.contains("missing_task_id") || result.contains("rejected"),
        "unexpected: {result}"
    );
}

// ===========================================================================
// kanban_link
// ===========================================================================

#[tokio::test]
async fn link_success_has_board_fields() {
    let _guard = ENV_LOCK.lock().await;
    let _home = setup_home();
    let mut store = KanbanStore::open_default().unwrap();
    let parent = store
        .create_task("parent", "agent", CreateTaskOptions::default())
        .unwrap();
    let child = store
        .create_task("child", "agent", CreateTaskOptions::default())
        .unwrap();
    let tool = KanbanLinkTool::new(make_dummy_store(), true);
    let result = tool
        .execute(json!({"parent_id": parent.id, "child_id": child.id}))
        .await
        .unwrap();
    assert_board_fields(&result);
    assert!(result.contains("\"ok\""), "unexpected: {result}");
}

#[tokio::test]
async fn link_rejection_cycle_has_board_fields() {
    let _guard = ENV_LOCK.lock().await;
    let _home = setup_home();
    let mut store = KanbanStore::open_default().unwrap();
    let t1 = store
        .create_task("t1", "agent", CreateTaskOptions::default())
        .unwrap();
    let t2 = store
        .create_task("t2", "agent", CreateTaskOptions::default())
        .unwrap();
    // Create t1→t2 link first.
    store.insert_link_checked(&t1.id, &t2.id).unwrap();
    // Now try t2→t1 (cycle).
    let tool = KanbanLinkTool::new(make_dummy_store(), true);
    let result = tool
        .execute(json!({"parent_id": t2.id, "child_id": t1.id}))
        .await
        .unwrap();
    assert_board_fields(&result);
    assert!(
        result.contains("link_cycle") || result.contains("rejected"),
        "unexpected: {result}"
    );
}

// ===========================================================================
// kanban_unblock
// ===========================================================================

#[tokio::test]
async fn unblock_success_has_board_fields() {
    let _guard = ENV_LOCK.lock().await;
    let _home = setup_home();
    let mut store = KanbanStore::open_default().unwrap();
    let opts = CreateTaskOptions::default();
    let task = store.create_task("unblock-me", "agent", opts).unwrap();
    // Put task in blocked state.
    store
        .conn
        .execute(
            "UPDATE tasks SET status = 'blocked' WHERE id = ?1",
            params![task.id],
        )
        .unwrap();
    let tool = KanbanUnblockTool::new(make_dummy_store(), true);
    let result = tool.execute(json!({"task_id": task.id})).await.unwrap();
    assert_board_fields(&result);
    assert!(result.contains("\"ok\""), "unexpected: {result}");
}

#[tokio::test]
async fn unblock_rejection_wrong_status_has_board_fields() {
    let _guard = ENV_LOCK.lock().await;
    let _home = setup_home();
    let mut store = KanbanStore::open_default().unwrap();
    let opts = CreateTaskOptions::default();
    let task = store.create_task("not-blocked", "agent", opts).unwrap();
    // Task is in 'ready' status (default), not 'blocked'.
    let tool = KanbanUnblockTool::new(make_dummy_store(), true);
    let result = tool.execute(json!({"task_id": task.id})).await.unwrap();
    assert_board_fields(&result);
    assert!(
        result.contains("invalid_status") || result.contains("rejected"),
        "unexpected: {result}"
    );
}

// ===========================================================================
// kanban_swarm
// ===========================================================================

#[tokio::test]
async fn swarm_success_has_board_fields() {
    let _guard = ENV_LOCK.lock().await;
    let _home = setup_home();
    // swarm uses HERMES_PROFILE to assign the root card when assignee is omitted.
    unsafe { std::env::set_var("HERMES_PROFILE", "orchestrator") };
    let tool = KanbanSwarmTool::new(make_dummy_store(), true);
    let result = tool
        .execute(json!({
            "goal": "implement the feature",
            "assignee": "orchestrator",
            "workers": [
                {"title": "worker A", "assignee": "agent-a"},
                {"title": "worker B", "assignee": "agent-b"}
            ]
        }))
        .await
        .unwrap();
    unsafe { std::env::remove_var("HERMES_PROFILE") };
    assert_board_fields(&result);
    assert!(
        result.contains("root_id") || result.contains("\"ok\""),
        "unexpected: {result}"
    );
}

#[tokio::test]
async fn swarm_rejection_missing_workers_has_board_fields() {
    let _guard = ENV_LOCK.lock().await;
    let _home = setup_home();
    let tool = KanbanSwarmTool::new(make_dummy_store(), true);
    // Missing required 'workers' field.
    let result = tool
        .execute(json!({"title": "swarm root", "assignee": "orchestrator"}))
        .await
        .unwrap();
    assert_board_fields(&result);
    assert!(
        result.contains("rejected") || result.contains("missing"),
        "unexpected: {result}"
    );
}

// ===========================================================================
// kanban_mention
// ===========================================================================

#[tokio::test]
async fn mention_success_has_board_fields() {
    let _guard = ENV_LOCK.lock().await;
    let _home = setup_home();
    // Ensure HERMES_KANBAN_TASK is not set from a previous test in the process.
    unsafe { std::env::remove_var("HERMES_KANBAN_TASK") };
    // Seed a parent task in the default board so mention can attach children.
    let mut store = KanbanStore::open_default().unwrap();
    let parent = store
        .create_task(
            "parent for mention",
            "orchestrator",
            CreateTaskOptions::default(),
        )
        .unwrap();
    drop(store);
    let tool = KanbanMentionTool::new(make_dummy_store(), true);
    let result = tool
        .execute(json!({
            "task_id": parent.id,
            "title": "fanout task",
            "body": "Hello @agent-alpha and @agent-beta please handle this."
        }))
        .await
        .unwrap();
    assert_board_fields(&result);
    // Should succeed with task IDs for each mention.
    assert!(
        result.contains("\"ok\"")
            || result.contains("task_id")
            || result.contains("tasks")
            || result.contains("children"),
        "unexpected: {result}"
    );
}

#[tokio::test]
async fn mention_rejection_no_mentions_has_board_fields() {
    let _guard = ENV_LOCK.lock().await;
    let _home = setup_home();
    let tool = KanbanMentionTool::new(make_dummy_store(), true);
    // No @mentions in body — should reject.
    let result = tool
        .execute(json!({
            "title": "task with no mentions",
            "body": "This body has no at-mentions at all."
        }))
        .await
        .unwrap();
    assert_board_fields(&result);
    assert!(
        result.contains("rejected") || result.contains("no_mentions") || result.contains("missing"),
        "unexpected: {result}"
    );
}

// ===========================================================================
// Board flag propagation: explicit board slug flows into envelope
// ===========================================================================

#[tokio::test]
async fn create_with_explicit_board_flag_shows_flag_source() {
    let _guard = ENV_LOCK.lock().await;
    let _home = setup_home();
    let tool = KanbanCreateTool::new(make_dummy_store(), true);
    let result = tool
        .execute(json!({"title": "board-flagged task", "assignee": "agent", "board": "default"}))
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["board"], "default", "unexpected board: {result}");
    assert_eq!(v["board_source"], "flag", "expected source=flag: {result}");
}

#[tokio::test]
async fn list_with_explicit_board_env_shows_env_source() {
    let _guard = ENV_LOCK.lock().await;
    let _home = setup_home();
    unsafe { std::env::set_var("HERMES_KANBAN_BOARD", "default") };
    let tool = KanbanListTool::new(make_dummy_store(), true);
    // No board arg — should pick up env.
    let result = tool.execute(json!({})).await.unwrap();
    unsafe { std::env::remove_var("HERMES_KANBAN_BOARD") };
    let v: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["board"], "default", "unexpected: {result}");
    assert_eq!(v["board_source"], "env", "expected source=env: {result}");
}

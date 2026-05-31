//! Integration tests for the decomposer kernel (Phase 36.3.7.10).
//!
//! Tests drive `specify_triage_task` and `decompose_triage_task` directly
//! with a mock [`DecomposeFn`] that returns a fixture [`DecomposeOutput`].
//! No real LLM, no real network — the closure-injection pattern makes the
//! decomposer logic fully testable without touching provider-client code.
//!
//! Run: cargo test -p ironhermes-kanban --test decomposer_logic -- --test-threads=1
//!
//! Test map:
//! - DEC-01: `specify_promotes_triage_to_todo` — happy path: specify rewrites body + promotes.
//! - DEC-02: `specify_noop_if_not_triage` — specify is a no-op when status != triage.
//! - DEC-03: `decompose_creates_children_atomically` — decompose fans out children + links.
//! - DEC-04: `decompose_double_is_noop` — double-decompose is idempotent (status guard).
//! - DEC-05: `decompose_failure_retains_triage` — LLM error retains triage + appends event.

use std::sync::Arc;

use tempfile::TempDir;
use tokio::sync::Mutex as TokioMutex;

use ironhermes_kanban::{KanbanConfig, KanbanStore};
use ironhermes_kanban::decomposer::{
    ChildSpec, DecomposeOutput, DecomposeFn, decompose_triage_task, specify_triage_task,
};
use ironhermes_kanban::store::CreateTaskOptions;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a mock [`DecomposeFn`] that always succeeds with the given output.
fn mock_decompose_fn(output: DecomposeOutput) -> DecomposeFn {
    Arc::new(move |_req| {
        let out = output.clone();
        Box::pin(async move { Ok(out) })
    })
}

/// Build a mock [`DecomposeFn`] that always fails with the given message.
fn mock_decompose_fn_err(msg: &'static str) -> DecomposeFn {
    Arc::new(move |_req| {
        Box::pin(async move {
            Err(ironhermes_kanban::error::KanbanError::Other(
                anyhow::anyhow!(msg),
            ))
        })
    })
}

/// Create a tempfile-backed [`KanbanStore`] wrapped in `Arc<TokioMutex<_>>`.
///
/// Uses `std::mem::forget` to keep the tempdir alive for the test duration
/// (matches dispatcher_logic.rs pattern).
fn make_store_arc() -> Arc<TokioMutex<KanbanStore>> {
    let dir = TempDir::new().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let store = KanbanStore::new(&db_path).expect("open store");
    std::mem::forget(dir);
    Arc::new(TokioMutex::new(store))
}

/// Seed a task in `triage` status. Returns the new task id.
async fn seed_triage_task(
    store: &Arc<TokioMutex<KanbanStore>>,
    title: &str,
    assignee: &str,
) -> String {
    let mut s = store.lock().await;
    s.create_task(
        title,
        assignee,
        CreateTaskOptions {
            triage: true,
            ..Default::default()
        },
    )
    .expect("create triage task")
    .id
}

/// Seed a task in default `ready` status. Returns the new task id.
async fn seed_ready_task(
    store: &Arc<TokioMutex<KanbanStore>>,
    title: &str,
    assignee: &str,
) -> String {
    let mut s = store.lock().await;
    s.create_task(title, assignee, CreateTaskOptions::default())
        .expect("create ready task")
        .id
}

// ---------------------------------------------------------------------------
// DEC-01: specify_promotes_triage_to_todo
// ---------------------------------------------------------------------------

/// Happy path: calling `specify_triage_task` on a triage card rewrites body +
/// promotes status to `todo` and appends a `specified` event.
#[tokio::test]
async fn specify_promotes_triage_to_todo() {
    let store = make_store_arc();
    let config = KanbanConfig::default();
    let task_id = seed_triage_task(&store, "add Stripe payments", "alice").await;

    let output = DecomposeOutput {
        new_title: "Add Stripe payment integration".to_string(),
        new_body: "## Goal\nIntegrate Stripe for subscription billing.".to_string(),
        children: vec![],
    };
    let decompose_fn = mock_decompose_fn(output.clone());

    let result = specify_triage_task(store.clone(), &task_id, &decompose_fn, &config)
        .await
        .expect("specify_triage_task should succeed");

    assert!(result.root_promoted, "root_promoted must be true on success");
    assert!(result.child_ids.is_empty(), "specify does not create children");

    // Verify the task was promoted.
    let s = store.lock().await;
    let task = s.get_task(&task_id).expect("task must exist");
    assert_eq!(task.status, "todo", "status must be promoted to todo");
    assert_eq!(task.title, output.new_title, "title must be rewritten");
    assert_eq!(
        task.body.as_deref(),
        Some(output.new_body.as_str()),
        "body must be rewritten"
    );

    // Verify a `specified` event was appended.
    let events = s.get_events(&task_id).expect("get_events");
    let has_specified = events.iter().any(|e| e.kind == "specified");
    assert!(has_specified, "a 'specified' event must be appended");
}

// ---------------------------------------------------------------------------
// DEC-02: specify_noop_if_not_triage
// ---------------------------------------------------------------------------

/// If the task status is not `triage`, `specify_triage_task` must return
/// `root_promoted = false` without modifying the task or appending an event.
#[tokio::test]
async fn specify_noop_if_not_triage() {
    let store = make_store_arc();
    let config = KanbanConfig::default();
    // Seed a `ready` task (default status from create_task).
    let task_id = seed_ready_task(&store, "already ready task", "alice").await;

    let output = DecomposeOutput {
        new_title: "Should not appear".to_string(),
        new_body: "Should not appear".to_string(),
        children: vec![],
    };
    let decompose_fn = mock_decompose_fn(output);

    // The mock fn should never be called because the early-exit fires.
    let result = specify_triage_task(store.clone(), &task_id, &decompose_fn, &config)
        .await
        .expect("specify_triage_task should return Ok even for non-triage");

    assert!(
        !result.root_promoted,
        "root_promoted must be false when task is not in triage"
    );

    // Verify the task is unchanged.
    let s = store.lock().await;
    let task = s.get_task(&task_id).expect("task must exist");
    assert_ne!(task.status, "todo", "non-triage task must not be promoted");
    assert_eq!(task.title, "already ready task", "title must be unchanged");

    // Verify no new event row was appended (fresh task has only its Created event).
    let events = s.get_events(&task_id).expect("get_events");
    let has_specified = events.iter().any(|e| e.kind == "specified");
    assert!(
        !has_specified,
        "no 'specified' event must be appended for non-triage task"
    );
}

// ---------------------------------------------------------------------------
// DEC-03: decompose_creates_children_atomically
// ---------------------------------------------------------------------------

/// Happy path: `decompose_triage_task` promotes the parent, creates 3 child
/// tasks with status `todo`, creates 3 `task_links` rows, and appends a
/// `decomposed` event with `child_count = 3`.
#[tokio::test]
async fn decompose_creates_children_atomically() {
    let store = make_store_arc();
    let config = KanbanConfig::default();
    let task_id = seed_triage_task(&store, "add payments", "alice").await;

    let output = DecomposeOutput {
        new_title: "Add payment system".to_string(),
        new_body: "## Goal\nShip payments.".to_string(),
        children: vec![
            ChildSpec {
                title: "Research gateway options".to_string(),
                assignee: "researcher".to_string(),
                body: None,
            },
            ChildSpec {
                title: "Implement Stripe integration".to_string(),
                assignee: "developer".to_string(),
                body: Some("Stripe SDK integration.".to_string()),
            },
            ChildSpec {
                title: "Write integration tests".to_string(),
                assignee: "developer".to_string(),
                body: None,
            },
        ],
    };
    let decompose_fn = mock_decompose_fn(output.clone());

    let result = decompose_triage_task(store.clone(), &task_id, &decompose_fn, &config)
        .await
        .expect("decompose_triage_task should succeed");

    assert!(result.root_promoted, "root_promoted must be true");
    assert_eq!(result.child_ids.len(), 3, "must have 3 child ids");

    let s = store.lock().await;

    // Parent must be promoted.
    let parent = s.get_task(&task_id).expect("parent task must exist");
    assert_eq!(parent.status, "todo", "parent status must be todo");
    assert_eq!(parent.title, output.new_title);

    // Each child must exist with status `todo` and a non-empty assignee.
    for child_id in &result.child_ids {
        let child = s.get_task(child_id).expect("child task must exist");
        assert_eq!(child.status, "todo", "child status must be todo");
        assert!(!child.assignee.is_empty(), "child assignee must not be empty");
    }

    // 3 task_links rows must exist.
    let links = s
        .list_links_for_parent(&task_id)
        .expect("list_links_for_parent");
    assert_eq!(links.len(), 3, "must have 3 task_links rows");
    for link in &links {
        assert_eq!(link.parent_id, task_id);
        assert!(
            result.child_ids.contains(&link.child_id),
            "link child_id must be in result.child_ids"
        );
    }

    // One `decomposed` event with child_count = 3.
    let events = s.get_events(&task_id).expect("get_events");
    let decomposed_events: Vec<_> = events.iter().filter(|e| e.kind == "decomposed").collect();
    assert_eq!(decomposed_events.len(), 1, "exactly one 'decomposed' event");
    let payload_str = decomposed_events[0]
        .payload
        .as_deref()
        .expect("decomposed event must have payload");
    let payload: serde_json::Value =
        serde_json::from_str(payload_str).expect("payload must be valid JSON");
    assert_eq!(
        payload["child_count"].as_u64(),
        Some(3),
        "child_count must be 3 in event payload"
    );
}

// ---------------------------------------------------------------------------
// DEC-04: decompose_double_is_noop
// ---------------------------------------------------------------------------

/// The second call to `decompose_triage_task` on the same task must be a
/// no-op: `root_promoted = false`, no new children, no new task_links, and
/// only one `decomposed` event total.
#[tokio::test]
async fn decompose_double_is_noop() {
    let store = make_store_arc();
    let config = KanbanConfig::default();
    let task_id = seed_triage_task(&store, "double decompose task", "alice").await;

    let make_output = || DecomposeOutput {
        new_title: "Double decompose title".to_string(),
        new_body: "## Goal\nDouble decompose body.".to_string(),
        children: vec![
            ChildSpec {
                title: "Child A".to_string(),
                assignee: "developer".to_string(),
                body: None,
            },
            ChildSpec {
                title: "Child B".to_string(),
                assignee: "researcher".to_string(),
                body: None,
            },
        ],
    };

    // First call — should succeed and promote.
    let result1 = decompose_triage_task(
        store.clone(),
        &task_id,
        &mock_decompose_fn(make_output()),
        &config,
    )
    .await
    .expect("first call should succeed");

    assert!(result1.root_promoted, "first call: root_promoted must be true");
    assert_eq!(result1.child_ids.len(), 2, "first call: 2 children");

    // Second call — should be a no-op (status guard fires).
    let result2 = decompose_triage_task(
        store.clone(),
        &task_id,
        &mock_decompose_fn(make_output()),
        &config,
    )
    .await
    .expect("second call should return Ok (idempotent, not Err)");

    assert!(
        !result2.root_promoted,
        "second call: root_promoted must be false"
    );
    assert!(result2.child_ids.is_empty(), "second call: no new children");

    // Verify no NEW children were added (still exactly 2).
    let s = store.lock().await;
    let links = s
        .list_links_for_parent(&task_id)
        .expect("list_links_for_parent");
    assert_eq!(links.len(), 2, "no new task_links must be added on second call");

    // Only one `decomposed` event total.
    let events = s.get_events(&task_id).expect("get_events");
    let decomposed_count = events.iter().filter(|e| e.kind == "decomposed").count();
    assert_eq!(
        decomposed_count, 1,
        "exactly 1 'decomposed' event even after double-call"
    );
}

// ---------------------------------------------------------------------------
// DEC-05: decompose_failure_retains_triage
// ---------------------------------------------------------------------------

/// When the [`DecomposeFn`] returns `Err`, the task must remain in `triage`,
/// a `decompose_failed` event must be appended with `reason = "llm_error"`,
/// and the error message must appear in the event payload.
#[tokio::test]
async fn decompose_failure_retains_triage() {
    let store = make_store_arc();
    let config = KanbanConfig::default();
    let task_id = seed_triage_task(&store, "failing decompose task", "alice").await;

    let error_msg = "simulated 500";
    let decompose_fn = mock_decompose_fn_err(error_msg);

    let result = decompose_triage_task(store.clone(), &task_id, &decompose_fn, &config).await;

    assert!(result.is_err(), "decompose_triage_task must return Err on LLM failure");

    // Task must still be in triage (NOT promoted).
    let s = store.lock().await;
    let task = s.get_task(&task_id).expect("task must still exist");
    assert_eq!(
        task.status, "triage",
        "task must remain in triage after LLM failure"
    );

    // A `decompose_failed` event must be appended.
    let events = s.get_events(&task_id).expect("get_events");
    let failed_events: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "decompose_failed")
        .collect();
    assert_eq!(
        failed_events.len(),
        1,
        "exactly one 'decompose_failed' event must be appended"
    );

    let payload_str = failed_events[0]
        .payload
        .as_deref()
        .expect("decompose_failed event must have payload");
    let payload: serde_json::Value =
        serde_json::from_str(payload_str).expect("payload must be valid JSON");
    assert_eq!(
        payload["reason"].as_str(),
        Some("llm_error"),
        "event payload must contain reason=llm_error"
    );
    let message = payload["message"].as_str().expect("payload must have message");
    assert!(
        message.contains(error_msg),
        "event payload message must contain the error string '{error_msg}'; got: '{message}'"
    );
}

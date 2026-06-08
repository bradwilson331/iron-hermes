//! Protocol-violation coverage for the kanban kernel (Plan 09, Task 1).
//!
//! Covers cases deferred from plan 03 task 2, plus the enum-level distinctness
//! assertion that `KanbanEventKind::Crashed` and `KanbanEventKind::ProtocolViolation`
//! are different event kinds in the wire format.
//!
//! # V1 heuristic note (from plan 03 SUMMARY)
//!
//! The dispatcher implements a **v1 heuristic** for protocol violation detection:
//! when a worker process exits with its task still `running`, the dispatcher
//! sees the dead PID and treats it as `crashed` (non-zero exit or signal kill
//! is assumed). Distinguishing "exited 0 without calling kanban_complete"
//! from "crashed" requires a dispatcher_state.json or a UNIX wait() exit-code
//! intercept — neither was implemented in v1 (plan 03 decision).
//!
//! Therefore:
//! - The `crashed` event IS emitted automatically by the dispatcher on dead-PID
//!   detection (tested in dispatcher_logic.rs `dead_pid_triggers_reclaim`).
//! - The `protocol_violation` event is emitted by the **store layer** in the
//!   D-22 `created_cards` / `expected_run_id` rejection paths (tested in
//!   tools_smoke.rs) AND as a distinct `KanbanEventKind` enum variant that
//!   can be emitted by future extension points.
//! - The v1 limit is documented: `protocol_violation_distinguished_from_crashed_requires_state_file`
//!   below is `#[ignore]`'d and explains why.
//!
//! # Active tests in this file
//!
//! 1. `crashed_and_protocol_violation_are_distinct_event_kinds` — asserts the
//!    two enum variants serialize to different strings ("crashed" vs
//!    "protocol_violation"). Prevents accidental conflation.
//!
//! 2. `impostor_worker_run_id_mismatch_emits_rejection` — a "worker" that holds
//!    a superseded run_id tries to call kanban_complete; expects structured
//!    rejection JSON (INV-36.3.7-02).
//!
//! 3. `phantom_created_cards_emits_completion_rejected_event` — a worker tries
//!    to complete with a non-existent task_id in created_cards; expects
//!    `completion_rejected` permanent event (INV-36.3.7-03).
//!
//! 4. `wrong_profile_created_cards_emits_completion_rejected_event` — a worker
//!    tries to claim a card created by a different profile; same rejection
//!    (INV-36.3.7-03 wrong_profile path).
//!
//! 5. `idempotency_key_dedup_prevents_duplicate_tasks` — two `kanban_create`
//!    calls with the same key return the same task_id (D-24 / INV-36.3.7-08).
//!
//! 6. `claim_lock_gated_write_emits_claim_expired` — a comment write from a
//!    worker whose claim_lock has been superseded no-ops and emits
//!    `claim_expired` (INV-36.3.7-09 / D-41).

use std::sync::Arc;

use ironhermes_kanban::events::KanbanEventKind;
use ironhermes_kanban::store::{CreateTaskOptions, KanbanStore};
use ironhermes_kanban::tools::{KanbanCommentTool, KanbanCompleteTool, KanbanCreateTool};
use ironhermes_tools::Tool;
use rusqlite::params;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::Mutex as TokioMutex;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn open_store(dir: &TempDir) -> KanbanStore {
    KanbanStore::new(dir.path().join("kanban.db")).expect("open test store")
}

fn make_store_arc(dir: &TempDir) -> Arc<TokioMutex<KanbanStore>> {
    Arc::new(TokioMutex::new(open_store(dir)))
}

/// Seed a running task directly via SQL (same pattern as dispatcher_logic.rs).
/// Returns (task_id, run_id).
fn seed_running(
    store: &mut KanbanStore,
    title: &str,
    assignee: &str,
    created_by: Option<&str>,
) -> (String, String) {
    let opts = CreateTaskOptions {
        created_by: created_by.map(String::from),
        ..Default::default()
    };
    let task = store.create_task(title, assignee, opts).unwrap();
    let task_id = task.id.clone();
    let run_id = format!("r_{}", uuid::Uuid::new_v4().simple());
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

    (task_id, run_id)
}

// ---------------------------------------------------------------------------
// Test 1: Enum distinctness
// ---------------------------------------------------------------------------

/// INV: `KanbanEventKind::Crashed` and `KanbanEventKind::ProtocolViolation`
/// must serialize to different snake_case strings so event logs can
/// distinguish "worker died" from "worker exited cleanly without completing".
///
/// This prevents the two kinds from being silently unified if someone
/// refactors the enum or the `as_str()` match arms.
#[test]
fn crashed_and_protocol_violation_are_distinct_event_kinds() {
    let crashed_str = KanbanEventKind::Crashed.as_str();
    let pv_str = KanbanEventKind::ProtocolViolation.as_str();

    assert_eq!(
        crashed_str, "crashed",
        "Crashed kind must serialize to 'crashed'"
    );
    assert_eq!(
        pv_str, "protocol_violation",
        "ProtocolViolation kind must serialize to 'protocol_violation'"
    );
    assert_ne!(
        crashed_str, pv_str,
        "Crashed and ProtocolViolation must be distinct event kind strings"
    );
}

// ---------------------------------------------------------------------------
// Test 2: Impostor worker (expected_run_id mismatch)
// ---------------------------------------------------------------------------

/// INV-36.3.7-02: A worker holding a superseded run_id (impostor) that calls
/// kanban_complete must receive a structured rejection, NOT complete the task.
///
/// Scenario: Task is running with run_A. A dispatcher reclaim resets the task
/// and assigns run_B. The old worker (still holding run_A) tries to complete.
/// The D-22 expected_run_id gate must reject it.
#[tokio::test]
async fn impostor_worker_run_id_mismatch_emits_rejection() {
    let dir = TempDir::new().unwrap();
    let store_arc = make_store_arc(&dir);

    let (task_id, stale_run_id) = {
        let mut store = store_arc.lock().await;
        seed_running(&mut store, "impostor-test task", "bot-worker", None)
    };

    // Simulate dispatcher reclaim: update current_run_id to a new run_B.
    let new_run_id = format!("r_{}", uuid::Uuid::new_v4().simple());
    {
        let store = store_arc.lock().await;
        store
            .conn
            .execute(
                "INSERT INTO task_runs (id, task_id, claim_lock, started_at) VALUES (?1, ?2, 'lock2', 1700001000.0)",
                params![new_run_id, task_id],
            )
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE tasks SET current_run_id=?1, claim_lock='lock2' WHERE id=?2",
                params![new_run_id, task_id],
            )
            .unwrap();
    }

    // Old worker (impostor) tries to complete with stale_run_id.
    let tool = KanbanCompleteTool::new(store_arc.clone(), true);
    let result = tool
        .execute(json!({
            "task_id": task_id,
            "expected_run_id": stale_run_id,
            "summary": "impostor completion — should be rejected"
        }))
        .await
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(
        parsed["status"], "rejected",
        "impostor kanban_complete must return status=rejected; got: {result}"
    );
    assert_eq!(
        parsed["reason"], "stale_run_id",
        "rejection reason must be stale_run_id; got: {result}"
    );
    assert_eq!(parsed["expected"], stale_run_id);
    assert_eq!(parsed["actual"], new_run_id);

    // Task must still be running (not completed by the impostor).
    let store = store_arc.lock().await;
    let task = store.get_task(&task_id).unwrap();
    assert_eq!(
        task.status, "running",
        "task must remain running after impostor rejection"
    );
}

// ---------------------------------------------------------------------------
// Test 3: Phantom created_cards rejection
// ---------------------------------------------------------------------------

/// INV-36.3.7-03: kanban_complete with a non-existent task_id in created_cards
/// must emit a permanent `completion_rejected` event and return structured rejection.
#[tokio::test]
async fn phantom_created_cards_emits_completion_rejected_event() {
    let dir = TempDir::new().unwrap();
    let store_arc = make_store_arc(&dir);

    let (task_id, run_id) = {
        let mut store = store_arc.lock().await;
        seed_running(
            &mut store,
            "phantom-cards task",
            "bot-worker",
            Some("bot-worker"),
        )
    };

    // Set the profile env so the tool reads the right profile.
    unsafe {
        std::env::set_var("HERMES_PROFILE", "bot-worker");
    }

    let phantom_id = "t_000000000000000000000000deadbeef";
    let tool = KanbanCompleteTool::new(store_arc.clone(), true);
    let result = tool
        .execute(json!({
            "task_id": task_id,
            "expected_run_id": run_id,
            "summary": "phantom cards test",
            "created_cards": [phantom_id]
        }))
        .await
        .unwrap();

    unsafe {
        std::env::remove_var("HERMES_PROFILE");
    }

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(
        parsed["status"], "rejected",
        "phantom created_cards must return rejected; got: {result}"
    );
    assert_eq!(parsed["reason"], "created_cards");

    // The permanent completion_rejected event must be in task_events.
    let store = store_arc.lock().await;
    let events = store.get_events(&task_id).unwrap();
    let perm_evt = events
        .iter()
        .find(|e| e.kind == "completion_rejected")
        .expect("completion_rejected event must be appended permanently");

    // Verify the phantom_id is mentioned in the payload.
    if let Some(payload_str) = &perm_evt.payload {
        assert!(
            payload_str.contains(phantom_id),
            "completion_rejected payload must mention the phantom id"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 4: Wrong-profile created_cards rejection
// ---------------------------------------------------------------------------

/// INV-36.3.7-03 (wrong_profile path): kanban_complete with a valid task_id but
/// one that was created by a different profile must also emit completion_rejected.
#[tokio::test]
async fn wrong_profile_created_cards_emits_completion_rejected_event() {
    let dir = TempDir::new().unwrap();
    let store_arc = make_store_arc(&dir);

    // Create a child task owned by "other-bot".
    let child_id = {
        let mut store = store_arc.lock().await;
        store
            .create_task(
                "child task (wrong profile)",
                "other-bot",
                CreateTaskOptions {
                    created_by: Some("other-bot".into()),
                    ..Default::default()
                },
            )
            .unwrap()
            .id
    };

    // Create and seed the worker's own task.
    let (task_id, run_id) = {
        let mut store = store_arc.lock().await;
        seed_running(
            &mut store,
            "wrong-profile-cards task",
            "bot-worker",
            Some("bot-worker"),
        )
    };

    unsafe {
        std::env::set_var("HERMES_PROFILE", "bot-worker");
    }

    let tool = KanbanCompleteTool::new(store_arc.clone(), true);
    let result = tool
        .execute(json!({
            "task_id": task_id,
            "expected_run_id": run_id,
            "summary": "wrong profile created_cards test",
            "created_cards": [child_id]
        }))
        .await
        .unwrap();

    unsafe {
        std::env::remove_var("HERMES_PROFILE");
    }

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(
        parsed["status"], "rejected",
        "wrong-profile created_cards must return rejected; got: {result}"
    );
    assert_eq!(parsed["reason"], "created_cards");

    // completion_rejected event must exist.
    let store = store_arc.lock().await;
    let events = store.get_events(&task_id).unwrap();
    assert!(
        events.iter().any(|e| e.kind == "completion_rejected"),
        "completion_rejected event must be appended for wrong-profile card"
    );
}

// ---------------------------------------------------------------------------
// Test 5: Idempotency key dedup
// ---------------------------------------------------------------------------

/// INV-36.3.7-08 (D-24): Two kanban_create calls with the same idempotency_key
/// must return the same task_id without creating a duplicate row.
#[tokio::test]
async fn idempotency_key_dedup_prevents_duplicate_tasks() {
    let dir = TempDir::new().unwrap();
    let store_arc = make_store_arc(&dir);

    let idem_key = "smoke-idem-key-001";

    let tool = KanbanCreateTool::new(store_arc.clone(), true);

    let result1 = tool
        .execute(json!({
            "title": "idempotent task",
            "assignee": "bot-worker",
            "idempotency_key": idem_key
        }))
        .await
        .unwrap();

    let result2 = tool
        .execute(json!({
            "title": "idempotent task (duplicate)",
            "assignee": "bot-worker",
            "idempotency_key": idem_key
        }))
        .await
        .unwrap();

    let parsed1: serde_json::Value = serde_json::from_str(&result1).unwrap();
    let parsed2: serde_json::Value = serde_json::from_str(&result2).unwrap();

    let id1 = parsed1["task_id"]
        .as_str()
        .expect("task_id in first response");
    let id2 = parsed2["task_id"]
        .as_str()
        .expect("task_id in second response");

    assert_eq!(
        id1, id2,
        "both kanban_create calls with the same idempotency_key must return the same task_id"
    );

    // Only one row must exist in the tasks table.
    let store = store_arc.lock().await;
    let count: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE idempotency_key=?1",
            params![idem_key],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 1,
        "exactly one task row must exist for the idempotency key"
    );
}

// ---------------------------------------------------------------------------
// Test 6: Claim-lock gated write (D-41)
// ---------------------------------------------------------------------------

/// INV-36.3.7-09 (D-41): A kanban_comment call from a worker whose claim_lock
/// has been superseded must no-op and emit a `claim_expired` advisory event.
#[tokio::test]
async fn claim_lock_gated_write_emits_claim_expired() {
    let dir = TempDir::new().unwrap();
    let store_arc = make_store_arc(&dir);

    let task_id = {
        let mut store = store_arc.lock().await;
        let task = store
            .create_task(
                "claim-lock-gated task",
                "bot-worker",
                CreateTaskOptions::default(),
            )
            .unwrap();
        let task_id = task.id.clone();

        // Seed the task as running with claim_lock="lock-A".
        let run_id = format!("r_{}", uuid::Uuid::new_v4().simple());
        store
            .conn
            .execute(
                "INSERT INTO task_runs (id, task_id, claim_lock, started_at) VALUES (?1, ?2, 'lock-A', 1700000000.0)",
                params![run_id, task_id],
            )
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE tasks SET status='running', current_run_id=?1, claim_lock='lock-A' WHERE id=?2",
                params![run_id, task_id],
            )
            .unwrap();

        task_id
    };

    // Worker holds claim_lock="lock-A" but task has since been reclaimed to "lock-B".
    {
        let store = store_arc.lock().await;
        store
            .conn
            .execute(
                "UPDATE tasks SET claim_lock='lock-B' WHERE id=?1",
                params![task_id],
            )
            .unwrap();
    }

    // Inject stale claim_lock in env so the comment tool reads it.
    unsafe {
        std::env::set_var("HERMES_KANBAN_TASK", &task_id);
        std::env::set_var("HERMES_KANBAN_CLAIM_LOCK", "lock-A");
    }

    let tool = KanbanCommentTool::new(store_arc.clone(), false);
    let result = tool
        .execute(json!({
            "body": "this comment should be dropped (stale claim_lock)"
        }))
        .await
        .unwrap();

    unsafe {
        std::env::remove_var("HERMES_KANBAN_TASK");
        std::env::remove_var("HERMES_KANBAN_CLAIM_LOCK");
    }

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(
        parsed["status"], "no-op",
        "stale-claim-lock comment must return status=no-op; got: {result}"
    );
    assert_eq!(parsed["reason"], "claim_expired");

    // Advisory claim_expired event must be appended.
    let store = store_arc.lock().await;
    let events = store.get_events(&task_id).unwrap();
    assert!(
        events.iter().any(|e| e.kind == "claim_expired"),
        "claim_expired advisory event must be appended when claim_lock is stale"
    );
}

// ---------------------------------------------------------------------------
// Manual-only test (v1 heuristic limit — documented, not runnable in CI)
// ---------------------------------------------------------------------------

/// V1 heuristic limitation: distinguishing "worker exited 0 without calling
/// kanban_complete" from "worker crashed" requires the dispatcher to intercept
/// the child process exit code across tick boundaries — either via a
/// dispatcher_state.json file or a UNIX wait() channel.
///
/// In v1 (plan 03 decision), the dispatcher does not write dispatcher_state.json
/// and cannot determine exit codes of already-reaped processes at the next tick.
/// Both cases appear as "dead PID with task still running", which the dispatcher
/// labels `crashed`.
///
/// The protocol violation event IS reachable via the tool layer (D-22 gates in
/// kanban_complete / kanban_block) and via future extension — it is NOT unreachable.
/// This invariant is tested manually during the plan 09 human-verify checkpoint
/// (Task 3: live worker spawn smoke).
///
/// To lift `#[ignore]` on this test, implement dispatcher_state.json reconciliation
/// (write the child's exit code to the state file before exec; read it on the next
/// tick to distinguish exit 0 from SIGKILL).
#[test]
#[ignore = "V1 heuristic cannot distinguish exit 0 without kanban_complete from crashed — \
            see protocol_violation.rs module doc and INV-36.3.7-10 in INV-36.3.7.md. \
            The manual checkpoint in plan 09 Task 3 covers this path."]
fn protocol_violation_distinguished_from_crashed_requires_state_file() {
    // When this test is un-ignored (by implementing dispatcher_state.json):
    //
    // 1. Seed a running task with claim_pid = 99999998 (guaranteed dead process
    //    that previously exited 0 without calling kanban_complete).
    // 2. Write a dispatcher_state.json mapping pid 99999998 → {task_id, run_id,
    //    exit_code: 0} — simulating what the dispatcher would record on spawn.
    // 3. Run one dispatcher tick.
    // 4. Assert the task has status="blocked" (auto-blocked per D-27).
    // 5. Assert task_events contains kind="protocol_violation" (NOT "crashed").
    //
    // This is the canonical test for INV-36.3.7-10.
    panic!(
        "This test is intentionally ignored in v1. Remove #[ignore] when \
         dispatcher_state.json reconciliation is implemented."
    );
}

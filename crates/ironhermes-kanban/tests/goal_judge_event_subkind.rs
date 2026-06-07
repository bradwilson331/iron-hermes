//! Phase 36.3.7.12 Plan 02 Task 3 — goal-loop event subkind producer tests.
//!
//! Pins the four `pub const &str` subkind tags exported from
//! `ironhermes_kanban::events` so Plan 04's wrapper imports by symbol
//! (typo-proof) and the `KanbanEventKind` enum stays frozen (D-04).
//!
//! Four tests:
//! 1. `subkind_consts_pin_exact_strings` — each CONST equals the locked
//!    literal. Guards against silent renames at the producer.
//! 2. `judge_verdict_subkind_round_trips_through_append_event` — write an
//!    `Edited` event with `subkind: GOAL_SUBKIND_JUDGE_VERDICT` and a
//!    structured payload; read it back via `get_events` and assert the
//!    JSON parses with the exact subkind string.
//! 3. `judge_error_subkind_round_trips` — same shape with
//!    `GOAL_SUBKIND_JUDGE_ERROR`.
//! 4. `frozen_surface_no_new_variants` — exhaustive `match` over every
//!    current `KanbanEventKind` variant (no catch-all). A new variant
//!    landing in `events.rs` would make this match non-exhaustive and
//!    BREAK THE BUILD. This is the load-bearing compile-time guard for
//!    Phase 36.3.7.6's frozen-enum decision (D-04).

use rusqlite::params;
use tempfile::TempDir;

use ironhermes_kanban::events::KanbanEventKind;
use ironhermes_kanban::store::{CreateTaskOptions, KanbanStore};
use ironhermes_kanban::{
    GOAL_SUBKIND_BUDGET_EXHAUSTED, GOAL_SUBKIND_JUDGE_ERROR, GOAL_SUBKIND_JUDGE_VERDICT,
    GOAL_SUBKIND_TURN_ADVANCED,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn open_store(dir: &TempDir) -> KanbanStore {
    KanbanStore::new(dir.path().join("kanban.db")).expect("open store")
}

/// Create a goal-mode task and seed a fake `running` claim so we can append
/// events with a real `run_id`. Returns `(task_id, run_id)`.
fn seed_goal_task_running(store: &mut KanbanStore) -> (String, String) {
    let opts = CreateTaskOptions {
        goal_mode: true,
        goal_max_turns: 5,
        ..Default::default()
    };
    let task = store
        .create_task("Translate docs", "alice", opts)
        .expect("create_task");
    let task_id = task.id.clone();
    let run_id = format!("r_{}", uuid::Uuid::new_v4().simple());

    let now = 1_700_000_000.0_f64;
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
// Tests
// ---------------------------------------------------------------------------

/// D-04: the four `pub const` tags equal the exact strings pinned in
/// RESEARCH.md "Claude's Discretion". A typo at the producer would
/// silently break dashboard / test filtering — pin them at the type
/// boundary.
#[test]
fn subkind_consts_pin_exact_strings() {
    assert_eq!(GOAL_SUBKIND_JUDGE_VERDICT, "judge_verdict");
    assert_eq!(GOAL_SUBKIND_JUDGE_ERROR, "judge_error");
    assert_eq!(GOAL_SUBKIND_TURN_ADVANCED, "goal_turn_advanced");
    assert_eq!(GOAL_SUBKIND_BUDGET_EXHAUSTED, "goal_budget_exhausted");
}

/// D-04 round-trip: a `judge_verdict` payload written through the existing
/// `append_event` API (KanbanEventKind::Edited + JSON subkind) deserializes
/// back to the exact subkind string on the consumer side.
#[test]
fn judge_verdict_subkind_round_trips_through_append_event() {
    let dir = TempDir::new().unwrap();
    let mut store = open_store(&dir);
    let (task_id, run_id) = seed_goal_task_running(&mut store);

    let payload = serde_json::json!({
        "subkind": GOAL_SUBKIND_JUDGE_VERDICT,
        "verdict": "not_met",
        "reason": "missing criterion 3",
        "turn": 7,
    });
    store
        .append_event(
            &task_id,
            Some(&run_id),
            KanbanEventKind::Edited,
            Some(&payload),
        )
        .expect("append_event");

    let events = store.get_events(&task_id).expect("get_events");
    // Find the most recent `edited` event — that's the one we just wrote
    // (created + claimed events precede it).
    let edited = events
        .iter()
        .rev()
        .find(|e| e.kind == "edited")
        .expect("edited event present");

    let parsed: serde_json::Value =
        serde_json::from_str(edited.payload.as_deref().expect("payload present"))
            .expect("payload is JSON");
    assert_eq!(
        parsed["subkind"].as_str(),
        Some("judge_verdict"),
        "subkind tag must equal the pinned string"
    );
    assert_eq!(parsed["verdict"].as_str(), Some("not_met"));
    assert_eq!(parsed["turn"].as_i64(), Some(7));
}

/// D-04 round-trip (judge_error variant): same shape with the JUDGE_ERROR
/// const. Pinned separately because dashboard filtering treats verdict and
/// error as distinct event types.
#[test]
fn judge_error_subkind_round_trips() {
    let dir = TempDir::new().unwrap();
    let mut store = open_store(&dir);
    let (task_id, run_id) = seed_goal_task_running(&mut store);

    let payload = serde_json::json!({
        "subkind": GOAL_SUBKIND_JUDGE_ERROR,
        "error": "provider 429 rate limit",
        "turn": 3,
    });
    store
        .append_event(
            &task_id,
            Some(&run_id),
            KanbanEventKind::Edited,
            Some(&payload),
        )
        .expect("append_event");

    let events = store.get_events(&task_id).expect("get_events");
    let edited = events
        .iter()
        .rev()
        .find(|e| e.kind == "edited")
        .expect("edited event present");

    let parsed: serde_json::Value =
        serde_json::from_str(edited.payload.as_deref().expect("payload present"))
            .expect("payload is JSON");
    assert_eq!(
        parsed["subkind"].as_str(),
        Some("judge_error"),
        "subkind tag must equal the pinned string"
    );
}

/// D-04 frozen-surface guard: the `KanbanEventKind` enum is locked by Phase
/// 36.3.7.6. A new variant landing in `events.rs` would make this exhaustive
/// `match` non-exhaustive → COMPILE BREAK. This is the load-bearing
/// compile-time guard. There is no catch-all arm.
///
/// If this test fails to compile, DO NOT add a `_ => ...` arm — the failure
/// is the contract. Either revert the new variant or amend this match AND
/// document why the frozen-surface decision is being lifted.
#[test]
fn frozen_surface_no_new_variants() {
    // A representative value — the match body is what matters, not the input.
    let kind = KanbanEventKind::Edited;

    let label: &str = match kind {
        // Lifecycle
        KanbanEventKind::Created => "created",
        KanbanEventKind::Promoted => "promoted",
        KanbanEventKind::Claimed => "claimed",
        KanbanEventKind::Completed => "completed",
        KanbanEventKind::Blocked => "blocked",
        KanbanEventKind::Unblocked => "unblocked",
        KanbanEventKind::Archived => "archived",
        // Edits
        KanbanEventKind::Assigned => "assigned",
        KanbanEventKind::Edited => "edited",
        KanbanEventKind::Reprioritized => "reprioritized",
        KanbanEventKind::Status => "status",
        // Worker telemetry
        KanbanEventKind::Spawned => "spawned",
        KanbanEventKind::Heartbeat => "heartbeat",
        KanbanEventKind::Reclaimed => "reclaimed",
        KanbanEventKind::Crashed => "crashed",
        KanbanEventKind::TimedOut => "timed_out",
        KanbanEventKind::Stale => "stale",
        KanbanEventKind::RespawnGuarded => "respawn_guarded",
        KanbanEventKind::SpawnFailed => "spawn_failed",
        KanbanEventKind::ProtocolViolation => "protocol_violation",
        KanbanEventKind::GaveUp => "gave_up",
        // v1-added
        KanbanEventKind::ClaimExtended => "claim_extended",
        KanbanEventKind::CompletionRejected => "completion_rejected",
        KanbanEventKind::HallucinatedRef => "hallucinated_ref",
        KanbanEventKind::TipScratchWorkspace => "tip_scratch_workspace",
        KanbanEventKind::ClaimExpired => "claim_expired",
        KanbanEventKind::SkippedNonspawnable => "skipped_nonspawnable",
        // Decomposer lifecycle (Phase 36.3.7.10)
        KanbanEventKind::Decomposed => "decomposed",
        KanbanEventKind::Specified => "specified",
        KanbanEventKind::DecomposeFailed => "decompose_failed",
    };
    assert_eq!(label, "edited");
}

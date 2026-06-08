//! Event kinds and the `task_events` row shape (D-05).
//!
//! `task_events` is append-only. Every transition appends one row with an
//! optional `run_id` so UIs can group events by attempt. The full inventory
//! below combines the upstream reference (`docs/kanban/reference.md`
//! "Event reference" section — Lifecycle + Edits + Worker telemetry tables)
//! with the v1-added advisory/safety kinds locked in CONTEXT.md (D-10
//! `claim_extended`, D-22 `completion_rejected` + `hallucinated_ref`, D-31
//! `tip_scratch_workspace`, D-41 `claim_expired`, worker-lanes
//! `skipped_nonspawnable`).
//!
//! Plan 02's `store.rs` will own the actual rusqlite `INSERT` — this
//! module exposes the static SQL via [`insert_event_sql()`] so plan 02
//! cannot drift from the schema column order.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// KanbanEventKind
// ---------------------------------------------------------------------------

/// One variant per `task_events.kind` recognized by the kernel.
///
/// Plain `String` is used at the crate boundary (`KanbanEvent::kind`) per
/// D-17; this enum exists for internal type safety + exhaustive matching
/// in dispatcher code paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KanbanEventKind {
    // ---------- Lifecycle (reference.md) ----------
    Created,
    Promoted,
    Claimed,
    Completed,
    Blocked,
    Unblocked,
    Archived,

    // ---------- Edits (reference.md) ----------
    Assigned,
    Edited,
    Reprioritized,
    Status,

    // ---------- Worker telemetry (reference.md) ----------
    Spawned,
    Heartbeat,
    Reclaimed,
    Crashed,
    TimedOut,
    Stale,
    RespawnGuarded,
    SpawnFailed,
    ProtocolViolation,
    GaveUp,

    // ---------- v1-added (CONTEXT.md + worker-lanes.md) ----------
    /// Live PID at TTL expiry → extend instead of reclaim (D-10 step 2).
    ClaimExtended,
    /// `created_cards=[...]` phantom-id rejection — permanent, non-deletable
    /// (D-22).
    CompletionRejected,
    /// Advisory: free-form prose `t_<hex>` reference in `summary` that does
    /// not resolve to a real task id (D-22). Non-blocking.
    HallucinatedRef,
    /// One-shot UX hint emitted on first-ever scratch workspace creation
    /// per install (D-31).
    TipScratchWorkspace,
    /// Worker write whose `claim_lock` no longer matches the current lock
    /// no-ops with this advisory event (D-41).
    ClaimExpired,
    /// Dispatcher skipped a `ready` task whose assignee profile is not
    /// spawnable on this host (per worker-lanes doc).
    SkippedNonspawnable,

    // ---------- Decomposer lifecycle (Phase 36.3.7.10) ----------
    /// LLM decomposer rewrote body + promoted triage task to todo (Phase 36.3.7.10).
    /// Success with fan-out: parent promoted, children created as todo.
    Decomposed,
    /// LLM specifier rewrote body + promoted triage task to todo without fan-out (Phase 36.3.7.10).
    /// Success without fan-out: body rewritten, task promoted, no children created.
    Specified,
    /// LLM decomposer failed; task retained in triage (Phase 36.3.7.10).
    /// Retain-in-triage policy: operator can see failure, fix title, re-trigger manually.
    DecomposeFailed,
}

// ---------------------------------------------------------------------------
// Phase 36.3.7.12 — Goal-loop event subkind tags.
// ---------------------------------------------------------------------------
//
// `KanbanEventKind` is FROZEN (Phase 36.3.7.6). Goal-loop events ride on
// `KanbanEventKind::Edited` with a JSON `subkind` payload tag, mirroring
// the nd7 quick-task precedent for comment events. No new enum variants.
//
// These `pub const &str` symbols are the **type-level contract** between
// the producer (the goal-loop wrapper at
// `crates/ironhermes-cli/src/kanban/goal_loop.rs`, landing in Plan 04) and
// any consumer that filters task_events by subkind (dashboard renderers,
// tests, future trajectory readers). Importing the symbol instead of
// duplicating the string literal at every emit site prevents the
// dashboard/test/emitter drift class of bug.
//
// The `frozen_surface_no_new_variants` test in
// `tests/goal_judge_event_subkind.rs` is the compile-time guard that the
// enum stays frozen.

/// Judge LLM emitted a met/not_met verdict for the last worker turn.
pub const GOAL_SUBKIND_JUDGE_VERDICT: &str = "judge_verdict";
/// Judge LLM call failed (network, rate-limit, malformed response).
pub const GOAL_SUBKIND_JUDGE_ERROR: &str = "judge_error";
/// The goal-loop wrapper bumped `tasks.goal_turns_used` via the CAS gate.
pub const GOAL_SUBKIND_TURN_ADVANCED: &str = "goal_turn_advanced";
/// The synthetic `kanban_block` reason emitted when the per-card turn
/// budget is exhausted without judge-met (D-03).
pub const GOAL_SUBKIND_BUDGET_EXHAUSTED: &str = "goal_budget_exhausted";

impl KanbanEventKind {
    /// snake_case kind name — the canonical wire / DB form.
    pub fn as_str(&self) -> &'static str {
        match self {
            KanbanEventKind::Created => "created",
            KanbanEventKind::Promoted => "promoted",
            KanbanEventKind::Claimed => "claimed",
            KanbanEventKind::Completed => "completed",
            KanbanEventKind::Blocked => "blocked",
            KanbanEventKind::Unblocked => "unblocked",
            KanbanEventKind::Archived => "archived",
            KanbanEventKind::Assigned => "assigned",
            KanbanEventKind::Edited => "edited",
            KanbanEventKind::Reprioritized => "reprioritized",
            KanbanEventKind::Status => "status",
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
            KanbanEventKind::ClaimExtended => "claim_extended",
            KanbanEventKind::CompletionRejected => "completion_rejected",
            KanbanEventKind::HallucinatedRef => "hallucinated_ref",
            KanbanEventKind::TipScratchWorkspace => "tip_scratch_workspace",
            KanbanEventKind::ClaimExpired => "claim_expired",
            KanbanEventKind::SkippedNonspawnable => "skipped_nonspawnable",
            KanbanEventKind::Decomposed => "decomposed",
            KanbanEventKind::Specified => "specified",
            KanbanEventKind::DecomposeFailed => "decompose_failed",
        }
    }
}

// ---------------------------------------------------------------------------
// KanbanEvent (row shape)
// ---------------------------------------------------------------------------

/// A row in the `task_events` table.
///
/// `payload` is a free-form JSON dict serialized to text — each kind
/// documents its expected payload shape in the reference doc / CONTEXT.md.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KanbanEvent {
    pub id: i64,
    pub task_id: String,
    pub run_id: Option<String>,
    /// Plain-String form of [`KanbanEventKind`] at the boundary (D-17).
    pub kind: String,
    pub payload: Option<String>,
    pub created_at: f64,
}

// ---------------------------------------------------------------------------
// SQL constants
// ---------------------------------------------------------------------------

/// Canonical INSERT statement for `task_events` rows.
///
/// Exposed as a `&'static str` so plan 02's `store.rs` cannot drift from
/// this column order. Bind order is `(task_id, run_id, kind, payload,
/// created_at)`.
pub fn insert_event_sql() -> &'static str {
    "INSERT INTO task_events (task_id, run_id, kind, payload, created_at) VALUES (?, ?, ?, ?, ?)"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_added_kinds_are_distinct() {
        // Sanity-check the snake_case round-trip for the v1-added kinds.
        assert_eq!(KanbanEventKind::ClaimExtended.as_str(), "claim_extended");
        assert_eq!(
            KanbanEventKind::CompletionRejected.as_str(),
            "completion_rejected"
        );
        assert_eq!(
            KanbanEventKind::HallucinatedRef.as_str(),
            "hallucinated_ref"
        );
        assert_eq!(
            KanbanEventKind::TipScratchWorkspace.as_str(),
            "tip_scratch_workspace"
        );
        assert_eq!(KanbanEventKind::ClaimExpired.as_str(), "claim_expired");
        assert_eq!(
            KanbanEventKind::SkippedNonspawnable.as_str(),
            "skipped_nonspawnable"
        );
        assert_eq!(KanbanEventKind::Decomposed.as_str(), "decomposed");
        assert_eq!(KanbanEventKind::Specified.as_str(), "specified");
        assert_eq!(
            KanbanEventKind::DecomposeFailed.as_str(),
            "decompose_failed"
        );
    }

    #[test]
    fn insert_sql_binds_five_params() {
        let sql = insert_event_sql();
        assert_eq!(sql.matches('?').count(), 5);
        assert!(sql.contains("task_events"));
    }
}

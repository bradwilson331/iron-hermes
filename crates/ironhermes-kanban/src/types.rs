//! Public type surface for the kanban kernel.
//!
//! Per PROJECT.md Key Decisions / CONTEXT.md D-17, **cross-crate boundaries
//! use plain `String`** for enumerable fields (status, etc.). The
//! [`KanbanStatus`] enum is exposed for internal type safety within
//! `ironhermes-kanban` (and for callers that explicitly opt in via
//! `FromStr` / `as_str`), but [`Task::status`] is a plain `String` so the
//! struct can flow into `ironhermes-cli`, `ironhermes-gateway`, and
//! `iron_hermes_ui` without forcing those crates to depend on this one
//! for the enum (which would create reverse / circular deps for some of
//! them).

use serde::{Deserialize, Serialize};
use std::str::FromStr;

use crate::error::KanbanError;

// ---------------------------------------------------------------------------
// Task
// ---------------------------------------------------------------------------

/// A row in the `tasks` table — a single logical unit of work on the board.
///
/// All fields use plain `String` / numeric types at the crate boundary
/// (D-17). The `skills` field is a JSON-serialized `Vec<String>` stored as
/// text to keep the SQLite schema flat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub body: Option<String>,
    pub assignee: String,
    /// One of the seven [`KanbanStatus`] variants serialized to lowercase
    /// (D-06). Plain `String` at the crate boundary per D-17.
    pub status: String,
    pub priority: i64,
    pub tenant: Option<String>,
    pub workspace: Option<String>,
    /// JSON-serialized `Vec<String>` of skill slugs (D-28: dispatcher emits
    /// `--skills <name>` for each on spawn).
    pub skills: Option<String>,
    /// Nullable-unique key for `kanban_create` short-circuit (D-24).
    pub idempotency_key: Option<String>,
    /// `<host>:<pid>:<uuid>` (D-08). NULL when no run is active.
    pub claim_lock: Option<String>,
    /// Unix epoch seconds (float) at which the active claim expires.
    pub claim_expires: Option<f64>,
    /// FK into `task_runs(id)`. NULL when no run is active.
    pub current_run_id: Option<String>,
    /// Per-task counter; reset on successful completion. Circuit breaker
    /// (D-12) trips at `failure_limit`.
    pub consecutive_failures: i64,
    /// Per-task override of the dispatcher's global retry cap.
    pub max_retries: Option<i64>,
    /// Wall-clock cap per attempt (D-10 step 4).
    pub max_runtime_seconds: Option<i64>,
    /// Dispatcher skips ready tasks whose `scheduled_at > now` (D-07).
    pub scheduled_at: Option<f64>,
    /// v2-reserved nullable fields — kernel ignores in v1 (D-07).
    pub workflow_template_id: Option<String>,
    pub current_step_key: Option<String>,
    /// Profile slug that created the task (used by the `created_cards`
    /// claim verification in `kanban_complete`, D-22).
    pub created_by: Option<String>,
    pub created_at: f64,
    pub started_at: Option<f64>,
    pub ended_at: Option<f64>,
}

// ---------------------------------------------------------------------------
// TaskRun
// ---------------------------------------------------------------------------

/// A row in the `task_runs` table — one attempt to execute a task (D-05).
///
/// The `task.current_run_id` pointer + `task_runs` row are written in the
/// same transaction (D-40), so they are coherent by construction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRun {
    pub id: String,
    pub task_id: String,
    pub claim_lock: String,
    pub claim_pid: Option<u32>,
    pub started_at: f64,
    pub ended_at: Option<f64>,
    /// One of: `completed`, `blocked`, `crashed`, `timed_out`, `spawn_failed`,
    /// `reclaimed`, `protocol_violation`, `stale` (or NULL while running).
    pub outcome: Option<String>,
    pub summary: Option<String>,
    /// Free-form JSON dict from `kanban_complete(metadata=...)`.
    pub metadata: Option<String>,
    pub error: Option<String>,
    pub log_path: Option<String>,
}

// ---------------------------------------------------------------------------
// TaskLink
// ---------------------------------------------------------------------------

/// A row in the `task_links` table — a parent → child dependency edge.
///
/// Cross-tenant links are rejected at link-creation time (D-39).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskLink {
    pub parent_id: String,
    pub child_id: String,
    pub created_at: f64,
}

// ---------------------------------------------------------------------------
// TaskComment
// ---------------------------------------------------------------------------

/// A row in the `task_comments` table — chronological discussion thread.
///
/// Comments are append-only; the worker context envelope replays them in
/// order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskComment {
    pub id: String,
    pub task_id: String,
    pub author: String,
    pub body: String,
    pub created_at: f64,
}

// ---------------------------------------------------------------------------
// KanbanStatus
// ---------------------------------------------------------------------------

/// The seven-state status machine (D-06).
///
/// Internal-only type — at the crate boundary the value is serialized to its
/// lowercase reference name in [`Task::status`] (D-17). Callers that want
/// type-safe handling can `KanbanStatus::from_str(&task.status)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KanbanStatus {
    Triage,
    Todo,
    Ready,
    Running,
    Blocked,
    Done,
    Archived,
}

impl KanbanStatus {
    /// Lowercase reference name (D-06).
    pub fn as_str(&self) -> &'static str {
        match self {
            KanbanStatus::Triage => "triage",
            KanbanStatus::Todo => "todo",
            KanbanStatus::Ready => "ready",
            KanbanStatus::Running => "running",
            KanbanStatus::Blocked => "blocked",
            KanbanStatus::Done => "done",
            KanbanStatus::Archived => "archived",
        }
    }
}

impl FromStr for KanbanStatus {
    type Err = KanbanError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "triage" => Ok(KanbanStatus::Triage),
            "todo" => Ok(KanbanStatus::Todo),
            "ready" => Ok(KanbanStatus::Ready),
            "running" => Ok(KanbanStatus::Running),
            "blocked" => Ok(KanbanStatus::Blocked),
            "done" => Ok(KanbanStatus::Done),
            "archived" => Ok(KanbanStatus::Archived),
            other => Err(KanbanError::ProtocolViolation(format!(
                "unknown kanban status: {other}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_round_trip() {
        for &s in &[
            "triage", "todo", "ready", "running", "blocked", "done", "archived",
        ] {
            let parsed = KanbanStatus::from_str(s).unwrap();
            assert_eq!(parsed.as_str(), s);
        }
    }

    #[test]
    fn status_unknown_rejected() {
        assert!(KanbanStatus::from_str("bogus").is_err());
    }
}

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
    // -----------------------------------------------------------------------
    // Phase 36.3.7.12 — Goal Mode (D-01).
    // -----------------------------------------------------------------------
    /// When `true`, the worker spawned for this card enters an in-session
    /// goal loop with an auxiliary judge LLM evaluating output against
    /// `title + body` between turns. CLI sets this via `--goal`; LLM tool
    /// sets via `kanban_create` arg.
    #[serde(default)]
    pub goal_mode: bool,
    /// Per-card turn budget for the goal loop (D-03). Budget exhaustion →
    /// worker emits a synthetic `kanban_block` before exit. Column DEFAULT 20
    /// and producer-level 0 → 20 coercion in `KanbanStore::create_task`
    /// guarantee a sane budget even when callers don't override.
    #[serde(default)]
    pub goal_max_turns: u32,
    /// Per-card turn counter, bumped by the worker harness via the
    /// CAS-gated `worker_write_gated` path between iterations. Reset to 0
    /// by reclaim+respawn (Pitfall 4).
    #[serde(default)]
    pub goal_turns_used: u32,
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
// Subscription (Phase 36.3.7.5 BUG-36.3.7.5-01)
// ---------------------------------------------------------------------------

/// A `kanban_subscriptions` row. One per (task, platform, chat_id, thread_id) tuple.
///
/// `source` is plain `String` at the boundary per D-17 — internal callers can match
/// against the CHECK constraint values `"auto"` and `"explicit"`. New sources MUST
/// be added to the CHECK in `schema.rs` first.
///
/// `thread_id` is stored as plain `String` (not `Option<String>`) because the schema
/// uses an empty-string default — SQLite `UNIQUE` treats `NULL` as distinct, which
/// would break the `(task_id, platform, chat_id, thread_id)` unique key for
/// thread-less chats. Callers pass `Option<&str>` at the CRUD boundary and the
/// store substitutes `""` for `None`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub id: i64,
    pub task_id: String,
    pub platform: String,
    pub chat_id: String,
    pub thread_id: String,
    pub source: String,
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

// ---------------------------------------------------------------------------
// Phase 36.3.7.7 — Swarm graph types (D-topology-shapes / D-worker-shape /
// D-blackboard-init). Used by `KanbanStore::create_swarm` (store.rs) and the
// `KanbanSwarmTool` LLM tool (tools/swarm.rs) and the `hermes kanban swarm`
// CLI verb (ironhermes-cli/src/kanban/commands.rs).
// ---------------------------------------------------------------------------

/// Per-worker carrier accepted by `KanbanStore::create_swarm`. Holds the
/// minimum info needed to materialize a worker card.
///
/// Two source shapes normalize into this type at the LLM-tool and CLI-verb
/// layers (Phase 36.3.7.7 D-worker-shape):
///
/// - **Flat form** (`Array<String>`): each string is an assignee slug. `title`
///   and `body` are both `None`; the store auto-generates a per-card title
///   `"<goal> — worker {i+1} of {N}"` and inherits the shared `body` from
///   [`SwarmGraphSpec::body`].
/// - **Rich form** (`Array<Object>`): each element is `{assignee, title?,
///   body?}`. Missing `title` falls back to the auto-generated form; missing
///   `body` falls back to the shared `SwarmGraphSpec::body`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KanbanWorkerSpec {
    /// Worker assignee slug — validated via
    /// `ironhermes_core::profile::validate_profile_name` before any INSERT.
    pub assignee: String,
    /// Per-card title override. `None` → auto-generated.
    pub title: Option<String>,
    /// Per-card body override. `None` → inherits `SwarmGraphSpec::body`.
    pub body: Option<String>,
}

/// Input to [`crate::store::KanbanStore::create_swarm`]. Mirrors
/// [`crate::store::CreateTaskOptions`] for the shared per-card fields, plus
/// the swarm-specific topology (workers / verifier / synthesizer / blackboard).
///
/// Per Phase 36.3.7.7 D-shared-vs-per-card-fields: `workspace`, `skills`,
/// `tenant`, `priority`, `max_runtime_seconds`, `max_retries` are SHARED
/// across all swarm cards (root + workers + verifier + synthesizer) and set
/// once at the tool level. `title` and `body` are PER-CARD (rich-form only).
/// `verifier` and `synthesizer` are PER-ROLE — one assignee each.
#[derive(Debug, Clone)]
pub struct SwarmGraphSpec {
    /// Shared goal / task description for the swarm. Auto-generated worker
    /// titles use this when no per-card title is supplied.
    pub goal: String,
    /// Worker carriers (>=1). Empty `workers` → store returns an error.
    pub workers: Vec<KanbanWorkerSpec>,
    /// Optional verifier assignee. When present, verifier card's `parents`
    /// are all worker IDs (Shape 2 or Shape 3).
    pub verifier: Option<String>,
    /// Optional synthesizer assignee. When present, synthesizer card's
    /// `parents` are the verifier (if present, Shape 3) or all workers
    /// (if no verifier, Shape 4 / P3 quorum).
    pub synthesizer: Option<String>,
    /// Opaque blackboard JSON. When `Some`, persisted as exactly one
    /// `task_comments` row on the root card with `author='swarm'` and
    /// `body=serde_json::to_string(&blackboard)?`. Per
    /// Phase 36.3.7.7 D-no-blackboard-validation, the store does NOT validate
    /// or impose any structure on this value.
    pub blackboard: Option<serde_json::Value>,
    /// Shared workspace path across all cards (per D-shared-vs-per-card-fields).
    pub workspace: Option<String>,
    /// Shared skills set across all cards.
    pub skills: Option<Vec<String>>,
    /// Shared tenant id across all cards.
    pub tenant: Option<String>,
    /// Shared priority across all cards.
    pub priority: Option<i64>,
    /// Shared max-runtime cap (seconds) across all cards.
    pub max_runtime_seconds: Option<i64>,
    /// Shared max-retries cap across all cards.
    pub max_retries: Option<i64>,
    /// Optional idempotency key. Per Phase 36.3.7.7 D-idempotency-suffix-scheme,
    /// per-card keys derive as `k:root`, `k:worker:{i}` (0-indexed),
    /// `k:verifier`, `k:synthesizer`. Re-invocation short-circuits via
    /// `find_by_idempotency_key("k:root")`.
    pub idempotency_key: Option<String>,
    /// Required orchestrator profile slug — populated from `$HERMES_PROFILE`
    /// at the tool/CLI layer (per D-root-assignee). Used as both the root
    /// card's `assignee` and the shared `created_by` for every card.
    pub created_by: Option<String>,
    /// Shared body fallback for flat-form workers (Claude's Discretion — the
    /// rich form's per-card `body` always wins when supplied).
    pub body: Option<String>,
}

/// Output of [`crate::store::KanbanStore::create_swarm`]. Tool/CLI consumers
/// serialize this struct directly as the success envelope.
///
/// Per Phase 36.3.7.7 D-idempotency-suffix-scheme, idempotency-replay
/// reconstructs this struct by reading the root card via the `k:root` key,
/// then walking `task_links` to gather worker/verifier/synthesizer IDs —
/// never inserting any new rows.
#[derive(Debug, Clone, Serialize)]
pub struct SwarmGraphIds {
    pub root_id: String,
    pub worker_ids: Vec<String>,
    pub verifier_id: Option<String>,
    pub synthesizer_id: Option<String>,
    /// `rowid` of the inserted `task_comments` blackboard row, or `None`
    /// when `SwarmGraphSpec.blackboard` was `None`.
    pub blackboard_event_id: Option<i64>,
}

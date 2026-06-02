//! Phase 36.3.7.11 Plan 01 — kanban dashboard read-side `#[server]` fns.
//!
//! Surface (D-05 / D-19 — board: Option<String> on every fn):
//! - `fetch_board(board)` — all non-archived tasks (D-09 status taxonomy).
//! - `fetch_task(task_id, board)` — worker_context envelope per Q5 / show.rs.
//! - `fetch_task_events(task_id, board, limit)` — recent task_events.
//! - `fetch_task_runs(task_id, board)` — all task_runs rows.
//! - `fetch_comments(task_id, board)` — all comment rows.
//!
//! Pattern A (PATTERNS.md): server-only ironhermes_kanban imports are
//! `#[cfg(feature = "server")]`-gated so the WASM client build never pulls
//! the kanban crate. The `#[server]` macro generates HTTP-call stubs on
//! the client and endpoints on the server.

use dioxus::prelude::*;

#[cfg(feature = "server")]
use ironhermes_kanban::store::{CreateTaskOptions, KanbanStore, ListFilters};

use crate::protocol::{
    CommentRow, CreateTaskPayload, DecomposeOrSpecify, DecomposeResult, KanbanEventRow,
    KanbanStatus, PromptPayload, TaskRow, TaskRunRow, WorkerContextEnvelope,
};

// Phase 36.3.7.11 Plan 02 (D-14): server-side defense-in-depth — the
// `patch_task_status` write fn enforces the SAME D-10 allowed-table the
// client uses for drag UX. KEEP IN SYNC: both call sites consume this
// single `is_drag_allowed` definition so the canonical table cannot drift
// between client and server.
use crate::kanban::transitions::is_drag_allowed;

// ---------------------------------------------------------------------------
// fetch_board — D-09 read-side board fetch
// ---------------------------------------------------------------------------

/// Phase 36.3.7.11 (D-05 / D-09 / D-18 / D-19): fetch the board's tasks for
/// the dashboard board view. Returns all non-archived tasks, plus archived
/// tasks when `include_archived = true` (BUG-1 fix from 36.3.7.11 UAT —
/// `quick-260602-ds9`). `board=None` resolves to the default board
/// (`~/.hermes/kanban.db`); `Some(slug)` opens the per-board DB.
///
/// Returns a `Vec<TaskRow>` ordered by the underlying `list_tasks` query
/// (created_at ASC). Plan 02 filters by status into per-column lists on the
/// client side.
#[server]
pub async fn fetch_board(
    board: Option<String>,
    include_archived: bool,
) -> Result<Vec<TaskRow>, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let board_owned = board;
        let tasks = tokio::task::spawn_blocking(move || -> Result<Vec<TaskRow>, String> {
            let store = match board_owned {
                Some(ref slug) => KanbanStore::open_for_board(slug)
                    .map_err(|e| format!("open_for_board('{}'): {e}", slug))?,
                None => KanbanStore::open_default()
                    .map_err(|e| format!("open_default: {e}"))?,
            };
            let filters = ListFilters {
                archived: include_archived,
                ..Default::default()
            };
            let tasks = store
                .list_tasks(filters)
                .map_err(|e| format!("list_tasks: {e}"))?;
            Ok(tasks
                .into_iter()
                .map(|t| TaskRow {
                    id: t.id,
                    title: t.title,
                    body: t.body,
                    assignee: t.assignee,
                    status: t.status,
                    priority: t.priority,
                    tenant: t.tenant,
                    workspace: t.workspace,
                    created_at: t.created_at,
                    started_at: t.started_at,
                    ended_at: t.ended_at,
                })
                .collect())
        })
        .await
        .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
        .map_err(ServerFnError::new)?;
        Ok(tasks)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (board, include_archived);
        Err(ServerFnError::new(
            "fetch_board unavailable without `server` feature",
        ))
    }
}

// ---------------------------------------------------------------------------
// fetch_task — D-20 / Q5 worker_context envelope
// ---------------------------------------------------------------------------

/// Phase 36.3.7.11 (D-19 / D-20 / Q5): fetch a single task's
/// worker_context envelope — the same shape `kanban_show` (show.rs lines
/// 218-232) returns to the LLM tool. The dashboard drawer renders this
/// 1:1.
#[server]
pub async fn fetch_task(
    task_id: String,
    board: Option<String>,
) -> Result<WorkerContextEnvelope, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let task_id_owned = task_id;
        let board_owned = board;
        let env = tokio::task::spawn_blocking(
            move || -> Result<WorkerContextEnvelope, String> {
                use rusqlite::params;
                let store = match board_owned {
                    Some(ref slug) => KanbanStore::open_for_board(slug)
                        .map_err(|e| format!("open_for_board('{}'): {e}", slug))?,
                    None => KanbanStore::open_default()
                        .map_err(|e| format!("open_default: {e}"))?,
                };
                let task = store
                    .get_task(&task_id_owned)
                    .map_err(|e| format!("get_task('{}'): {e}", task_id_owned))?;

                // Workspace fallback — when task.workspace is None the
                // canonical contract is to substitute the per-task scratch
                // path. For the dashboard read surface we surface the
                // task's recorded value (or empty string) since the
                // scratch-path helper lives behind a feature gate in the
                // kanban crate. Plan 02 / 03 can revisit if needed.
                let workspace = task.workspace.clone().unwrap_or_default();

                // Parent handoffs (show.rs lines 144-182).
                let parent_ids: Vec<String> = {
                    let mut stmt = store
                        .conn
                        .prepare(
                            "SELECT parent_id FROM task_links WHERE child_id = ?1 \
                             ORDER BY created_at ASC",
                        )
                        .map_err(|e| format!("prepare parents: {e}"))?;
                    let mapped = stmt
                        .query_map(params![task_id_owned], |r| r.get::<_, String>(0))
                        .map_err(|e| format!("query parents: {e}"))?;
                    let mut out: Vec<String> = Vec::new();
                    for row in mapped {
                        out.push(row.map_err(|e| format!("collect parents: {e}"))?);
                    }
                    out
                };
                let mut parent_handoffs: Vec<serde_json::Value> = Vec::new();
                for pid in &parent_ids {
                    if let Ok(parent) = store.get_task(pid) {
                        let run: Option<(Option<String>, Option<String>)> = store
                            .conn
                            .query_row(
                                "SELECT summary, metadata FROM task_runs \
                                 WHERE task_id = ?1 AND outcome = 'completed' \
                                 ORDER BY ended_at DESC LIMIT 1",
                                params![pid],
                                |r| Ok((r.get(0)?, r.get(1)?)),
                            )
                            .ok();
                        let (summary, metadata_str) = run.unwrap_or((None, None));
                        let metadata: Option<serde_json::Value> = metadata_str
                            .as_deref()
                            .and_then(|s| serde_json::from_str(s).ok());
                        parent_handoffs.push(serde_json::json!({
                            "parent_id": pid,
                            "parent_title": parent.title,
                            "parent_status": parent.status,
                            "summary": summary,
                            "metadata": metadata,
                        }));
                    }
                }

                // Prior attempts (show.rs lines 185-200).
                let prior_attempts: Vec<serde_json::Value> = {
                    let mut stmt = store
                        .conn
                        .prepare(
                            "SELECT outcome, summary, error, started_at, ended_at \
                             FROM task_runs WHERE task_id = ?1 ORDER BY started_at ASC",
                        )
                        .map_err(|e| format!("prepare runs: {e}"))?;
                    let mapped = stmt
                        .query_map(params![task_id_owned], |r| {
                            Ok(serde_json::json!({
                                "outcome": r.get::<_, Option<String>>(0)?,
                                "summary": r.get::<_, Option<String>>(1)?,
                                "error": r.get::<_, Option<String>>(2)?,
                                "started_at": r.get::<_, f64>(3)?,
                                "ended_at": r.get::<_, Option<f64>>(4)?,
                            }))
                        })
                        .map_err(|e| format!("query runs: {e}"))?;
                    let mut out: Vec<serde_json::Value> = Vec::new();
                    for row in mapped {
                        out.push(row.map_err(|e| format!("collect runs: {e}"))?);
                    }
                    out
                };

                // Comments (show.rs lines 203-216).
                let comments: Vec<serde_json::Value> = {
                    let mut stmt = store
                        .conn
                        .prepare(
                            "SELECT author, body, created_at FROM task_comments \
                             WHERE task_id = ?1 ORDER BY created_at ASC",
                        )
                        .map_err(|e| format!("prepare comments: {e}"))?;
                    let mapped = stmt
                        .query_map(params![task_id_owned], |r| {
                            Ok(serde_json::json!({
                                "author": r.get::<_, String>(0)?,
                                "body": r.get::<_, String>(1)?,
                                "created_at": r.get::<_, f64>(2)?,
                            }))
                        })
                        .map_err(|e| format!("query comments: {e}"))?;
                    let mut out: Vec<serde_json::Value> = Vec::new();
                    for row in mapped {
                        out.push(row.map_err(|e| format!("collect comments: {e}"))?);
                    }
                    out
                };

                Ok(WorkerContextEnvelope {
                    task_id: task.id,
                    title: task.title,
                    body: task.body,
                    status: task.status,
                    assignee: task.assignee,
                    tenant: task.tenant,
                    workspace,
                    priority: task.priority,
                    parent_handoffs,
                    prior_attempts,
                    comments,
                })
            },
        )
        .await
        .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
        .map_err(ServerFnError::new)?;
        Ok(env)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (task_id, board);
        Err(ServerFnError::new(
            "fetch_task unavailable without `server` feature",
        ))
    }
}

// ---------------------------------------------------------------------------
// fetch_task_events — D-20 last-N event stream
// ---------------------------------------------------------------------------

/// Phase 36.3.7.11 (D-19 / D-20): fetch the last `limit` task_events for a
/// task in id ASC order. `limit == 0` is treated as 20 (drawer default).
#[server]
pub async fn fetch_task_events(
    task_id: String,
    board: Option<String>,
    limit: u32,
) -> Result<Vec<KanbanEventRow>, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let task_id_owned = task_id;
        let board_owned = board;
        let effective_limit: u32 = if limit == 0 { 20 } else { limit };
        let events = tokio::task::spawn_blocking(
            move || -> Result<Vec<KanbanEventRow>, String> {
                let store = match board_owned {
                    Some(ref slug) => KanbanStore::open_for_board(slug)
                        .map_err(|e| format!("open_for_board('{}'): {e}", slug))?,
                    None => KanbanStore::open_default()
                        .map_err(|e| format!("open_default: {e}"))?,
                };
                let mut events = store
                    .get_events(&task_id_owned)
                    .map_err(|e| format!("get_events: {e}"))?;
                // get_events returns id ASC; trim to the last `effective_limit`.
                if events.len() > effective_limit as usize {
                    let drop_count = events.len() - effective_limit as usize;
                    events.drain(0..drop_count);
                }
                Ok(events
                    .into_iter()
                    .map(|e| KanbanEventRow {
                        id: e.id,
                        task_id: e.task_id,
                        kind: e.kind,
                        payload: e.payload,
                        created_at: e.created_at,
                    })
                    .collect())
            },
        )
        .await
        .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
        .map_err(ServerFnError::new)?;
        Ok(events)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (task_id, board, limit);
        Err(ServerFnError::new(
            "fetch_task_events unavailable without `server` feature",
        ))
    }
}

// ---------------------------------------------------------------------------
// fetch_task_runs — D-20 run history for the drawer
// ---------------------------------------------------------------------------

/// Phase 36.3.7.11 (D-19 / D-20): fetch all `task_runs` rows for a task in
/// started_at ASC order. Read-only — drawer Run History section.
#[server]
pub async fn fetch_task_runs(
    task_id: String,
    board: Option<String>,
) -> Result<Vec<TaskRunRow>, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let task_id_owned = task_id;
        let board_owned = board;
        let runs = tokio::task::spawn_blocking(
            move || -> Result<Vec<TaskRunRow>, String> {
                let store = match board_owned {
                    Some(ref slug) => KanbanStore::open_for_board(slug)
                        .map_err(|e| format!("open_for_board('{}'): {e}", slug))?,
                    None => KanbanStore::open_default()
                        .map_err(|e| format!("open_default: {e}"))?,
                };
                let rows = store
                    .get_runs(&task_id_owned)
                    .map_err(|e| format!("get_runs: {e}"))?;
                Ok(rows
                    .into_iter()
                    .map(|r| {
                        let elapsed_ms = r
                            .ended_at
                            .map(|end| ((end - r.started_at) * 1000.0) as i64);
                        TaskRunRow {
                            run_id: r.id,
                            outcome: r.outcome,
                            started_at: r.started_at,
                            ended_at: r.ended_at,
                            elapsed_ms,
                            summary: r.summary,
                            error: r.error,
                            worker: None,
                        }
                    })
                    .collect())
            },
        )
        .await
        .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
        .map_err(ServerFnError::new)?;
        Ok(runs)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (task_id, board);
        Err(ServerFnError::new(
            "fetch_task_runs unavailable without `server` feature",
        ))
    }
}

// ---------------------------------------------------------------------------
// fetch_comments — D-20 drawer comment thread
// ---------------------------------------------------------------------------

/// Phase 36.3.7.11 (D-19 / D-20): fetch all `task_comments` for a task in
/// chronological order (insertion order).
#[server]
pub async fn fetch_comments(
    task_id: String,
    board: Option<String>,
) -> Result<Vec<CommentRow>, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let task_id_owned = task_id;
        let board_owned = board;
        let rows = tokio::task::spawn_blocking(
            move || -> Result<Vec<CommentRow>, String> {
                use rusqlite::params;
                let store = match board_owned {
                    Some(ref slug) => KanbanStore::open_for_board(slug)
                        .map_err(|e| format!("open_for_board('{}'): {e}", slug))?,
                    None => KanbanStore::open_default()
                        .map_err(|e| format!("open_default: {e}"))?,
                };
                let mut stmt = store
                    .conn
                    .prepare(
                        "SELECT author, body, created_at FROM task_comments \
                         WHERE task_id = ?1 ORDER BY created_at ASC",
                    )
                    .map_err(|e| format!("prepare comments: {e}"))?;
                let mapped = stmt
                    .query_map(params![task_id_owned], |r| {
                        Ok(CommentRow {
                            author: r.get(0)?,
                            body: r.get(1)?,
                            created_at: r.get(2)?,
                        })
                    })
                    .map_err(|e| format!("query comments: {e}"))?;
                let mut rows: Vec<CommentRow> = Vec::new();
                for row in mapped {
                    rows.push(row.map_err(|e| format!("collect comments: {e}"))?);
                }
                Ok(rows)
            },
        )
        .await
        .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
        .map_err(ServerFnError::new)?;
        Ok(rows)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (task_id, board);
        Err(ServerFnError::new(
            "fetch_comments unavailable without `server` feature",
        ))
    }
}

// ===========================================================================
// Phase 36.3.7.11 Plan 02 — write-side `#[server]` fns
// ===========================================================================
//
// D-13 surface (four write fns):
//   - patch_task_status   — status transitions + Complete/Block via PromptPayload
//   - post_comment        — append a comment to a task
//   - create_task         — create a new task (D-06 default status logic)
//   - run_decompose_or_specify — Q9 BRANCH (b): NotWired (see below)
//
// D-14 defense-in-depth: patch_task_status calls `is_drag_allowed` BEFORE
// any mutation; Done/Blocked require the matching PromptPayload variant
// AND a non-empty summary/reason (Risk 8). The client-side validator runs
// the same predicate for UX; the server is truth (D-14).
//
// D-19: every fn takes `board: Option<String>` — `None` resolves to the
// default board via `KanbanStore::open_default`.
//
// Pattern A (PATTERNS.md): the `ironhermes_kanban` imports are
// `#[cfg(feature = "server")]`-gated; client builds get an `Err` stub.

// ---------------------------------------------------------------------------
// patch_task_status — D-13 / D-14 / Risk 8
// ---------------------------------------------------------------------------

/// Phase 36.3.7.11 Plan 02 (D-13 / D-14 / D-19 / Risk 8): apply a kanban
/// status transition to a task.
///
/// Validates server-side against the SAME `is_drag_allowed` predicate the
/// client uses (D-14). For Done / Blocked targets the matching
/// `PromptPayload` is required AND must be non-empty (Risk 8 — the
/// canonical `kanban_complete` / `kanban_block` contracts both reject
/// empty summary / empty reason).
///
/// Allowed paths:
/// - `prompt_payload = None` + `is_drag_allowed(cur, new) == true`
///   → direct status update (Triage→Todo, Todo→Ready, Blocked→Ready, *→Archived).
/// - `prompt_payload = Some(Complete { summary, metadata })` + new == Done
///   + !summary.is_empty()  → `KanbanStore::complete_task`.
/// - `prompt_payload = Some(Block { reason })` + new == Blocked
///   + !reason.is_empty()   → `KanbanStore::block_task`.
///
/// Everything else returns `ServerFnError` with a descriptive message.
#[server]
pub async fn patch_task_status(
    task_id: String,
    board: Option<String>,
    new_status: KanbanStatus,
    prompt_payload: Option<PromptPayload>,
) -> Result<TaskRow, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let task_id_owned = task_id.clone();
        let board_owned = board.clone();
        let new_status_owned = new_status;
        let prompt_payload_owned = prompt_payload.clone();

        let row = tokio::task::spawn_blocking(move || -> Result<TaskRow, String> {
            // Open store (D-18: None → default).
            let mut store = match board_owned {
                Some(ref slug) => KanbanStore::open_for_board(slug)
                    .map_err(|e| format!("open_for_board('{}'): {e}", slug))?,
                None => KanbanStore::open_default()
                    .map_err(|e| format!("open_default: {e}"))?,
            };

            // Read current status — required for D-14 validation.
            let current = store
                .get_task(&task_id_owned)
                .map_err(|e| format!("get_task('{}'): {e}", task_id_owned))?;
            let current_status = KanbanStatus::from_wire_str(&current.status).ok_or_else(
                || format!("unknown current status on task: {:?}", current.status),
            )?;

            // D-14 / Risk 8 branching:
            match (new_status_owned, prompt_payload_owned) {
                // Done branch — REQUIRES PromptPayload::Complete + non-empty summary.
                (KanbanStatus::Done, Some(PromptPayload::Complete { summary, metadata })) => {
                    if summary.is_empty() {
                        return Err(
                            "Risk 8: completing a task requires a non-empty summary"
                                .to_string(),
                        );
                    }
                    // The canonical kanban_complete contract: current_profile
                    // attribution defaults to "ui" for dashboard writes. The
                    // store's complete_task validates the run_id gate when
                    // expected_run_id is Some; we pass None (dashboard writes
                    // don't carry the LLM tool's run_id assertion).
                    store
                        .complete_task(
                            &task_id_owned,
                            Some(summary.as_str()),
                            metadata.as_ref(),
                            None,
                            None,
                            None,
                            "ui",
                        )
                        .map_err(|e| format!("complete_task: {e}"))?;
                }
                // Blocked branch — REQUIRES PromptPayload::Block + non-empty reason.
                (KanbanStatus::Blocked, Some(PromptPayload::Block { reason })) => {
                    if reason.is_empty() {
                        return Err(
                            "Risk 8: blocking a task requires a non-empty reason"
                                .to_string(),
                        );
                    }
                    store
                        .block_task(&task_id_owned, reason.as_str(), None)
                        .map_err(|e| format!("block_task: {e}"))?;
                }
                // Wrong PromptPayload variant for the target status.
                (KanbanStatus::Done, Some(PromptPayload::Block { .. })) => {
                    return Err(
                        "D-13: Done target requires PromptPayload::Complete, not Block"
                            .to_string(),
                    );
                }
                (KanbanStatus::Blocked, Some(PromptPayload::Complete { .. })) => {
                    return Err(
                        "D-13: Blocked target requires PromptPayload::Block, not Complete"
                            .to_string(),
                    );
                }
                // Done / Blocked with None — disallowed per Risk 8.
                (KanbanStatus::Done, None) => {
                    return Err(
                        "Risk 8: Done transition requires PromptPayload::Complete with summary"
                            .to_string(),
                    );
                }
                (KanbanStatus::Blocked, None) => {
                    return Err(
                        "Risk 8: Blocked transition requires PromptPayload::Block with reason"
                            .to_string(),
                    );
                }
                // Allowed drag transitions with no payload — direct status update.
                (target, None) => {
                    // D-14: enforce the same allowed-table the client uses.
                    if !is_drag_allowed(current_status, target) {
                        return Err(format!(
                            "D-14: transition not allowed: {:?} → {:?}",
                            current_status, target,
                        ));
                    }
                    // Special-cases that have dedicated store helpers:
                    match (current_status, target) {
                        // Blocked→Ready uses unblock_task (clears claim state).
                        (KanbanStatus::Blocked, KanbanStatus::Ready) => {
                            store
                                .unblock_task(&task_id_owned)
                                .map_err(|e| format!("unblock_task: {e}"))?;
                        }
                        // *→Archived uses archive_task (handles RUNNING reclamation).
                        (_, KanbanStatus::Archived) => {
                            store
                                .archive_task(&task_id_owned)
                                .map_err(|e| format!("archive_task: {e}"))?;
                        }
                        // Triage→Todo, Todo→Ready: direct UPDATE + Promoted event.
                        // No `update_task_status` helper exists in
                        // ironhermes-kanban (verified by grep). Use a
                        // parameterized UPDATE with a `promoted` event append
                        // — the same idiomatic shape used by store.rs's
                        // block_task / archive_task. NEVER format!() the SQL.
                        _ => {
                            use rusqlite::params;
                            let target_wire = target.as_wire_str();
                            store
                                .conn
                                .execute(
                                    "UPDATE tasks SET status = ?1 WHERE id = ?2",
                                    params![target_wire, task_id_owned],
                                )
                                .map_err(|e| format!("update status: {e}"))?;
                            // Append a `promoted` event so the dashboard tail
                            // consumer (D-15) surfaces the change via WS.
                            let payload = serde_json::json!({
                                "from": current_status.as_wire_str(),
                                "to": target_wire,
                                "source": "ui",
                            });
                            store
                                .append_event(
                                    &task_id_owned,
                                    None,
                                    ironhermes_kanban::events::KanbanEventKind::Promoted,
                                    Some(&payload),
                                )
                                .map_err(|e| format!("append promoted event: {e}"))?;
                        }
                    }
                }
                // Any other (target, Some(prompt)) is a misuse — only
                // Done+Complete and Blocked+Block are accepted with payloads.
                (other_target, Some(_)) => {
                    return Err(format!(
                        "D-13: PromptPayload is only valid for Done/Blocked targets, got {:?}",
                        other_target,
                    ));
                }
            }

            // Re-fetch the updated task to return the canonical row.
            let updated = store
                .get_task(&task_id_owned)
                .map_err(|e| format!("get_task after update: {e}"))?;
            Ok(TaskRow {
                id: updated.id,
                title: updated.title,
                body: updated.body,
                assignee: updated.assignee,
                status: updated.status,
                priority: updated.priority,
                tenant: updated.tenant,
                workspace: updated.workspace,
                created_at: updated.created_at,
                started_at: updated.started_at,
                ended_at: updated.ended_at,
            })
        })
        .await
        .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
        .map_err(ServerFnError::new)?;
        Ok(row)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (task_id, board, new_status, prompt_payload);
        Err(ServerFnError::new(
            "patch_task_status unavailable without `server` feature",
        ))
    }
}

// ---------------------------------------------------------------------------
// post_comment — D-13 / D-19
// ---------------------------------------------------------------------------

/// Phase 36.3.7.11 Plan 02 (D-13 / D-19): append a comment to a task.
///
/// Rejects empty body with `ServerFnError`. Author is recorded as `"ui"`
/// (no existing convention in api.rs to source from; matches the dashboard
/// surface naming).
#[server]
pub async fn post_comment(
    task_id: String,
    board: Option<String>,
    body: String,
) -> Result<CommentRow, ServerFnError> {
    #[cfg(feature = "server")]
    {
        if body.is_empty() {
            return Err(ServerFnError::new("Comment body cannot be empty"));
        }
        let task_id_owned = task_id.clone();
        let body_owned = body.clone();
        let board_owned = board.clone();
        let row = tokio::task::spawn_blocking(move || -> Result<CommentRow, String> {
            let mut store = match board_owned {
                Some(ref slug) => KanbanStore::open_for_board(slug)
                    .map_err(|e| format!("open_for_board('{}'): {e}", slug))?,
                None => KanbanStore::open_default()
                    .map_err(|e| format!("open_default: {e}"))?,
            };
            let comment = store
                .add_comment(&task_id_owned, "ui", &body_owned)
                .map_err(|e| format!("add_comment: {e}"))?;
            Ok(CommentRow {
                author: comment.author,
                body: comment.body,
                created_at: comment.created_at,
            })
        })
        .await
        .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
        .map_err(ServerFnError::new)?;
        Ok(row)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (task_id, board, body);
        Err(ServerFnError::new(
            "post_comment unavailable without `server` feature",
        ))
    }
}

// ---------------------------------------------------------------------------
// create_task — D-13 / D-19
// ---------------------------------------------------------------------------

/// Phase 36.3.7.11 Plan 02 (D-13 / D-19): create a new task on the board.
///
/// Empty title returns `ServerFnError`. The `start_in_triage` flag on the
/// payload maps to `CreateTaskOptions.triage` — the store's
/// `create_task` then applies the D-06 initial-status rule (Triage if
/// triage=true, Ready if parents empty, Todo otherwise).
#[server]
pub async fn create_task(
    board: Option<String>,
    payload: CreateTaskPayload,
) -> Result<TaskRow, ServerFnError> {
    #[cfg(feature = "server")]
    {
        if payload.title.is_empty() {
            return Err(ServerFnError::new("Task title is required"));
        }
        let payload_owned = payload.clone();
        let board_owned = board.clone();
        let row = tokio::task::spawn_blocking(move || -> Result<TaskRow, String> {
            let mut store = match board_owned {
                Some(ref slug) => KanbanStore::open_for_board(slug)
                    .map_err(|e| format!("open_for_board('{}'): {e}", slug))?,
                None => KanbanStore::open_default()
                    .map_err(|e| format!("open_default: {e}"))?,
            };
            let assignee = payload_owned
                .assignee
                .clone()
                .unwrap_or_else(|| "unassigned".to_string());
            let opts = CreateTaskOptions {
                body: payload_owned.body.clone(),
                parents: payload_owned.parents.clone(),
                tenant: payload_owned.tenant.clone(),
                priority: Some(payload_owned.priority),
                triage: payload_owned.start_in_triage,
                created_by: Some("ui".to_string()),
                ..Default::default()
            };
            let task = store
                .create_task(&payload_owned.title, &assignee, opts)
                .map_err(|e| format!("create_task: {e}"))?;
            Ok(TaskRow {
                id: task.id,
                title: task.title,
                body: task.body,
                assignee: task.assignee,
                status: task.status,
                priority: task.priority,
                tenant: task.tenant,
                workspace: task.workspace,
                created_at: task.created_at,
                started_at: task.started_at,
                ended_at: task.ended_at,
            })
        })
        .await
        .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
        .map_err(ServerFnError::new)?;
        Ok(row)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (board, payload);
        Err(ServerFnError::new(
            "create_task unavailable without `server` feature",
        ))
    }
}

// ---------------------------------------------------------------------------
// run_decompose_or_specify — D-13 / Q9 BRANCH
// ---------------------------------------------------------------------------

// Q9 BRANCH (b): AppState lacks the kernel's `DecomposeFn` aux client.
//
// Kernel signatures consulted (verified in this worktree):
//   - crates/ironhermes-kanban/src/decomposer.rs:238
//     pub async fn decompose_triage_task(
//         store: Arc<TokioMutex<KanbanStore>>,
//         task_id: &str,
//         decompose_fn: &DecomposeFn,  // Arc<dyn Fn(DecomposeRequest)
//                                       //   -> BoxFuture<Result<DecomposeOutput>>
//                                       //   + Send + Sync>
//         config: &KanbanConfig,
//     ) -> Result<DecomposedIds>
//   - crates/ironhermes-kanban/src/decomposer.rs:161
//     pub async fn specify_triage_task(
//         store: Arc<TokioMutex<KanbanStore>>,
//         task_id: &str,
//         decompose_fn: &DecomposeFn,
//         config: &KanbanConfig,
//     ) -> Result<DecomposedIds>
//
// AppState audit (crates/iron_hermes_ui/src/server/state.rs):
//   - `resolver: Arc<ProviderResolver>` — yes, but `DecomposeFn` is a
//     closure that calls an LLM and parses structured JSON output. Building
//     it requires a request builder + provider client + JSON parser glue
//     that does NOT live in iron_hermes_ui today.
//   - `runtime: Arc<AgentRuntime>` — exposes a `.client()` provider client,
//     but `AgentRuntime::run_turn` is an agent-loop entry, not a one-shot
//     structured-output call.
//   - No `decompose_fn: DecomposeFn` field — wiring it is the same
//     unresolved gap the LLM `kanban_decompose` tool flags as `no_aux_client`.
//
// Branch (b) is therefore the correct v1 behavior: surface a `NotWired`
// envelope pointing the operator at the CLI. The dashboard UI renders
// this as a tooltip on the Decompose / Specify buttons. Branch (a)
// (wiring the DecomposeFn from `runtime.client()`) is a future-phase
// upgrade — explicitly out of scope per CONTEXT v1.

/// Phase 36.3.7.11 Plan 02 (D-13 / Q9): invoke the decomposer / specifier
/// kernel for a triage task.
///
/// Q9 BRANCH (b) — see comment above. The dashboard's `AppState` does not
/// expose a kernel-compatible `DecomposeFn`, so this fn returns
/// `DecomposeResult::NotWired` with the equivalent CLI command. The UI
/// surfaces the message as a tooltip on the Decompose / Specify buttons.
#[server]
pub async fn run_decompose_or_specify(
    task_id: String,
    board: Option<String>,
    action: DecomposeOrSpecify,
) -> Result<DecomposeResult, ServerFnError> {
    #[cfg(feature = "server")]
    {
        // Q9 BRANCH (b): no DecomposeFn available on AppState; return
        // NotWired with the CLI hint. The kernel signatures live at
        // decomposer.rs:238 (`decompose_triage_task`) and decomposer.rs:161
        // (`specify_triage_task`).
        let slug = action.slug();
        let message = format!("Use: hermes kanban {} {}", slug, task_id);
        let _ = board; // unused in branch (b) — kept for D-19 signature
        Ok(DecomposeResult::NotWired { message })
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (task_id, board, action);
        Err(ServerFnError::new(
            "run_decompose_or_specify unavailable without `server` feature",
        ))
    }
}

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
use ironhermes_kanban::store::{KanbanStore, ListFilters};

use crate::protocol::{
    CommentRow, KanbanEventRow, TaskRow, TaskRunRow, WorkerContextEnvelope,
};

// ---------------------------------------------------------------------------
// fetch_board — D-09 read-side board fetch
// ---------------------------------------------------------------------------

/// Phase 36.3.7.11 (D-05 / D-09 / D-18 / D-19): fetch the board's non-archived
/// tasks for the dashboard board view. `board=None` resolves to the default
/// board (`~/.hermes/kanban.db`); `Some(slug)` opens the per-board DB.
///
/// Returns a `Vec<TaskRow>` ordered by the underlying `list_tasks` query
/// (created_at ASC). Plan 02 filters by status into per-column lists on the
/// client side.
#[server]
pub async fn fetch_board(board: Option<String>) -> Result<Vec<TaskRow>, ServerFnError> {
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
                archived: false,
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
        let _ = board;
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

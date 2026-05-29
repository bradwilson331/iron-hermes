//! `kanban_show` — read the current task and return the worker_context envelope (D-21).
//!
//! Defaults `task_id` to `$HERMES_KANBAN_TASK` when called with no args so workers
//! can simply call `kanban_show({})` without threading the task id explicitly.
//!
//! The `worker_context` envelope shape (reference.md "How workers interact with the board"):
//! - title, body, status, assignee, tenant
//! - workspace path
//! - parent_handoffs: most recent completed run's {summary, metadata} per parent
//! - prior_attempts: per-run {outcome, summary, error, started_at, ended_at}
//! - comments: full chronological [{author, body, created_at}]

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use ironhermes_tools::Tool;
use rusqlite::params;
use serde_json::{Value, json};
use tokio::sync::Mutex as TokioMutex;

use crate::paths::kanban_workspace_for;
use crate::store::KanbanStore;

/// LLM tool: read the current kanban task and return the worker_context envelope.
pub struct KanbanShowTool {
    store: Arc<TokioMutex<KanbanStore>>,
    explicit_enable: bool,
}

impl KanbanShowTool {
    pub fn new(store: Arc<TokioMutex<KanbanStore>>, explicit_enable: bool) -> Self {
        Self {
            store,
            explicit_enable,
        }
    }
}

#[async_trait]
impl Tool for KanbanShowTool {
    fn name(&self) -> &str {
        "kanban_show"
    }

    fn toolset(&self) -> &str {
        "kanban"
    }

    fn description(&self) -> &str {
        "Read the current Kanban task: title, body, prior attempts, parent handoffs, comments, \
         workspace. Defaults task_id to $HERMES_KANBAN_TASK when omitted."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "kanban_show",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "Task ID to inspect. Omit to use $HERMES_KANBAN_TASK (worker mode default)."
                    }
                },
                "required": []
            }),
        )
    }

    /// Available when `HERMES_KANBAN_TASK` is set (worker mode) or `explicit_enable` is true
    /// (orchestrator mode) — D-20.
    fn is_available(&self) -> bool {
        std::env::var("HERMES_KANBAN_TASK").is_ok() || self.explicit_enable
    }

    async fn execute(&self, args: Value) -> anyhow::Result<String> {
        // Resolve task_id: from arg, then from HERMES_KANBAN_TASK env.
        let task_id = args
            .get("task_id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| std::env::var("HERMES_KANBAN_TASK").ok())
            .ok_or_else(|| {
                anyhow::anyhow!("task_id required when HERMES_KANBAN_TASK is not set")
            })?;

        let store = self.store.lock().await;

        // Load the task.
        let task = store.get_task(&task_id)?;

        // Resolve workspace path.
        let workspace = task
            .workspace
            .clone()
            .unwrap_or_else(|| {
                kanban_workspace_for(&task.id)
                    .to_string_lossy()
                    .into_owned()
            });

        // Load parent handoffs: for each parent link, load the parent task and
        // select the most recently completed run's summary + metadata.
        let parent_ids: Vec<String> = {
            let mut stmt = store.conn.prepare(
                "SELECT parent_id FROM task_links WHERE child_id = ?1 ORDER BY created_at ASC",
            )?;
            stmt.query_map(params![task_id], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };

        let mut parent_handoffs: Vec<Value> = Vec::new();
        for pid in &parent_ids {
            if let Ok(parent) = store.get_task(pid) {
                // Most recent completed run for this parent.
                let run: Option<(Option<String>, Option<String>)> = store
                    .conn
                    .query_row(
                        "SELECT summary, metadata FROM task_runs \
                         WHERE task_id = ?1 AND outcome = 'completed' \
                         ORDER BY ended_at DESC LIMIT 1",
                        params![pid],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;

                let (summary, metadata_str) = run.unwrap_or((None, None));
                let metadata: Option<Value> = metadata_str
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok());

                parent_handoffs.push(json!({
                    "parent_id": pid,
                    "parent_title": parent.title,
                    "parent_status": parent.status,
                    "summary": summary,
                    "metadata": metadata,
                }));
            }
        }

        // Load prior attempts (all runs for this task, ordered by started_at).
        let prior_attempts: Vec<Value> = {
            let mut stmt = store.conn.prepare(
                "SELECT outcome, summary, error, started_at, ended_at \
                 FROM task_runs WHERE task_id = ?1 ORDER BY started_at ASC",
            )?;
            stmt.query_map(params![task_id], |r| {
                Ok(json!({
                    "outcome": r.get::<_, Option<String>>(0)?,
                    "summary": r.get::<_, Option<String>>(1)?,
                    "error": r.get::<_, Option<String>>(2)?,
                    "started_at": r.get::<_, f64>(3)?,
                    "ended_at": r.get::<_, Option<f64>>(4)?,
                }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };

        // Load comments in chronological order.
        let comments: Vec<Value> = {
            let mut stmt = store.conn.prepare(
                "SELECT author, body, created_at FROM task_comments \
                 WHERE task_id = ?1 ORDER BY created_at ASC",
            )?;
            stmt.query_map(params![task_id], |r| {
                Ok(json!({
                    "author": r.get::<_, String>(0)?,
                    "body": r.get::<_, String>(1)?,
                    "created_at": r.get::<_, f64>(2)?,
                }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };

        // Assemble the worker_context envelope.
        let worker_context = json!({
            "task_id": task.id,
            "title": task.title,
            "body": task.body,
            "status": task.status,
            "assignee": task.assignee,
            "tenant": task.tenant,
            "workspace": workspace,
            "priority": task.priority,
            "parent_handoffs": parent_handoffs,
            "prior_attempts": prior_attempts,
            "comments": comments,
        });

        Ok(serde_json::to_string_pretty(&worker_context)?)
    }
}

// ---------------------------------------------------------------------------
// Optional extension for Transaction queries
// ---------------------------------------------------------------------------

trait OptionalExtension {
    fn optional(self) -> rusqlite::Result<Option<(Option<String>, Option<String>)>>;
}

impl OptionalExtension for rusqlite::Result<(Option<String>, Option<String>)> {
    fn optional(self) -> rusqlite::Result<Option<(Option<String>, Option<String>)>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_store() -> Arc<TokioMutex<KanbanStore>> {
        let dir = tempfile::tempdir().unwrap();
        let store = KanbanStore::new(dir.path().join("test.db")).unwrap();
        // Keep tempdir alive by leaking — acceptable in tests
        std::mem::forget(dir);
        Arc::new(TokioMutex::new(store))
    }

    #[test]
    fn is_available_respects_env() {
        // Remove the env var to ensure a clean state.
        unsafe { std::env::remove_var("HERMES_KANBAN_TASK"); }
        let store = make_store();
        let tool = KanbanShowTool::new(store.clone(), false);
        assert!(!tool.is_available(), "should be unavailable without env or explicit_enable");

        unsafe { std::env::set_var("HERMES_KANBAN_TASK", "t_test123"); }
        let tool2 = KanbanShowTool::new(store.clone(), false);
        assert!(tool2.is_available(), "should be available when HERMES_KANBAN_TASK is set");
        unsafe { std::env::remove_var("HERMES_KANBAN_TASK"); }

        let tool3 = KanbanShowTool::new(store, true);
        assert!(tool3.is_available(), "should be available with explicit_enable=true");
    }
}

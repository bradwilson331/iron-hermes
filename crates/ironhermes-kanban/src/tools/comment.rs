//! `kanban_comment` — append a comment to a task (D-25).
//!
//! In worker mode (HERMES_KANBAN_CLAIM_LOCK set), the comment INSERT is wrapped
//! in `worker_write_gated` to honor D-41 stale-claim no-op: if the worker's
//! claim has been superseded, the write is a no-op and a `claim_expired`
//! advisory event is emitted instead.
//!
//! In orchestrator mode (explicit_enable=true, no claim lock), the comment is
//! inserted directly via `store.add_comment`.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use ironhermes_tools::Tool;
use rusqlite::params;
use serde_json::{Value, json};
use tokio::sync::Mutex as TokioMutex;

use crate::store::KanbanStore;

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn new_id(prefix: &str) -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &id[..16])
}

/// LLM tool: append a comment to a task.
pub struct KanbanCommentTool {
    store: Arc<TokioMutex<KanbanStore>>,
    explicit_enable: bool,
}

impl KanbanCommentTool {
    pub fn new(store: Arc<TokioMutex<KanbanStore>>, explicit_enable: bool) -> Self {
        Self {
            store,
            explicit_enable,
        }
    }
}

#[async_trait]
impl Tool for KanbanCommentTool {
    fn name(&self) -> &str {
        "kanban_comment"
    }

    fn toolset(&self) -> &str {
        "kanban"
    }

    fn description(&self) -> &str {
        "Append a comment to a Kanban task. Defaults task_id to $HERMES_KANBAN_TASK in worker \
         mode. In worker mode with HERMES_KANBAN_CLAIM_LOCK set, the write is gated on claim \
         validity (D-41)."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "kanban_comment",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "Task ID to comment on. Omit to use $HERMES_KANBAN_TASK."
                    },
                    "body": {
                        "type": "string",
                        "description": "Comment text."
                    }
                },
                "required": ["body"]
            }),
        )
    }

    fn is_available(&self) -> bool {
        std::env::var("HERMES_KANBAN_TASK").is_ok() || self.explicit_enable
    }

    async fn execute(&self, args: Value) -> anyhow::Result<String> {
        // Resolve task_id.
        let task_id = args
            .get("task_id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| std::env::var("HERMES_KANBAN_TASK").ok())
            .ok_or_else(|| {
                anyhow::anyhow!("task_id required when HERMES_KANBAN_TASK is not set")
            })?;

        let body = args
            .get("body")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("body is required"))?
            .to_string();

        let author = std::env::var("HERMES_PROFILE").unwrap_or_else(|_| "unknown".into());

        // Check for worker-mode claim lock (D-41).
        let claim_lock_env = std::env::var("HERMES_KANBAN_CLAIM_LOCK").ok();

        if let Some(claim_lock) = claim_lock_env {
            // Worker mode — gate the write on claim validity (D-41).
            let mut store = self.store.lock().await;

            let comment_id = new_id("c");
            let task_id_c = task_id.clone();
            let author_c = author.clone();
            let body_c = body.clone();
            let comment_id_c = comment_id.clone();

            let gated = crate::cas::worker_write_gated(
                &mut store.conn,
                &task_id,
                &claim_lock,
                move |tx| {
                    let now = now_secs();
                    tx.execute(
                        "INSERT INTO task_comments (id, task_id, author, body, created_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![comment_id_c, task_id_c, author_c, body_c, now],
                    )
                    .map_err(crate::error::KanbanError::from)?;
                    Ok(())
                },
            )?;

            if gated {
                Ok(serde_json::to_string_pretty(&json!({
                    "comment_id": comment_id,
                    "task_id": task_id,
                }))?)
            } else {
                Ok(serde_json::to_string_pretty(&json!({
                    "status": "no-op",
                    "reason": "claim_expired",
                }))?)
            }
        } else {
            // Orchestrator mode or unclaimed worker — direct insert (HERMES_KANBAN_CLAIM_LOCK not set).
            let mut store = self.store.lock().await;
            let comment = store.add_comment(&task_id, &author, &body)?;
            Ok(serde_json::to_string_pretty(&json!({
                "comment_id": comment.id,
                "task_id": comment.task_id,
            }))?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_store() -> Arc<TokioMutex<KanbanStore>> {
        let dir = tempfile::tempdir().unwrap();
        let store = KanbanStore::new(dir.path().join("test.db")).unwrap();
        std::mem::forget(dir);
        Arc::new(TokioMutex::new(store))
    }

    #[test]
    fn is_available_respects_env() {
        unsafe { std::env::remove_var("HERMES_KANBAN_TASK"); }
        let store = make_store();
        let tool = KanbanCommentTool::new(store.clone(), false);
        assert!(!tool.is_available());

        unsafe { std::env::set_var("HERMES_KANBAN_TASK", "t_test"); }
        let tool2 = KanbanCommentTool::new(store.clone(), false);
        assert!(tool2.is_available());
        unsafe { std::env::remove_var("HERMES_KANBAN_TASK"); }

        let tool3 = KanbanCommentTool::new(store, true);
        assert!(tool3.is_available());
    }
}

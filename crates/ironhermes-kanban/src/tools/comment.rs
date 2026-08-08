//! `kanban_comment` — append a comment to a task (D-25).
//!
//! In worker mode (IRONHERMES_KANBAN_CLAIM_LOCK set), the comment INSERT is wrapped
//! in `worker_write_gated` to honor D-41 stale-claim no-op: if the worker's
//! claim has been superseded, the write is a no-op and a `claim_expired`
//! advisory event is emitted instead.
//!
//! In orchestrator mode (explicit_enable=true, no claim lock), the comment is
//! inserted directly via `store.add_comment`.
//!
//! # Multi-board (Plan 07, D-08)
//!
//! Accepts an optional `board` parameter. Resolves the board context at the top
//! of `execute()` before any DB access. Injects `board` + `board_source` into
//! every success and rejection envelope (T-5 mitigation).

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
    #[allow(dead_code)]
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
        "Append a comment to a Kanban task. Defaults task_id to $IRONHERMES_KANBAN_TASK in worker \
         mode. In worker mode with IRONHERMES_KANBAN_CLAIM_LOCK set, the write is gated on claim \
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
                        "description": "Task ID to comment on. Omit to use $IRONHERMES_KANBAN_TASK."
                    },
                    "body": {
                        "type": "string",
                        "description": "Comment text."
                    },
                    "board": {
                        "type": "string",
                        "description": "Board slug to target. Omit to use IRONHERMES_KANBAN_BOARD env / current file / 'default' (4-tier resolution)."
                    }
                },
                "required": ["body"]
            }),
        )
    }

    fn is_available(&self) -> bool {
        crate::kanban_env("TASK").is_some() || self.explicit_enable
    }

    async fn execute(&self, args: Value) -> anyhow::Result<String> {
        // D-08: resolve board context at the top, before any DB access.
        let (board_ctx, board_err) = crate::tools::common::resolve_board_context_from_args(&args);
        if let Some(err) = board_err {
            return Ok(crate::tools::common::reject_with_board(
                "invalid_board",
                &format!("{}", err),
                Some(&board_ctx),
            ));
        }

        // Resolve task_id via dual-read (IRONIRONHERMES_KANBAN_TASK first, legacy fallback).
        let task_id = match args
            .get("task_id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| crate::kanban_env("TASK"))
        {
            Some(t) => t,
            None => {
                return Ok(crate::tools::common::reject_with_board(
                    "missing_task_id",
                    "task_id is required when $IRONHERMES_KANBAN_TASK is not set",
                    Some(&board_ctx),
                ));
            }
        };

        let body = match args.get("body").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => {
                return Ok(crate::tools::common::reject_with_board(
                    "missing_required_arg",
                    "body is required",
                    Some(&board_ctx),
                ));
            }
        };

        let author = std::env::var("HERMES_PROFILE").unwrap_or_else(|_| "unknown".into());

        // Check for worker-mode claim lock (D-41). Dual-read via kanban_env.
        let claim_lock_env = crate::kanban_env("CLAIM_LOCK");

        if let Some(claim_lock) = claim_lock_env {
            // Worker mode — gate the write on claim validity (D-41).
            // Open per-board store.
            // Phase 36.3.7.13 D-A2: env wins; slug is fallback hint.
            let mut store = KanbanStore::open_from_env_or_board(Some(&board_ctx.slug))
                .map_err(|e| anyhow::anyhow!("open board '{}': {}", board_ctx.slug, e))?;

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
                crate::tools::common::ok_with_board(
                    json!({
                        "comment_id": comment_id,
                        "task_id": task_id,
                    }),
                    &board_ctx,
                )
            } else {
                crate::tools::common::ok_with_board(
                    json!({
                        "status": "no-op",
                        "reason": "claim_expired",
                    }),
                    &board_ctx,
                )
            }
        } else {
            // Orchestrator mode or unclaimed worker — direct insert.
            // Phase 36.3.7.13 D-A2: env wins; slug is fallback hint.
            let mut store = KanbanStore::open_from_env_or_board(Some(&board_ctx.slug))
                .map_err(|e| anyhow::anyhow!("open board '{}': {}", board_ctx.slug, e))?;
            let comment = store.add_comment(&task_id, &author, &body)?;
            crate::tools::common::ok_with_board(
                json!({
                    "comment_id": comment.id,
                    "task_id": comment.task_id,
                }),
                &board_ctx,
            )
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
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var("IRONHERMES_KANBAN_TASK");
        }
        let store = make_store();
        let tool = KanbanCommentTool::new(store.clone(), false);
        assert!(!tool.is_available());

        unsafe {
            std::env::set_var("IRONHERMES_KANBAN_TASK", "t_test");
        }
        let tool2 = KanbanCommentTool::new(store.clone(), false);
        assert!(tool2.is_available());
        unsafe {
            std::env::remove_var("IRONHERMES_KANBAN_TASK");
        }

        let tool3 = KanbanCommentTool::new(store, true);
        assert!(tool3.is_available());
    }

    #[test]
    fn schema_contains_board_property() {
        let store = make_store();
        let tool = KanbanCommentTool::new(store, true);
        let schema_str = serde_json::to_string(&tool.schema()).unwrap();
        assert!(
            schema_str.contains("\"board\""),
            "schema missing board property: {schema_str}"
        );
    }
}

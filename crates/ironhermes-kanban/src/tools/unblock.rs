//! `kanban_unblock` — move a blocked task back to ready, with handler-side
//! status-precondition gate (Phase 36.3.7.6 BUG-36.3.7.6-03).
//!
//! Per D-unblock-status-precondition: this LLM-tool handler MUST gate on
//! `task.status == "blocked"` BEFORE calling `store.unblock_task`. The
//! underlying `store.unblock_task` (store.rs:881) is operator-trusted (called
//! from `cmd_unblock`) and remains UNCHANGED in this phase — operators can
//! intentionally rescue a stuck `done`/`archived` task. The LLM-tool surface
//! is NOT operator-trusted; workers should never silently move a `done` task
//! back to `ready`.
//!
//! Per Q6 / CONTEXT.md::<notes>, there is no orchestrator-only gate on this
//! tool: the status precondition is the structural safeguard. A worker calling
//! unblock on its own currently-`running` task hits the wrong-status rejection;
//! a worker calling unblock on a foreign `blocked` task is a feature.
//!
//! # Multi-board (Plan 07, D-08)
//!
//! Accepts an optional `board` parameter. Resolves the board context at the top
//! of `execute()` before any DB access. Injects `board` + `board_source` into
//! every success and rejection envelope (T-5 mitigation).

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use ironhermes_tools::Tool;
use serde_json::{Value, json};
use tokio::sync::Mutex as TokioMutex;

use crate::error::KanbanError;
use crate::store::KanbanStore;

/// LLM tool: move a blocked task back to ready (handler-side precondition).
pub struct KanbanUnblockTool {
    #[allow(dead_code)]
    store: Arc<TokioMutex<KanbanStore>>,
    explicit_enable: bool,
}

impl KanbanUnblockTool {
    pub fn new(store: Arc<TokioMutex<KanbanStore>>, explicit_enable: bool) -> Self {
        Self {
            store,
            explicit_enable,
        }
    }
}

#[async_trait]
impl Tool for KanbanUnblockTool {
    fn name(&self) -> &str {
        "kanban_unblock"
    }

    fn toolset(&self) -> &str {
        "kanban"
    }

    fn description(&self) -> &str {
        "(Orchestrators) move a blocked task back to ready. Fails closed if the task is \
         not currently in `blocked` status — workers should not silently revive \
         `done`/`running` tasks via the LLM-tool surface."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "kanban_unblock",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "Task ID to unblock. Omit to use $HERMES_KANBAN_TASK."
                    },
                    "board": {
                        "type": "string",
                        "description": "Board slug to target. Omit to use HERMES_KANBAN_BOARD env / current file / 'default' (4-tier resolution)."
                    }
                },
                "required": []
            }),
        )
    }

    fn is_available(&self) -> bool {
        std::env::var("HERMES_KANBAN_TASK").is_ok() || self.explicit_enable
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

        // Resolve task_id from arg, then from env.
        let task_id = args
            .get("task_id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| std::env::var("HERMES_KANBAN_TASK").ok());

        let task_id = match task_id {
            Some(t) => t,
            None => {
                return Ok(crate::tools::common::reject_with_board(
                    "missing_task_id",
                    "task_id is required when $HERMES_KANBAN_TASK is not set",
                    Some(&board_ctx),
                ));
            }
        };

        // Open per-board store (D-08).
        // Phase 36.3.7.13 D-A2: env wins; slug is fallback hint.
        let mut store = KanbanStore::open_from_env_or_board(Some(&board_ctx.slug))
            .map_err(|e| anyhow::anyhow!("open board '{}': {}", board_ctx.slug, e))?;

        // Read current status. TaskNotFound surfaces as a structured rejection.
        let task = match store.get_task(&task_id) {
            Ok(t) => t,
            Err(KanbanError::TaskNotFound(_)) => {
                return Ok(crate::tools::common::reject_with_board(
                    "task_not_found",
                    &format!("task '{}' not found", task_id),
                    Some(&board_ctx),
                ));
            }
            Err(other) => return Err(other.into()),
        };

        // D-unblock-status-precondition: status MUST be exactly "blocked".
        if task.status != "blocked" {
            return crate::tools::common::reject_value_with_board(
                json!({
                    "status": "rejected",
                    "reason": "invalid_status",
                    "task_id": task_id,
                    "current": task.status,
                    "expected": "blocked",
                }),
                Some(&board_ctx),
            );
        }

        // Status is `blocked` — delegate to the unchanged store method.
        match store.unblock_task(&task_id) {
            Ok(()) => crate::tools::common::ok_with_board(
                json!({
                    "status": "ok",
                    "task_id": task_id,
                }),
                &board_ctx,
            ),
            Err(other) => Err(other.into()),
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
            std::env::remove_var("HERMES_KANBAN_TASK");
        }
        let store = make_store();
        let tool = KanbanUnblockTool::new(store.clone(), false);
        assert!(!tool.is_available());

        unsafe {
            std::env::set_var("HERMES_KANBAN_TASK", "t_test");
        }
        let tool2 = KanbanUnblockTool::new(store.clone(), false);
        assert!(tool2.is_available());
        unsafe {
            std::env::remove_var("HERMES_KANBAN_TASK");
        }

        let tool3 = KanbanUnblockTool::new(store, true);
        assert!(tool3.is_available());
    }

    #[test]
    fn schema_contains_board_property() {
        let store = make_store();
        let tool = KanbanUnblockTool::new(store, true);
        let schema_str = serde_json::to_string(&tool.schema()).unwrap();
        assert!(
            schema_str.contains("\"board\""),
            "schema missing board property: {schema_str}"
        );
    }
}

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
        // Resolve task_id from arg, then from env.
        let task_id = args
            .get("task_id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| std::env::var("HERMES_KANBAN_TASK").ok());

        let task_id = match task_id {
            Some(t) => t,
            None => {
                return Ok(serde_json::to_string(&json!({
                    "status": "rejected",
                    "reason": "missing_task_id",
                }))?);
            }
        };

        let mut store = self.store.lock().await;

        // Read current status. TaskNotFound surfaces as a structured rejection.
        let task = match store.get_task(&task_id) {
            Ok(t) => t,
            Err(KanbanError::TaskNotFound(_)) => {
                return Ok(serde_json::to_string(&json!({
                    "status": "rejected",
                    "reason": "task_not_found",
                    "task_id": task_id,
                }))?);
            }
            Err(other) => return Err(other.into()),
        };

        // D-unblock-status-precondition: status MUST be exactly "blocked".
        if task.status != "blocked" {
            return Ok(serde_json::to_string(&json!({
                "status": "rejected",
                "reason": "invalid_status",
                "task_id": task_id,
                "current": task.status,
                "expected": "blocked",
            }))?);
        }

        // Status is `blocked` — delegate to the unchanged store method.
        match store.unblock_task(&task_id) {
            Ok(()) => Ok(serde_json::to_string(&json!({
                "status": "ok",
                "task_id": task_id,
            }))?),
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
        unsafe { std::env::remove_var("HERMES_KANBAN_TASK"); }
        let store = make_store();
        let tool = KanbanUnblockTool::new(store.clone(), false);
        assert!(!tool.is_available());

        unsafe { std::env::set_var("HERMES_KANBAN_TASK", "t_test"); }
        let tool2 = KanbanUnblockTool::new(store.clone(), false);
        assert!(tool2.is_available());
        unsafe { std::env::remove_var("HERMES_KANBAN_TASK"); }

        let tool3 = KanbanUnblockTool::new(store, true);
        assert!(tool3.is_available());
    }
}

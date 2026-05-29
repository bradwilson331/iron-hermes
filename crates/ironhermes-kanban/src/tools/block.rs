//! `kanban_block` — protocol terminator: block a task with a reason (D-23).
//!
//! Shares the `expected_run_id` gate with `kanban_complete`: stale-run writes
//! are rejected with structured JSON so the LLM can read and handle.
//!
//! `reason` strings starting with `"review-required:"` are doc-flagged (D-23);
//! the dashboard treatment deferred to a follow-up phase. This tool detects
//! the prefix and emits an advisory tracing log.

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use ironhermes_tools::Tool;
use serde_json::{Value, json};
use tokio::sync::Mutex as TokioMutex;

use crate::error::KanbanError;
use crate::store::KanbanStore;

/// LLM tool: block a kanban task (protocol terminator).
pub struct KanbanBlockTool {
    store: Arc<TokioMutex<KanbanStore>>,
    explicit_enable: bool,
}

impl KanbanBlockTool {
    pub fn new(store: Arc<TokioMutex<KanbanStore>>, explicit_enable: bool) -> Self {
        Self {
            store,
            explicit_enable,
        }
    }
}

#[async_trait]
impl Tool for KanbanBlockTool {
    fn name(&self) -> &str {
        "kanban_block"
    }

    fn toolset(&self) -> &str {
        "kanban"
    }

    fn description(&self) -> &str {
        "Block a Kanban task with a reason. Use 'review-required: <detail>' as the reason \
         prefix when human review is needed (D-23 convention). Validates expected_run_id \
         to reject stale-run writes."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "kanban_block",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "Task ID to block. Omit to use $HERMES_KANBAN_TASK."
                    },
                    "reason": {
                        "type": "string",
                        "description": "Reason for blocking. Use 'review-required: <detail>' \
                                        prefix to flag for human review."
                    },
                    "expected_run_id": {
                        "type": "string",
                        "description": "Run ID that must still be the active run. Defaults to $HERMES_KANBAN_RUN_ID."
                    }
                },
                "required": ["reason"]
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

        let reason = args
            .get("reason")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("reason is required"))?
            .to_string();

        // expected_run_id: from arg, then from env (D-22 defense-in-depth).
        let expected_run_id = args
            .get("expected_run_id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| std::env::var("HERMES_KANBAN_RUN_ID").ok());

        // D-23: advisory log for review-required: prefix.
        if reason.starts_with("review-required:") {
            tracing::info!(
                task_id = %task_id,
                reason = %reason,
                "kanban_block: review-required block — flagged for human review (D-23)"
            );
        }

        let mut store = self.store.lock().await;

        match store.block_task(&task_id, &reason, expected_run_id.as_deref()) {
            Ok(()) => Ok(serde_json::to_string_pretty(&json!({
                "status": "ok",
                "task_id": task_id,
            }))?),

            Err(KanbanError::StaleRunId { expected, actual }) => {
                Ok(serde_json::to_string_pretty(&json!({
                    "status": "rejected",
                    "reason": "stale_run_id",
                    "task_id": task_id,
                    "expected": expected,
                    "actual": actual,
                }))?)
            }

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
        let tool = KanbanBlockTool::new(store.clone(), false);
        assert!(!tool.is_available());

        unsafe { std::env::set_var("HERMES_KANBAN_TASK", "t_test"); }
        let tool2 = KanbanBlockTool::new(store.clone(), false);
        assert!(tool2.is_available());
        unsafe { std::env::remove_var("HERMES_KANBAN_TASK"); }

        let tool3 = KanbanBlockTool::new(store, true);
        assert!(tool3.is_available());
    }
}

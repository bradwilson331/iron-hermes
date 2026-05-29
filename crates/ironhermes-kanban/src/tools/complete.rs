//! `kanban_complete` — protocol terminator: mark a task done (D-22).
//!
//! Gates:
//! (a) `expected_run_id` mismatch → structured `{"status":"rejected","reason":"stale_run_id"}`
//!     — returned as Ok so the LLM can read the rejection and decide what to do.
//! (b) `created_cards=[...]` with phantom ids or wrong-profile ids → structured
//!     `{"status":"rejected","reason":"created_cards"}` + permanent `completion_rejected`
//!     event.
//! (c) Free-form prose scan for unresolved `t_<hex>` refs → advisory
//!     `hallucinated_ref` event (non-blocking, handled by store).
//!
//! `expected_run_id` defaults to `$HERMES_KANBAN_RUN_ID` env when the caller
//! omits it — defense-in-depth so workers don't have to thread it explicitly.

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use ironhermes_tools::Tool;
use serde_json::{Value, json};
use tokio::sync::Mutex as TokioMutex;

use crate::error::KanbanError;
use crate::store::KanbanStore;

/// LLM tool: complete a kanban task (protocol terminator).
pub struct KanbanCompleteTool {
    store: Arc<TokioMutex<KanbanStore>>,
    explicit_enable: bool,
}

impl KanbanCompleteTool {
    pub fn new(store: Arc<TokioMutex<KanbanStore>>, explicit_enable: bool) -> Self {
        Self {
            store,
            explicit_enable,
        }
    }
}

#[async_trait]
impl Tool for KanbanCompleteTool {
    fn name(&self) -> &str {
        "kanban_complete"
    }

    fn toolset(&self) -> &str {
        "kanban"
    }

    fn description(&self) -> &str {
        "Mark a Kanban task as done. Requires at least one of `summary` or `result`. \
         Validates expected_run_id (stale-run rejection) and created_cards (phantom-id / \
         wrong-profile rejection). Both rejection types are returned as structured JSON \
         so the LLM can handle them without crashing the tool call."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "kanban_complete",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "Task ID to complete. Omit to use $HERMES_KANBAN_TASK."
                    },
                    "summary": {
                        "type": "string",
                        "description": "Human-readable summary of what was accomplished."
                    },
                    "result": {
                        "type": "string",
                        "description": "Structured result string (machine-readable output)."
                    },
                    "metadata": {
                        "type": "object",
                        "description": "Free-form JSON metadata dict for the completed run."
                    },
                    "expected_run_id": {
                        "type": "string",
                        "description": "Run ID that must still be the active run. Defaults to $HERMES_KANBAN_RUN_ID."
                    },
                    "created_cards": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Task IDs created by this worker during this run. Each must exist and have created_by matching $HERMES_PROFILE."
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
        // Resolve task_id.
        let task_id = args
            .get("task_id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| std::env::var("HERMES_KANBAN_TASK").ok())
            .ok_or_else(|| {
                anyhow::anyhow!("task_id required when HERMES_KANBAN_TASK is not set")
            })?;

        // Resolve current_profile.
        let current_profile =
            std::env::var("HERMES_PROFILE").unwrap_or_else(|_| "unknown".into());

        // expected_run_id: from arg, then from HERMES_KANBAN_RUN_ID env (D-22 defense-in-depth).
        let expected_run_id = args
            .get("expected_run_id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| std::env::var("HERMES_KANBAN_RUN_ID").ok());

        let summary = args
            .get("summary")
            .and_then(|v| v.as_str())
            .map(String::from);
        let result = args
            .get("result")
            .and_then(|v| v.as_str())
            .map(String::from);
        let metadata: Option<Value> = args.get("metadata").cloned();

        let created_cards: Option<Vec<String>> = args
            .get("created_cards")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            });

        let mut store = self.store.lock().await;

        match store.complete_task(
            &task_id,
            summary.as_deref(),
            metadata.as_ref(),
            result.as_deref(),
            expected_run_id.as_deref(),
            created_cards.as_deref(),
            &current_profile,
        ) {
            Ok(()) => Ok(serde_json::to_string_pretty(&json!({
                "status": "ok",
                "task_id": task_id,
            }))?),

            Err(KanbanError::StaleRunId { expected, actual }) => {
                // Structured rejection — return Ok so the LLM can read and decide.
                Ok(serde_json::to_string_pretty(&json!({
                    "status": "rejected",
                    "reason": "stale_run_id",
                    "task_id": task_id,
                    "expected": expected,
                    "actual": actual,
                }))?)
            }

            Err(KanbanError::CreatedCardsRejected {
                phantom,
                wrong_profile,
            }) => {
                // Structured rejection — permanent completion_rejected event already written.
                Ok(serde_json::to_string_pretty(&json!({
                    "status": "rejected",
                    "reason": "created_cards",
                    "task_id": task_id,
                    "phantom_ids": phantom,
                    "wrong_profile_ids": wrong_profile,
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
        let tool = KanbanCompleteTool::new(store.clone(), false);
        assert!(!tool.is_available());

        unsafe { std::env::set_var("HERMES_KANBAN_TASK", "t_test"); }
        let tool2 = KanbanCompleteTool::new(store.clone(), false);
        assert!(tool2.is_available());
        unsafe { std::env::remove_var("HERMES_KANBAN_TASK"); }

        let tool3 = KanbanCompleteTool::new(store, true);
        assert!(tool3.is_available());
    }
}

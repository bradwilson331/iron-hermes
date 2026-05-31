//! `kanban_heartbeat` — signal worker liveness during long operations (Phase 36.3.7.6 BUG-36.3.7.6-01).
//!
//! Appends a `KanbanEventKind::Heartbeat` row to `task_events` via
//! `store.append_event(task_id, run_id, KanbanEventKind::Heartbeat, payload)`.
//! Workers running > 1 hour should call this at least hourly so the dispatcher's
//! staleness reclaim (reference.md §869) does not consider the run stranded.
//!
//! Per D-heartbeat-impl, this is an append-only event row — there is no
//! `tasks.last_heartbeat_at` column. The dispatcher's existing staleness reader
//! is the single consumer of these rows; keeping the source of truth on
//! `task_events` keeps producer + consumer aligned.
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
use crate::events::KanbanEventKind;
use crate::store::KanbanStore;

/// LLM tool: append a `heartbeat` event row for a task.
pub struct KanbanHeartbeatTool {
    #[allow(dead_code)]
    store: Arc<TokioMutex<KanbanStore>>,
    explicit_enable: bool,
}

impl KanbanHeartbeatTool {
    pub fn new(store: Arc<TokioMutex<KanbanStore>>, explicit_enable: bool) -> Self {
        Self {
            store,
            explicit_enable,
        }
    }
}

#[async_trait]
impl Tool for KanbanHeartbeatTool {
    fn name(&self) -> &str {
        "kanban_heartbeat"
    }

    fn toolset(&self) -> &str {
        "kanban"
    }

    fn description(&self) -> &str {
        "Signal liveness during long operations. Appends a heartbeat event row to task_events. \
         Workers running > 1 hour should call this at least hourly to avoid dispatcher staleness reclaim."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "kanban_heartbeat",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "Task ID to heartbeat. Omit to use $HERMES_KANBAN_TASK."
                    },
                    "note": {
                        "type": "string",
                        "description": "Optional free-form progress note."
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

    /// Available when `HERMES_KANBAN_TASK` is set (worker mode) or `explicit_enable` is true
    /// (orchestrator mode) — D-20.
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

        // Resolve task_id: from arg, then from HERMES_KANBAN_TASK env.
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

        // Resolve run_id: optional, from HERMES_KANBAN_RUN_ID env. `None` is fine —
        // the column allows NULL.
        let run_id = std::env::var("HERMES_KANBAN_RUN_ID").ok();

        // Build payload from optional note arg.
        let payload: Option<Value> = args.get("note").map(|n| json!({ "note": n }));

        // Open per-board store (D-08).
        let mut store = KanbanStore::open_for_board(&board_ctx.slug)
            .map_err(|e| anyhow::anyhow!("open board '{}': {}", board_ctx.slug, e))?;

        match store.append_event(
            &task_id,
            run_id.as_deref(),
            KanbanEventKind::Heartbeat,
            payload.as_ref(),
        ) {
            Ok(event_id) => crate::tools::common::ok_with_board(json!({
                "status": "ok",
                "task_id": task_id,
                "event_id": event_id,
            }), &board_ctx),
            Err(KanbanError::TaskNotFound(id)) => {
                Ok(crate::tools::common::reject_with_board(
                    "task_not_found",
                    &format!("task '{}' not found", id),
                    Some(&board_ctx),
                ))
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
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { std::env::remove_var("HERMES_KANBAN_TASK"); }
        let store = make_store();
        let tool = KanbanHeartbeatTool::new(store.clone(), false);
        assert!(!tool.is_available());

        unsafe { std::env::set_var("HERMES_KANBAN_TASK", "t_test"); }
        let tool2 = KanbanHeartbeatTool::new(store.clone(), false);
        assert!(tool2.is_available());
        unsafe { std::env::remove_var("HERMES_KANBAN_TASK"); }

        let tool3 = KanbanHeartbeatTool::new(store, true);
        assert!(tool3.is_available());
    }

    #[test]
    fn schema_contains_board_property() {
        let store = make_store();
        let tool = KanbanHeartbeatTool::new(store, true);
        let schema_str = serde_json::to_string(&tool.schema()).unwrap();
        assert!(schema_str.contains("\"board\""), "schema missing board property: {schema_str}");
    }
}

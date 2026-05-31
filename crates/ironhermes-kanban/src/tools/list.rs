//! `kanban_list` — list tasks on the board with optional filters (D-25).
//!
//! Returns a compact JSON array of task summaries (id, title, status, assignee,
//! tenant, priority, created_at) — deliberately omits body/comments to keep
//! the response token-efficient per D-25.
//!
//! # Multi-board (Plan 07, D-08)
//!
//! Accepts an optional `board` parameter. Resolves the board context at the top
//! of `execute()` before any DB access. Injects `board` + `board_source` into
//! the success envelope (T-5 mitigation).

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use ironhermes_tools::Tool;
use serde_json::{Value, json};
use tokio::sync::Mutex as TokioMutex;

use crate::store::{KanbanStore, ListFilters};

/// LLM tool: list tasks with optional filters.
pub struct KanbanListTool {
    #[allow(dead_code)]
    store: Arc<TokioMutex<KanbanStore>>,
    explicit_enable: bool,
}

impl KanbanListTool {
    pub fn new(store: Arc<TokioMutex<KanbanStore>>, explicit_enable: bool) -> Self {
        Self {
            store,
            explicit_enable,
        }
    }
}

#[async_trait]
impl Tool for KanbanListTool {
    fn name(&self) -> &str {
        "kanban_list"
    }

    fn toolset(&self) -> &str {
        "kanban"
    }

    fn description(&self) -> &str {
        "List Kanban tasks. Returns compact summaries (id, title, status, assignee, tenant, \
         priority, created_at). Filters: assignee, status, tenant, archived, limit."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "kanban_list",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "assignee": {
                        "type": "string",
                        "description": "Filter by profile slug assigned to the task."
                    },
                    "status": {
                        "type": "string",
                        "description": "Filter by status: triage|todo|ready|running|blocked|done|archived."
                    },
                    "tenant": {
                        "type": "string",
                        "description": "Filter by tenant name."
                    },
                    "archived": {
                        "type": "boolean",
                        "description": "When true, include archived tasks (default false).",
                        "default": false
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of tasks to return (default 50).",
                        "default": 50
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

        let filters = ListFilters {
            assignee: args
                .get("assignee")
                .and_then(|v| v.as_str())
                .map(String::from),
            status: args
                .get("status")
                .and_then(|v| v.as_str())
                .map(String::from),
            tenant: args
                .get("tenant")
                .and_then(|v| v.as_str())
                .map(String::from),
            archived: args
                .get("archived")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            limit: args
                .get("limit")
                .and_then(|v| v.as_i64())
                .or(Some(50)),
        };

        // Open the per-board store (D-08: always use the resolved board's DB).
        let store = KanbanStore::open_for_board(&board_ctx.slug)
            .map_err(|e| anyhow::anyhow!("open board '{}': {}", board_ctx.slug, e))?;
        let tasks = store.list_tasks(filters)?;

        // Return compact summaries — omit body/comments per D-25.
        let summaries: Vec<Value> = tasks
            .into_iter()
            .map(|t| {
                json!({
                    "id": t.id,
                    "title": t.title,
                    "status": t.status,
                    "assignee": t.assignee,
                    "tenant": t.tenant,
                    "priority": t.priority,
                    "created_at": t.created_at,
                })
            })
            .collect();

        // Wrap in an object so inject_board_fields can add board/board_source at the same level.
        crate::tools::common::ok_with_board(json!({"tasks": summaries}), &board_ctx)
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
        let tool = KanbanListTool::new(store.clone(), false);
        assert!(!tool.is_available());

        unsafe { std::env::set_var("HERMES_KANBAN_TASK", "t_test"); }
        let tool2 = KanbanListTool::new(store.clone(), false);
        assert!(tool2.is_available());
        unsafe { std::env::remove_var("HERMES_KANBAN_TASK"); }

        let tool3 = KanbanListTool::new(store, true);
        assert!(tool3.is_available());
    }

    #[test]
    fn schema_contains_board_property() {
        let store = make_store();
        let tool = KanbanListTool::new(store, true);
        let schema_str = serde_json::to_string(&tool.schema()).unwrap();
        assert!(schema_str.contains("\"board\""), "schema missing board property: {schema_str}");
    }
}

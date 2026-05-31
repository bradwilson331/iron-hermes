//! `kanban_specify` — LLM-driven triage specifier (Phase 36.3.7.10).
//!
//! Rewrites a triage task's body with a structured spec (goal/approach/
//! acceptance/risk/skills) + promotes `triage → todo` via the
//! [`crate::decomposer::specify_triage_task`] kernel. No child fan-out.
//!
//! # v1 Quiescence
//!
//! In v1 the LLM tool surface is **observable but quiescent**: the tool is
//! registered in the schema (orchestrators can see `kanban_specify`) but its
//! `execute()` method returns a structured `no_aux_client` rejection envelope
//! directing the orchestrator to use the `hermes kanban specify <id>` CLI
//! verb instead. The runtime DecomposeFn wiring is deferred to a follow-up
//! phase; this preserves the crate-isolation fence (no provider-client dep in
//! `ironhermes-kanban`).
//!
//! # Multi-board (D-08)
//!
//! Accepts an optional `board` parameter. Resolves the board context at the top
//! of `execute()` before any DB access. Injects `board` + `board_source` into
//! every rejection envelope (T-5 mitigation).

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use ironhermes_tools::Tool;
use serde_json::{Value, json};
use tokio::sync::Mutex as TokioMutex;

use crate::store::KanbanStore;

/// LLM tool: rewrite triage task body with structured spec + promote to todo (kanban_specify).
pub struct KanbanSpecifyTool {
    #[allow(dead_code)]
    store: Arc<TokioMutex<KanbanStore>>,
    explicit_enable: bool,
}

impl KanbanSpecifyTool {
    pub fn new(store: Arc<TokioMutex<KanbanStore>>, explicit_enable: bool) -> Self {
        Self {
            store,
            explicit_enable,
        }
    }
}

#[async_trait]
impl Tool for KanbanSpecifyTool {
    fn name(&self) -> &str {
        "kanban_specify"
    }

    fn toolset(&self) -> &str {
        "kanban"
    }

    fn description(&self) -> &str {
        "Run the LLM specifier on a triage task. Rewrites the task body with a structured spec \
         (goal/approach/acceptance/risk/skills) + promotes triage→todo. No fan-out. Returns \
         {ok, task_id, new_title}. v1: returns `no_aux_client` if the specifier is not wired in \
         this runtime — use `hermes kanban specify <id>` CLI verb instead."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "kanban_specify",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "Triage task ID (e.g. t_abc123). Required."
                    },
                    "board": {
                        "type": "string",
                        "description": "Board slug (overrides 4-tier resolution). Optional."
                    }
                },
                "required": ["task_id"]
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

        // Validate task_id is present and non-empty.
        let _task_id = match args["task_id"].as_str() {
            Some(s) if !s.is_empty() => s,
            _ => {
                return Ok(crate::tools::common::reject_with_board(
                    "missing_task_id",
                    "task_id is required",
                    Some(&board_ctx),
                ));
            }
        };

        // v1: DecomposeFn not wired in the LLM tool runtime.
        // Orchestrators should use `hermes kanban specify <id>` CLI verb instead.
        // The runtime injection mechanism is queued for a follow-up phase (crate-isolation fence).
        Ok(crate::tools::common::reject_with_board(
            "no_aux_client",
            "Specifier not wired in this LLM tool runtime — use `hermes kanban specify <id>` CLI verb instead",
            Some(&board_ctx),
        ))
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
    fn kanban_specify_name_and_toolset() {
        let store = make_store();
        let tool = KanbanSpecifyTool::new(store, true);
        assert_eq!(tool.name(), "kanban_specify");
        assert_eq!(tool.toolset(), "kanban");
    }

    #[test]
    fn kanban_specify_is_available_gate() {
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        unsafe { std::env::remove_var("HERMES_KANBAN_TASK"); }
        let store = make_store();
        let tool = KanbanSpecifyTool::new(store.clone(), false);
        assert!(!tool.is_available(), "should not be available without env or explicit_enable");

        unsafe { std::env::set_var("HERMES_KANBAN_TASK", "t_test"); }
        let tool2 = KanbanSpecifyTool::new(store.clone(), false);
        assert!(tool2.is_available(), "should be available when HERMES_KANBAN_TASK is set");
        unsafe { std::env::remove_var("HERMES_KANBAN_TASK"); }

        let tool3 = KanbanSpecifyTool::new(store, true);
        assert!(tool3.is_available(), "should be available with explicit_enable=true");
    }

    #[test]
    fn schema_contains_task_id_and_board_properties() {
        let store = make_store();
        let tool = KanbanSpecifyTool::new(store, true);
        let schema_str = serde_json::to_string(&tool.schema()).unwrap();
        assert!(
            schema_str.contains("\"task_id\""),
            "schema missing task_id property: {schema_str}"
        );
        assert!(
            schema_str.contains("\"board\""),
            "schema missing board property: {schema_str}"
        );
    }
}

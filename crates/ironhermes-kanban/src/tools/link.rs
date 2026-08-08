//! `kanban_link` — add a `parent_id` → `child_id` dependency edge with cycle
//! detection (Phase 36.3.7.6 BUG-36.3.7.6-02).
//!
//! Orchestrator-shaped tool. Both `parent_id` and `child_id` are required (no
//! env defaults). Calls `store.insert_link_checked` (NEW sibling of
//! `store.insert_link`) which runs a `WITH RECURSIVE` descendant-walk cycle
//! check + the existing tenant gate inside a `BEGIN IMMEDIATE` transaction
//! (D-link-cycle-detection). FK-style phantom-id rejection is mapped to
//! `task_not_found`; cycle rejection to `link_cycle` (D-link-fk-enforcement).
//!
//! Per D-link-cycle-detection, the existing `store.insert_link` stays unchanged
//! for the legacy `kanban_create::parents` path (cycles impossible by
//! construction at create time).
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

/// LLM tool: add a parent → child dependency link with cycle detection.
pub struct KanbanLinkTool {
    #[allow(dead_code)]
    store: Arc<TokioMutex<KanbanStore>>,
    explicit_enable: bool,
}

impl KanbanLinkTool {
    pub fn new(store: Arc<TokioMutex<KanbanStore>>, explicit_enable: bool) -> Self {
        Self {
            store,
            explicit_enable,
        }
    }
}

#[async_trait]
impl Tool for KanbanLinkTool {
    fn name(&self) -> &str {
        "kanban_link"
    }

    fn toolset(&self) -> &str {
        "kanban"
    }

    fn description(&self) -> &str {
        "(Orchestrators) add a parent_id → child_id dependency edge after the fact. \
         Rejects cycles (descendant-walk via WITH RECURSIVE CTE) and cross-tenant links. \
         For create-time parents, use parents=[...] on kanban_create instead."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "kanban_link",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "parent_id": {
                        "type": "string",
                        "description": "Parent task ID."
                    },
                    "child_id": {
                        "type": "string",
                        "description": "Child task ID."
                    },
                    "board": {
                        "type": "string",
                        "description": "Board slug to target. Omit to use IRONHERMES_KANBAN_BOARD env / current file / 'default' (4-tier resolution)."
                    }
                },
                "required": ["parent_id", "child_id"]
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

        let parent_id = match args.get("parent_id").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => {
                return Ok(crate::tools::common::reject_with_board(
                    "missing_required_arg",
                    "parent_id is required",
                    Some(&board_ctx),
                ));
            }
        };
        let child_id = match args.get("child_id").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => {
                return Ok(crate::tools::common::reject_with_board(
                    "missing_required_arg",
                    "child_id is required",
                    Some(&board_ctx),
                ));
            }
        };

        // Open per-board store (D-08).
        // Phase 36.3.7.13 D-A2: env wins; slug is fallback hint.
        let mut store = KanbanStore::open_from_env_or_board(Some(&board_ctx.slug))
            .map_err(|e| anyhow::anyhow!("open board '{}': {}", board_ctx.slug, e))?;

        match store.insert_link_checked(&parent_id, &child_id) {
            Ok(()) => crate::tools::common::ok_with_board(
                json!({
                    "status": "ok",
                    "parent_id": parent_id,
                    "child_id": child_id,
                }),
                &board_ctx,
            ),
            Err(KanbanError::LinkCycle {
                parent_id: p,
                child_id: c,
            }) => crate::tools::common::reject_value_with_board(
                json!({
                    "status": "rejected",
                    "reason": "link_cycle",
                    "parent_id": p,
                    "child_id": c,
                }),
                Some(&board_ctx),
            ),
            Err(KanbanError::TaskNotFound(id)) => crate::tools::common::reject_value_with_board(
                json!({
                    "status": "rejected",
                    "reason": "task_not_found",
                    "id": id,
                }),
                Some(&board_ctx),
            ),
            Err(KanbanError::TenantMismatch { parent, child }) => {
                crate::tools::common::reject_value_with_board(
                    json!({
                        "status": "rejected",
                        "reason": "tenant_mismatch",
                        "parent": parent,
                        "child": child,
                    }),
                    Some(&board_ctx),
                )
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
        unsafe {
            std::env::remove_var("IRONHERMES_KANBAN_TASK");
        }
        let store = make_store();
        let tool = KanbanLinkTool::new(store.clone(), false);
        assert!(!tool.is_available());

        unsafe {
            std::env::set_var("IRONHERMES_KANBAN_TASK", "t_test");
        }
        let tool2 = KanbanLinkTool::new(store.clone(), false);
        assert!(tool2.is_available());
        unsafe {
            std::env::remove_var("IRONHERMES_KANBAN_TASK");
        }

        let tool3 = KanbanLinkTool::new(store, true);
        assert!(tool3.is_available());
    }

    #[test]
    fn schema_contains_board_property() {
        let store = make_store();
        let tool = KanbanLinkTool::new(store, true);
        let schema_str = serde_json::to_string(&tool.schema()).unwrap();
        assert!(
            schema_str.contains("\"board\""),
            "schema missing board property: {schema_str}"
        );
    }
}

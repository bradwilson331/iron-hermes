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
                    }
                },
                "required": ["parent_id", "child_id"]
            }),
        )
    }

    fn is_available(&self) -> bool {
        std::env::var("HERMES_KANBAN_TASK").is_ok() || self.explicit_enable
    }

    async fn execute(&self, args: Value) -> anyhow::Result<String> {
        let parent_id = match args.get("parent_id").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => {
                return Ok(serde_json::to_string(&json!({
                    "status": "rejected",
                    "reason": "missing_required_arg",
                    "arg": "parent_id",
                }))?);
            }
        };
        let child_id = match args.get("child_id").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => {
                return Ok(serde_json::to_string(&json!({
                    "status": "rejected",
                    "reason": "missing_required_arg",
                    "arg": "child_id",
                }))?);
            }
        };

        let mut store = self.store.lock().await;

        match store.insert_link_checked(&parent_id, &child_id) {
            Ok(()) => Ok(serde_json::to_string(&json!({
                "status": "ok",
                "parent_id": parent_id,
                "child_id": child_id,
            }))?),
            Err(KanbanError::LinkCycle {
                parent_id: p,
                child_id: c,
            }) => Ok(serde_json::to_string(&json!({
                "status": "rejected",
                "reason": "link_cycle",
                "parent_id": p,
                "child_id": c,
            }))?),
            Err(KanbanError::TaskNotFound(id)) => Ok(serde_json::to_string(&json!({
                "status": "rejected",
                "reason": "task_not_found",
                "id": id,
            }))?),
            Err(KanbanError::TenantMismatch { parent, child }) => {
                Ok(serde_json::to_string(&json!({
                    "status": "rejected",
                    "reason": "tenant_mismatch",
                    "parent": parent,
                    "child": child,
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
        let tool = KanbanLinkTool::new(store.clone(), false);
        assert!(!tool.is_available());

        unsafe { std::env::set_var("HERMES_KANBAN_TASK", "t_test"); }
        let tool2 = KanbanLinkTool::new(store.clone(), false);
        assert!(tool2.is_available());
        unsafe { std::env::remove_var("HERMES_KANBAN_TASK"); }

        let tool3 = KanbanLinkTool::new(store, true);
        assert!(tool3.is_available());
    }
}

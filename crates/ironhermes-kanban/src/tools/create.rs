//! `kanban_create` — orchestrator fanout: create a new task (D-24).
//!
//! Features:
//! - Assignee validated via `ironhermes_core::profile::validate_profile_name` (D-17).
//! - `idempotency_key` short-circuits to the existing task when already present (D-24).
//! - `parents=[...]` causes initial status = 'todo' (all parents must be done before
//!   the dispatcher promotes to 'ready').
//! - `dir:<relative>` workspace rejected at create time (D-31 confused-deputy gate).
//! - `max_runtime` accepts integer seconds OR human-readable strings ("30m", "2h", "1d").
//! - Tenant passed as-given from args — no auto-inherit per D-38 (explicit-is-better-than-implicit).
//! - `created_by` set to `$HERMES_PROFILE` env (D-22 created_cards verification basis).
//!
//! # Multi-board (Plan 07, D-08)
//!
//! Accepts an optional `board` parameter. Resolves the board context at the top
//! of `execute()` before any DB access. Creates the task in the resolved board's DB.
//! Injects `board` + `board_source` into the success envelope (T-5 mitigation).

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use ironhermes_tools::Tool;
use serde_json::{Value, json};
use tokio::sync::Mutex as TokioMutex;

use crate::store::{CreateTaskOptions, KanbanStore};

/// LLM tool: create a new kanban task.
pub struct KanbanCreateTool {
    #[allow(dead_code)]
    store: Arc<TokioMutex<KanbanStore>>,
    explicit_enable: bool,
}

impl KanbanCreateTool {
    pub fn new(store: Arc<TokioMutex<KanbanStore>>, explicit_enable: bool) -> Self {
        Self {
            store,
            explicit_enable,
        }
    }
}

/// Parse a human-readable max_runtime string OR integer (seconds).
///
/// Accepts:
/// - Integer: treated as seconds directly.
/// - Strings like "30s", "30m", "2h", "1d".
///
/// Returns `None` for invalid / unsupported formats (caller decides to error or ignore).
pub fn parse_max_runtime(v: &Value) -> Option<i64> {
    if let Some(n) = v.as_i64() {
        return Some(n);
    }
    if let Some(s) = v.as_str() {
        return parse_duration_str(s);
    }
    None
}

fn parse_duration_str(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Find where the numeric part ends.
    let split_pos = s.find(|c: char| !c.is_ascii_digit())?;
    let (num_str, suffix) = s.split_at(split_pos);
    let n: i64 = num_str.parse().ok()?;
    match suffix {
        "s" => Some(n),
        "m" => Some(n * 60),
        "h" => Some(n * 3600),
        "d" => Some(n * 86400),
        _ => None,
    }
}

#[async_trait]
impl Tool for KanbanCreateTool {
    fn name(&self) -> &str {
        "kanban_create"
    }

    fn toolset(&self) -> &str {
        "kanban"
    }

    fn description(&self) -> &str {
        "Create a new Kanban task. Returns {task_id, status}. Supports idempotency_key to \
         deduplicate: if the key already exists, returns the existing task. Parents gate \
         promotion: child starts in 'todo' until all parents are 'done'."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "kanban_create",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Task title."
                    },
                    "assignee": {
                        "type": "string",
                        "description": "Profile slug to assign the task to."
                    },
                    "body": {
                        "type": "string",
                        "description": "Full task description / instructions."
                    },
                    "parents": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Parent task IDs. Child starts in 'todo' until all parents are 'done'."
                    },
                    "tenant": {
                        "type": "string",
                        "description": "Tenant name. Not auto-inherited — pass explicitly if needed."
                    },
                    "workspace": {
                        "type": "string",
                        "description": "Workspace spec: 'scratch' (default), 'dir:/abs/path', or 'worktree'."
                    },
                    "skills": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Extra skill slugs to pass on spawn."
                    },
                    "priority": {
                        "type": "integer",
                        "description": "Task priority (default 0; higher = more urgent)."
                    },
                    "idempotency_key": {
                        "type": "string",
                        "description": "When provided, return the existing task if this key already exists."
                    },
                    "scheduled_at": {
                        "type": "number",
                        "description": "Unix epoch seconds. Dispatcher skips the task until this time."
                    },
                    "max_runtime": {
                        "description": "Max wall-clock time per attempt. Integer seconds or string: '30m', '2h', '1d'.",
                        "oneOf": [
                            { "type": "integer" },
                            { "type": "string" }
                        ]
                    },
                    "max_retries": {
                        "type": "integer",
                        "description": "Per-task retry cap (overrides global failure_limit)."
                    },
                    "triage": {
                        "type": "boolean",
                        "description": "When true, task starts in 'triage' status (needs manual specification).",
                        "default": false
                    },
                    "goal_mode": {
                        "type": "boolean",
                        "description": "Enable goal mode: worker enters in-session loop with judge LLM evaluating against title + body.",
                        "default": false
                    },
                    "goal_max_turns": {
                        "type": "integer",
                        "description": "Per-task turn budget when goal_mode=true. Budget exhaustion → kanban_block. Default 20.",
                        "default": 20
                    },
                    "board": {
                        "type": "string",
                        "description": "Board slug to target. Omit to use HERMES_KANBAN_BOARD env / current file / 'default' (4-tier resolution)."
                    }
                },
                "required": ["title", "assignee"]
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

        let title = match args.get("title").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => {
                return Ok(crate::tools::common::reject_with_board(
                    "missing_required_arg",
                    "title is required",
                    Some(&board_ctx),
                ));
            }
        };

        let assignee = match args.get("assignee").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => {
                return Ok(crate::tools::common::reject_with_board(
                    "missing_required_arg",
                    "assignee is required",
                    Some(&board_ctx),
                ));
            }
        };

        // Validate assignee (D-17 / D-20).
        ironhermes_core::profile::validate_profile_name(&assignee).map_err(|e| {
            anyhow::anyhow!(
                r#"{{"status":"rejected","reason":"invalid_assignee","detail":"{}"}}"#,
                e
            )
        })?;

        let parents: Vec<String> = args
            .get("parents")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let skills: Option<Vec<String>> = args
            .get("skills")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            });

        // Parse max_runtime (integer seconds OR human-readable string).
        let max_runtime_seconds: Option<i64> = args
            .get("max_runtime")
            .and_then(|v| parse_max_runtime(v));

        let opts = CreateTaskOptions {
            body: args
                .get("body")
                .and_then(|v| v.as_str())
                .map(String::from),
            parents,
            // Tenant passed as-given — no auto-inherit per D-38.
            tenant: args
                .get("tenant")
                .and_then(|v| v.as_str())
                .map(String::from),
            workspace: args
                .get("workspace")
                .and_then(|v| v.as_str())
                .map(String::from),
            skills,
            priority: args.get("priority").and_then(|v| v.as_i64()),
            idempotency_key: args
                .get("idempotency_key")
                .and_then(|v| v.as_str())
                .map(String::from),
            scheduled_at: args
                .get("scheduled_at")
                .and_then(|v| v.as_f64()),
            max_runtime_seconds,
            max_retries: args.get("max_retries").and_then(|v| v.as_i64()),
            triage: args
                .get("triage")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            // D-22: created_by tracks which profile created this task for created_cards gate.
            created_by: std::env::var("HERMES_PROFILE").ok(),
            // Phase 36.3.7.12 D-01: goal_mode + goal_max_turns producer fields.
            // Plan 02 will add full tool-schema entries + JSON parsing; for now
            // accept the args opportunistically so this site stays forward-compat
            // with the planned schema additions. Absent args → defaults (false/0
            // → coerced to 20 by KanbanStore::create_task per D-03).
            goal_mode: args
                .get("goal_mode")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            goal_max_turns: args
                .get("goal_max_turns")
                .and_then(|v| v.as_u64())
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0),
            // Phase 36.3.7.13 D-G1: goal_toolset from tool args → store.
            // None → NULL in DB → resolves to "restricted" at spawn (D-E1).
            goal_toolset: args
                .get("goal_toolset")
                .and_then(|v| v.as_str())
                .map(String::from),
        };

        // Open per-board store (D-08: create task in the resolved board's DB).
        // Phase 36.3.7.13 D-A2: env wins; slug is fallback hint.
        let mut store = KanbanStore::open_from_env_or_board(Some(&board_ctx.slug))
            .map_err(|e| anyhow::anyhow!("open board '{}': {}", board_ctx.slug, e))?;
        let task = store.create_task(&title, &assignee, opts)?;

        crate::tools::common::ok_with_board(json!({
            "task_id": task.id,
            "status": task.status,
        }), &board_ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_max_runtime_integer() {
        assert_eq!(parse_max_runtime(&json!(120)), Some(120));
    }

    #[test]
    fn parse_max_runtime_human() {
        assert_eq!(parse_max_runtime(&json!("30m")), Some(1800));
        assert_eq!(parse_max_runtime(&json!("2h")), Some(7200));
        assert_eq!(parse_max_runtime(&json!("1d")), Some(86400));
        assert_eq!(parse_max_runtime(&json!("30s")), Some(30));
    }

    #[test]
    fn parse_max_runtime_invalid() {
        assert_eq!(parse_max_runtime(&json!("30x")), None);
        assert_eq!(parse_max_runtime(&json!("")), None);
    }

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
        let tool = KanbanCreateTool::new(store.clone(), false);
        assert!(!tool.is_available());

        unsafe { std::env::set_var("HERMES_KANBAN_TASK", "t_test"); }
        let tool2 = KanbanCreateTool::new(store.clone(), false);
        assert!(tool2.is_available());
        unsafe { std::env::remove_var("HERMES_KANBAN_TASK"); }

        let tool3 = KanbanCreateTool::new(store, true);
        assert!(tool3.is_available());
    }

    #[test]
    fn schema_contains_board_property() {
        let store = make_store();
        let tool = KanbanCreateTool::new(store, true);
        let schema_str = serde_json::to_string(&tool.schema()).unwrap();
        assert!(schema_str.contains("\"board\""), "schema missing board property: {schema_str}");
    }
}

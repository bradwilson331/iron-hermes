//! `kanban_swarm` — orchestrator-mode atomic fan-out tool (Phase 36.3.7.7).
//!
//! Materializes a multi-card swarm graph in a single atomic transaction via
//! [`KanbanStore::create_swarm`]: root card (status=`done`, blackboard
//! holder) + N worker cards (status=`todo`, parents=[root]) + optional
//! verifier card + optional synthesizer card + optional blackboard comment.
//!
//! Graph shapes (D-topology-shapes) map directly onto the multi-agent
//! patterns table at `docs/kanban/reference.md` §740:
//!
//! | Shape | Topology                                          | Pattern         |
//! |-------|---------------------------------------------------|-----------------|
//! | 1     | `root → [w_1..w_N]`                               | P1 Fan-out      |
//! | 2     | `root → [w_1..w_N] → verifier`                    | Fan-out+verify  |
//! | 3     | `root → [w_1..w_N] → verifier → synthesizer`      | Full 4-tier (§664) |
//! | 4     | `root → [w_1..w_N] → synthesizer`                 | P3 Voting/quorum |
//!
//! Worker root-discovery (D-root-discovery): once spawned, each worker walks
//! the existing `kanban_show` `parent_handoffs` field (2 hops max — worker
//! → root, then `kanban_show root_id` to read the shared blackboard JSON
//! from `comments[0].body`). The 9-env-var worker spawn contract is
//! UNTOUCHED — no `HERMES_KANBAN_SWARM_ROOT` env var is added.
//!
//! Reference example from `docs/kanban/reference.md` §664:
//! `hermes kanban swarm "Design a multi-region failover plan" --workers
//! researcher,architect,sre --verifier reviewer --synthesizer writer`
//!
//! # Multi-board (Plan 07, D-08)
//!
//! Accepts an optional `board` parameter. Resolves the board context at the top
//! of `execute()` before any DB access. Creates the swarm graph in the resolved
//! board's DB. Injects `board` + `board_source` into every success and rejection
//! envelope (T-5 mitigation).

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use ironhermes_tools::Tool;
use serde_json::{Value, json};
use tokio::sync::Mutex as TokioMutex;

use crate::store::KanbanStore;
use crate::tools::create::parse_max_runtime;
use crate::types::{KanbanWorkerSpec, SwarmGraphSpec};

/// LLM tool: materialize a multi-card swarm graph atomically.
pub struct KanbanSwarmTool {
    #[allow(dead_code)]
    store: Arc<TokioMutex<KanbanStore>>,
    explicit_enable: bool,
}

impl KanbanSwarmTool {
    pub fn new(store: Arc<TokioMutex<KanbanStore>>, explicit_enable: bool) -> Self {
        Self {
            store,
            explicit_enable,
        }
    }
}

/// Normalize the `workers` arg from either `Array<String>` (flat form) or
/// `Array<Object>` (rich form). Mixed arrays are rejected.
///
/// Returns:
/// - `Ok(specs)` on success (non-empty),
/// - `Err(rejection_json_string)` on shape error (empty / mixed / non-array
///   / object missing required `assignee`).
fn normalize_workers(
    v: &Value,
    board_ctx: &crate::board::BoardContext,
) -> Result<Vec<KanbanWorkerSpec>, String> {
    let arr = v.as_array().ok_or_else(|| {
        crate::tools::common::reject_with_board(
            "invalid_workers_shape",
            "workers must be an array",
            Some(board_ctx),
        )
    })?;

    if arr.is_empty() {
        return Err(crate::tools::common::reject_with_board(
            "empty_workers",
            "workers array must contain at least one entry",
            Some(board_ctx),
        ));
    }

    let all_strings = arr.iter().all(|e| e.is_string());
    let all_objects = arr.iter().all(|e| e.is_object());

    if all_strings {
        Ok(arr
            .iter()
            .map(|e| KanbanWorkerSpec {
                assignee: e.as_str().unwrap_or("").to_string(),
                title: None,
                body: None,
            })
            .collect())
    } else if all_objects {
        let mut specs = Vec::with_capacity(arr.len());
        for (i, e) in arr.iter().enumerate() {
            let spec: KanbanWorkerSpec = serde_json::from_value(e.clone()).map_err(|err| {
                crate::tools::common::reject_with_board(
                    "invalid_workers_shape",
                    &format!("workers[{i}] failed to parse as object: {err}"),
                    Some(board_ctx),
                )
            })?;
            if spec.assignee.is_empty() {
                return Err(crate::tools::common::reject_with_board(
                    "invalid_workers_shape",
                    &format!("workers[{i}].assignee is required"),
                    Some(board_ctx),
                ));
            }
            specs.push(spec);
        }
        Ok(specs)
    } else {
        Err(crate::tools::common::reject_with_board(
            "invalid_workers_shape",
            "workers array must be all-strings or all-objects (not mixed)",
            Some(board_ctx),
        ))
    }
}

#[async_trait]
impl Tool for KanbanSwarmTool {
    fn name(&self) -> &str {
        "kanban_swarm"
    }

    fn toolset(&self) -> &str {
        "kanban"
    }

    fn description(&self) -> &str {
        "Create N parallel worker cards + optional verifier + optional synthesizer + \
         blackboard root, in one atomic transaction. Implements the multi-agent patterns \
         documented at docs/kanban/reference.md §740 (P1 fan-out, fan-out+verify, full \
         4-tier §664 example, P3 quorum). Workers discover the root via existing \
         kanban_show parent_handoffs walk; no new spawn-env contract."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "kanban_swarm",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "goal": {
                        "type": "string",
                        "description": "Shared goal / task description. Auto-generated worker titles use this when no per-card title is supplied."
                    },
                    "workers": {
                        "description": "Worker assignees. String array (flat — auto-title per card) or object array (rich, per-card title/body). minItems=1 for both branches.",
                        "oneOf": [
                            { "type": "array", "items": { "type": "string" }, "minItems": 1 },
                            { "type": "array",
                              "items": {
                                  "type": "object",
                                  "properties": {
                                      "assignee": { "type": "string" },
                                      "title":    { "type": "string" },
                                      "body":     { "type": "string" }
                                  },
                                  "required": ["assignee"]
                              },
                              "minItems": 1
                            }
                        ]
                    },
                    "verifier": {
                        "type": "string",
                        "description": "Optional verifier assignee. When present, verifier card's parents are all worker IDs."
                    },
                    "synthesizer": {
                        "type": "string",
                        "description": "Optional synthesizer assignee. Parents are [verifier] when verifier present, else all worker IDs."
                    },
                    "blackboard": {
                        "description": "Opaque JSON seed comment on root card. Persisted as one task_comments row with author='swarm'; workers read it via kanban_show <root_id> on the comments field."
                    },
                    "workspace": {
                        "type": "string",
                        "description": "Shared workspace spec: 'scratch' (default), 'dir:/abs/path', or 'worktree'."
                    },
                    "skills": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Shared skill slugs to pass on spawn for every card."
                    },
                    "tenant": {
                        "type": "string",
                        "description": "Shared tenant id across all cards."
                    },
                    "priority": {
                        "type": "integer",
                        "description": "Shared priority across all cards (default 0)."
                    },
                    "max_runtime": {
                        "description": "Shared max wall-clock time per attempt. Integer seconds or string: '30m', '2h', '1d'.",
                        "oneOf": [
                            { "type": "integer" },
                            { "type": "string" }
                        ]
                    },
                    "max_retries": {
                        "type": "integer",
                        "description": "Shared per-task retry cap (overrides global failure_limit)."
                    },
                    "idempotency_key": {
                        "type": "string",
                        "description": "When provided, re-invocation with the same key short-circuits to the existing graph IDs without inserting new rows."
                    },
                    "board": {
                        "type": "string",
                        "description": "Board slug to target. Omit to use HERMES_KANBAN_BOARD env / current file / 'default' (4-tier resolution)."
                    }
                },
                "required": ["goal", "workers"]
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

        // 1. goal (required)
        let goal = match args.get("goal").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => {
                return Ok(crate::tools::common::reject_with_board(
                    "invalid_arg",
                    "goal is required",
                    Some(&board_ctx),
                ));
            }
        };

        // 2. workers (required, union of two array shapes)
        let workers_val = match args.get("workers") {
            Some(v) => v,
            None => {
                return Ok(crate::tools::common::reject_with_board(
                    "invalid_arg",
                    "workers is required",
                    Some(&board_ctx),
                ));
            }
        };
        let worker_specs = match normalize_workers(workers_val, &board_ctx) {
            Ok(v) => v,
            Err(envelope) => return Ok(envelope),
        };

        // 3-6. optional fields
        let verifier: Option<String> = args
            .get("verifier")
            .and_then(|v| v.as_str())
            .map(String::from);
        let synthesizer: Option<String> = args
            .get("synthesizer")
            .and_then(|v| v.as_str())
            .map(String::from);
        let workspace: Option<String> = args
            .get("workspace")
            .and_then(|v| v.as_str())
            .map(String::from);
        let tenant: Option<String> = args
            .get("tenant")
            .and_then(|v| v.as_str())
            .map(String::from);
        let priority: Option<i64> = args.get("priority").and_then(|v| v.as_i64());
        let max_retries: Option<i64> = args.get("max_retries").and_then(|v| v.as_i64());
        let idempotency_key: Option<String> = args
            .get("idempotency_key")
            .and_then(|v| v.as_str())
            .map(String::from);
        let max_runtime_seconds: Option<i64> =
            args.get("max_runtime").and_then(parse_max_runtime);
        let skills: Option<Vec<String>> = args
            .get("skills")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            });
        let blackboard: Option<Value> = args.get("blackboard").cloned();

        // 7. created_by from $HERMES_PROFILE (root assignee + shared created_by)
        let created_by = match std::env::var("HERMES_PROFILE").ok() {
            Some(p) if !p.is_empty() => p,
            _ => {
                return Ok(crate::tools::common::reject_with_board(
                    "missing_profile",
                    "HERMES_PROFILE env var is required to set root card assignee",
                    Some(&board_ctx),
                ));
            }
        };

        // 8. validate every assignee (matches create.rs shape)
        for w in &worker_specs {
            if let Err(e) = ironhermes_core::profile::validate_profile_name(&w.assignee) {
                return Ok(crate::tools::common::reject_with_board(
                    "invalid_assignee",
                    &format!("{e}"),
                    Some(&board_ctx),
                ));
            }
        }
        if let Some(ref v) = verifier {
            if let Err(e) = ironhermes_core::profile::validate_profile_name(v) {
                return Ok(crate::tools::common::reject_with_board(
                    "invalid_assignee",
                    &format!("{e}"),
                    Some(&board_ctx),
                ));
            }
        }
        if let Some(ref s) = synthesizer {
            if let Err(e) = ironhermes_core::profile::validate_profile_name(s) {
                return Ok(crate::tools::common::reject_with_board(
                    "invalid_assignee",
                    &format!("{e}"),
                    Some(&board_ctx),
                ));
            }
        }
        if let Err(e) = ironhermes_core::profile::validate_profile_name(&created_by) {
            return Ok(crate::tools::common::reject_with_board(
                "invalid_assignee",
                &format!("{e}"),
                Some(&board_ctx),
            ));
        }

        // 9. build SwarmGraphSpec
        let spec = SwarmGraphSpec {
            goal,
            workers: worker_specs,
            verifier,
            synthesizer,
            blackboard,
            workspace,
            skills,
            tenant,
            priority,
            max_runtime_seconds,
            max_retries,
            idempotency_key,
            created_by: Some(created_by),
            body: None,
        };

        // 10. open per-board store + create_swarm (D-08)
        let mut store = KanbanStore::open_for_board(&board_ctx.slug)
            .map_err(|e| anyhow::anyhow!("open board '{}': {}", board_ctx.slug, e))?;
        let ids = match store.create_swarm(spec) {
            Ok(v) => v,
            Err(crate::error::KanbanError::Other(e)) => {
                let msg = format!("{e:#}");
                let reason = if msg.contains("invalid assignee") {
                    "invalid_assignee"
                } else if msg.contains("empty workers") {
                    "empty_workers"
                } else {
                    "store_error"
                };
                return Ok(crate::tools::common::reject_with_board(
                    reason,
                    &msg,
                    Some(&board_ctx),
                ));
            }
            Err(e) => {
                let reason = match &e {
                    crate::error::KanbanError::RelativeDirWorkspace(_) => "invalid_workspace",
                    crate::error::KanbanError::TenantMismatch { .. } => "tenant_mismatch",
                    crate::error::KanbanError::LinkCycle { .. } => "link_cycle",
                    crate::error::KanbanError::TaskNotFound(_) => "task_not_found",
                    _ => "store_error",
                };
                return Ok(crate::tools::common::reject_with_board(
                    reason,
                    &format!("{e}"),
                    Some(&board_ctx),
                ));
            }
        };

        // 11. success envelope with board provenance.
        crate::tools::common::ok_with_board(json!({
            "root_id": ids.root_id,
            "worker_ids": ids.worker_ids,
            "verifier_id": ids.verifier_id,
            "synthesizer_id": ids.synthesizer_id,
            "blackboard_event_id": ids.blackboard_event_id,
        }), &board_ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::KanbanStore;

    fn make_store() -> Arc<TokioMutex<KanbanStore>> {
        let dir = tempfile::tempdir().unwrap();
        let store = KanbanStore::new(dir.path().join("test.db")).unwrap();
        std::mem::forget(dir);
        Arc::new(TokioMutex::new(store))
    }

    #[test]
    fn is_available_respects_env() {
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tool = KanbanSwarmTool::new(make_store(), false);
        unsafe {
            std::env::remove_var("HERMES_KANBAN_TASK");
        }
        assert!(!tool.is_available());
        let tool2 = KanbanSwarmTool::new(make_store(), true);
        assert!(tool2.is_available());
    }

    #[test]
    fn schema_contains_board_property() {
        let store = make_store();
        let tool = KanbanSwarmTool::new(store, true);
        let schema_str = serde_json::to_string(&tool.schema()).unwrap();
        assert!(schema_str.contains("\"board\""), "schema missing board property: {schema_str}");
    }
}

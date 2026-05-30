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

/// Build the locked rejection JSON envelope (matches sibling 9-tool pattern).
fn reject(reason: &str, detail: &str) -> String {
    // Sanitize detail to keep it valid inside a JSON string literal.
    let safe_detail = detail.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        r#"{{"status":"rejected","reason":"{reason}","detail":"{safe_detail}"}}"#,
        reason = reason,
        safe_detail = safe_detail,
    )
}

/// Normalize the `workers` arg from either `Array<String>` (flat form) or
/// `Array<Object>` (rich form). Mixed arrays are rejected.
///
/// Returns:
/// - `Ok(specs)` on success (non-empty),
/// - `Err(rejection_json_string)` on shape error (empty / mixed / non-array
///   / object missing required `assignee`).
fn normalize_workers(v: &Value) -> Result<Vec<KanbanWorkerSpec>, String> {
    let arr = v
        .as_array()
        .ok_or_else(|| reject("invalid_workers_shape", "workers must be an array"))?;

    if arr.is_empty() {
        return Err(reject(
            "empty_workers",
            "workers array must contain at least one entry",
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
                reject(
                    "invalid_workers_shape",
                    &format!("workers[{i}] failed to parse as object: {err}"),
                )
            })?;
            if spec.assignee.is_empty() {
                return Err(reject(
                    "invalid_workers_shape",
                    &format!("workers[{i}].assignee is required"),
                ));
            }
            specs.push(spec);
        }
        Ok(specs)
    } else {
        Err(reject(
            "invalid_workers_shape",
            "workers array must be all-strings or all-objects (not mixed)",
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
        // 1. goal (required)
        let goal = match args.get("goal").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => return Ok(reject("invalid_arg", "goal is required")),
        };

        // 2. workers (required, union of two array shapes)
        let workers_val = match args.get("workers") {
            Some(v) => v,
            None => return Ok(reject("invalid_arg", "workers is required")),
        };
        let worker_specs = match normalize_workers(workers_val) {
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
                return Ok(reject(
                    "missing_profile",
                    "HERMES_PROFILE env var is required to set root card assignee",
                ));
            }
        };

        // 8. validate every assignee (matches create.rs:178-183 shape)
        for w in &worker_specs {
            if let Err(e) = ironhermes_core::profile::validate_profile_name(&w.assignee) {
                return Ok(reject("invalid_assignee", &format!("{e}")));
            }
        }
        if let Some(ref v) = verifier {
            if let Err(e) = ironhermes_core::profile::validate_profile_name(v) {
                return Ok(reject("invalid_assignee", &format!("{e}")));
            }
        }
        if let Some(ref s) = synthesizer {
            if let Err(e) = ironhermes_core::profile::validate_profile_name(s) {
                return Ok(reject("invalid_assignee", &format!("{e}")));
            }
        }
        if let Err(e) = ironhermes_core::profile::validate_profile_name(&created_by) {
            return Ok(reject("invalid_assignee", &format!("{e}")));
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

        // 10. store lock + create_swarm
        let mut store = self.store.lock().await;
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
                return Ok(reject(reason, &msg));
            }
            // WR-04 (Phase 36.3.7.7 code review): map the typed KanbanError
            // variants create_swarm can return to specific rejection-reason
            // codes so orchestrators can pattern-match on the envelope.
            // Mirrors the discipline of sibling per-tool envelopes
            // (KanbanLinkTool maps LinkCycle → "link_cycle", KanbanUnblockTool
            // maps RelativeDirWorkspace → "invalid_workspace", etc.).
            Err(e) => {
                let reason = match &e {
                    crate::error::KanbanError::RelativeDirWorkspace(_) => "invalid_workspace",
                    crate::error::KanbanError::TenantMismatch { .. } => "tenant_mismatch",
                    crate::error::KanbanError::LinkCycle { .. } => "link_cycle",
                    crate::error::KanbanError::TaskNotFound(_) => "task_not_found",
                    _ => "store_error",
                };
                return Ok(reject(reason, &format!("{e}")));
            }
        };

        // 11. success envelope
        Ok(serde_json::to_string_pretty(&json!({
            "root_id": ids.root_id,
            "worker_ids": ids.worker_ids,
            "verifier_id": ids.verifier_id,
            "synthesizer_id": ids.synthesizer_id,
            "blackboard_event_id": ids.blackboard_event_id,
        }))?)
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
        let tool = KanbanSwarmTool::new(make_store(), false);
        // SAFETY: tests run sequentially under cargo test --test-threads=1
        // for env-touching cases; this read-only check is harmless when
        // either value is set.
        unsafe {
            std::env::remove_var("HERMES_KANBAN_TASK");
        }
        assert!(!tool.is_available());
        let tool2 = KanbanSwarmTool::new(make_store(), true);
        assert!(tool2.is_available());
    }
}

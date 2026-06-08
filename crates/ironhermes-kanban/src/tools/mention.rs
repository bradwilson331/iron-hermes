//! `kanban_mention` — inline @mention delegation tool (Phase 36.3.7.8).
//!
//! Parses `@<handle>` mentions in a parent task body, resolves each handle
//! via the pure-function resolver, and materialises one child task card per
//! resolved mention in a single atomic transaction via
//! [`crate::store::KanbanStore::create_mention_children`].
//!
//! # Wiring
//!
//! ```text
//! args
//!  │
//!  ├─ task_id (or $HERMES_KANBAN_TASK) ──► store.get_task()
//!  │                                              │
//!  │                                         parent.body
//!  │                                              │
//!  ├─ body_override ────────────────────────► parse_mentions()
//!  │                                              │
//!  │                                         MentionSpan[]
//!  │                                              │
//!  ├─ fallback_policy ──────────────────────► resolve_mention()  ◄── known_assignees
//!  │                                              │
//!  │                                     Resolution[]{Resolved/Fallback/Skipped}
//!  │                                              │
//!  └─ idempotency_key ──────────────────────► store.create_mention_children(plan)
//!                                                 │
//!                                           MentionResult { children }
//!                                                 │
//!                                           JSON success envelope
//! ```
//!
//! # Threat mitigations (T-36.3.7.8-03-*)
//!
//! - T-01 (Tampering/panics): all `args` access uses `.and_then` + Option
//!   semantics; zero runtime `.unwrap()`.
//! - T-02 (Info leak): rejection detail strings never include task body
//!   content — only handle names, task IDs, and error strings.
//! - T-03 (DoS / lock held during parse): `parse_mentions` and `resolve_mention`
//!   run WITHOUT the store lock; only `get_task`, `list_known_assignees` inline
//!   query, and `create_mention_children` run under the lock.
//! - T-04 (EoP / self-mention): resolver layer-1 + store layer-2 ALWAYS run.
//! - T-05 (Spoofing / cross-tenant): tenant inherited from parent; cross-tenant
//!   links rejected inside `create_mention_children`.
//!
//! # Multi-board (Plan 07, D-08)
//!
//! Accepts an optional `board` parameter. Resolves the board context at the top
//! of `execute()` before any DB access. Opens the per-board store. Injects
//! `board` + `board_source` into every success and rejection envelope (T-5 mitigation).

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use ironhermes_tools::Tool;
use serde_json::{Value, json};
use tokio::sync::Mutex as TokioMutex;

use crate::mention::{
    FallbackPolicy, FallbackReason, Resolution, ResolverCtx, SkipReason, parse_mentions,
    resolve_mention,
};
use crate::store::{KanbanStore, MentionChildSpec, MentionPlan};

// ---------------------------------------------------------------------------
// Tool struct
// ---------------------------------------------------------------------------

/// LLM tool: fan out one child task per `@handle` mention found in a task body.
pub struct KanbanMentionTool {
    #[allow(dead_code)]
    store: Arc<TokioMutex<KanbanStore>>,
    explicit_enable: bool,
}

impl KanbanMentionTool {
    pub fn new(store: Arc<TokioMutex<KanbanStore>>, explicit_enable: bool) -> Self {
        Self {
            store,
            explicit_enable,
        }
    }
}

// ---------------------------------------------------------------------------
// Tool trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Tool for KanbanMentionTool {
    fn name(&self) -> &str {
        "kanban_mention"
    }

    fn toolset(&self) -> &str {
        "kanban"
    }

    fn description(&self) -> &str {
        "Parse @<handle> mentions in a task body and fan out one child task per resolved handle. \
         Use to delegate work via inline prose. Returns mentions_parsed, children_created, and skipped."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            self.name(),
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "Parent task ID (defaults to $HERMES_KANBAN_TASK)."
                    },
                    "body_override": {
                        "type": "string",
                        "description": "Parse this string instead of the task body (preview)."
                    },
                    "fallback_policy": {
                        "type": "string",
                        "enum": ["skip", "pending", "error"],
                        "default": "skip",
                        "description": "Behavior for unknown handles."
                    },
                    "idempotency_key": {
                        "type": "string",
                        "description": "Replay key — re-invocation returns same children."
                    },
                    "board": {
                        "type": "string",
                        "description": "Board slug to target. Omit to use HERMES_KANBAN_BOARD env / current file / 'default' (4-tier resolution)."
                    }
                },
                "additionalProperties": false
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

        // ── Step 1: Resolve task_id ─────────────────────────────────────────
        let task_id = match args
            .get("task_id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| std::env::var("HERMES_KANBAN_TASK").ok())
        {
            Some(id) if !id.is_empty() => id,
            _ => {
                return Ok(crate::tools::common::reject_with_board(
                    "invalid_task_id",
                    "task_id is required when $HERMES_KANBAN_TASK is not set",
                    Some(&board_ctx),
                ));
            }
        };

        // ── Step 2: Parse fallback_policy ───────────────────────────────────
        let policy: FallbackPolicy = match args
            .get("fallback_policy")
            .and_then(|v| v.as_str())
            .unwrap_or("skip")
            .parse()
        {
            Ok(p) => p,
            Err(e) => {
                return Ok(crate::tools::common::reject_with_board(
                    "invalid_policy",
                    &format!("{e}"),
                    Some(&board_ctx),
                ));
            }
        };

        // ── Step 3: Read optional idempotency_key ───────────────────────────
        let idempotency_key: Option<String> = args
            .get("idempotency_key")
            .and_then(|v| v.as_str())
            .map(String::from);

        // ── Step 4: Open per-board store; fetch parent task ─────────────────
        // T-03: store opened ONCE per call; parse_mentions runs in Step 6 after we
        // have the body. Brief extra time holding the connection is acceptable and
        // simpler than a two-open dance.
        // Phase 36.3.7.13 D-A2: env wins; slug is fallback hint.
        let mut store = KanbanStore::open_from_env_or_board(Some(&board_ctx.slug))
            .map_err(|e| anyhow::anyhow!("open board '{}': {}", board_ctx.slug, e))?;

        let parent = match store.get_task(&task_id) {
            Ok(t) => t,
            Err(e) => {
                let msg = format!("{e}");
                return Ok(crate::tools::common::reject_with_board(
                    "task_not_found",
                    &msg,
                    Some(&board_ctx),
                ));
            }
        };

        // ── Step 5: Determine body to parse ────────────────────────────────
        let body = args
            .get("body_override")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| parent.body.clone().unwrap_or_default());

        // ── Step 6: Parse mentions (pure, no lock needed for the fn itself) ─
        let spans = parse_mentions(&body);

        // ── Step 7: Build known assignees HashSet ───────────────────────────
        // FALLBACK: list_known_assignees not present in store; SELECT inline.
        // Uses params![] only — zero string interpolation (T-36.3.7-02-05).
        let known_assignees: HashSet<String> = {
            let mut set = HashSet::new();
            let mut stmt = store
                .conn
                .prepare(
                    "SELECT DISTINCT assignee FROM tasks WHERE assignee IS NOT NULL AND assignee != ''",
                )
                .map_err(|e| anyhow::anyhow!("prepare known_assignees query: {e}"))?;
            let rows = stmt
                .query_map(rusqlite::params![], |row| row.get::<_, String>(0))
                .map_err(|e| anyhow::anyhow!("query known_assignees: {e}"))?;
            for a in rows.flatten() {
                set.insert(a.to_lowercase());
            }
            set
        };

        // Parent assignee (lowercased) for self-mention check.
        let parent_assignee = parent.assignee.to_lowercase();

        // ── Step 8: Resolve each mention ────────────────────────────────────
        let ctx = ResolverCtx {
            parent_task_id: task_id.clone(),
            parent_assignee: parent_assignee.clone(),
            known_assignees: &known_assignees,
            fallback_policy: policy,
        };

        let mut children: Vec<MentionChildSpec> = Vec::new();
        let mut skipped: Vec<serde_json::Value> = Vec::new();
        let mut unknown_handle_error: Option<String> = None;

        for (i, span) in spans.iter().enumerate() {
            let resolution = resolve_mention(&span.handle, &ctx);
            match resolution {
                Resolution::Resolved(assignee) => {
                    children.push(MentionChildSpec {
                        handle: span.handle.clone(),
                        assignee,
                        mention_index: i,
                        title: format!("@{} mention from {}", span.handle, task_id),
                        body: None,
                    });
                }
                Resolution::Fallback(assignee, FallbackReason::UnknownHandle) => {
                    children.push(MentionChildSpec {
                        handle: span.handle.clone(),
                        assignee,
                        mention_index: i,
                        title: format!("@{} mention from {}", span.handle, task_id),
                        body: None,
                    });
                }
                Resolution::Skipped(SkipReason::UnknownHandle) => {
                    // When policy=Error this is a hard error — abort the batch.
                    if policy == FallbackPolicy::Error {
                        unknown_handle_error = Some(span.handle.clone());
                        break;
                    }
                    skipped.push(json!({
                        "handle": span.handle,
                        "reason": "unknown_handle"
                    }));
                }
                Resolution::Skipped(SkipReason::Malformed) => {
                    skipped.push(json!({
                        "handle": span.handle,
                        "reason": "malformed"
                    }));
                }
                Resolution::Skipped(SkipReason::SelfReference) => {
                    skipped.push(json!({
                        "handle": span.handle,
                        "reason": "self_reference"
                    }));
                }
                Resolution::Skipped(SkipReason::Cycle) => {
                    // WR-04 (Phase 36.3.7.8 code review): the resolver does NOT
                    // produce this variant today — layer-2 ancestor-chain cycle
                    // detection lives in store.rs::create_mention_children and
                    // raises mention_cycle as a whole-batch KanbanError, not a
                    // per-handle skip. This arm exists for forward compatibility
                    // with a future resolver-side cycle pre-check (see
                    // SkipReason::Cycle doc comment).
                    skipped.push(json!({
                        "handle": span.handle,
                        "reason": "cycle"
                    }));
                }
            }
        }

        // Emit hard error for policy=error + unknown handle.
        if let Some(handle) = unknown_handle_error {
            return Ok(crate::tools::common::reject_with_board(
                "unknown_handle",
                &format!(
                    "handle '{}' did not resolve under fallback_policy=error",
                    handle
                ),
                Some(&board_ctx),
            ));
        }

        // ── Step 9: Build MentionPlan ────────────────────────────────────────
        let created_by = std::env::var("HERMES_PROFILE").ok();

        // Deserialize skills from JSON string stored in task.skills column.
        let skills: Option<Vec<String>> = parent
            .skills
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());

        let plan = MentionPlan {
            parent_id: task_id.clone(),
            children,
            idempotency_key,
            workspace: parent.workspace.clone(),
            skills,
            tenant: parent.tenant.clone(),
            created_by,
        };

        // ── Step 10: Materialize children via store ──────────────────────────
        let result = match store.create_mention_children(plan) {
            Ok(r) => r,
            Err(e) => {
                let msg = format!("{e}");
                let reason = if msg.contains("invalid assignee") {
                    "invalid_assignee"
                } else if msg.contains("mention_cycle") || msg.contains("cycle") {
                    "mention_cycle"
                } else {
                    "store_error"
                };
                return Ok(crate::tools::common::reject_with_board(
                    reason,
                    &msg,
                    Some(&board_ctx),
                ));
            }
        };

        // ── Step 11: Format success response ────────────────────────────────
        let children_created: Vec<serde_json::Value> = result
            .children
            .iter()
            .map(|c| {
                json!({
                    "child_id": c.child_id,
                    "assignee": c.assignee,
                    "handle": c.handle
                })
            })
            .collect();

        // WR-08 (Phase 36.3.7.8 code review): include "status":"ok" so the
        // tool's success envelope can be disambiguated from the reject()
        // envelope by a single discriminator field.
        crate::tools::common::ok_with_board(
            json!({
                "status": "ok",
                "task_id": task_id,
                "mentions_parsed": spans.len(),
                "children_created": children_created,
                "skipped": skipped
            }),
            &board_ctx,
        )
    }
}

// ---------------------------------------------------------------------------
// In-file smoke tests
// ---------------------------------------------------------------------------

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
    fn tool_name_and_toolset_are_kanban_mention() {
        let tool = KanbanMentionTool::new(make_store(), false);
        assert_eq!(tool.name(), "kanban_mention");
        assert_eq!(tool.toolset(), "kanban");
    }

    #[test]
    fn tool_schema_lists_fallback_policy_enum() {
        let tool = KanbanMentionTool::new(make_store(), false);
        let schema = tool.schema();
        // Serialize to JSON and check that "skip","pending","error" are present.
        let schema_str = serde_json::to_string(&schema).unwrap();
        assert!(
            schema_str.contains("\"skip\""),
            "schema missing 'skip': {schema_str}"
        );
        assert!(
            schema_str.contains("\"pending\""),
            "schema missing 'pending': {schema_str}"
        );
        assert!(
            schema_str.contains("\"error\""),
            "schema missing 'error': {schema_str}"
        );
    }

    #[test]
    fn schema_contains_board_property() {
        let tool = KanbanMentionTool::new(make_store(), false);
        let schema_str = serde_json::to_string(&tool.schema()).unwrap();
        assert!(
            schema_str.contains("\"board\""),
            "schema missing board property: {schema_str}"
        );
    }

    /// Test `is_available` logic.
    ///
    /// SAFETY: env-mutating tests MUST run with `--test-threads=1` to avoid
    /// races. The verify step uses `-- --test-threads=1`. This test is safe
    /// when run in that mode.
    #[test]
    fn tool_is_available_respects_env_and_explicit_enable() {
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // explicit_enable=true → always available regardless of env.
        let tool_explicit = KanbanMentionTool::new(make_store(), true);
        assert!(
            tool_explicit.is_available(),
            "should be available when explicit_enable=true"
        );

        // explicit_enable=false + no HERMES_KANBAN_TASK → not available.
        // SAFETY: see doc comment above — only safe under --test-threads=1.
        unsafe {
            std::env::remove_var("HERMES_KANBAN_TASK");
        }
        let tool_no_env = KanbanMentionTool::new(make_store(), false);
        assert!(
            !tool_no_env.is_available(),
            "should not be available when explicit_enable=false and env not set"
        );

        // explicit_enable=false + HERMES_KANBAN_TASK set → available.
        unsafe {
            std::env::set_var("HERMES_KANBAN_TASK", "t_test001");
        }
        let tool_with_env = KanbanMentionTool::new(make_store(), false);
        assert!(
            tool_with_env.is_available(),
            "should be available when HERMES_KANBAN_TASK is set"
        );

        // Clean up.
        unsafe {
            std::env::remove_var("HERMES_KANBAN_TASK");
        }
    }
}

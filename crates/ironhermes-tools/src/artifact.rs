//! `artifact` tool — publishes a self-contained HTML/Markdown page capturing a
//! piece of work (Phase 46.6 D-01/D-03/D-05/D-06).
//!
//! Rendered in a sandboxed viewer inside `iron_hermes_ui` (Plan 03) — never a
//! live app. Chat, delegate, and kanban surfaces all reach this tool through
//! the `"artifacts"` toolset (a normal `ALL_TOOLSETS` member, NOT the
//! MCP/KANBAN exemption path — RESEARCH Pitfall 4).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ironhermes_artifacts::{ArtifactError, ArtifactStore, PublishInput, SourceFormat};
use ironhermes_core::{AuditLog, ToolSchema};
use serde_json::json;

use crate::registry::Tool;

/// D-06 discretion — per-run/per-task cap for unattended producers (kanban/delegate).
/// Interactive chat callers (no run/task key resolved) are never capped.
pub const DEFAULT_ARTIFACT_CAP: u32 = 10;

/// Tool that publishes a self-contained HTML/Markdown artifact page.
pub struct ArtifactTool {
    /// Lazily-opened per-profile artifact store — opened on first `execute()`
    /// so construction (e.g. inside `register_artifact_tool`) stays infallible.
    store: Arc<Mutex<Option<ArtifactStore>>>,
    audit: Arc<AuditLog>,
    /// Per-run/per-task publish counters (D-06), keyed by the resolved
    /// unattended-producer key.
    counters: Arc<Mutex<HashMap<String, u32>>>,
    cap: u32,
}

impl ArtifactTool {
    pub fn new(audit: Arc<AuditLog>, cap: u32) -> Self {
        Self {
            store: Arc::new(Mutex::new(None)),
            audit,
            counters: Arc::new(Mutex::new(HashMap::new())),
            cap,
        }
    }

    /// Test-only constructor: pre-populate the store handle (bypasses the
    /// lazy `open_default()` in `execute()`) so tests can point at a tempdir.
    #[cfg(test)]
    fn with_store(audit: Arc<AuditLog>, cap: u32, store: ArtifactStore) -> Self {
        Self {
            store: Arc::new(Mutex::new(Some(store))),
            audit,
            counters: Arc::new(Mutex::new(HashMap::new())),
            cap,
        }
    }
}

/// Resolves the profile an artifact is stamped with on publish (D-04 / Option A).
///
/// The `IRONHERMES_ARTIFACTS_PROFILE` override wins when set — the kanban
/// dispatcher sets it to the OPERATOR's profile (`build_kanban_worker_env`) so
/// an unattended worker's publishes land in the operator's gallery bucket, not
/// the worker's assignee bucket (mirrors the `IRONHERMES_ARTIFACTS_DB` store
/// redirect). Otherwise the canonical `ironhermes_core::current_profile()`:
/// chat/delegate run in the operator's OWN process, so that already resolves to
/// the operator's profile — the exact value the gallery lists by
/// (`api::active_profile_identifier` → the same `current_profile`). Publish and
/// list therefore never disagree.
fn publish_profile() -> String {
    match std::env::var(ironhermes_core::ARTIFACTS_PROFILE_ENV) {
        Ok(p) if !p.is_empty() => p,
        _ => ironhermes_core::current_profile(),
    }
}

/// Derives a title from the body when the caller omits one (RESEARCH Open
/// Question 2): the first non-empty line, stripped of a leading Markdown
/// heading marker, or "Untitled artifact" when the body has no such line.
fn derive_title(body: &str) -> String {
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let stripped = trimmed.trim_start_matches('#').trim();
        if !stripped.is_empty() {
            return stripped.to_string();
        }
    }
    "Untitled artifact".to_string()
}

/// Resolve the unattended-producer cap key + surface (D-06).
///
/// The TRUSTED kanban-worker env (`env_task`, from `IRONHERMES_KANBAN_TASK`) wins
/// over the model-supplied `run_key` arg (WR-01): inside a kanban worker a
/// misbehaving/looping LLM could otherwise pass a fresh `run_key` per call,
/// giving each its own counter and defeating the cap entirely (unbounded 16 MiB
/// publishes → local disk exhaustion). Same trust-boundary inversion the project
/// already fixed for `expected_run_id`. Only when NOT in a worker does the
/// `run_key` arg apply (the delegate surface has no worker env to trust). Neither
/// present means interactive chat, which is never capped.
fn resolve_producer(
    env_task: Option<String>,
    run_key_arg: Option<&str>,
) -> (Option<String>, &'static str) {
    if let Some(k) = env_task {
        (Some(k), "kanban")
    } else if let Some(k) = run_key_arg {
        (Some(k.to_string()), "delegate")
    } else {
        (None, "chat")
    }
}

#[async_trait]
impl Tool for ArtifactTool {
    fn name(&self) -> &str {
        "artifact"
    }

    fn toolset(&self) -> &str {
        // Phase 46.6 D-01: a normal ALL_TOOLSETS member (constants.rs), NOT the
        // MCP_TOOLSET/KANBAN_TOOLSET exemption path — the load-bearing fix for the
        // documented kanban-worker toolset-invisibility failure mode
        // (RESEARCH Pitfall 4 / project memory: kanban worker completion protocol miss).
        "artifacts"
    }

    fn description(&self) -> &str {
        "Publish a self-contained HTML/Markdown page capturing a piece of work \
         (diff walkthrough, dashboard, comparison, checklist). Rendered in a \
         sandboxed viewer — never a live app. Pass 'update_id' to update an \
         existing artifact in place; omit it to create a new one."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "artifact",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Optional artifact title. Derived from the body's first heading/line when omitted."
                    },
                    "icon": {
                        "type": "string",
                        "description": "Optional emoji/glyph shown next to the title in the gallery."
                    },
                    "body": {
                        "type": "string",
                        "description": "The artifact's HTML or Markdown source content (required). No external requests, no backend — a single self-contained page."
                    },
                    "source_format": {
                        "type": "string",
                        "enum": ["html", "md"],
                        "description": "Source format of 'body'. Defaults to \"md\" when omitted."
                    },
                    "update_id": {
                        "type": "string",
                        "description": "Id of an existing artifact to update in place, appending a new version. Omit to create a new artifact."
                    }
                },
                "required": ["body"]
            }),
        )
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let body = args
            .get("body")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: body"))?;

        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let icon = args
            .get("icon")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let update_id = args
            .get("update_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let source_format_str = args
            .get("source_format")
            .and_then(|v| v.as_str())
            .unwrap_or("md");
        let source_format = SourceFormat::parse(source_format_str).ok_or_else(|| {
            anyhow::anyhow!(
                "Invalid source_format '{source_format_str}': expected \"html\" or \"md\""
            )
        })?;

        // D-06: resolve the unattended-producer cap key + surface. The trusted
        // kanban-worker env wins over the model-supplied `run_key` arg — see
        // `resolve_producer` (WR-01).
        let (producer_key, surface) = resolve_producer(
            std::env::var("IRONHERMES_KANBAN_TASK").ok(),
            args.get("run_key").and_then(|v| v.as_str()),
        );

        // WR-04: check the cap WITHOUT incrementing. A publish that fails after
        // this point (e.g. TooLarge, or a transient store error) must NOT consume
        // a quota slot — the counter is bumped only after a successful publish
        // (below). `published` is the count of successful publishes for this key;
        // reject once it has reached the cap.
        if let Some(ref key) = producer_key {
            let counters = self.counters.lock().unwrap();
            let published = counters.get(key).copied().unwrap_or(0);
            if published >= self.cap {
                tracing::warn!(
                    target: "ironhermes_tools::artifact",
                    producer_key = %key,
                    cap = self.cap,
                    "artifact publish cap exceeded for unattended producer"
                );
                anyhow::bail!(
                    "Artifact publish cap ({}) exceeded for producer '{}'; unattended \
                     producers (kanban/delegate) are limited to {} artifacts per run/task",
                    self.cap,
                    key,
                    self.cap
                );
            }
        }

        // Derive a title on create only (RESEARCH Open Question 2). On update,
        // an omitted title stays None so ArtifactStore::publish preserves the
        // prior value rather than clobbering it (Plan 01 decision).
        let title = if update_id.is_none() {
            Some(title.unwrap_or_else(|| derive_title(body)))
        } else {
            title
        };

        // Phase 46.6 gap-closure: stamp the producing surface + its key. This
        // (a) gives the gallery a source pill (KANBAN/DELEGATE) and (b) lets a
        // completed kanban task link back to the artifact it produced —
        // `source_ref` carries the kanban task id (or the delegate run key).
        // Interactive chat carries no back-reference.
        let (source_kind, source_ref) = match surface {
            "kanban" | "delegate" => (Some(surface.to_string()), producer_key.clone()),
            _ => (None, None),
        };

        let id = {
            let mut guard = self.store.lock().unwrap();
            if guard.is_none() {
                let opened = ArtifactStore::open_default()
                    .map_err(|e| anyhow::anyhow!("failed to open artifact store: {e}"))?;
                *guard = Some(opened);
            }
            let store = guard.as_mut().expect("store opened above");
            store
                .publish(PublishInput {
                    profile: publish_profile(),
                    update_id,
                    title,
                    icon,
                    source_kind,
                    source_ref,
                    source_format,
                    body: body.to_string(),
                })
                .map_err(|e| match e {
                    ArtifactError::TooLarge => {
                        anyhow::anyhow!("Artifact exceeds the 16 MiB rendered-size cap")
                    }
                    other => anyhow::anyhow!("artifact publish failed: {other}"),
                })?
        };

        // WR-04: the publish succeeded — now (and only now) count it against the
        // producer's cap, so failed attempts above never burned a slot.
        if let Some(ref key) = producer_key {
            let mut counters = self.counters.lock().unwrap();
            *counters.entry(key.clone()).or_insert(0) += 1;
        }

        // D-06: audit every successful publish. WR-03: the artifact row is already
        // committed, so a failed audit append is logged loudly but does NOT fail
        // the call. Returning Err here would both hide a successful publish and,
        // because a create carries no `update_id`, make an LLM retry produce a
        // DUPLICATE artifact. (Auditing before publish isn't possible: the id is
        // generated inside `publish` and is this entry's resource id.)
        let entry = self.audit.make_entry(
            "artifact_publish",
            id.clone(),
            None,
            producer_key.clone().unwrap_or_else(|| "chat".to_string()),
            surface,
            producer_key.clone().unwrap_or_default(),
            "artifact publish",
            "approved",
            "auto",
            &args,
        );
        if let Err(e) = self.audit.append(&entry).await {
            tracing::warn!(
                target: "ironhermes_tools::artifact",
                artifact_id = %id,
                error = %e,
                "artifact published but audit append failed (publish NOT rolled back)"
            );
        }

        Ok(format!("Published artifact {id} — view at /artifacts/{id}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::AuditConfig;

    fn make_tool() -> ArtifactTool {
        ArtifactTool::new(
            Arc::new(AuditLog::load(AuditConfig::default())),
            DEFAULT_ARTIFACT_CAP,
        )
    }

    /// Builds an `ArtifactTool` backed by a tempdir-scoped store + audit log
    /// (Task 2: execute() behavior tests). The `TempDir` guard must outlive
    /// the tool/assertions.
    fn make_tool_with_store(cap: u32) -> (ArtifactTool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(dir.path().join("artifacts.db")).unwrap();
        let audit = Arc::new(AuditLog::with_path(
            dir.path().join("audit.jsonl"),
            AuditConfig::default(),
        ));
        (ArtifactTool::with_store(audit, cap, store), dir)
    }

    fn read_audit_lines(dir: &tempfile::TempDir) -> Vec<serde_json::Value> {
        let path = dir.path().join("audit.jsonl");
        std::fs::read_to_string(&path)
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn artifact_tool_name_and_toolset() {
        let tool = make_tool();
        assert_eq!(tool.name(), "artifact");
        assert_eq!(
            tool.toolset(),
            "artifacts",
            "toolset() must be \"artifacts\" — a new ALL_TOOLSETS member, not the MCP/KANBAN exemption path (RESEARCH Pitfall 4)"
        );
    }

    #[test]
    fn artifact_schema_shape() {
        let tool = make_tool();
        let schema = tool.schema();
        let params = &schema.function.parameters;
        let props = &params["properties"];

        for field in ["title", "icon", "body", "source_format", "update_id"] {
            assert!(
                props.get(field).is_some(),
                "schema properties must include '{field}'; got: {props}"
            );
        }

        let required = params["required"]
            .as_array()
            .expect("required must be an array");
        let required_names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(
            required_names.contains(&"body"),
            "required must include 'body'; got: {required_names:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Task 2: execute() — validation, create/update-by-id, cap, audit
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn artifact_tool_creates() {
        let (tool, dir) = make_tool_with_store(DEFAULT_ARTIFACT_CAP);

        let result = tool
            .execute(json!({ "body": "# Hello world", "source_format": "md" }))
            .await
            .expect("create with a valid markdown body must succeed");

        // Extract the id from the store rather than parsing the return string,
        // so this assertion stays valid regardless of the exact response format.
        let artifacts = {
            let mut guard = tool.store.lock().unwrap();
            guard.as_mut().unwrap().list_for_profile("default").unwrap()
        };
        assert_eq!(
            artifacts.len(),
            1,
            "one execute() call must create exactly one artifact row"
        );
        let id = &artifacts[0].id;
        assert!(
            result.contains(id.as_str()),
            "returned string must contain the new artifact id ({id}); got: {result}"
        );

        // Republish via update_id must update in place — no new artifacts row.
        tool.execute(json!({
            "body": "# Hello world v2",
            "source_format": "md",
            "update_id": id,
        }))
        .await
        .expect("republish by id must succeed");
        let artifacts_after = {
            let mut guard = tool.store.lock().unwrap();
            guard.as_mut().unwrap().list_for_profile("default").unwrap()
        };
        assert_eq!(
            artifacts_after.len(),
            1,
            "republish by update_id must update in place, not create a second artifact"
        );
        assert_eq!(
            &artifacts_after[0].id, id,
            "artifact id must be unchanged on update"
        );

        // Every successful publish is audited with kind "artifact_publish" (D-06).
        let entries = read_audit_lines(&dir);
        assert_eq!(entries.len(), 2, "one audit entry per successful publish");
        for entry in &entries {
            assert_eq!(entry["kind"], "artifact_publish");
        }
    }

    #[tokio::test]
    async fn artifact_execute_missing_body_is_err() {
        let (tool, dir) = make_tool_with_store(DEFAULT_ARTIFACT_CAP);
        let result = tool.execute(json!({})).await;
        assert!(result.is_err(), "missing body must return Err");

        let artifacts = {
            let mut guard = tool.store.lock().unwrap();
            guard.as_mut().unwrap().list_for_profile("default").unwrap()
        };
        assert!(
            artifacts.is_empty(),
            "missing-body execute must leave the store empty"
        );
        assert!(
            read_audit_lines(&dir).is_empty(),
            "missing-body execute must write no audit entry"
        );
    }

    #[tokio::test]
    async fn artifact_cap_enforced() {
        let cap = 2u32;
        let (tool, _dir) = make_tool_with_store(cap);

        // Unattended producer (explicit run_key): 1..=cap succeed.
        for i in 0..cap {
            let result = tool
                .execute(json!({ "body": format!("body {i}"), "run_key": "task-1" }))
                .await;
            assert!(
                result.is_ok(),
                "publish {} of {} within the cap must succeed: {:?}",
                i + 1,
                cap,
                result
            );
        }

        // The (cap+1)th publish for the same run/task key must be rejected.
        let over_cap = tool
            .execute(json!({ "body": "one too many", "run_key": "task-1" }))
            .await;
        let err = over_cap
            .expect_err("exceeding the cap must return Err")
            .to_string();
        assert!(
            err.contains(&cap.to_string()),
            "cap-exceeded error must name the cap ({cap}); got: {err}"
        );

        // Interactive chat (no run_key) is never capped — publishes well past `cap` succeed.
        for i in 0..(cap as usize + 3) {
            let result = tool
                .execute(json!({ "body": format!("chat body {i}") }))
                .await;
            assert!(
                result.is_ok(),
                "interactive chat (no run_key) must never be capped: {:?}",
                result
            );
        }
    }

    #[test]
    fn resolve_producer_env_beats_run_key() {
        // WR-01: inside a kanban worker the trusted env key wins over any
        // model-supplied run_key, so a worker LLM cannot re-key to escape the cap.
        let (key, surface) = resolve_producer(Some("t_kanban".to_string()), Some("model-supplied"));
        assert_eq!(key.as_deref(), Some("t_kanban"));
        assert_eq!(surface, "kanban");

        // No worker env → the delegate run_key applies.
        let (key, surface) = resolve_producer(None, Some("run-abc"));
        assert_eq!(key.as_deref(), Some("run-abc"));
        assert_eq!(surface, "delegate");

        // Neither → interactive chat, uncapped.
        let (key, surface) = resolve_producer(None, None);
        assert_eq!(key, None);
        assert_eq!(surface, "chat");
    }

    #[tokio::test]
    async fn artifact_failed_publish_does_not_consume_cap() {
        let (tool, _dir) = make_tool_with_store(1); // cap = 1

        // A body over the 16 MiB rendered cap fails with TooLarge — AFTER the cap
        // check. WR-04: this must NOT burn the single quota slot.
        let huge = "a".repeat(17 * 1024 * 1024);
        let over = tool
            .execute(json!({ "body": huge, "source_format": "html", "run_key": "task-1" }))
            .await;
        assert!(over.is_err(), "an oversized publish must fail (TooLarge)");

        // The slot was never consumed, so a valid publish for the same key succeeds.
        let ok = tool
            .execute(
                json!({ "body": "small ok body", "source_format": "html", "run_key": "task-1" }),
            )
            .await;
        assert!(
            ok.is_ok(),
            "a failed publish must not consume the cap; the next valid publish must succeed: {ok:?}"
        );
    }
}

//! `kanban_complete` — protocol terminator: mark a task done (D-22).
//!
//! Gates:
//! (a) `expected_run_id` mismatch → structured `{"status":"rejected","reason":"stale_run_id"}`
//!     — returned as Ok so the LLM can read the rejection and decide what to do.
//! (b) `created_cards=[...]` with phantom ids or wrong-profile ids → structured
//!     `{"status":"rejected","reason":"created_cards"}` + permanent `completion_rejected`
//!     event.
//! (c) Free-form prose scan for unresolved `t_<hex>` refs → advisory
//!     `hallucinated_ref` event (non-blocking, handled by store).
//!
//! `expected_run_id` defaults to `$IRONHERMES_KANBAN_RUN_ID` env when the caller
//! omits it — defense-in-depth so workers don't have to thread it explicitly.
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

/// LLM tool: complete a kanban task (protocol terminator).
pub struct KanbanCompleteTool {
    #[allow(dead_code)]
    store: Arc<TokioMutex<KanbanStore>>,
    explicit_enable: bool,
}

impl KanbanCompleteTool {
    pub fn new(store: Arc<TokioMutex<KanbanStore>>, explicit_enable: bool) -> Self {
        Self {
            store,
            explicit_enable,
        }
    }
}

#[async_trait]
impl Tool for KanbanCompleteTool {
    fn name(&self) -> &str {
        "kanban_complete"
    }

    fn toolset(&self) -> &str {
        "kanban"
    }

    fn description(&self) -> &str {
        "Mark a Kanban task as done. Requires at least one of `summary` or `result`. \
         Validates expected_run_id (stale-run rejection) and created_cards (phantom-id / \
         wrong-profile rejection). Both rejection types are returned as structured JSON \
         so the LLM can handle them without crashing the tool call."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "kanban_complete",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "Task ID to complete. Omit to use $IRONHERMES_KANBAN_TASK."
                    },
                    "summary": {
                        "type": "string",
                        "description": "Human-readable summary of what was accomplished."
                    },
                    "result": {
                        "type": "string",
                        "description": "Structured result string (machine-readable output)."
                    },
                    "metadata": {
                        "type": "object",
                        "description": "Free-form JSON metadata dict for the completed run."
                    },
                    "expected_run_id": {
                        "type": "string",
                        "description": "Run ID that must still be the active run. Defaults to $IRONHERMES_KANBAN_RUN_ID."
                    },
                    "created_cards": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Task IDs created by this worker during this run. Each must exist and have created_by matching $HERMES_PROFILE."
                    },
                    "board": {
                        "type": "string",
                        "description": "Board slug to target. Omit to use IRONHERMES_KANBAN_BOARD env / current file / 'default' (4-tier resolution)."
                    },
                    "output_path": {
                        "type": "string",
                        "description": "Optional pointer to where the task's output lives (e.g. a deploy path or artifact location). Persisted to the task record; a subsequent complete call that omits this does not clear a previously-set value."
                    }
                },
                "required": []
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

        // Resolve task_id via dual-read (IRONIRONHERMES_KANBAN_TASK first, legacy fallback).
        let task_id = args
            .get("task_id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| crate::kanban_env("TASK"))
            .ok_or_else(|| {
                anyhow::anyhow!("task_id required when IRONIRONHERMES_KANBAN_TASK is not set")
            })?;

        // Resolve current_profile.
        let current_profile = std::env::var("HERMES_PROFILE").unwrap_or_else(|_| "unknown".into());

        // expected_run_id: the trusted env `IRONHERMES_KANBAN_RUN_ID` is AUTHORITATIVE over any
        // model-supplied arg — see `resolve_expected_run_id` (D-22 defense-in-depth).
        let expected_run_id = crate::tools::common::resolve_expected_run_id(
            args.get("expected_run_id").and_then(|v| v.as_str()),
        );

        let summary = args
            .get("summary")
            .and_then(|v| v.as_str())
            .map(String::from);
        let result = args
            .get("result")
            .and_then(|v| v.as_str())
            .map(String::from);
        let metadata: Option<Value> = args.get("metadata").cloned();

        let created_cards: Option<Vec<String>> = args
            .get("created_cards")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            });
        let output_path = args
            .get("output_path")
            .and_then(|v| v.as_str())
            .map(String::from);

        // Open per-board store (D-08).
        // Phase 36.3.7.13 D-A2: env wins; slug is fallback hint.
        let mut store = KanbanStore::open_from_env_or_board(Some(&board_ctx.slug))
            .map_err(|e| anyhow::anyhow!("open board '{}': {}", board_ctx.slug, e))?;

        match store.complete_task(
            &task_id,
            summary.as_deref(),
            metadata.as_ref(),
            result.as_deref(),
            expected_run_id.as_deref(),
            created_cards.as_deref(),
            &current_profile,
            output_path.as_deref(),
        ) {
            Ok(()) => {
                // Deterministic artifact capture (option a): publish the task's
                // workspace deliverable (index.html) as an artifact so a
                // completing worker leaves one in the operator gallery whether or
                // not the LLM chose to call the `artifact` tool. Best-effort —
                // never blocks completion.
                let task_title = store
                    .get_task(&task_id)
                    .map(|t| t.title)
                    .unwrap_or_default();
                let mut payload = json!({
                    "status": "ok",
                    "task_id": task_id,
                });
                if let Some(artifact_id) = capture_completion_artifact(&task_id, &task_title) {
                    tracing::info!(
                        task_id = %task_id,
                        artifact_id = %artifact_id,
                        "captured completion artifact"
                    );
                    payload["artifact_id"] = json!(artifact_id);
                }
                crate::tools::common::ok_with_board(payload, &board_ctx)
            }

            Err(KanbanError::StaleRunId { expected, actual }) => {
                // Persist a diagnostic completion_rejected event (best-effort). The stale-run
                // gate previously left NO trace (unlike created_cards rejections, which persist),
                // so a rejected-but-completed worker was indistinguishable from a crash in the
                // event log. Record expected/actual so future supersession incidents are
                // debuggable. Never fail the tool on a logging error.
                let payload = json!({
                    "reason": "stale_run_id",
                    "expected": expected.as_str(),
                    "actual": actual.as_str(),
                });
                if let Err(e) = store.append_event(
                    &task_id,
                    Some(expected.as_str()),
                    crate::events::KanbanEventKind::CompletionRejected,
                    Some(&payload),
                ) {
                    tracing::warn!(error = %e, task_id = %task_id, "failed to persist completion_rejected (stale_run_id) event");
                }
                // Structured rejection — return Ok so the LLM can read and decide.
                crate::tools::common::ok_with_board(
                    json!({
                        "status": "rejected",
                        "reason": "stale_run_id",
                        "task_id": task_id,
                        "expected": expected,
                        "actual": actual,
                    }),
                    &board_ctx,
                )
            }

            Err(KanbanError::CreatedCardsRejected {
                phantom,
                wrong_profile,
            }) => {
                // Structured rejection — permanent completion_rejected event already written.
                crate::tools::common::ok_with_board(
                    json!({
                        "status": "rejected",
                        "reason": "created_cards",
                        "task_id": task_id,
                        "phantom_ids": phantom,
                        "wrong_profile_ids": wrong_profile,
                    }),
                    &board_ctx,
                )
            }

            Err(other) => Err(other.into()),
        }
    }
}

/// Deliverable files the deterministic completion-capture publishes as
/// artifacts, in priority order — the first existing match wins. Keyed on
/// `index.html` (option a) because that is the de-facto filename kanban workers
/// already emit for a standalone web page. Extend this list to capture more
/// deliverable types later (e.g. `("README.md", SourceFormat::Markdown)`).
const CAPTURE_CANDIDATES: &[(&str, ironhermes_artifacts::SourceFormat)] =
    &[("index.html", ironhermes_artifacts::SourceFormat::Html)];

/// Resolve the completing task's deliverable file + format. Searches, in
/// priority order:
///  1. the worker's current directory — the dispatcher spawns the worker with
///     its CWD set to the *resolved* workspace (`worker_spawn.rs`
///     `.current_dir(&resolved_workspace)`), which honors the
///     `IRONHERMES_KANBAN_WORKSPACES_ROOT` redirect. This is the authoritative
///     location the worker actually wrote to. It differs from a home-relative
///     path whenever the operator redirects workspaces to a shared root — which
///     the dispatcher does for profile-scoped workers, so a naive
///     `kanban_workspace_for` (off the worker's profile home) points at the
///     wrong, empty directory.
///  2. the home-relative scratch path (`kanban_workspace_for`) — a fallback for
///     non-worker callers and tests, where CWD is not the workspace.
fn locate_deliverable(
    task_id: &str,
) -> Option<(std::path::PathBuf, ironhermes_artifacts::SourceFormat)> {
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd);
    }
    roots.push(crate::paths::kanban_workspace_for(task_id));
    find_deliverable_in(&roots)
}

/// Pure root search for the task's deliverable. For each root, in order:
///  1. an exact [`CAPTURE_CANDIDATES`] filename (e.g. `index.html`);
///  2. otherwise the primary `*.html` file in that root — workers name the
///     deliverable arbitrarily (`spider-man-poem.html`, not always
///     `index.html`), so keying only on the canonical name silently misses them.
///
/// The first root that yields a match wins.
fn find_deliverable_in(
    roots: &[std::path::PathBuf],
) -> Option<(std::path::PathBuf, ironhermes_artifacts::SourceFormat)> {
    for root in roots {
        // 1. Exact-name candidates (index.html; extend for other types later).
        for (name, fmt) in CAPTURE_CANDIDATES {
            let candidate = root.join(name);
            if candidate.is_file() {
                return Some((candidate, *fmt));
            }
        }
        // 2. Fallback: the primary standalone HTML page under an arbitrary name.
        if let Some(html) = primary_html_in(root) {
            return Some((html, ironhermes_artifacts::SourceFormat::Html));
        }
    }
    None
}

/// The primary `*.html` file directly under `root` (non-recursive): the largest
/// by size, ties broken by path for determinism. Size is a good "main page"
/// heuristic — a scratch/test page is smaller than the actual deliverable.
/// Returns `None` when the directory has no HTML file (or can't be read).
fn primary_html_in(root: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut htmls: Vec<(u64, std::path::PathBuf)> = std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let is_html = path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("html"));
            if is_html && path.is_file() {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                Some((size, path))
            } else {
                None
            }
        })
        .collect();
    // Largest first; tie-break by path so the choice is deterministic.
    htmls.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    htmls.into_iter().next().map(|(_, path)| path)
}

/// Deterministically publish a completing task's deliverable as an artifact so
/// one appears in the operator gallery WITHOUT relying on the worker LLM to call
/// the `artifact` tool (option a). Scans the task's scratch workspace for the
/// first [`CAPTURE_CANDIDATES`] file; if present, publishes it to the artifact
/// store — routed to the operator's root store + profile bucket by the same
/// `IRONHERMES_ARTIFACTS_DB` / `IRONHERMES_ARTIFACTS_PROFILE` env the dispatcher
/// sets for the `artifact` tool, so captured artifacts land in the same gallery.
/// Idempotent per task: a re-complete versions the task's existing artifact
/// (looked up via `latest_for_source`) rather than creating a duplicate.
///
/// Best-effort: every failure path logs and returns `None` so a capture problem
/// never blocks task completion. A task with no deliverable file (e.g. a pure
/// code task) simply returns `None`. Returns the artifact id on success.
///
/// Caveat: artifacts are a single self-contained document under a strict sandbox
/// CSP (D-02) — a deliverable that references external CSS/JS renders without
/// them. Inlining assets at capture time is a future enhancement.
fn capture_completion_artifact(task_id: &str, task_title: &str) -> Option<String> {
    let (path, source_format) = locate_deliverable(task_id)?; // none → nothing to capture

    let body = match std::fs::read_to_string(&path) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(
                task_id = %task_id, path = %path.display(), error = %e,
                "completion artifact capture: failed to read deliverable"
            );
            return None;
        }
    };

    // Operator override (dispatcher-set) wins, else the canonical profile — the
    // same resolution the `artifact` tool uses, so publish and list agree (D-04 /
    // Option A).
    let profile = std::env::var(ironhermes_core::ARTIFACTS_PROFILE_ENV)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(ironhermes_core::current_profile);

    let mut store = match ironhermes_artifacts::ArtifactStore::open_default() {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                task_id = %task_id, error = %e,
                "completion artifact capture: failed to open artifact store"
            );
            return None;
        }
    };

    // Idempotent per task: version the task's existing artifact instead of
    // creating a duplicate on a re-run / re-complete.
    let update_id = store
        .latest_for_source("kanban", task_id)
        .ok()
        .flatten()
        .map(|summary| summary.id);

    let title = if task_title.trim().is_empty() {
        format!("Task {task_id}")
    } else {
        task_title.to_string()
    };

    match store.publish(ironhermes_artifacts::PublishInput {
        profile,
        update_id,
        title: Some(title),
        icon: None,
        source_kind: Some("kanban".to_string()),
        source_ref: Some(task_id.to_string()),
        source_format,
        body,
    }) {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!(
                task_id = %task_id, error = %e,
                "completion artifact capture: publish failed"
            );
            None
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
        let tool = KanbanCompleteTool::new(store.clone(), false);
        assert!(!tool.is_available());

        unsafe {
            std::env::set_var("IRONHERMES_KANBAN_TASK", "t_test");
        }
        let tool2 = KanbanCompleteTool::new(store.clone(), false);
        assert!(tool2.is_available());
        unsafe {
            std::env::remove_var("IRONHERMES_KANBAN_TASK");
        }

        let tool3 = KanbanCompleteTool::new(store, true);
        assert!(tool3.is_available());
    }

    #[test]
    fn schema_contains_board_property() {
        let store = make_store();
        let tool = KanbanCompleteTool::new(store, true);
        let schema_str = serde_json::to_string(&tool.schema()).unwrap();
        assert!(
            schema_str.contains("\"board\""),
            "schema missing board property: {schema_str}"
        );
    }

    /// Deterministic capture (option a): an `index.html` in the task workspace is
    /// published as a kanban-sourced artifact, and a re-capture versions the same
    /// artifact rather than creating a duplicate.
    #[test]
    fn capture_completion_artifact_publishes_and_is_idempotent() {
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let prev_home = std::env::var("IRONHERMES_HOME").ok();
        let prev_prof = std::env::var("IRONHERMES_ARTIFACTS_PROFILE").ok();
        let prev_db = std::env::var("IRONHERMES_ARTIFACTS_DB").ok();
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home.path());
            // Exercise the canonical current_profile() path (no operator override).
            std::env::remove_var("IRONHERMES_ARTIFACTS_PROFILE");
            std::env::remove_var("IRONHERMES_ARTIFACTS_DB");
        }

        let task_id = "t_capturetest01";
        let ws = crate::paths::kanban_workspace_for(task_id);
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join("index.html"), "<h1>Deliverable</h1>").unwrap();

        let id =
            capture_completion_artifact(task_id, "Animated Poem").expect("index.html is published");
        assert!(!id.is_empty());

        let store = ironhermes_artifacts::ArtifactStore::open_default().unwrap();
        let summary = store
            .latest_for_source("kanban", task_id)
            .unwrap()
            .expect("artifact recorded for the task");
        assert_eq!(summary.title, "Animated Poem");
        assert_eq!(summary.source_kind.as_deref(), Some("kanban"));
        assert_eq!(summary.source_ref.as_deref(), Some(task_id));

        // A pure-code task (no candidate file) captures nothing.
        assert!(
            capture_completion_artifact("t_nodeliverable9", "Backend wiring").is_none(),
            "a task with no index.html must not produce an artifact"
        );

        // Re-capture versions the SAME artifact (dedup by source), never a dupe.
        let id2 = capture_completion_artifact(task_id, "Animated Poem v2").expect("re-publish");
        assert_eq!(id, id2, "re-capture must version the existing artifact");

        unsafe {
            match prev_home {
                Some(v) => std::env::set_var("IRONHERMES_HOME", v),
                None => std::env::remove_var("IRONHERMES_HOME"),
            }
            match prev_prof {
                Some(v) => std::env::set_var("IRONHERMES_ARTIFACTS_PROFILE", v),
                None => std::env::remove_var("IRONHERMES_ARTIFACTS_PROFILE"),
            }
            match prev_db {
                Some(v) => std::env::set_var("IRONHERMES_ARTIFACTS_DB", v),
                None => std::env::remove_var("IRONHERMES_ARTIFACTS_DB"),
            }
        }
    }

    /// Regression (round 7): the deliverable is found under the FIRST root that
    /// has it, so an empty/wrong root (e.g. the profile-home path when the
    /// dispatcher redirected the real workspace elsewhere) is skipped in favour
    /// of the root that actually holds `index.html` (the worker's CWD).
    #[test]
    fn find_deliverable_searches_roots_in_order() {
        let empty = tempfile::tempdir().unwrap();
        let real = tempfile::tempdir().unwrap();
        std::fs::write(real.path().join("index.html"), "<h1>x</h1>").unwrap();

        let (path, fmt) =
            find_deliverable_in(&[empty.path().to_path_buf(), real.path().to_path_buf()])
                .expect("finds index.html in the second (non-empty) root");
        assert!(path.ends_with("index.html"));
        assert_eq!(fmt, ironhermes_artifacts::SourceFormat::Html);

        // No candidate under any root → nothing to capture.
        assert!(find_deliverable_in(&[empty.path().to_path_buf()]).is_none());
    }

    /// Round 8: workers name the deliverable arbitrarily — capture the primary
    /// `*.html` even when it isn't `index.html` (the Spider-Man case: the file
    /// was `spider-man-poem.html`, so keying only on `index.html` missed it).
    #[test]
    fn find_deliverable_captures_arbitrary_html_name() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(
            ws.path().join("spider-man-poem.html"),
            "<h1>web-slinger</h1>",
        )
        .unwrap();
        std::fs::write(ws.path().join("README.md"), "# notes").unwrap();
        let (path, fmt) = find_deliverable_in(&[ws.path().to_path_buf()])
            .expect("captures the arbitrarily-named html deliverable");
        assert_eq!(path.file_name().unwrap(), "spider-man-poem.html");
        assert_eq!(fmt, ironhermes_artifacts::SourceFormat::Html);
    }

    /// `index.html` (an exact candidate) wins over any other, larger `.html`.
    #[test]
    fn find_deliverable_prefers_index_over_larger_html() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("index.html"), "<h1>x</h1>").unwrap();
        std::fs::write(
            ws.path().join("aaa-big.html"),
            "<h1>a much longer body than index</h1>",
        )
        .unwrap();
        let (path, _) = find_deliverable_in(&[ws.path().to_path_buf()]).unwrap();
        assert_eq!(
            path.file_name().unwrap(),
            "index.html",
            "an exact candidate must beat the largest-html fallback"
        );
    }
}

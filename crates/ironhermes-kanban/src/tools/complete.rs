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
         `result` is the task's work product and is published as an artifact when present \
         (a file written into the workspace takes precedence over it). \
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
                        "description": "The work product itself — the full text of what the task produced, in markdown or plain text. When present it is published as an artifact (a file written into the workspace wins over this text). Omit this field when there is nothing to deliver."
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
                //
                // The task record is fetched once here and its `body` (the
                // instruction text an operator's opt-out phrase would be in,
                // D-12) and `assignee` (a pointer's producer name) are passed
                // into the capture fn rather than re-fetched inside it.
                let task_record = store.get_task(&task_id).ok();
                let task_title = task_record
                    .as_ref()
                    .map(|t| t.title.clone())
                    .unwrap_or_default();
                let instruction_text = task_record
                    .as_ref()
                    .and_then(|t| t.body.clone())
                    .filter(|b| !b.trim().is_empty())
                    .unwrap_or_else(|| task_title.clone());
                let assignee = task_record
                    .as_ref()
                    .map(|t| t.assignee.clone())
                    .unwrap_or_default();
                let mut payload = json!({
                    "status": "ok",
                    "task_id": task_id,
                });
                if let Some(artifact_id) = capture_completion_artifact(
                    &task_id,
                    &task_title,
                    result.as_deref(),
                    &instruction_text,
                    &assignee,
                ) {
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

/// Resolve the completing task's deliverable file + format. Tries, in
/// priority order, each candidate root through the shared widened producer
/// engine (`ironhermes_tools::chat_capture::locate_producer_deliverable`,
/// D-01):
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
///
/// `since: None` — kanban has no per-completion freshness bound, matching the
/// legacy engine's behavior exactly.
fn locate_deliverable(
    task_id: &str,
) -> Option<(std::path::PathBuf, ironhermes_artifacts::SourceFormat)> {
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd);
    }
    roots.push(crate::paths::kanban_workspace_for(task_id));
    for root in &roots {
        if let Some(found) = ironhermes_tools::chat_capture::locate_producer_deliverable(root, None)
        {
            return Some(found);
        }
    }
    None
}

/// Deterministically publish a completing task's deliverable as an artifact so
/// one appears in the operator gallery WITHOUT relying on the worker LLM to call
/// the `artifact` tool (D-01/D-06). Delegates to the shared producer engine
/// (`ironhermes_tools::chat_capture::publish_producer_deliverable`): a file in
/// the workspace wins (D-04); when no file exists, `result` (the tool's
/// declared deliverable, D-05/D-06) publishes as Markdown when non-blank.
/// Idempotent per task: a re-complete versions the task's existing artifact
/// rather than creating a duplicate.
///
/// `instruction_text` (the completing task's `body`, or its `title` when the
/// body is absent — D-12) is checked via
/// `ironhermes_tools::chat_capture::detect_turn_opt_out`. On the opt-out
/// branch, any produced output (a workspace file, or declared `result` text)
/// still gets a record — demoted to a pointer via
/// `ironhermes_tools::chat_capture::publish_pointer_artifact` under the SAME
/// `source_kind`/`source_ref` the full artifact would have used, naming
/// `assignee` as the producer. Output that does not exist at all publishes
/// nothing — there is no output to record.
///
/// Best-effort: every failure path is a `tracing::warn!` (inside the shared
/// engine) plus a `None` return so a capture problem never blocks task
/// completion. A task with no deliverable file and no `result` text simply
/// returns `None`. Returns the artifact id on success.
///
/// Caveat: artifacts are a single self-contained document under a strict sandbox
/// CSP (D-02) — a deliverable that references external CSS/JS renders without
/// them. Inlining assets at capture time is a future enhancement.
fn capture_completion_artifact(
    task_id: &str,
    task_title: &str,
    result: Option<&str>,
    instruction_text: &str,
    assignee: &str,
) -> Option<String> {
    let title = if task_title.trim().is_empty() {
        format!("Task {task_id}")
    } else {
        task_title.to_string()
    };

    // A single locate call, reused by both branches below (D-04 file-wins
    // ordering holds on the opt-out branch too).
    let located = locate_deliverable(task_id);

    if ironhermes_tools::chat_capture::detect_turn_opt_out(instruction_text) {
        // D-12: the operator opted out — demote to a marked pointer rather
        // than suppressing. Occupies the same source kind/ref the full
        // artifact would have, so it versions in place on a later re-complete.
        return match located {
            Some((path, _)) => match std::fs::read(&path) {
                Ok(bytes) => ironhermes_tools::chat_capture::publish_pointer_artifact(
                    "kanban",
                    task_id,
                    &title,
                    assignee,
                    &path.to_string_lossy(),
                    &bytes,
                ),
                Err(e) => {
                    tracing::warn!(
                        task_id = %task_id, path = %path.display(), error = %e,
                        "pointer capture: failed to read deliverable"
                    );
                    None
                }
            },
            None => {
                let fallback = result.map(str::trim).filter(|s| !s.is_empty());
                match fallback {
                    Some(text) => ironhermes_tools::chat_capture::publish_pointer_artifact(
                        "kanban",
                        task_id,
                        &title,
                        assignee,
                        "declared result text (no file)",
                        text.as_bytes(),
                    ),
                    None => None, // no output at all — nothing to record
                }
            }
        };
    }

    // The root of the first `locate_deliverable` root that holds a
    // deliverable file wins (D-04 file-wins). When no root has a file,
    // publish against the home-relative scratch root anyway — with no file
    // present, `publish_producer_deliverable`'s internal scan finds nothing
    // regardless of which root is passed, so it falls through to the
    // `result` text (D-05/D-06).
    let scan_root = located
        .and_then(|(path, _)| path.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| crate::paths::kanban_workspace_for(task_id));

    ironhermes_tools::chat_capture::publish_producer_deliverable(
        ironhermes_tools::chat_capture::ProducerPublish {
            scan_root: &scan_root,
            since: None,
            source_kind: "kanban",
            source_ref: task_id,
            title: &title,
            fallback_body: result,
        },
    )
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

        let id = capture_completion_artifact(task_id, "Animated Poem", None, "build the thing", "test-bot")
            .expect("index.html is published");
        assert!(!id.is_empty());

        let store = ironhermes_artifacts::ArtifactStore::open_default().unwrap();
        let summary = store
            .latest_for_source("kanban", task_id)
            .unwrap()
            .expect("artifact recorded for the task");
        assert_eq!(summary.title, "Animated Poem");
        assert_eq!(summary.source_kind.as_deref(), Some("kanban"));
        assert_eq!(summary.source_ref.as_deref(), Some(task_id));

        // A pure-code task (no candidate file, no result) captures nothing.
        assert!(
            capture_completion_artifact("t_nodeliverable9", "Backend wiring", None, "build the thing", "test-bot").is_none(),
            "a task with no index.html and no result must not produce an artifact"
        );

        // Re-capture versions the SAME artifact (dedup by source), never a dupe.
        let id2 = capture_completion_artifact(task_id, "Animated Poem v2", None, "build the thing", "test-bot")
            .expect("re-publish");
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

    /// Test-only guard around `locate_deliverable`/`capture_completion_artifact`
    /// tests that mutate process-global `IRONHERMES_HOME` and CWD: takes the
    /// crate ENV_LOCK, captures the pre-test values, and restores both on drop
    /// (including on an assertion panic mid-test — a bare "restore after the
    /// asserts" block, the pre-existing style in this module, leaves the
    /// process CWD mutated for every later test in the binary if an earlier
    /// assertion panics first).
    struct RootsEnvGuard {
        _env_guard: std::sync::MutexGuard<'static, ()>,
        prev_home: Option<String>,
        prev_cwd: std::path::PathBuf,
    }

    impl RootsEnvGuard {
        fn new(new_home: &std::path::Path, new_cwd: &std::path::Path) -> Self {
            let env_guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prev_home = std::env::var("IRONHERMES_HOME").ok();
            let prev_cwd = std::env::current_dir().unwrap();
            unsafe {
                std::env::set_var("IRONHERMES_HOME", new_home);
            }
            std::env::set_current_dir(new_cwd).unwrap();
            Self {
                _env_guard: env_guard,
                prev_home,
                prev_cwd,
            }
        }
    }

    impl Drop for RootsEnvGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.prev_cwd);
            unsafe {
                match &self.prev_home {
                    Some(v) => std::env::set_var("IRONHERMES_HOME", v),
                    None => std::env::remove_var("IRONHERMES_HOME"),
                }
            }
        }
    }

    /// Regression (round 7): the deliverable is found under the FIRST root that
    /// has it, so an empty/wrong root (the worker's CWD, when it holds nothing)
    /// is skipped in favour of the root that actually holds `index.html` (the
    /// home-relative scratch workspace, `locate_deliverable`'s second root).
    #[test]
    fn locate_deliverable_searches_roots_in_order() {
        let home = tempfile::tempdir().unwrap();
        let empty_cwd = tempfile::tempdir().unwrap();
        let _guard = RootsEnvGuard::new(home.path(), empty_cwd.path());

        let task_id = "t_rootorder01";
        let ws = crate::paths::kanban_workspace_for(task_id);
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join("index.html"), "<h1>x</h1>").unwrap();

        let (path, fmt) = locate_deliverable(task_id)
            .expect("finds index.html in the second (home-relative) root when CWD is empty");
        assert!(path.ends_with("index.html"));
        assert_eq!(fmt, ironhermes_artifacts::SourceFormat::Html);

        // No candidate under any root → nothing to capture.
        assert!(locate_deliverable("t_rootorder_nocandidate").is_none());
    }

    /// Round 8: workers name the deliverable arbitrarily — the widened engine
    /// still captures the primary `*.html` even when it isn't `index.html`
    /// (the Spider-Man case: the file was `spider-man-poem.html`, so keying
    /// only on `index.html` missed it).
    #[test]
    fn locate_deliverable_captures_arbitrary_html_name() {
        let home = tempfile::tempdir().unwrap();
        let empty_cwd = tempfile::tempdir().unwrap();
        let _guard = RootsEnvGuard::new(home.path(), empty_cwd.path());

        let task_id = "t_arbitraryname01";
        let ws = crate::paths::kanban_workspace_for(task_id);
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(
            ws.join("spider-man-poem.html"),
            "<h1>web-slinger</h1>",
        )
        .unwrap();
        std::fs::write(ws.join("README.md"), "# notes").unwrap();

        let (path, fmt) = locate_deliverable(task_id)
            .expect("captures the arbitrarily-named html file");
        assert_eq!(path.file_name().unwrap(), "spider-man-poem.html");
        assert_eq!(fmt, ironhermes_artifacts::SourceFormat::Html);
    }

    /// `index.html` (an exact candidate) wins over any other, larger `.html`.
    #[test]
    fn locate_deliverable_prefers_index_over_larger_html() {
        let home = tempfile::tempdir().unwrap();
        let empty_cwd = tempfile::tempdir().unwrap();
        let _guard = RootsEnvGuard::new(home.path(), empty_cwd.path());

        let task_id = "t_indexpref01";
        let ws = crate::paths::kanban_workspace_for(task_id);
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join("index.html"), "<h1>x</h1>").unwrap();
        std::fs::write(
            ws.join("aaa-big.html"),
            "<h1>a much longer body than index</h1>",
        )
        .unwrap();

        let (path, _) = locate_deliverable(task_id).unwrap();
        assert_eq!(
            path.file_name().unwrap(),
            "index.html",
            "an exact candidate must beat the largest-html fallback"
        );
    }

    /// End-to-end (Task 1, D-01/D-02): a `.py` file written into a MARKED
    /// kanban scratch workspace and captured through the completion path
    /// publishes an artifact whose rendered output is an escaped
    /// plain-monospace code block.
    #[test]
    fn capture_completion_artifact_publishes_code_deliverable_as_escaped_code_block() {
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let prev_home = std::env::var("IRONHERMES_HOME").ok();
        let prev_prof = std::env::var("IRONHERMES_ARTIFACTS_PROFILE").ok();
        let prev_db = std::env::var("IRONHERMES_ARTIFACTS_DB").ok();
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home.path());
            std::env::remove_var("IRONHERMES_ARTIFACTS_PROFILE");
            std::env::remove_var("IRONHERMES_ARTIFACTS_DB");
        }

        let task_id = "t_codecapturetest01";
        let ws = crate::paths::kanban_workspace_for(task_id);
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(ws.join(ironhermes_tools::chat_capture::WORKSPACE_MARKER_DIR))
            .unwrap();
        let body = "print('<script>alert(1)</script>')";
        std::fs::write(ws.join("report.py"), body).unwrap();

        let id = capture_completion_artifact(task_id, "Code Task", None, "build the thing", "test-bot")
            .expect("a code deliverable in a marked workspace must be published");

        let store = ironhermes_artifacts::ArtifactStore::open_default().unwrap();
        let html = store
            .load_latest_html(&id)
            .expect("rendered html must exist for the published artifact");
        assert!(html.starts_with("<pre><code>"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));

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

    /// Task 2 (D-04/D-06): with BOTH a deliverable file present and a
    /// non-blank `result` argument, the published body is the FILE's
    /// contents — `result` is ignored (file wins).
    #[test]
    fn capture_completion_artifact_prefers_file_over_result_text() {
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let prev_home = std::env::var("IRONHERMES_HOME").ok();
        let prev_prof = std::env::var("IRONHERMES_ARTIFACTS_PROFILE").ok();
        let prev_db = std::env::var("IRONHERMES_ARTIFACTS_DB").ok();
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home.path());
            std::env::remove_var("IRONHERMES_ARTIFACTS_PROFILE");
            std::env::remove_var("IRONHERMES_ARTIFACTS_DB");
        }

        let task_id = "t_filewinsresult01";
        let ws = crate::paths::kanban_workspace_for(task_id);
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join("index.html"), "<h1>from file</h1>").unwrap();

        let id = capture_completion_artifact(task_id, "Task", Some("this text must be ignored"), "build the thing", "test-bot")
            .expect("file must be published");
        let store = ironhermes_artifacts::ArtifactStore::open_default().unwrap();
        let html = store.load_latest_html(&id).unwrap();
        assert!(html.contains("from file"));
        assert!(!html.contains("this text must be ignored"));

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

    /// Task 2 (D-05/D-06): with no deliverable file and a non-blank
    /// `result`, an artifact publishes with the markdown wire format and the
    /// `result` text as its body.
    #[test]
    fn capture_completion_artifact_publishes_result_text_when_no_file() {
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let prev_home = std::env::var("IRONHERMES_HOME").ok();
        let prev_prof = std::env::var("IRONHERMES_ARTIFACTS_PROFILE").ok();
        let prev_db = std::env::var("IRONHERMES_ARTIFACTS_DB").ok();
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home.path());
            std::env::remove_var("IRONHERMES_ARTIFACTS_PROFILE");
            std::env::remove_var("IRONHERMES_ARTIFACTS_DB");
        }

        let task_id = "t_resulttextnofile01";
        let id = capture_completion_artifact(task_id, "Task", Some("declared result text"), "build the thing", "test-bot")
            .expect("result text must be published as markdown when no file exists");
        let store = ironhermes_artifacts::ArtifactStore::open_default().unwrap();
        let summary = store
            .latest_for_source("kanban", task_id)
            .unwrap()
            .expect("artifact recorded for the task");
        assert_eq!(summary.id, id);
        let html = store.load_latest_html(&id).unwrap();
        assert!(html.contains("declared result text"));

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

    /// Task 2 (D-06): with no deliverable file and a `result` that is
    /// absent, empty, or whitespace-only, nothing is published.
    #[test]
    fn capture_completion_artifact_publishes_nothing_for_blank_result_and_no_file() {
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let prev_home = std::env::var("IRONHERMES_HOME").ok();
        let prev_prof = std::env::var("IRONHERMES_ARTIFACTS_PROFILE").ok();
        let prev_db = std::env::var("IRONHERMES_ARTIFACTS_DB").ok();
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home.path());
            std::env::remove_var("IRONHERMES_ARTIFACTS_PROFILE");
            std::env::remove_var("IRONHERMES_ARTIFACTS_DB");
        }

        assert!(
            capture_completion_artifact("t_blankresult_none01", "Task", None, "build the thing", "test-bot").is_none(),
            "no file and no result must publish nothing"
        );
        assert!(
            capture_completion_artifact("t_blankresult_empty01", "Task", Some(""), "build the thing", "test-bot").is_none(),
            "no file and an empty result must publish nothing"
        );
        assert!(
            capture_completion_artifact("t_blankresult_ws01", "Task", Some("   \n\t  "), "build the thing", "test-bot").is_none(),
            "no file and a whitespace-only result must publish nothing"
        );

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

    /// Task 2 (D-12): a task whose instruction text opts out, and which wrote
    /// a deliverable file, gets a marked, body-free pointer record — not
    /// suppression — naming the task's assignee as the producer.
    #[test]
    fn capture_completion_artifact_writes_pointer_on_opt_out() {
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let prev_home = std::env::var("IRONHERMES_HOME").ok();
        let prev_prof = std::env::var("IRONHERMES_ARTIFACTS_PROFILE").ok();
        let prev_db = std::env::var("IRONHERMES_ARTIFACTS_DB").ok();
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home.path());
            std::env::remove_var("IRONHERMES_ARTIFACTS_PROFILE");
            std::env::remove_var("IRONHERMES_ARTIFACTS_DB");
        }

        let task_id = "t_optoutpointer01";
        let ws = crate::paths::kanban_workspace_for(task_id);
        std::fs::create_dir_all(&ws).unwrap();
        let deliverable_text = "THE DELIVERABLE'S OWN SECRET BODY, NEVER STORED IN A POINTER";
        std::fs::write(ws.join("index.html"), deliverable_text).unwrap();

        let id = capture_completion_artifact(
            task_id,
            "Task",
            None,
            "just show it inline, don't publish",
            "alice-bot",
        )
        .expect("an opt-out with a produced deliverable must still write a pointer record");

        let store = ironhermes_artifacts::ArtifactStore::open_default().unwrap();
        let (fmt, body) = store.load_latest_source(&id).unwrap();
        assert_eq!(fmt, ironhermes_artifacts::SourceFormat::Markdown);
        assert!(
            body.starts_with(ironhermes_tools::chat_capture::POINTER_ARTIFACT_MARKER),
            "pointer body must carry the marker"
        );
        assert!(body.contains("alice-bot"), "pointer must name the assignee as producer");
        assert!(
            !body.contains(deliverable_text),
            "the deliverable's own text must never appear in the stored pointer body"
        );

        // Same source kind/ref as the full artifact would have used.
        let summary = store
            .latest_for_source("kanban", task_id)
            .unwrap()
            .expect("pointer occupies the same source kind/ref key");
        assert_eq!(summary.id, id);

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

    /// Task 2 (D-12): a task whose instruction text opts out and which
    /// produced nothing at all (no file, no result) publishes nothing —
    /// there is no output to record.
    #[test]
    fn capture_completion_artifact_writes_nothing_on_opt_out_with_no_output() {
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let prev_home = std::env::var("IRONHERMES_HOME").ok();
        let prev_prof = std::env::var("IRONHERMES_ARTIFACTS_PROFILE").ok();
        let prev_db = std::env::var("IRONHERMES_ARTIFACTS_DB").ok();
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home.path());
            std::env::remove_var("IRONHERMES_ARTIFACTS_PROFILE");
            std::env::remove_var("IRONHERMES_ARTIFACTS_DB");
        }

        assert!(
            capture_completion_artifact(
                "t_optoutnooutput01",
                "Task",
                None,
                "no artifact please, just show it inline",
                "alice-bot",
            )
            .is_none(),
            "an opt-out with no deliverable at all must publish nothing"
        );

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
}

//! Decomposer kernel for Phase 36.3.7.10.
//!
//! This module exposes the [`DecomposeFn`] injection-point typedef, the
//! request/output/child-spec data shapes, and the two atomic store-facing
//! async functions:
//!
//! - [`specify_triage_task`] — rewrites `tasks.body` in-place **without**
//!   fan-out, and promotes `triage → todo` in one `BEGIN IMMEDIATE`
//!   transaction.
//! - [`decompose_triage_task`] — same body rewrite + promotion, **plus**
//!   child-task INSERT + `task_links` INSERT per child.
//!
//! Both functions are deliberately provider-agnostic: the actual LLM call is
//! performed by the caller-supplied [`DecomposeFn`] closure. This mirrors the
//! `SpawnFn` / `SendFn` injection pattern from `dispatcher.rs` and
//! `notifier.rs` and keeps `ironhermes-kanban` free of
//! `ironhermes-agent` / provider-client dependencies.
//!
//! # Failure-mode policy (RESEARCH §Q5, locked)
//!
//! - LLM error → retain task in `triage`; append
//!   [`KanbanEventKind::DecomposeFailed`] event with
//!   `{reason: "llm_error", message: <err>}`; return `Err` to caller.
//! - Double-call race → second `UPDATE … WHERE status='triage'` returns 0
//!   rows; both `apply_specify` and `apply_decompose` short-circuit with
//!   `Ok(DecomposedIds { root_promoted: false, child_ids: vec![] })`.
//!
//! # Body-schema contract (RESEARCH §Q8)
//!
//! The store kernel is **shape-agnostic**: it writes whatever the
//! [`DecomposeFn`] returns in `new_body` verbatim to `tasks.body`.
//! The production implementation in Plan 05's `cmd_decompose` enforces the
//! canonical Markdown schema:
//!
//! ```text
//! ## Goal
//! ## Approach
//! ## Acceptance Criteria
//! ## Work Breakdown
//! ## Risk Register
//! ## Suggested Skills
//! ```

use std::sync::Arc;

use serde_json::json;
use tokio::sync::Mutex as TokioMutex;

use crate::config::KanbanConfig;
use crate::error::Result;
use crate::events::KanbanEventKind;
use crate::store::KanbanStore;

// ---------------------------------------------------------------------------
// BoxFuture alias — mirrors dispatcher.rs:62 exactly.
// ---------------------------------------------------------------------------

type BoxFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>;

// ---------------------------------------------------------------------------
// Public data structures
// ---------------------------------------------------------------------------

/// Input payload passed to the injected [`DecomposeFn`] closure.
///
/// All fields are owned (`String`) so the closure can be invoked without
/// holding the store mutex (see safety note in [`decompose_triage_task`]).
#[derive(Debug, Clone)]
pub struct DecomposeRequest {
    /// The triage task's DB id (e.g. `t_abc1234`).
    pub task_id: String,
    /// Current one-liner title of the triage task.
    pub title: String,
    /// Current body (usually `None` or a brief description).
    pub body: Option<String>,
    /// Current assignee profile slug.
    pub assignee: String,
    /// Active kanban config (carries decomposer_model, default_assignee, etc.).
    pub config: KanbanConfig,
}

/// Structured output returned by the [`DecomposeFn`] closure.
///
/// `Clone` is required because the mock in tests returns a pre-built fixture
/// multiple times.
#[derive(Debug, Clone)]
pub struct DecomposeOutput {
    /// New title written to `tasks.title`.
    pub new_title: String,
    /// New body written to `tasks.body` verbatim.
    pub new_body: String,
    /// Child tasks to create (empty for the *specify* path — no fan-out).
    pub children: Vec<ChildSpec>,
}

/// A single child task specification returned inside [`DecomposeOutput`].
///
/// `Clone` is required for the same reason as [`DecomposeOutput`].
#[derive(Debug, Clone)]
pub struct ChildSpec {
    pub title: String,
    /// Profile slug. Empty string must never reach the DB; the fallback
    /// cascade in [`decompose_triage_task`] enforces a non-empty value.
    pub assignee: String,
    pub body: Option<String>,
}

/// Return value of both public async functions in this module.
#[derive(Debug, Clone)]
pub struct DecomposedIds {
    /// IDs of the freshly inserted child tasks (empty for specify-only path).
    pub child_ids: Vec<String>,
    /// `true` when the parent task was promoted from `triage → todo`.
    /// `false` signals that the status guard observed 0 rows affected — the
    /// task was already processed (idempotent double-call).
    pub root_promoted: bool,
}

impl DecomposedIds {
    /// Convenience constructor for the already-processed / double-call path.
    pub fn already_processed() -> Self {
        Self {
            child_ids: vec![],
            root_promoted: false,
        }
    }
}

// ---------------------------------------------------------------------------
// DecomposeFn typedef — mirrors SpawnFn from dispatcher.rs:62-66.
// ---------------------------------------------------------------------------

/// Injectable LLM call.
///
/// Production binding: calls the configured provider client (wired at the
/// CLI `cmd_decompose` / gateway runner level).
/// Tests: returns a fixture [`DecomposeOutput`] via a mock closure.
///
/// The closure takes an **owned** [`DecomposeRequest`] (no borrowed args) so
/// no lifetime annotation is needed — matches `SpawnFn`, not `SendFn`.
pub type DecomposeFn =
    Arc<dyn Fn(DecomposeRequest) -> BoxFuture<Result<DecomposeOutput>> + Send + Sync>;

// ---------------------------------------------------------------------------
// Public async functions
// ---------------------------------------------------------------------------

/// Specify a triage task: rewrite body in-place + promote `triage → todo`.
///
/// # Flow
///
/// 1. Acquires the store lock and reads the task. If `status != "triage"`,
///    returns [`DecomposedIds::already_processed`] immediately.
/// 2. **Drops the lock** before calling the LLM (never hold the mutex across
///    an async network call — that would serialize the entire dispatcher tick).
/// 3. Awaits `decompose_fn(req)`.
/// 4. On success: re-acquires the lock, calls
///    [`KanbanStore::apply_specify`].
/// 5. On failure: re-acquires the lock, appends a
///    [`KanbanEventKind::DecomposeFailed`] event, returns `Err`.
pub async fn specify_triage_task(
    store: Arc<TokioMutex<KanbanStore>>,
    task_id: &str,
    decompose_fn: &DecomposeFn,
    config: &KanbanConfig,
) -> Result<DecomposedIds> {
    // Step 1: read the task under a brief lock.
    let task = {
        let s = store.lock().await;
        s.get_task(task_id)?
    };

    // Early exit: not in triage — already processed.
    if task.status != "triage" {
        return Ok(DecomposedIds::already_processed());
    }

    // Step 2: build request from owned data (lock dropped here).
    let req = DecomposeRequest {
        task_id: task_id.to_string(),
        title: task.title.clone(),
        body: task.body.clone(),
        assignee: task.assignee.clone(),
        config: config.clone(),
    };

    // Step 3: call the LLM (lock NOT held).
    let output = match decompose_fn(req).await {
        Ok(o) => o,
        Err(e) => {
            // Step 5 failure path: retain in triage, append event.
            let payload = json!({
                "reason": "llm_error",
                "message": e.to_string(),
            });
            let mut s = store.lock().await;
            s.append_event(
                task_id,
                None,
                KanbanEventKind::DecomposeFailed,
                Some(&payload),
            )?;
            return Err(e);
        }
    };

    // Step 4: apply atomically.
    // WR-11 fix: thread `config.orchestrator_profile` into the apply layer so
    // the `specified` event payload carries author attribution (the
    // cmd_specify docstring promised this; previously the payload only
    // carried `new_title`).
    let mut s = store.lock().await;
    let promoted = s.apply_specify(
        task_id,
        &output.new_title,
        &output.new_body,
        &config.orchestrator_profile,
    )?;
    Ok(DecomposedIds {
        child_ids: vec![],
        root_promoted: promoted,
    })
}

/// Decompose a triage task: rewrite body + fan out into child tasks atomically.
///
/// # Flow
///
/// Identical to [`specify_triage_task`] except on success it calls
/// [`KanbanStore::apply_decompose`] which also inserts child task rows and
/// `task_links` rows inside the same `BEGIN IMMEDIATE` transaction.
///
/// # Assignee fallback cascade (RESEARCH §Q5)
///
/// When a [`ChildSpec::assignee`] is empty, the store-level `apply_decompose`
/// falls back first to `config.default_assignee`, then to the parent task's
/// `assignee`. A child with `assignee = ""` never reaches the DB.
pub async fn decompose_triage_task(
    store: Arc<TokioMutex<KanbanStore>>,
    task_id: &str,
    decompose_fn: &DecomposeFn,
    config: &KanbanConfig,
) -> Result<DecomposedIds> {
    // Step 1: read the task under a brief lock.
    let task = {
        let s = store.lock().await;
        s.get_task(task_id)?
    };

    // Early exit: not in triage — already processed.
    if task.status != "triage" {
        return Ok(DecomposedIds::already_processed());
    }

    // Step 2: build request from owned data (lock dropped here).
    let req = DecomposeRequest {
        task_id: task_id.to_string(),
        title: task.title.clone(),
        body: task.body.clone(),
        assignee: task.assignee.clone(),
        config: config.clone(),
    };

    // Step 3: call the LLM (lock NOT held).
    let output = match decompose_fn(req).await {
        Ok(o) => o,
        Err(e) => {
            // Failure path: retain in triage, append event.
            let payload = json!({
                "reason": "llm_error",
                "message": e.to_string(),
            });
            let mut s = store.lock().await;
            s.append_event(
                task_id,
                None,
                KanbanEventKind::DecomposeFailed,
                Some(&payload),
            )?;
            return Err(e);
        }
    };

    // Step 4: apply atomically (parent UPDATE + children INSERT + links INSERT).
    let parent_assignee = task.assignee.clone();
    let mut s = store.lock().await;
    s.apply_decompose(
        task_id,
        &output.new_title,
        &output.new_body,
        &output.children,
        &parent_assignee,
        config,
    )
}

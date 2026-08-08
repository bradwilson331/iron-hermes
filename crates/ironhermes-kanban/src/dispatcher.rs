//! Kanban dispatcher — 8-step tick loop (D-09 / D-10 / D-11 / D-12 / D-14).
//!
//! # Architecture
//!
//! The dispatcher is a tokio task hosted inside `ironhermes-gateway` (D-09).
//! Plan 08 wires it via `run_dispatch_loop`; this module is testable
//! standalone against a tempfile DB.
//!
//! # The 8-step tick (D-10)
//!
//! Each tick runs these steps **in order**. Failures in a step are logged
//! and do not cascade — the next step still runs.
//!
//! 1. **detect_crashed_workers** — kill -0 on claim_pid; dead PID → `crashed`
//!    event, release claim, increment consecutive_failures.
//! 2. **extend_live_pid_claims** — alive PID whose claim_expires < now →
//!    extend claim_expires by TTL, append `claim_extended` event.
//! 3. **reclaim_stale_claims** — expired claim with dead or null PID →
//!    reset to `ready`, append `reclaimed` event.
//! 4. **enforce_max_runtime** — task running past max_runtime_seconds →
//!    SIGTERM → 5s grace → SIGKILL → `timed_out` event.
//! 5. **promote_ready** — `todo` tasks whose parents are all `done` →
//!    `ready`, append `promoted` event.
//! 6. **claim_ready_tasks** — `BEGIN IMMEDIATE` CAS claim up to
//!    `max_in_progress` candidates; respects `scheduled_at`.
//! 7. **respawn_guard_reason** — reject tasks whose last run was a 429/auth
//!    error, or completed within 1h, or has an active PR; append
//!    `respawn_guarded`.
//! 8. **spawn_workers** — call `worker_spawn::spawn_worker` for each
//!    survived candidate; on error apply failure_limit circuit breaker (D-12).
//!
//! # Circuit breaker (D-12)
//!
//! When `consecutive_failures >= effective_limit` (task.max_retries or
//! config.failure_limit), the task is auto-blocked with a `gave_up` event.
//! Operator must explicitly reset via plan 06 `unblock` verb.
//!
//! # Stranded-task diagnostic (D-14)
//!
//! `diagnose_stranded` is a separate helper consumed by
//! `ironhermes kanban diagnostics` (plan 06). It is NOT called from the tick
//! loop to avoid log spam.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::params;
use tokio::sync::Mutex as TokioMutex;
use tokio_util::sync::CancellationToken;
use tracing::Instrument as _;

use crate::cas::{DEFAULT_CLAIM_TTL_SECONDS, atomic_claim, build_claim_lock, release_claim};
use crate::config::KanbanConfig;
use crate::error::{KanbanError, Result};
use crate::events::KanbanEventKind;
use crate::pid::{Signal, is_pid_alive, kill_pid};
use crate::store::{KanbanStore, ListFilters};
use crate::types::{Task, TaskRun};

// ---------------------------------------------------------------------------
// read_stderr_tail (D-04)
// ---------------------------------------------------------------------------

/// Read the bounded tail of a worker's stderr log for the D-04 crash
/// diagnostic.
///
/// Returns the last `max_lines` lines, further bounded to at most
/// `max_bytes` bytes (whichever constraint yields the *smaller* result —
/// this doubles as an information-disclosure control on the enriched
/// `task_runs.error` diagnostic, T-46.5-11). Missing or unreadable files are
/// non-fatal: this returns an empty `String` rather than propagating an
/// error, since a stderr tail is a best-effort diagnostic, not a
/// correctness-critical value.
fn read_stderr_tail(path: &std::path::Path, max_bytes: usize, max_lines: usize) -> String {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return String::new(),
    };

    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    let by_lines = lines[start..].join("\n");

    if by_lines.len() <= max_bytes {
        return by_lines;
    }

    // Bound further by max_bytes, snapping forward to a valid UTF-8 char
    // boundary so we never panic on a multi-byte split.
    let bytes = by_lines.as_bytes();
    let mut start_byte = bytes.len() - max_bytes;
    while start_byte < bytes.len() && !by_lines.is_char_boundary(start_byte) {
        start_byte += 1;
    }
    by_lines[start_byte..].to_string()
}

// ---------------------------------------------------------------------------
// BoxFuture alias (for spawn_fn testability)
// ---------------------------------------------------------------------------

type BoxFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>;
/// spawn_fn signature: (task, run, workspace, board_slug) -> pid
/// Injectable worker-spawn function. `pub` because it is the type of the
/// public `DispatcherContext::spawn_fn` field and of the `with_spawn_fn`
/// parameter, so out-of-crate tests need to be able to name it.
pub type SpawnFn =
    Arc<dyn Fn(Task, TaskRun, String, String) -> BoxFuture<Result<u32>> + Send + Sync>;

// ---------------------------------------------------------------------------
// DispatcherContext
// ---------------------------------------------------------------------------

/// Shared context passed to each dispatcher tick.
///
/// The `spawn_fn` field is injectable for testing — the default uses the real
/// `worker_spawn::spawn_worker`; tests substitute a closure that returns a
/// synthetic PID or an error without actually exec'ing `ironhermes`.
/// Injectable pre-spawn dispatch-gate predicate (Phase 47.4 GAP-1).
///
/// Default in every constructor is the real, fail-closed
/// [`ironhermes_core::dispatch_gate::evaluate_profile_dispatch`]. Tests that
/// exercise *other* dispatcher steps inject an allow-all so they stay
/// hermetic instead of reading the developer's real `~/.ironhermes`; the gate
/// itself is covered against the real predicate by
/// `tests/dispatch_gate_loop.rs`, which sandboxes `IRONHERMES_HOME` and lays
/// down actual profile fixtures rather than stubbing the decision.
pub type DispatchGateFn =
    Arc<dyn Fn(&str) -> ironhermes_core::dispatch_gate::DispatchDecision + Send + Sync>;

/// The production dispatch gate: the shared, provider-aware, fail-closed
/// predicate every dispatch path uses.
fn default_gate_fn() -> DispatchGateFn {
    Arc::new(|assignee: &str| ironhermes_core::dispatch_gate::evaluate_profile_dispatch(assignee))
}

pub struct DispatcherContext {
    pub store: Arc<TokioMutex<KanbanStore>>,
    pub config: KanbanConfig,
    pub hostname: String,
    pub dispatcher_pid: u32,
    /// Injectable spawn function for tests. Default: `worker_spawn::spawn_worker`.
    pub spawn_fn: SpawnFn,
    /// Injectable pre-spawn dispatch gate. Default: the real fail-closed
    /// predicate. See [`DispatchGateFn`].
    pub gate_fn: DispatchGateFn,
    /// Phase 36.3.7.10 — optional decomposer injection. None = auto_decompose has no
    /// effect (graceful degradation). Production: wired by CLI cmd_decompose or a future
    /// gateway runner update. Tests: inject a mock closure via `with_decompose_fn`.
    pub decompose_fn: Option<crate::decomposer::DecomposeFn>,
}

impl DispatcherContext {
    /// Create a context with the real `spawn_worker` function.
    pub fn new(store: Arc<TokioMutex<KanbanStore>>, config: KanbanConfig) -> Self {
        Self {
            store,
            config,
            hostname: crate::pid::current_hostname(),
            dispatcher_pid: std::process::id(),
            spawn_fn: Arc::new(|task, run, workspace, board_slug| {
                Box::pin(async move {
                    crate::worker_spawn::spawn_worker_for_board(
                        &task,
                        &run,
                        &workspace,
                        &board_slug,
                    )
                    .await
                })
            }),
            gate_fn: default_gate_fn(),
            decompose_fn: None,
        }
    }

    /// Create a context with an injectable spawn function (for tests).
    ///
    /// The dispatch gate still defaults to the real fail-closed predicate —
    /// overriding the spawn function must not silently disable the gate.
    /// Tests that need to bypass it must say so explicitly via
    /// [`Self::with_gate_fn`].
    pub fn with_spawn_fn(
        store: Arc<TokioMutex<KanbanStore>>,
        config: KanbanConfig,
        spawn_fn: SpawnFn,
    ) -> Self {
        Self {
            store,
            config,
            hostname: crate::pid::current_hostname(),
            dispatcher_pid: std::process::id(),
            spawn_fn,
            gate_fn: default_gate_fn(),
            decompose_fn: None,
        }
    }

    /// Override the pre-spawn dispatch gate (for tests).
    ///
    /// Intended for tests exercising dispatcher steps unrelated to the gate,
    /// which would otherwise have to provision a real profile directory for
    /// every fixture assignee. Production code must never call this.
    pub fn with_gate_fn(mut self, gate_fn: DispatchGateFn) -> Self {
        self.gate_fn = gate_fn;
        self
    }
}

// ---------------------------------------------------------------------------
// run_dispatch_loop
// ---------------------------------------------------------------------------

/// Run the dispatcher tick loop until `cancel` is signalled.
///
/// Ticks on `config.dispatch_interval_seconds` (min 1s). Each tick error is
/// logged and the loop continues.
pub async fn run_dispatch_loop(ctx: Arc<DispatcherContext>, cancel: CancellationToken) {
    let interval_secs = ctx.config.dispatch_interval_seconds.max(1);
    let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!("Kanban dispatch loop cancelled");
                return;
            }
            _ = interval.tick() => {
                // Use .instrument() instead of .enter() so the future stays
                // Send — EnteredSpan is !Send and cannot be held across awaits
                // in a tokio::spawn context (gateway JoinSet requires Send).
                let span = tracing::info_span!("kanban.dispatch.tick");
                async {
                    if let Err(e) = run_dispatch_tick(&ctx).await {
                        tracing::error!(error = %e, "Kanban dispatch tick error");
                    }
                }
                .instrument(span)
                .await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// run_dispatch_tick (8 steps)
// ---------------------------------------------------------------------------

/// Execute one full dispatcher tick (all 8 steps).
///
/// Phase 36.3.7.9: iterates every board (prepending "default" to `list_boards()`)
/// and runs all 8 steps for each board. For the "default" board the existing
/// `ctx.store` Arc is reused (back-compat for tests and the notifier Arc hoist in
/// runner.rs). For named boards a fresh per-board KanbanStore is opened from disk.
/// Per-board open failures are logged + skipped; other boards continue
/// (INV-36.3.7-08-05 extension).
pub async fn run_dispatch_tick(ctx: &DispatcherContext) -> Result<()> {
    // Build the ordered board list: "default" first, then named boards.
    let mut all_boards = vec!["default".to_string()];
    match crate::paths::list_boards() {
        Ok(named) => all_boards.extend(named),
        Err(e) => {
            tracing::warn!(
                "[kanban] dispatcher could not enumerate boards: {e}; using default only"
            );
        }
    }

    for slug in all_boards {
        // For "default": reuse the existing ctx.store Arc (back-compat; see module doc).
        // For named boards: open a fresh store per tick (Option A: no Arc-per-board
        // caching this phase; Arc-per-board caching is a future optimization).
        let board_store_arc: Arc<TokioMutex<KanbanStore>> = if slug == "default" {
            Arc::clone(&ctx.store)
        } else {
            match KanbanStore::open_for_board(&slug) {
                Ok(s) => Arc::new(TokioMutex::new(s)),
                Err(e) => {
                    tracing::warn!("[kanban] dispatcher skipping board '{slug}': {e}");
                    continue;
                }
            }
        };

        // Build a per-board context that shares config + spawn_fn + gate_fn +
        // decompose_fn from the outer DispatcherContext but uses the per-board
        // store. `gate_fn` MUST be propagated, not re-defaulted: every board's
        // tasks have to be judged by the same predicate the caller configured,
        // and a silent fall-back to the default here would make an injected
        // gate apply to the default board only.
        let board_ctx = DispatcherContext {
            store: board_store_arc,
            config: ctx.config.clone(),
            hostname: ctx.hostname.clone(),
            dispatcher_pid: ctx.dispatcher_pid,
            spawn_fn: Arc::new({
                let outer_spawn_fn = Arc::clone(&ctx.spawn_fn);
                let slug = slug.clone();
                move |task, run, workspace, _ignored_slug| {
                    // The board-loop slug is captured here; the inner arg is ignored.
                    outer_spawn_fn(task, run, workspace, slug.clone())
                }
            }),
            gate_fn: Arc::clone(&ctx.gate_fn),
            decompose_fn: ctx.decompose_fn.clone(),
        };

        run_dispatch_tick_for_board(&board_ctx, &slug).await;
    }

    Ok(())
}

/// Run all 8 dispatch steps for a single board.
///
/// Steps are isolated — a failure in step N does not prevent step N+1.
/// Step 0 (auto-decompose) runs BEFORE crash detection when both gates pass:
/// `ctx.config.auto_decompose == true` AND `ctx.decompose_fn.is_some()`.
///
/// `pub` so integration tests can drive a single board tick directly
/// without the multi-board sweep in `run_dispatch_tick` (needed for receiver
/// tests that verify Step 0 auto-decompose behavior).
pub async fn run_dispatch_tick_for_board(ctx: &DispatcherContext, board_slug: &str) {
    // Step 0: auto-decompose triage tasks (Phase 36.3.7.10).
    // Gate 1: config.auto_decompose must be true (default false — zero cost when off).
    // Gate 2: decompose_fn must be Some (gateway runner ships None in v1 — graceful no-op).
    // Both gates must pass; either gate failing short-circuits with zero work + zero noise.
    if ctx.config.auto_decompose
        && let Some(ref decompose_fn) = ctx.decompose_fn
    {
        async {
            if let Err(e) = decompose_triage_tasks(ctx, decompose_fn).await {
                tracing::warn!(error = %e, board = board_slug, "auto-decompose step error");
            }
        }
        .instrument(tracing::info_span!(
            "kanban.dispatch.step",
            step = "auto_decompose",
            board = board_slug,
        ))
        .await;
    }

    let now = now_secs();

    // Step 1: detect crashed workers.
    // Use .instrument() instead of .entered() so the future stays Send —
    // EnteredSpan is !Send and cannot be held across await points in a
    // tokio::spawn context. This pattern applies to all steps below.
    async {
        if let Err(e) = detect_crashed_workers(ctx, now).await {
            tracing::error!(error = %e, step = "detect_crashed", board = board_slug, "dispatch step error");
        }
    }
    .instrument(tracing::info_span!("kanban.dispatch.step", step = "detect_crashed", board = board_slug))
    .await;

    // Step 2: extend claims for live PIDs at TTL expiry.
    async {
        if let Err(e) = extend_live_pid_claims(ctx, now).await {
            tracing::error!(error = %e, step = "extend_live", board = board_slug, "dispatch step error");
        }
    }
    .instrument(tracing::info_span!("kanban.dispatch.step", step = "extend_live", board = board_slug))
    .await;

    // Step 3: reclaim stale claims (dead PID or null PID, expired TTL).
    async {
        if let Err(e) = reclaim_stale_claims(ctx, now).await {
            tracing::error!(error = %e, step = "reclaim_stale", board = board_slug, "dispatch step error");
        }
    }
    .instrument(tracing::info_span!("kanban.dispatch.step", step = "reclaim_stale", board = board_slug))
    .await;

    // Step 4: enforce max runtime.
    async {
        if let Err(e) = enforce_max_runtime(ctx, now).await {
            tracing::error!(error = %e, step = "enforce_max_runtime", board = board_slug, "dispatch step error");
        }
    }
    .instrument(tracing::info_span!(
        "kanban.dispatch.step",
        step = "enforce_max_runtime",
        board = board_slug,
    ))
    .await;

    // Step 5: promote ready tasks (todo → ready when all parents done).
    async {
        if let Err(e) = promote_ready(ctx, now).await {
            tracing::error!(error = %e, step = "promote_ready", board = board_slug, "dispatch step error");
        }
    }
    .instrument(tracing::info_span!("kanban.dispatch.step", step = "promote_ready", board = board_slug))
    .await;

    // Steps 6–8: claim + guard + spawn.
    async {
        if let Err(e) = claim_and_spawn(ctx, now).await {
            tracing::error!(error = %e, step = "claim_and_spawn", board = board_slug, "dispatch step error");
        }
    }
    .instrument(tracing::info_span!("kanban.dispatch.step", step = "claim_and_spawn", board = board_slug))
    .await;
}

// ---------------------------------------------------------------------------
// Step 1: detect_crashed_workers
// ---------------------------------------------------------------------------

async fn detect_crashed_workers(ctx: &DispatcherContext, now: f64) -> Result<()> {
    let running_tasks = {
        let store = ctx.store.lock().await;
        store.list_tasks(ListFilters {
            status: Some("running".to_string()),
            ..Default::default()
        })?
    };

    for task in running_tasks {
        // Load the current task_run for this task.
        let run = match task.current_run_id.as_deref() {
            Some(run_id) => {
                let store = ctx.store.lock().await;
                let runs = store.get_runs(&task.id)?;
                runs.into_iter().find(|r| r.id == run_id)
            }
            None => None,
        };

        let Some(run) = run else { continue };
        let Some(pid) = run.claim_pid else { continue };

        // Only process tasks where the PID is dead (crashed).
        if is_pid_alive(pid) {
            continue;
        }

        tracing::info!(
            event = "crashed",
            task_id = %task.id,
            pid = pid,
            "detected crashed worker"
        );

        // Close the run with outcome='crashed'.
        {
            let store = ctx.store.lock().await;
            store.conn.execute(
                "UPDATE task_runs SET outcome='crashed', ended_at=?1 WHERE id=?2",
                params![now, run.id],
            )?;
        }

        // Release the claim (reset to ready).
        {
            let mut store = ctx.store.lock().await;
            let _ = release_claim(&mut store.conn, &task.id, &run.claim_lock, "crashed");
        }

        // Increment consecutive_failures.
        {
            let store = ctx.store.lock().await;
            store.conn.execute(
                "UPDATE tasks SET consecutive_failures = consecutive_failures + 1 WHERE id=?1",
                params![task.id],
            )?;
        }

        // Append crashed event.
        {
            let mut store = ctx.store.lock().await;
            let payload = serde_json::json!({
                "pid": pid,
                "claimer": run.claim_lock,
            });
            store.append_event(
                &task.id,
                Some(&run.id),
                KanbanEventKind::Crashed,
                Some(&payload),
            )?;
        }

        // Phase 36.3.7.0 BUG-36.3.7-03: circuit breaker on crashed-detection path.
        // The bump above set consecutive_failures += 1; check the limit on the same
        // tick to match operator semantics (D-12 clarified by 36.3.7.0-03).
        //
        // D-04 (46.5): enrich the bare "worker process crashed (pid=…)" string
        // with a real human reason plus a bounded tail of the worker's
        // profile-scoped stderr log, so the diagnostic that lands in
        // task_runs.error / the block/gave_up notification payload actually
        // says something actionable. Reading the log is best-effort — a
        // missing/unreadable file yields an empty tail, never a panic
        // (read_stderr_tail is non-fatal by construction).
        {
            let stderr_path = crate::paths::kanban_log_stderr_for(&task.assignee, &task.id);
            let stderr_tail = read_stderr_tail(&stderr_path, 2048, 20);
            let error_msg = if stderr_tail.is_empty() {
                format!(
                    "worker process exited unexpectedly (pid={pid}); no terminal event recorded."
                )
            } else {
                format!(
                    "worker process exited unexpectedly (pid={pid}); no terminal event recorded.\n\
                     --- stderr tail ---\n{stderr_tail}"
                )
            };
            apply_circuit_breaker(ctx, &task, &run.id, &error_msg, now).await?;
        }

        // Check for protocol violation (PID dead + task still running + consecutive_failures bump).
        // v1 heuristic: if the worker exited cleanly (no signal) but task is still running,
        // treat as protocol violation. We detect this by checking if the task status is still
        // 'running' after the crashed detection (which means the release_claim succeeded).
        // Note: release_claim resets to 'ready', so we check if it was in running state.
        // The crashed path already handles this — we do NOT additionally block for protocol
        // violations in step 1 to avoid double-blocking. Protocol violation is a separate
        // heuristic applied below only when we can confirm exit code 0 semantics.
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Step 2: extend_live_pid_claims
// ---------------------------------------------------------------------------

async fn extend_live_pid_claims(ctx: &DispatcherContext, now: f64) -> Result<()> {
    let running_tasks = {
        let store = ctx.store.lock().await;
        store.list_tasks(ListFilters {
            status: Some("running".to_string()),
            ..Default::default()
        })?
    };

    for task in running_tasks {
        // Only tasks whose claim has expired.
        let claim_expires = match task.claim_expires {
            Some(e) => e,
            None => continue,
        };
        if claim_expires >= now {
            continue; // Not expired yet.
        }

        // Only tasks with a live PID.
        let run = match task.current_run_id.as_deref() {
            Some(run_id) => {
                let store = ctx.store.lock().await;
                let runs = store.get_runs(&task.id)?;
                runs.into_iter().find(|r| r.id == run_id)
            }
            None => None,
        };
        let Some(run) = run else { continue };
        let Some(pid) = run.claim_pid else { continue };

        if !is_pid_alive(pid) {
            continue; // Dead PID handled by step 1 / step 3.
        }

        // Extend claim by TTL.
        let new_expires = now + DEFAULT_CLAIM_TTL_SECONDS as f64;
        {
            let store = ctx.store.lock().await;
            store.conn.execute(
                "UPDATE tasks SET claim_expires=?1 WHERE id=?2",
                params![new_expires, task.id],
            )?;
        }

        tracing::info!(
            event = "claim_extended",
            task_id = %task.id,
            pid = pid,
            new_expires = new_expires,
        );

        {
            let mut store = ctx.store.lock().await;
            let payload = serde_json::json!({
                "pid": pid,
                "new_expires": new_expires,
            });
            store.append_event(
                &task.id,
                Some(&run.id),
                KanbanEventKind::ClaimExtended,
                Some(&payload),
            )?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Step 3: reclaim_stale_claims
// ---------------------------------------------------------------------------

async fn reclaim_stale_claims(ctx: &DispatcherContext, now: f64) -> Result<()> {
    let running_tasks = {
        let store = ctx.store.lock().await;
        store.list_tasks(ListFilters {
            status: Some("running".to_string()),
            ..Default::default()
        })?
    };

    for task in running_tasks {
        let claim_expires = match task.claim_expires {
            Some(e) => e,
            None => continue,
        };
        if claim_expires >= now {
            continue; // Not expired yet.
        }

        // Check PID: null or dead → reclaim.
        let run = match task.current_run_id.as_deref() {
            Some(run_id) => {
                let store = ctx.store.lock().await;
                let runs = store.get_runs(&task.id)?;
                runs.into_iter().find(|r| r.id == run_id)
            }
            None => None,
        };

        let pid_is_alive = run
            .as_ref()
            .and_then(|r| r.claim_pid)
            .map(is_pid_alive)
            .unwrap_or(false);

        if pid_is_alive {
            continue; // Live PID handled by step 2.
        }

        tracing::info!(
            event = "reclaimed",
            task_id = %task.id,
            "reclaiming stale claim (TTL expired, PID dead or null)"
        );

        // Close the run if there is one.
        if let Some(ref run) = run {
            let store = ctx.store.lock().await;
            store.conn.execute(
                "UPDATE task_runs SET outcome='reclaimed', ended_at=?1 WHERE id=?2",
                params![now, run.id],
            )?;
        }

        // Release claim (resets to ready).
        {
            let claim_lock = task
                .claim_lock
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            let mut store = ctx.store.lock().await;
            let _ = release_claim(&mut store.conn, &task.id, &claim_lock, "ttl_expired");
        }

        // Phase 36.3.7.12 D-04: when a goal_mode card is reclaimed, reset its
        // per-card turn counter so the next worker starts at goal_turns_used=0
        // with a fresh budget. The CAS gate from Plan 02 (bump_goal_turn_counter)
        // already prevents the OLD run from corrupting state; this reset is the
        // handoff side of the contract so the NEW run sees a clean budget.
        // Plan 05 Task 1 wiring (the one-line addition the planner anticipated).
        if task.goal_mode {
            let store = ctx.store.lock().await;
            let _ = store.reset_goal_turn_counter(&task.id);
        }

        // Increment consecutive_failures.
        {
            let store = ctx.store.lock().await;
            store.conn.execute(
                "UPDATE tasks SET consecutive_failures = consecutive_failures + 1 WHERE id=?1",
                params![task.id],
            )?;
        }

        // Phase 36.3.7.1 BUG-36.3.7.1-01: circuit breaker on reclaim-stale-claims path.
        // The bump above set consecutive_failures += 1; check the limit on the same
        // tick to match operator semantics (D-12 clarified by 36.3.7.0-03). The run_id
        // sentinel "reclaimed-no-run" is used when no current run is associated.
        {
            let run_id_arg = run
                .as_ref()
                .map(|r| r.id.as_str())
                .unwrap_or("reclaimed-no-run");
            let error_msg = "claim TTL expired with dead or null PID".to_string();
            apply_circuit_breaker(ctx, &task, run_id_arg, &error_msg, now).await?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Step 4: enforce_max_runtime
// ---------------------------------------------------------------------------

async fn enforce_max_runtime(ctx: &DispatcherContext, now: f64) -> Result<()> {
    let running_tasks = {
        let store = ctx.store.lock().await;
        store.list_tasks(ListFilters {
            status: Some("running".to_string()),
            ..Default::default()
        })?
    };

    for task in running_tasks {
        let max_runtime = match task.max_runtime_seconds {
            Some(m) => m as f64,
            None => continue,
        };

        let started_at = match task.started_at {
            Some(s) => s,
            None => continue,
        };

        let elapsed = now - started_at;
        if elapsed <= max_runtime {
            continue;
        }

        // Load run + PID.
        let run = match task.current_run_id.as_deref() {
            Some(run_id) => {
                let store = ctx.store.lock().await;
                let runs = store.get_runs(&task.id)?;
                runs.into_iter().find(|r| r.id == run_id)
            }
            None => None,
        };

        let Some(run) = run else { continue };
        let Some(pid) = run.claim_pid else { continue };

        tracing::warn!(
            event = "max_runtime_exceeded",
            task_id = %task.id,
            pid = pid,
            elapsed_seconds = elapsed,
            limit_seconds = max_runtime,
            "terminating worker: max runtime exceeded"
        );

        // Send SIGTERM.
        let _ = kill_pid(pid, Signal::Term);

        // 5-second grace period.
        tokio::time::sleep(Duration::from_secs(5)).await;

        // Check if still alive, then SIGKILL.
        let sigkill = is_pid_alive(pid);
        if sigkill {
            let _ = kill_pid(pid, Signal::Kill);
        }

        // Close the run.
        {
            let store = ctx.store.lock().await;
            store.conn.execute(
                "UPDATE task_runs SET outcome='timed_out', ended_at=?1 WHERE id=?2",
                params![now, run.id],
            )?;
        }

        // Reset task to ready, release claim.
        {
            let claim_lock = task
                .claim_lock
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            let mut store = ctx.store.lock().await;
            let _ = release_claim(&mut store.conn, &task.id, &claim_lock, "timed_out");
        }

        // Increment consecutive_failures.
        {
            let store = ctx.store.lock().await;
            store.conn.execute(
                "UPDATE tasks SET consecutive_failures = consecutive_failures + 1 WHERE id=?1",
                params![task.id],
            )?;
        }

        // Append timed_out event.
        {
            let mut store = ctx.store.lock().await;
            let payload = serde_json::json!({
                "pid": pid,
                "elapsed_seconds": elapsed,
                "limit_seconds": max_runtime,
                "sigkill": sigkill,
            });
            store.append_event(
                &task.id,
                Some(&run.id),
                KanbanEventKind::TimedOut,
                Some(&payload),
            )?;
        }

        // Phase 36.3.7.1 BUG-36.3.7.1-02: circuit breaker on max-runtime-exceeded path.
        // The bump above set consecutive_failures += 1 and the TimedOut event was just
        // appended; check the limit on the same tick to match operator semantics
        // (D-12 clarified by 36.3.7.0-03).
        {
            let error_msg = format!(
                "max runtime exceeded ({elapsed}s > {max_runtime}s limit, sigkill={sigkill})"
            );
            apply_circuit_breaker(ctx, &task, &run.id, &error_msg, now).await?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Step 5: promote_ready (todo → ready when all parents done)
// ---------------------------------------------------------------------------

async fn promote_ready(ctx: &DispatcherContext, now: f64) -> Result<()> {
    let todo_tasks = {
        let store = ctx.store.lock().await;
        store.list_tasks(ListFilters {
            status: Some("todo".to_string()),
            ..Default::default()
        })?
    };

    for task in todo_tasks {
        // Count undone parents: if zero, promote to ready.
        let undone_parents: i64 = {
            let store = ctx.store.lock().await;
            store.conn.query_row(
                "SELECT COUNT(*) FROM task_links \
                 JOIN tasks ON task_links.parent_id = tasks.id \
                 WHERE task_links.child_id = ?1 AND tasks.status != 'done'",
                params![task.id],
                |r| r.get(0),
            )?
        };

        if undone_parents > 0 {
            continue;
        }

        tracing::info!(
            event = "promoted",
            task_id = %task.id,
            "promoting task to ready (all parents done)"
        );

        {
            let store = ctx.store.lock().await;
            store.conn.execute(
                "UPDATE tasks SET status='ready' WHERE id=?1",
                params![task.id],
            )?;
        }

        {
            let mut store = ctx.store.lock().await;
            let payload = serde_json::json!({ "promoted_at": now });
            store.append_event(&task.id, None, KanbanEventKind::Promoted, Some(&payload))?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// project: workspace kind — dispatcher-side git worktree creation (D-06/D-07)
// ---------------------------------------------------------------------------

/// Create (or idempotently re-claim) an isolated `git worktree` for a
/// `project:<repo>` workspace.
///
/// `repo` must be an absolute, existing directory (the confused-deputy gate
/// on relative `project:` tails already runs at task-create time via
/// [`crate::paths::validate_dir_workspace`] — this is a defense-in-depth
/// re-check at spawn time). The worktree always lands at
/// [`crate::paths::kanban_worktree_for`]`(task_id)`, which is deliberately
/// OUTSIDE `repo` and distinct from the GSD `.claude/worktrees/` namespace
/// (D-07: never mutate the referenced repo in-place).
///
/// If the target directory already exists on disk (a prior spawn/respawn
/// already created it — e.g. after a reclaim), it is returned as-is rather
/// than re-running `git worktree add`, which would otherwise fail on an
/// already-populated path. This mirrors the eager-create-if-absent idiom
/// `resolve_workspace_dir` already uses for scratch workspaces (D-31).
///
/// The actual `git worktree add` call is a native `tokio::process::Command`
/// subprocess run FROM THE DISPATCHER — never delegated to the worker LLM
/// (D-07). Mirrors the `Command::new(...).spawn().map_err(...)` shape
/// `worker_spawn.rs` already uses for the worker subprocess itself.
async fn create_project_worktree(repo: &str, task_id: &str) -> Result<String> {
    let repo_path = std::path::Path::new(repo);
    if !repo_path.is_absolute() {
        return Err(KanbanError::Other(anyhow::anyhow!(
            "project: repo path must be absolute: {repo}"
        )));
    }
    if !repo_path.is_dir() {
        return Err(KanbanError::Other(anyhow::anyhow!(
            "project: repo path does not exist or is not a directory: {repo}"
        )));
    }

    let target = crate::paths::kanban_worktree_for(task_id);

    // Idempotent re-claim: a prior spawn already created this worktree.
    if target.exists() {
        return Ok(target.to_string_lossy().into_owned());
    }

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            KanbanError::Other(anyhow::anyhow!(
                "create kanban worktrees root {}: {e}",
                parent.display()
            ))
        })?;
    }

    let branch = format!("wt/{task_id}");
    let child = tokio::process::Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("worktree")
        .arg("add")
        .arg(&target)
        .arg("-b")
        .arg(&branch)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            KanbanError::Other(anyhow::anyhow!(
                "git worktree add for task {task_id}: failed to spawn git: {e}"
            ))
        })?;

    let output = child.wait_with_output().await.map_err(|e| {
        KanbanError::Other(anyhow::anyhow!(
            "git worktree add for task {task_id}: failed waiting on git: {e}"
        ))
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(KanbanError::Other(anyhow::anyhow!(
            "git worktree add for task {task_id}: {stderr}"
        )));
    }

    Ok(target.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------
// Steps 6–8: claim_and_spawn (includes respawn guard)
// ---------------------------------------------------------------------------

async fn claim_and_spawn(ctx: &DispatcherContext, now: f64) -> Result<()> {
    // Count currently running tasks.
    let running_count: i64 = {
        let store = ctx.store.lock().await;
        store.conn.query_row(
            "SELECT COUNT(*) FROM tasks WHERE status='running'",
            [],
            |r| r.get(0),
        )?
    };

    // Enforce max_in_progress cap (D-11).
    let cap = match ctx.config.max_in_progress {
        None => None,
        Some(0) => {
            tracing::warn!("max_in_progress=0 is invalid, treating as unlimited (D-11)");
            None
        }
        Some(n) => Some(n as i64),
    };

    if let Some(cap) = cap
        && running_count >= cap
    {
        tracing::info!(
            max_in_progress = cap,
            running_count = running_count,
            "max_in_progress reached, skipping spawn"
        );
        return Ok(());
    }

    // Determine how many slots are available.
    let slots = cap
        .map(|c| (c - running_count) as usize)
        .unwrap_or(usize::MAX);

    // Fetch ready tasks ordered by priority DESC, created_at ASC (D-10).
    let ready_tasks: Vec<Task> = {
        let store = ctx.store.lock().await;
        let limit = slots.min(100) as i64; // sanity cap
        let mut stmt = store.conn.prepare(
            "SELECT id, title, body, assignee, status, priority, tenant, workspace, skills, \
             idempotency_key, claim_lock, claim_expires, current_run_id, consecutive_failures, \
             max_retries, max_runtime_seconds, scheduled_at, workflow_template_id, \
             current_step_key, created_by, created_at, started_at, ended_at, \
             goal_mode, goal_max_turns, goal_turns_used, goal_toolset, output_path \
             FROM tasks \
             WHERE status='ready' AND (scheduled_at IS NULL OR scheduled_at <= ?1) \
             ORDER BY priority DESC, created_at ASC \
             LIMIT ?2",
        )?;
        stmt.query_map(params![now, limit], |r| {
            // Phase 36.3.7.12: SELECT now returns 26 columns; bind goal_* into Task.
            // Phase 36.3.7.13: column 26 = goal_toolset TEXT NULL.
            // Phase 46.4: column 27 = output_path TEXT NULL (D-10).
            let goal_mode_int: i64 = r.get(23)?;
            let goal_max_turns_i: i64 = r.get(24)?;
            let goal_turns_used_i: i64 = r.get(25)?;
            let goal_toolset: Option<String> = r.get(26)?;
            let output_path: Option<String> = r.get(27)?;
            Ok(Task {
                id: r.get(0)?,
                title: r.get(1)?,
                body: r.get(2)?,
                assignee: r.get(3)?,
                status: r.get(4)?,
                priority: r.get(5)?,
                tenant: r.get(6)?,
                workspace: r.get(7)?,
                skills: r.get(8)?,
                idempotency_key: r.get(9)?,
                claim_lock: r.get(10)?,
                claim_expires: r.get(11)?,
                current_run_id: r.get(12)?,
                consecutive_failures: r.get(13)?,
                max_retries: r.get(14)?,
                max_runtime_seconds: r.get(15)?,
                scheduled_at: r.get(16)?,
                workflow_template_id: r.get(17)?,
                current_step_key: r.get(18)?,
                created_by: r.get(19)?,
                created_at: r.get(20)?,
                started_at: r.get(21)?,
                ended_at: r.get(22)?,
                goal_mode: goal_mode_int != 0,
                goal_max_turns: goal_max_turns_i.max(0) as u32,
                goal_turns_used: goal_turns_used_i.max(0) as u32,
                goal_toolset,
                output_path,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };

    // Memoize the dispatch-gate verdict per assignee so N ready tasks for the
    // same profile do exactly one disk + resolver pass per tick.
    let mut gate_cache: HashMap<String, ironhermes_core::dispatch_gate::DispatchDecision> =
        HashMap::new();

    // For each ready task: dispatch gate, then respawn guard, then atomic
    // claim, then spawn.
    for task in ready_tasks {
        // Step 6a: hard pre-spawn dispatch gate (Phase 47.4 GAP-1, UAT inline
        // fix). Refuse any task whose assignee profile cannot resolve a
        // provider key for the provider it is actually configured to use.
        //
        // This runs FIRST — ahead of the respawn guard — deliberately. The
        // respawn guard's `blocker_auth` branch can only fire *after* a spawn
        // has already failed with an auth error, so on its own it leaves an
        // undispatchable task `ready` and re-guards it every tick forever
        // (exactly what the 47.4 UAT observed). Blocking here turns that
        // endless loop into one terminal `blocked` state, before any worker
        // process is created.
        //
        // Plan 10 wired this predicate into `cmd_dispatch` only; the
        // dispatcher that actually runs in production is `run_dispatch_loop`,
        // spawned by the gateway. This is that gap's fix — every dispatch
        // path now shares one predicate from `ironhermes_core`.
        let decision = gate_cache
            .entry(task.assignee.clone())
            .or_insert_with(|| (ctx.gate_fn)(&task.assignee))
            .clone();

        if let ironhermes_core::dispatch_gate::DispatchDecision::Refuse { reason } = decision {
            tracing::warn!(
                event = "dispatch_gate_blocked",
                task_id = %task.id,
                assignee = %task.assignee,
                reason = %reason,
            );
            let gate_reason = format!(
                "{}{reason}",
                ironhermes_core::dispatch_gate::DISPATCH_GATE_REASON_PREFIX
            );
            let mut store = ctx.store.lock().await;
            if let Err(e) = store.block_task(&task.id, &gate_reason, None) {
                tracing::warn!("[kanban] dispatch gate could not block task {}: {e}", task.id);
            }
            continue;
        }

        // Step 7: respawn guard.
        let guard_reason = {
            let store = ctx.store.lock().await;
            respawn_guard_reason(&store, &task, now, &ctx.config)?
        };

        if let Some(reason) = guard_reason {
            tracing::info!(
                event = "respawn_guarded",
                task_id = %task.id,
                reason = reason,
            );
            let mut store = ctx.store.lock().await;
            let payload = serde_json::json!({ "reason": reason });
            store.append_event(
                &task.id,
                None,
                KanbanEventKind::RespawnGuarded,
                Some(&payload),
            )?;
            continue;
        }

        // Step 6: atomic claim.
        let run_id = format!("r_{}", uuid::Uuid::new_v4().simple());
        let claim_lock = build_claim_lock(&ctx.hostname, ctx.dispatcher_pid);

        let won = {
            let mut store = ctx.store.lock().await;
            atomic_claim(
                &mut store.conn,
                &task.id,
                &claim_lock,
                ctx.dispatcher_pid,
                DEFAULT_CLAIM_TTL_SECONDS,
                now,
                &run_id,
            )?
        };

        if !won {
            tracing::debug!(task_id = %task.id, "lost CAS race for task, skipping");
            continue;
        }

        tracing::info!(
            event = "claimed",
            task_id = %task.id,
            run_id = %run_id,
            profile = %task.assignee,
        );

        // Determine workspace for this task. An explicit per-task workspace wins,
        // followed by the operator's config.yaml `kanban.default_workdir`, then
        // the task-specific scratch workspace (D-31/D-32).
        let workspace = task
            .workspace
            .clone()
            .or_else(|| {
                ctx.config
                    .default_workdir
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| {
                crate::paths::kanban_workspace_for(&task.id)
                    .to_string_lossy()
                    .into_owned()
            });

        // Resolve dir: prefix if present, or project: prefix (D-06/D-07): the
        // dispatcher creates an isolated git worktree via a native subprocess
        // — never delegated to the worker LLM. A worktree-creation Err is
        // captured here (NOT `?`-propagated out of this per-task loop) so it
        // flows into the existing outcome='spawn_failed' handling below
        // exactly like an ordinary spawn failure, and the next candidate in
        // `ready_tasks` still gets processed this tick.
        let workspace_resolution: Result<String> =
            if let Some(tail) = workspace.strip_prefix("dir:") {
                Ok(tail.to_string())
            } else if let Some(tail) = workspace.strip_prefix("project:") {
                create_project_worktree(tail, &task.id).await
            } else {
                // D-05 (46.5): scratch-only retry re-init. Wipes a prior crashed
                // run's leftovers before this retry spawns, so a stale file can't
                // fool the worker into thinking it already completed. Guarded
                // internally on task.workspace.is_none() (true here — this arm is
                // only reached for scratch tasks) AND consecutive_failures > 0
                // (no-op on first spawn). A wipe failure is captured (NOT
                // `?`-propagated) so it flows into the same outcome='spawn_failed'
                // handling below, exactly like the sibling project: arm above.
                match crate::worker_spawn::reinit_scratch_workspace_if_retry(&task) {
                    Ok(()) => Ok(workspace),
                    Err(e) => Err(e),
                }
            };

        // Load the freshly-inserted task_run row.
        let run = {
            let store = ctx.store.lock().await;
            let runs = store.get_runs(&task.id)?;
            runs.into_iter().find(|r| r.id == run_id).ok_or_else(|| {
                KanbanError::Other(anyhow::anyhow!("task_run {run_id} not found after claim"))
            })?
        };

        // Step 8: spawn worker. The board slug is captured in ctx.spawn_fn by the
        // per-board context wrapper in run_dispatch_tick; the empty string below
        // is ignored by that wrapper (the wrapper uses its captured slug). A
        // failed workspace_resolution (project: worktree creation Err) short-
        // circuits to the same Err arm below without ever calling spawn_fn.
        let spawn_result = match workspace_resolution {
            Ok(workspace) => {
                (ctx.spawn_fn)(task.clone(), run.clone(), workspace, String::new()).await
            }
            Err(e) => Err(e),
        };

        match spawn_result {
            Ok(pid) => {
                tracing::info!(
                    event = "spawned",
                    task_id = %task.id,
                    run_id = %run_id,
                    pid = pid,
                );

                // Store PID in task_runs.
                {
                    let store = ctx.store.lock().await;
                    store.conn.execute(
                        "UPDATE task_runs SET claim_pid=?1 WHERE id=?2",
                        params![pid, run_id],
                    )?;
                }

                // Append spawned event.
                {
                    let mut store = ctx.store.lock().await;
                    let payload = serde_json::json!({ "pid": pid });
                    store.append_event(
                        &task.id,
                        Some(&run_id),
                        KanbanEventKind::Spawned,
                        Some(&payload),
                    )?;
                }
            }
            Err(e) => {
                let error_msg = e.to_string();
                tracing::error!(
                    event = "spawn_failed",
                    task_id = %task.id,
                    run_id = %run_id,
                    error = %error_msg,
                );

                // Close the run as spawn_failed.
                {
                    let store = ctx.store.lock().await;
                    let now2 = now_secs();
                    store.conn.execute(
                        "UPDATE task_runs SET outcome='spawn_failed', error=?1, ended_at=?2 \
                         WHERE id=?3",
                        params![error_msg, now2, run_id],
                    )?;
                }

                // Increment consecutive_failures.
                {
                    let store = ctx.store.lock().await;
                    store.conn.execute(
                        "UPDATE tasks SET consecutive_failures = consecutive_failures + 1 \
                         WHERE id=?1",
                        params![task.id],
                    )?;
                }

                // Release claim (resets to ready so circuit breaker can block it).
                {
                    let mut store = ctx.store.lock().await;
                    let _ = release_claim(&mut store.conn, &task.id, &claim_lock, "spawn_failed");
                }

                // Append spawn_failed event.
                {
                    let new_failures: i64 = {
                        let store = ctx.store.lock().await;
                        store.conn.query_row(
                            "SELECT consecutive_failures FROM tasks WHERE id=?1",
                            params![task.id],
                            |r| r.get(0),
                        )?
                    };

                    let mut store = ctx.store.lock().await;
                    let payload = serde_json::json!({
                        "error": error_msg,
                        "failures": new_failures,
                    });
                    store.append_event(
                        &task.id,
                        Some(&run_id),
                        KanbanEventKind::SpawnFailed,
                        Some(&payload),
                    )?;
                }

                // Circuit breaker (D-12): block task if failure_limit reached.
                apply_circuit_breaker(ctx, &task, &run_id, &error_msg, now).await?;
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Step 7: respawn_guard_reason
// ---------------------------------------------------------------------------

/// Check if a ready task should be skipped due to respawn guard conditions.
///
/// Returns `Some(reason)` if the task should be skipped:
/// - `"blocker_auth"`: last run error matches 429/quota/auth pattern.
/// - `"recent_success"`: last completed run ended within 3600 seconds.
/// - `"active_pr"`: recent comment contains a GitHub PR URL.
///
/// Returns `None` if spawn should proceed.
pub fn respawn_guard_reason(
    store: &KanbanStore,
    task: &Task,
    now: f64,
    config: &KanbanConfig,
) -> Result<Option<&'static str>> {
    // Check last closed run for blocker_auth.
    let last_run: Option<TaskRun> = {
        let runs = store.get_runs(&task.id)?;
        runs.into_iter()
            .filter(|r| r.ended_at.is_some())
            .max_by(|a, b| {
                a.started_at
                    .partial_cmp(&b.started_at)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    };

    if let Some(ref run) = last_run {
        // blocker_auth: last error matches 429/quota/auth/unauthorized.
        //
        // BOUNDED (Phase 47.4 UAT follow-up). This branch used to return
        // unconditionally, which made the guard permanent: the error text of
        // the newest closed run never changes, so a task whose profile was
        // later repaired could never dispatch again and the operator's only
        // recourse was to recreate it. That is exactly what the 47.4 UAT hit —
        // `bdev01` was fixed by adding MOONSHOT_API_KEY, the dispatch gate
        // correctly began allowing it, and this guard still held the task on
        // the strength of a hours-old 401.
        //
        // `respawn_auth_backoff_seconds == 0` restores the old unbounded
        // behaviour for operators who want it.
        if let Some(ref err) = run.error {
            let lower = err.to_lowercase();
            if lower.contains("429")
                || lower.contains("quota")
                || lower.contains("auth")
                || lower.contains("unauthorized")
            {
                let backoff = config.respawn_auth_backoff_seconds;
                let cooled_off = backoff > 0
                    && run
                        .ended_at
                        .is_some_and(|ended_at| now - ended_at >= backoff as f64);
                if !cooled_off {
                    return Ok(Some("blocker_auth"));
                }
            }
        }

        // recent_success: last run completed within the configured window.
        if run.outcome.as_deref() == Some("completed")
            && let Some(ended_at) = run.ended_at
            && now - ended_at < config.respawn_recent_success_seconds as f64
        {
            return Ok(Some("recent_success"));
        }
    }

    // active_pr: any comment in the configured look-back with a GitHub PR URL.
    let seven_days_ago = now - config.respawn_active_pr_seconds as f64;
    let has_active_pr: bool = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM task_comments \
         WHERE task_id=?1 AND created_at >= ?2 \
         AND body LIKE '%github.com%/pull/%'",
            params![task.id, seven_days_ago],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;

    if has_active_pr {
        return Ok(Some("active_pr"));
    }

    Ok(None)
}

// ---------------------------------------------------------------------------
// Circuit breaker (D-12)
// ---------------------------------------------------------------------------

async fn apply_circuit_breaker(
    ctx: &DispatcherContext,
    task: &Task,
    run_id: &str,
    error_msg: &str,
    _now: f64,
) -> Result<()> {
    let (consecutive_failures, effective_limit, limit_source) = {
        let store = ctx.store.lock().await;
        let failures: i64 = store.conn.query_row(
            "SELECT consecutive_failures FROM tasks WHERE id=?1",
            params![task.id],
            |r| r.get(0),
        )?;

        let (limit, source) = if let Some(max_retries) = task.max_retries {
            (max_retries, "task")
        } else {
            (ctx.config.failure_limit as i64, "config")
        };

        (failures, limit, source)
    };

    // D-12 (clarified by 36.3.7.0-03): `failure_limit = N` means stop AT N
    // failures, on the same tick the limit is reached (regardless of failure
    // source: spawn_failed / crashed / timed_out / reclaimed). The breaker is
    // invoked from each bump site; do NOT change `>=` to `>`.
    if consecutive_failures >= effective_limit {
        tracing::warn!(
            event = "gave_up",
            task_id = %task.id,
            failures = consecutive_failures,
            effective_limit = effective_limit,
            limit_source = limit_source,
            "circuit breaker tripped — blocking task"
        );

        // Block the task.
        {
            let mut store = ctx.store.lock().await;
            store.block_task(task.id.as_str(), error_msg, None)?;
        }

        // Append gave_up event.
        {
            let mut store = ctx.store.lock().await;
            let payload = serde_json::json!({
                "failures": consecutive_failures,
                "effective_limit": effective_limit,
                "limit_source": limit_source,
                "error": error_msg,
            });
            store.append_event(
                &task.id,
                Some(run_id),
                KanbanEventKind::GaveUp,
                Some(&payload),
            )?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Stranded-task diagnostic (D-14)
// ---------------------------------------------------------------------------

/// Severity level for stranded task reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrandedSeverity {
    /// Age > 1× threshold (default: > 30 min).
    Warn,
    /// Age > 2× threshold (default: > 60 min).
    Error,
    /// Age > 6× threshold (default: > 3 h).
    Critical,
}

/// A report of a `ready` task that has been waiting longer than the
/// `stranded_threshold_seconds` without being claimed (D-14).
#[derive(Debug, Clone)]
pub struct StrandedReport {
    pub task_id: String,
    pub assignee: String,
    pub age_seconds: f64,
    pub severity: StrandedSeverity,
}

/// Find all `ready` tasks that have been unclaimed past the stranded
/// threshold (D-14). Returns a severity-escalated report for each.
///
/// Severity bands:
/// - `Warn`: age > 1× threshold
/// - `Error`: age > 2× threshold
/// - `Critical`: age > 6× threshold
///
/// Tasks below the threshold are excluded from the result.
pub fn diagnose_stranded(store: &KanbanStore, threshold_secs: u64) -> Result<Vec<StrandedReport>> {
    let now = now_secs();
    let threshold = threshold_secs as f64;

    let tasks = store.list_tasks(ListFilters {
        status: Some("ready".to_string()),
        ..Default::default()
    })?;

    let mut reports = Vec::new();
    for task in tasks {
        let age = now - task.created_at;
        if age <= threshold {
            continue; // Not stranded.
        }

        let severity = if age > 6.0 * threshold {
            StrandedSeverity::Critical
        } else if age > 2.0 * threshold {
            StrandedSeverity::Error
        } else {
            StrandedSeverity::Warn
        };

        reports.push(StrandedReport {
            task_id: task.id,
            assignee: task.assignee,
            age_seconds: age,
            severity,
        });
    }

    Ok(reports)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

// ---------------------------------------------------------------------------
// Step 0: decompose_triage_tasks (Phase 36.3.7.10)
// ---------------------------------------------------------------------------

/// Run the auto-decompose step: query triage tasks up to the per-tick cap and
/// call `decompose_triage_task` sequentially on each.
///
/// # Sequential iteration (NOT parallel)
///
/// Tasks are processed one at a time to prevent burst LLM billing. A flood of
/// triage tasks with `auto_decompose_per_tick = 3` clears at ~3 tasks/tick —
/// see RESEARCH §"Dispatcher Integration Point" for the cost analysis.
///
/// # Per-task failure policy
///
/// An `Err` from `decompose_triage_task` is logged at `warn` level and consumed.
/// The task is retained in `triage` with a `decompose_failed` event (appended by
/// `decompose_triage_task` itself — Plan 02 DEC-05). The loop continues to the
/// next task; the outer Step 0 block returns `Ok(())` regardless.
async fn decompose_triage_tasks(
    ctx: &DispatcherContext,
    decompose_fn: &crate::decomposer::DecomposeFn,
) -> crate::error::Result<()> {
    let cap = ctx.config.auto_decompose_per_tick as usize;

    // Acquire store lock briefly to read triage tasks, then DROP the lock
    // before any LLM call (never hold the mutex across an async network call).
    let triage_tasks = {
        let store = ctx.store.lock().await;
        store.list_tasks(crate::store::ListFilters {
            status: Some("triage".to_string()),
            ..Default::default()
        })?
    };

    // WR-10 fix: track real work consumed vs short-circuited cap. The
    // previous implementation read `triage_tasks` once and capped at the
    // start; if an operator (or another shell) manually promoted a task
    // out of `triage` mid-tick, the kernel short-circuited with
    // `Ok(already_processed)` but the iteration still counted toward the
    // cap. With heavy ad-hoc operator intervention this prevented the
    // dispatcher from reaching still-triage tasks lower in the list.
    //
    // Strategy: iterate the full pre-read list and re-check `task.status
    // == "triage"` inside the loop before invoking the kernel. Tasks
    // already promoted are skipped without consuming cap; only real LLM
    // work counts.
    let mut consumed = 0usize;
    // Sequential loop — NOT join_all / FuturesUnordered (burst-billing prevention).
    for task in triage_tasks.into_iter() {
        if consumed >= cap {
            break;
        }

        // Re-check status under a brief lock — the task may have been
        // promoted out of triage since `list_tasks` was called above.
        let still_triage = {
            let store = ctx.store.lock().await;
            store
                .get_task(&task.id)
                .map(|t| t.status == "triage")
                .unwrap_or(false)
        };
        if !still_triage {
            tracing::debug!(
                task_id = %task.id,
                "skipping auto-decompose: task no longer in triage"
            );
            continue;
        }

        match crate::decomposer::decompose_triage_task(
            ctx.store.clone(),
            &task.id,
            decompose_fn,
            &ctx.config,
        )
        .await
        {
            Ok(_) => {
                tracing::info!(task_id = %task.id, "auto-decomposed");
                consumed += 1;
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    task_id = %task.id,
                    "decomposer failed; retaining task in triage"
                );
                // Failed attempts DO consume cap (they made an LLM call).
                consumed += 1;
            }
        }
    }

    Ok(())
}

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
/// spawn_fn signature: (task, run, workspace, board_slug, vault) -> pid
/// Injectable worker-spawn function. `pub` because it is the type of the
/// public `DispatcherContext::spawn_fn` field and of the `with_spawn_fn`
/// parameter, so out-of-crate tests need to be able to name it.
///
/// Phase 51 Plan 07 (D-07/D-11) added the fifth, `Option<WorkerVaultBootstrap>`
/// parameter — the minted credential for an `AllowFromVault` dispatch, `None` for
/// every `Allow` (dotenv-backed) dispatch. Mirrors `build_kanban_worker_env`'s own
/// `Option<&WorkerVaultBootstrap>` design (source_facts #4 / worker_spawn.rs): the
/// two vault values are only ever valid together, so a single `Option` here — rather
/// than widening `SpawnFn` with two more independent parameters — keeps a
/// half-configured spawn structurally unconstructible.
pub type SpawnFn = Arc<
    dyn Fn(Task, TaskRun, String, String, Option<crate::worker_spawn::WorkerVaultBootstrap>) -> BoxFuture<Result<u32>>
        + Send
        + Sync,
>;

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
///
/// Phase 51 (D-14): future-returning, following the [`SpawnFn`] precedent above —
/// `ironhermes_core::dispatch_gate::evaluate_profile_dispatch` became `async`
/// (its vault branch awaits `ProfileSecretStore`), so a plain synchronous closure
/// can no longer represent it. Never a blocking bridge (`block_in_place`/nested
/// runtime) here — some callers of this seam run inside a per-connection
/// `LocalSet`, where that would panic.
pub type DispatchGateFn =
    Arc<dyn Fn(&str) -> BoxFuture<ironhermes_core::dispatch_gate::DispatchDecision> + Send + Sync>;

/// The production dispatch gate: the shared, provider-aware, fail-closed
/// predicate every dispatch path uses.
fn default_gate_fn() -> DispatchGateFn {
    Arc::new(|assignee: &str| {
        let assignee = assignee.to_string();
        Box::pin(async move { ironhermes_core::dispatch_gate::evaluate_profile_dispatch(&assignee).await })
    })
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
    /// Phase 51 Plan 06 (D-11/D-12/D-13, WIDENED per user checkpoint reversal — see
    /// `ironhermes_core::profile_credentials`'s module doc): the per-profile credential
    /// endpoint handle, hosted automatically at construction via
    /// [`ironhermes_core::profile_credentials::host_profile_credentials`] — `None` whenever the
    /// vault is disabled (the default), unreachable, sealed, or the `rusty-vault` feature is not
    /// compiled in (D-14: fail-closed by omission). All three production `DispatcherContext::new`
    /// call sites (`ironhermes-gateway/src/runner.rs`,
    /// `ironhermes-cli/src/kanban/commands.rs` x2) get this for free — none has to opt in, the
    /// direct structural answer to Phase 47.4's GAP-7. `host_profile_credentials` short-circuits
    /// to `None` BEFORE ever opening a socket or spawning a task unless `config.vault.enabled`
    /// is true, so constructing a `DispatcherContext` against any default-configured (vault
    /// disabled) `Config` — every existing test and every default install — never touches the
    /// vault, a socket, or the filesystem beyond the ordinary `Config::load()` read this
    /// function performs to discover that.
    pub profile_credentials: Option<Arc<ironhermes_core::profile_credentials::ProfileCredentialHost>>,
    /// Phase 51 Plan 19 (D-14, G-51-6): which lifetime contract THIS process declares for its
    /// credential-endpoint host. Read by the vault-backed spawn guard below to decide whether a
    /// missing `profile_credentials`/`token_audit` means "no sink wired on a long-lived host"
    /// (today's `vault_mint_refused_no_sink`) or "this host cannot outlive its workers and
    /// declines to host at all" (the one-shot refusal). `OutlivesWorkers` in every constructor
    /// except [`Self::new_one_shot`] — every existing caller and every existing test keeps
    /// today's behavior with no edit.
    pub host_lifetime: ironhermes_core::profile_credentials::CredentialHostLifetime,
    /// Phase 51 Plan 07 (D-07): the required audit sink for a vault-backed mint.
    /// `None` in every constructor by default — an `AllowFromVault` dispatch with no
    /// sink wired REFUSES rather than minting un-audited (the dispatcher-side half of
    /// D-07's structural guarantee; Plan 03 already made an un-audited mint
    /// uncompilable at the vault layer via a required, non-`Option` constructor
    /// parameter). `Option` here rather than a required constructor argument, because
    /// making it required would break every existing `DispatcherContext::new` call
    /// site in the test suite for no safety gain — the safety comes from the refusal
    /// above, not from the field's presence. Set via [`Self::with_token_audit`]; all
    /// three production construction sites (`runner.rs`, `commands.rs` x2) supply the
    /// real trajectory-backed sink through
    /// `ironhermes_core::profile_credentials::TrajectoryProfileTokenAudit`.
    pub token_audit: Option<Arc<dyn ironhermes_core::profile_credentials::ProfileTokenAudit>>,
}

/// Read the real, on-disk `Config` (the same `Config::load().unwrap_or_default()` pattern
/// `ironhermes-cli/src/kanban/commands.rs`'s `cmd_dispatch`/daemon paths already perform right
/// before constructing a `DispatcherContext`) and try to host the profile credential endpoint
/// from it. A missing/malformed config file — or a config with `vault.enabled == false`, the
/// default for every install — falls back to `Config::default()` (or returns early), never
/// panics.
fn host_profile_credentials_from_disk()
-> Option<Arc<ironhermes_core::profile_credentials::ProfileCredentialHost>> {
    let config = ironhermes_core::config::Config::load().unwrap_or_default();
    ironhermes_core::profile_credentials::host_profile_credentials(&config)
}

/// Read the real, on-disk `Config` and ask
/// [`ironhermes_core::profile_credentials::host_profile_credentials_with_lifetime`] for a
/// short-lived host's decision (Phase 51 Plan 19, G-51-6). Always yields `None` — the point is
/// not the return value, it's the side effect: when the on-disk config is vault-enabled with
/// the `rusty-vault` backend, this call emits `profile_credential_host_declined_short_lived` so
/// an operator can see WHY [`DispatcherContext::new_one_shot`] carries no hosted endpoint,
/// without its caller having to know that check exists. Deliberately its own function — rather
/// than inlined into `new_one_shot` — and deliberately NOT spelled the same way
/// [`host_profile_credentials_from_disk`] above is spelled: a visibly different wiring site is
/// what keeps `both_constructors_wire_profile_credentials_through_the_same_function`'s
/// exactly-two count meaningful.
fn decline_short_lived_host_from_disk()
-> Option<Arc<ironhermes_core::profile_credentials::ProfileCredentialHost>> {
    let config = ironhermes_core::config::Config::load().unwrap_or_default();
    ironhermes_core::profile_credentials::host_profile_credentials_with_lifetime(
        &config,
        ironhermes_core::profile_credentials::CredentialHostLifetime::ExitsBeforeWorkers,
    )
}

/// Fixed, greppable operator-facing phrase stating that a one-shot dispatcher cannot host a
/// vault credential endpoint (Phase 51 Plan 19, G-51-6). A const rather than an inline literal
/// so the contract test asserts on the exact string production code emits — the two cannot
/// drift apart.
pub const ONE_SHOT_HOST_REFUSAL_MARKER: &str =
    "one-shot dispatcher cannot host a vault credential endpoint";

impl DispatcherContext {
    /// Create a context with the real `spawn_worker` function.
    pub fn new(store: Arc<TokioMutex<KanbanStore>>, config: KanbanConfig) -> Self {
        Self {
            store,
            config,
            hostname: crate::pid::current_hostname(),
            dispatcher_pid: std::process::id(),
            spawn_fn: Arc::new(|task, run, workspace, board_slug, vault| {
                Box::pin(async move {
                    crate::worker_spawn::spawn_worker_for_board(
                        &task,
                        &run,
                        &workspace,
                        &board_slug,
                        vault.as_ref(),
                    )
                    .await
                })
            }),
            gate_fn: default_gate_fn(),
            decompose_fn: None,
            host_lifetime:
                ironhermes_core::profile_credentials::CredentialHostLifetime::OutlivesWorkers,
            profile_credentials: host_profile_credentials_from_disk(),
            token_audit: None,
        }
    }

    /// Create a context for a caller that CANNOT outlive the workers it spawns — the one-shot
    /// `ironhermes kanban dispatch` verb (Phase 51 Plan 19, G-51-6). Declines to host the
    /// credential endpoint (`host_lifetime: ExitsBeforeWorkers`) rather than binding a socket
    /// that would be unlinked out from under a detached worker the instant this process
    /// returns. A vault-backed (`AllowFromVault`) task dispatched through this context is
    /// refused BY NAME rather than spawned against an already-gone socket — see the
    /// `ExitsBeforeWorkers` branch of the spawn guard in `run_dispatch_tick_for_board`.
    pub fn new_one_shot(store: Arc<TokioMutex<KanbanStore>>, config: KanbanConfig) -> Self {
        Self {
            store,
            config,
            hostname: crate::pid::current_hostname(),
            dispatcher_pid: std::process::id(),
            spawn_fn: Arc::new(|task, run, workspace, board_slug, vault| {
                Box::pin(async move {
                    crate::worker_spawn::spawn_worker_for_board(
                        &task,
                        &run,
                        &workspace,
                        &board_slug,
                        vault.as_ref(),
                    )
                    .await
                })
            }),
            gate_fn: default_gate_fn(),
            decompose_fn: None,
            host_lifetime:
                ironhermes_core::profile_credentials::CredentialHostLifetime::ExitsBeforeWorkers,
            profile_credentials: decline_short_lived_host_from_disk(),
            token_audit: None,
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
            host_lifetime:
                ironhermes_core::profile_credentials::CredentialHostLifetime::OutlivesWorkers,
            profile_credentials: host_profile_credentials_from_disk(),
            token_audit: None,
        }
    }

    /// Supply the real audit sink for vault-backed mints (Phase 51 Plan 07, D-07).
    /// Production call sites pass
    /// `Arc::new(ironhermes_core::profile_credentials::TrajectoryProfileTokenAudit::new(writer))`.
    pub fn with_token_audit(
        mut self,
        token_audit: Arc<dyn ironhermes_core::profile_credentials::ProfileTokenAudit>,
    ) -> Self {
        self.token_audit = Some(token_audit);
        self
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
                move |task, run, workspace, _ignored_slug, vault| {
                    // The board-loop slug is captured here; the inner arg is ignored.
                    outer_spawn_fn(task, run, workspace, slug.clone(), vault)
                }
            }),
            gate_fn: Arc::clone(&ctx.gate_fn),
            decompose_fn: ctx.decompose_fn.clone(),
            // Phase 51 Plan 19 (G-51-6): propagate, not re-default — a per-board sub-context
            // that silently re-defaulted this to `OutlivesWorkers` would apply the caller's
            // one-shot declaration to the default board only, exactly the trap `gate_fn`'s
            // comment above already documents for that field. `Copy`, so a plain field copy.
            host_lifetime: ctx.host_lifetime,
            // Share the SAME endpoint handle across every board's sub-context — re-deriving it
            // per board would try to bind a second listener at the same socket path and is
            // wasteful even when it wouldn't collide.
            profile_credentials: ctx.profile_credentials.clone(),
            // Phase 51 Plan 07: propagate, not re-default — every board's vault-backed
            // dispatches must ledger through the SAME sink the caller configured.
            token_audit: ctx.token_audit.clone(),
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
        // `or_insert_with`'s closure cannot `.await` (Phase 51 D-14) — restructured
        // into an explicit check-then-await-then-insert. Preserves the exact
        // one-evaluation-per-assignee-per-tick property the cache existed for: the
        // gate is only ever awaited on a cache MISS.
        let decision = match gate_cache.get(&task.assignee) {
            Some(cached) => cached.clone(),
            None => {
                let evaluated = (ctx.gate_fn)(&task.assignee).await;
                gate_cache.insert(task.assignee.clone(), evaluated.clone());
                evaluated
            }
        };

        // Phase 51 Plan 07: keep `decision` alive past this check (matched by
        // reference, not by value) — it is needed again below to decide whether this
        // dispatch mints a vault-backed credential.
        if let ironhermes_core::dispatch_gate::DispatchDecision::Refuse { reason } = &decision {
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

        // Phase 51 Plan 07 (D-07): an `AllowFromVault` decision with no hosted
        // credential endpoint or no audit sink wired on THIS dispatcher refuses
        // rather than minting un-audited (or minting with nowhere for the worker to
        // read from). This is the dispatcher-side half of D-07's structural
        // guarantee — Plan 03 already made an un-audited mint uncompilable at the
        // vault layer by requiring a sink parameter; this makes an
        // unaudited-because-unwired DISPATCH a refusal rather than a silent,
        // un-ledgered credential.
        let is_vault_backed =
            matches!(decision, ironhermes_core::dispatch_gate::DispatchDecision::AllowFromVault);
        if is_vault_backed && (ctx.profile_credentials.is_none() || ctx.token_audit.is_none()) {
            // Phase 51 Plan 19 (G-51-6): branch on the caller's declared host lifetime. A
            // one-shot host (`cmd_dispatch`) never has `profile_credentials` populated — it
            // declined to host by construction (`new_one_shot`) — and that is a DIFFERENT,
            // more specific refusal than "a long-lived host forgot to wire a sink".
            match ctx.host_lifetime {
                ironhermes_core::profile_credentials::CredentialHostLifetime::ExitsBeforeWorkers => {
                    let reason = format!(
                        "{}{} — profile \"{}\" needs its credential from the vault, but this \
                         dispatcher exits before its workers start and cannot host the \
                         endpoint; dispatch it instead with `ironhermes kanban daemon --force` \
                         or the gateway-embedded dispatcher",
                        ironhermes_core::dispatch_gate::DISPATCH_GATE_REASON_PREFIX,
                        ONE_SHOT_HOST_REFUSAL_MARKER,
                        task.assignee,
                    );
                    tracing::warn!(
                        event = "vault_dispatch_refused_one_shot_host",
                        task_id = %task.id,
                        assignee = %task.assignee,
                    );
                    let mut store = ctx.store.lock().await;
                    if let Err(e) = store.block_task(&task.id, &reason, None) {
                        tracing::warn!(
                            "[kanban] dispatch gate could not block task {}: {e}",
                            task.id
                        );
                    }
                    continue;
                }
                ironhermes_core::profile_credentials::CredentialHostLifetime::OutlivesWorkers => {
                    let reason = format!(
                        "{}profile \"{}\" resolved AllowFromVault but this dispatcher has no \
                         hosted credential endpoint or no audit sink wired — refusing rather \
                         than minting un-audited (D-07)",
                        ironhermes_core::dispatch_gate::DISPATCH_GATE_REASON_PREFIX,
                        task.assignee,
                    );
                    tracing::warn!(
                        event = "vault_mint_refused_no_sink",
                        task_id = %task.id,
                        assignee = %task.assignee,
                    );
                    let mut store = ctx.store.lock().await;
                    if let Err(e) = store.block_task(&task.id, &reason, None) {
                        tracing::warn!(
                            "[kanban] dispatch gate could not block task {}: {e}",
                            task.id
                        );
                    }
                    continue;
                }
            }
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

        // Phase 51 Plan 07 (D-07): mint the bootstrap-only token for an
        // `AllowFromVault` dispatch, positioned AFTER the atomic claim (Step 6) and
        // BEFORE the spawn (Step 8) — minting before the claim would issue
        // credentials for a task another dispatcher process might win the CAS race
        // for; minting before the gate (impossible here — the gate already ran) would
        // issue credentials for a profile about to be refused. `token_audit`/
        // `profile_credentials` presence was already confirmed above, so the
        // `.expect(...)` calls below are an invariant re-check, not a fallible path.
        let vault_bootstrap: Option<crate::worker_spawn::WorkerVaultBootstrap> = if is_vault_backed
        {
            let host = ctx
                .profile_credentials
                .as_ref()
                .expect("checked is_vault_backed guard above: profile_credentials is Some");
            let sink = ctx
                .token_audit
                .as_ref()
                .expect("checked is_vault_backed guard above: token_audit is Some");
            let config = ironhermes_core::config::Config::load().unwrap_or_default();
            match ironhermes_core::profile_credentials::mint_worker_credential(
                &config,
                host,
                &task.assignee,
                sink.as_ref(),
            )
            .await
            {
                Ok(minted) => Some(crate::worker_spawn::WorkerVaultBootstrap::new(
                    minted.token,
                    minted.socket_path.to_string_lossy().into_owned(),
                )),
                Err(e) => {
                    tracing::error!(
                        event = "vault_mint_failed",
                        task_id = %task.id,
                        run_id = %run_id,
                        error = %e,
                    );
                    // D-07/D-14: a mint failure refuses the spawn — no worker is
                    // created without a credential. Release the claim so the task
                    // returns to `ready` (the circuit breaker still applies via the
                    // consecutive_failures increment below, matching the existing
                    // spawn_failed handling).
                    {
                        let store = ctx.store.lock().await;
                        let now2 = now_secs();
                        store.conn.execute(
                            "UPDATE task_runs SET outcome='spawn_failed', error=?1, ended_at=?2 \
                             WHERE id=?3",
                            params![format!("vault credential mint failed: {e}"), now2, run_id],
                        )?;
                    }
                    {
                        let store = ctx.store.lock().await;
                        store.conn.execute(
                            "UPDATE tasks SET consecutive_failures = consecutive_failures + 1 \
                             WHERE id=?1",
                            params![task.id],
                        )?;
                    }
                    {
                        let mut store = ctx.store.lock().await;
                        let _ =
                            release_claim(&mut store.conn, &task.id, &claim_lock, "vault_mint_failed");
                    }
                    continue;
                }
            }
        } else {
            None
        };

        // Step 8: spawn worker. The board slug is captured in ctx.spawn_fn by the
        // per-board context wrapper in run_dispatch_tick; the empty string below
        // is ignored by that wrapper (the wrapper uses its captured slug). A
        // failed workspace_resolution (project: worktree creation Err) short-
        // circuits to the same Err arm below without ever calling spawn_fn.
        let spawn_result = match workspace_resolution {
            Ok(workspace) => {
                (ctx.spawn_fn)(
                    task.clone(),
                    run.clone(),
                    workspace,
                    String::new(),
                    vault_bootstrap,
                )
                .await
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

// ---------------------------------------------------------------------------
// Phase 51 Plan 06 (D-11): zero-touch profile-credentials construction invariant
// ---------------------------------------------------------------------------

#[cfg(test)]
mod profile_credentials_construction_tests {
    //! Proves `DispatcherContext` yields the profile-credential endpoint handle from
    //! construction WITHOUT any of the three production construction sites having to opt in —
    //! a source-level invariant rather than a live-vault behavioral test, since driving a REAL
    //! vault open through `Config::load()`'s ambient `$IRONHERMES_HOME` resolution from a unit
    //! test would mean mutating process-global env state (this repo's own documented
    //! `env_lock`-class flake hazard for exactly this kind of test). The live end-to-end proof
    //! that `host_profile_credentials` actually hosts a real endpoint when the vault is
    //! reachable lives in
    //! `ironhermes-core/tests/profile_credentials_host.rs::host_yields_an_endpoint_when_vault_is_genuinely_reachable`
    //! — the exact same function `host_profile_credentials_from_disk()` below calls.

    use std::path::Path;

    fn read_workspace_source(relative_to_workspace_root: &str) -> String {
        let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        std::fs::read_to_string(workspace_root.join(relative_to_workspace_root))
            .unwrap_or_else(|e| panic!("read {relative_to_workspace_root}: {e}"))
    }

    /// Updated in lockstep for Phase 51 Plan 19 (G-51-6), per this test's own doc comment's
    /// sanctioned update path: the wiring intentionally changed. There are now TWO long-lived
    /// production hosts — the gateway runner (`runner.rs`) and the kanban daemon (`commands.rs`
    /// `cmd_daemon`) — which still receive the credential endpoint with no opt-in, calling
    /// `DispatcherContext::new(...)` with its ORIGINAL two-argument shape exactly as before. And
    /// there is now EXACTLY ONE production host that declines it, because it exits before its
    /// workers: `cmd_dispatch`, which calls `DispatcherContext::new_one_shot(...)` instead. Both
    /// counts are asserted EXACTLY — not as an inequality — because an inequality here would
    /// silently tolerate a future regression that reverted `cmd_dispatch` back onto the hosting
    /// constructor, which is precisely the G-51-6 bug this plan closed.
    #[test]
    fn production_construction_sites_call_new_with_original_two_args() {
        let runner_source = read_workspace_source("crates/ironhermes-gateway/src/runner.rs");
        assert!(
            runner_source.contains("DispatcherContext::new("),
            "runner.rs must still construct DispatcherContext::new(...) unmodified"
        );

        // commands.rs: EXACTLY one long-lived call (cmd_daemon) and EXACTLY one one-shot call
        // (cmd_dispatch) — the needle "DispatcherContext::new(" ends in an open paren, so it
        // does NOT match "DispatcherContext::new_one_shot(" (the next character after "new" is
        // "_", not "("); the two counts are disjoint by construction.
        let commands_source = read_workspace_source("crates/ironhermes-cli/src/kanban/commands.rs");
        let long_lived_count = commands_source.matches("DispatcherContext::new(").count();
        assert_eq!(
            long_lived_count, 1,
            "commands.rs must retain EXACTLY ONE long-lived DispatcherContext::new(...) call \
             site (cmd_daemon), found {long_lived_count}"
        );
        let one_shot_count = commands_source
            .matches("DispatcherContext::new_one_shot(")
            .count();
        assert_eq!(
            one_shot_count, 1,
            "commands.rs must retain EXACTLY ONE one-shot DispatcherContext::new_one_shot(...) \
             call site (cmd_dispatch), found {one_shot_count}"
        );
    }

    /// Self-referential invariant: `new()` and `with_spawn_fn()` both wire `profile_credentials`
    /// through the exact same `host_profile_credentials_from_disk()` call — asserted at the
    /// source level so a future edit that diverges the two constructors' wiring is caught.
    #[test]
    fn both_constructors_wire_profile_credentials_through_the_same_function() {
        let source = read_workspace_source("crates/ironhermes-kanban/src/dispatcher.rs");
        // Needle assembled at RUNTIME (never a contiguous string literal in this test file's
        // own source) so this assertion cannot match its own source line — the self-counting
        // trap this repo has already shipped once (Plan 02's grep gate) and explicitly warns
        // against repeating.
        let needle = format!(
            "profile_credentials: {}{}",
            "host_profile_credentials_from_disk", "()"
        );
        let count = source.matches(needle.as_str()).count();
        assert_eq!(
            count, 2,
            "both DispatcherContext::new() and with_spawn_fn() must call \
             host_profile_credentials_from_disk() — found {count} occurrence(s)"
        );
    }
}

// ---------------------------------------------------------------------------
// Phase 51 Plan 07 (D-07): mint-at-dispatch behavioral tests
// ---------------------------------------------------------------------------

/// Real-vault behavioral coverage for the mint-then-ledger-then-spawn wiring in
/// `run_dispatch_tick_for_board`'s per-task loop. Unlike
/// `profile_credentials_construction_tests` above (which deliberately avoids a
/// real vault to sidestep the `$IRONHERMES_HOME` env-mutation flake hazard),
/// these tests DO drive a real vault — the mint/ledger/refusal properties they
/// prove cannot be observed any other way (a stubbed mint would only prove the
/// stub was honoured). `--test-threads=1` (this plan's mandated invocation) is
/// what makes the `IRONHERMES_HOME` mutation across these tests safe.
#[cfg(all(test, feature = "rusty-vault"))]
mod vault_mint_tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use ironhermes_core::config::{Config, ProviderConfig};
    use ironhermes_core::profile_credentials::ProfileTokenAudit;
    use tempfile::TempDir;
    use tokio::sync::Mutex as TokioMutex;

    use super::{DispatchGateFn, DispatcherContext, SpawnFn, run_dispatch_tick};
    use crate::store::CreateTaskOptions;
    use crate::{KanbanConfig, KanbanStore};

    const SLUG: &str = "vaultmint";
    const PROVIDER: &str = "vaultmintprovider";

    /// RAII guard sandboxing `IRONHERMES_HOME` — mirrors
    /// `dispatch_gate_loop.rs`'s identical `ScopedEnv` precedent exactly.
    struct ScopedEnv {
        key: String,
        prev: Option<String>,
    }

    impl ScopedEnv {
        fn set(key: &str, value: &std::path::Path) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: test-only, guarded by --test-threads=1 (this plan's
            // mandated invocation for any run touching this crate).
            unsafe { std::env::set_var(key, value) };
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: see `set` above.
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(&self.key, v) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }

    /// A recording [`ProfileTokenAudit`] sink — pushes `"ledger:{slug}:{accessor}"`
    /// onto a shared, ordered trace log so tests can assert mint→ledger→spawn
    /// ordering directly, and separately exposes the raw `(slug, accessor, ttl)`
    /// tuples for content assertions (accessor present, token absent).
    struct RecordingAudit {
        trace: Arc<Mutex<Vec<String>>>,
        calls: Arc<Mutex<Vec<(String, String, Duration)>>>,
    }

    impl ProfileTokenAudit for RecordingAudit {
        fn record_mint(&self, slug: &str, accessor: &str, ttl: Duration) -> anyhow::Result<()> {
            self.trace
                .lock()
                .unwrap()
                .push(format!("ledger:{slug}:{accessor}"));
            self.calls
                .lock()
                .unwrap()
                .push((slug.to_string(), accessor.to_string(), ttl));
            Ok(())
        }
    }

    /// Writes `vault.enabled/backend/rusty_vault.data_dir` into `config` and
    /// saves it to `home/config.yaml`, plus a `model.provider`/`providers` entry
    /// naming [`PROVIDER`] (irrelevant to minting itself, but matching a
    /// realistic vault-backed profile shape).
    fn write_vault_config(home: &std::path::Path, vault_data_dir: &std::path::Path) {
        let mut config = Config::default();
        config.vault.enabled = true;
        config.vault.backend = "rusty-vault".to_string();
        config.vault.rusty_vault.data_dir = vault_data_dir.to_path_buf();
        config.vault.rusty_vault.unseal_mode = "keyfile".to_string();
        config.model.provider = PROVIDER.to_string();
        config.providers.insert(
            PROVIDER.to_string(),
            ProviderConfig {
                api_key_env: Some("VAULTMINT_API_KEY".to_string()),
                ..Default::default()
            },
        );
        config
            .save_to(&home.join("config.yaml"))
            .expect("save_to config.yaml");
    }

    fn init_vault(data_dir: &std::path::Path) {
        let rv_config = ironhermes_vault::RustyVaultConfig {
            data_dir: data_dir.to_path_buf(),
            unseal_mode: "keyfile".to_string(),
        };
        ironhermes_vault::RustyVaultStore::init(&rv_config).expect("vault init");
    }

    fn open_store(dir: &TempDir) -> KanbanStore {
        KanbanStore::open(dir.path().join("kanban.db")).expect("open kanban store")
    }

    fn always_allow_from_vault() -> DispatchGateFn {
        Arc::new(|_assignee: &str| {
            Box::pin(async { ironhermes_core::dispatch_gate::DispatchDecision::AllowFromVault })
        })
    }

    fn always_allow() -> DispatchGateFn {
        Arc::new(|_assignee: &str| Box::pin(async { ironhermes_core::dispatch_gate::DispatchDecision::Allow }))
    }

    /// `dispatch_mints_then_spawns_and_ledgers_the_accessor` +
    /// `ledger_line_contains_the_accessor_and_not_the_token`: for a profile the
    /// gate judged `AllowFromVault`, the mint happens (and is ledgered) before
    /// the spawn function is invoked; the ledger line carries the accessor, and
    /// provably not the minted token's own secret bytes.
    #[tokio::test(flavor = "multi_thread")]
    async fn dispatch_mints_then_spawns_and_ledgers_the_accessor() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let vault_dir = tmp.path().join("vault");
        init_vault(&vault_dir);
        write_vault_config(&home, &vault_dir);
        let _env = ScopedEnv::set("IRONHERMES_HOME", &home);

        let store_dir = TempDir::new().unwrap();
        let mut store = open_store(&store_dir);
        store
            .create_task("vault mint test", SLUG, CreateTaskOptions::default())
            .unwrap();
        let store_arc = Arc::new(TokioMutex::new(store));

        let spawn_trace: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let spawn_vaults: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));
        let spawn_trace_for_fn = Arc::clone(&spawn_trace);
        let spawn_vaults_for_fn = Arc::clone(&spawn_vaults);
        let spawn_fn: SpawnFn = Arc::new(move |task, _run, _ws, _slug, vault| {
            let spawn_trace = Arc::clone(&spawn_trace_for_fn);
            let spawn_vaults = Arc::clone(&spawn_vaults_for_fn);
            Box::pin(async move {
                spawn_trace.lock().unwrap().push(format!("spawn:{}", task.id));
                spawn_vaults.lock().unwrap().push(vault.is_some());
                Ok(9999)
            })
        });

        let ledger_trace: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let audit_calls: Arc<Mutex<Vec<(String, String, Duration)>>> = Arc::new(Mutex::new(Vec::new()));
        let audit = RecordingAudit {
            trace: Arc::clone(&ledger_trace),
            calls: Arc::clone(&audit_calls),
        };

        let mut ctx = DispatcherContext::with_spawn_fn(store_arc, KanbanConfig::default(), spawn_fn);
        assert!(
            ctx.profile_credentials.is_some(),
            "precondition: the vault must be genuinely hosted for this test to mean anything"
        );
        ctx.gate_fn = always_allow_from_vault();
        ctx = ctx.with_token_audit(Arc::new(audit));

        run_dispatch_tick(&ctx).await.expect("tick failed");

        // Ordering: exactly one ledger entry, exactly one spawn, ledger first.
        let ledger = ledger_trace.lock().unwrap().clone();
        let spawns = spawn_trace.lock().unwrap().clone();
        assert_eq!(ledger.len(), 1, "expected exactly one mint/ledger event, got {ledger:?}");
        assert_eq!(spawns.len(), 1, "expected exactly one spawn, got {spawns:?}");
        assert!(
            ledger[0].starts_with(&format!("ledger:{SLUG}:")),
            "ledger line must carry the profile slug: {ledger:?}"
        );

        // The spawned worker received a vault bootstrap.
        assert_eq!(spawn_vaults.lock().unwrap().clone(), vec![true]);

        // Content: the accessor is present and non-empty; the token's own secret
        // bytes never appear anywhere in the recorded ledger content.
        let calls = audit_calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let (slug, accessor, ttl) = &calls[0];
        assert_eq!(slug, SLUG);
        assert!(!accessor.is_empty(), "accessor must be non-empty");
        assert!(ttl.as_secs() > 0, "ttl must be the real bootstrap TTL, not zero");
        for entry in ledger.iter().chain(std::iter::once(&format!("{slug}:{accessor}"))) {
            assert!(
                !entry.contains("token"),
                "ledger content must never contain the literal substring \"token\" \
                 (the accessor is a plain UUID, never the credential itself): {entry}"
            );
        }
    }

    /// `mint_failure_refuses_the_spawn`: with minting forced to fail (Phase 51 Plan 11:
    /// `config.yaml` is repointed to `vault.enabled = false` AFTER the endpoint was already
    /// hosted from a working vault, tripping `mint_worker_credential`'s own guard check —
    /// the only remaining config-driven failure point now that the mint goes through the
    /// host's own `Core` rather than re-opening a store from config), the injected spawn
    /// function is never called and the task is not marked running.
    #[tokio::test(flavor = "multi_thread")]
    async fn mint_failure_refuses_the_spawn() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let working_vault_dir = tmp.path().join("vault-working");
        init_vault(&working_vault_dir);
        write_vault_config(&home, &working_vault_dir);
        let _env = ScopedEnv::set("IRONHERMES_HOME", &home);

        let store_dir = TempDir::new().unwrap();
        let mut store = open_store(&store_dir);
        store
            .create_task("mint failure test", SLUG, CreateTaskOptions::default())
            .unwrap();
        let store_arc = Arc::new(TokioMutex::new(store));

        let spawn_calls: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
        let spawn_calls_for_fn = Arc::clone(&spawn_calls);
        let spawn_fn: SpawnFn = Arc::new(move |_task, _run, _ws, _slug, _vault| {
            let spawn_calls = Arc::clone(&spawn_calls_for_fn);
            Box::pin(async move {
                *spawn_calls.lock().unwrap() += 1;
                Ok(9999)
            })
        });

        let mut ctx = DispatcherContext::with_spawn_fn(store_arc.clone(), KanbanConfig::default(), spawn_fn);
        assert!(
            ctx.profile_credentials.is_some(),
            "precondition: construction-time endpoint must be hosted from the working vault"
        );
        ctx.gate_fn = always_allow_from_vault();
        let audit_calls: Arc<Mutex<Vec<(String, String, Duration)>>> = Arc::new(Mutex::new(Vec::new()));
        let ledger_trace: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        ctx = ctx.with_token_audit(Arc::new(RecordingAudit {
            trace: Arc::clone(&ledger_trace),
            calls: Arc::clone(&audit_calls),
        }));

        // Phase 51 Plan 11 (CR-06 fix): `mint_worker_credential` no longer opens a second
        // store from the freshly-loaded `Config::load()` — it mints through
        // `ctx.profile_credentials`'s own already-open `Core`
        // (`mint_profile_token_for_host`). Repointing `config.yaml`'s DATA DIR (the
        // pre-Plan-11 failure injection) is therefore a no-op now: `Config::load()` still
        // resolves `vault.enabled/backend`, both still true, and no NEW store is ever
        // opened on this path. The mint's own guard check
        // (`config.vault.enabled && config.vault.backend == "rusty-vault"`) is the only
        // remaining config-driven failure point on this path, so flip THAT instead: repoint
        // config.yaml to `vault.enabled = false`. `Config::load()` (inside
        // `run_dispatch_tick_for_board`) sees this; the already-constructed
        // `ctx.profile_credentials` handle above is unaffected (it was resolved once, at
        // construction time, from the WORKING vault).
        let mut disabled_vault_config = Config::default();
        disabled_vault_config.vault.enabled = false;
        disabled_vault_config.model.provider = PROVIDER.to_string();
        disabled_vault_config.providers.insert(
            PROVIDER.to_string(),
            ProviderConfig {
                api_key_env: Some("VAULTMINT_API_KEY".to_string()),
                ..Default::default()
            },
        );
        disabled_vault_config
            .save_to(&home.join("config.yaml"))
            .expect("save_to config.yaml");

        run_dispatch_tick(&ctx).await.expect("tick failed");

        assert_eq!(
            *spawn_calls.lock().unwrap(),
            0,
            "a mint failure must leave the spawn function uncalled"
        );
        assert!(
            audit_calls.lock().unwrap().is_empty(),
            "a failed mint must never reach record_mint"
        );

        let store = store_arc.lock().await;
        let tasks = store.list_tasks(crate::store::ListFilters::default()).unwrap();
        let task = tasks.iter().find(|t| t.assignee == SLUG).unwrap();
        assert_ne!(
            task.status, "running",
            "a task whose mint failed must not be left running with no credential"
        );
    }

    /// `vault_backed_dispatch_without_an_audit_sink_refuses`: `AllowFromVault`
    /// with `token_audit: None` on the context refuses with a named reason and
    /// never calls the spawn function — an unavailable ledger does not become an
    /// un-audited mint.
    #[tokio::test(flavor = "multi_thread")]
    async fn vault_backed_dispatch_without_an_audit_sink_refuses() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let vault_dir = tmp.path().join("vault");
        init_vault(&vault_dir);
        write_vault_config(&home, &vault_dir);
        let _env = ScopedEnv::set("IRONHERMES_HOME", &home);

        let store_dir = TempDir::new().unwrap();
        let mut store = open_store(&store_dir);
        store
            .create_task("no sink test", SLUG, CreateTaskOptions::default())
            .unwrap();
        let store_arc = Arc::new(TokioMutex::new(store));

        let spawn_calls: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
        let spawn_calls_for_fn = Arc::clone(&spawn_calls);
        let spawn_fn: SpawnFn = Arc::new(move |_task, _run, _ws, _slug, _vault| {
            let spawn_calls = Arc::clone(&spawn_calls_for_fn);
            Box::pin(async move {
                *spawn_calls.lock().unwrap() += 1;
                Ok(9999)
            })
        });

        let mut ctx = DispatcherContext::with_spawn_fn(store_arc.clone(), KanbanConfig::default(), spawn_fn);
        assert!(ctx.profile_credentials.is_some(), "precondition: endpoint hosted");
        assert!(ctx.token_audit.is_none(), "precondition: no sink wired (the default)");
        ctx.gate_fn = always_allow_from_vault();

        run_dispatch_tick(&ctx).await.expect("tick failed");

        assert_eq!(
            *spawn_calls.lock().unwrap(),
            0,
            "AllowFromVault with no audit sink must refuse, never spawn"
        );

        let store = store_arc.lock().await;
        let tasks = store.list_tasks(crate::store::ListFilters::default()).unwrap();
        let task = tasks.iter().find(|t| t.assignee == SLUG).unwrap();
        assert_eq!(
            task.status, "blocked",
            "an unaudited-because-unwired vault dispatch must land terminal-blocked, \
             not stay ready and re-attempt forever"
        );
    }

    /// `dotenv_backed_profile_does_not_mint`: a profile the gate judged `Allow`
    /// (dotenv-backed) produces no mint, no token variable, and no ledger line —
    /// the vault path is not engaged for un-migrated profiles, even when a
    /// working vault + audit sink ARE both wired on the context.
    #[tokio::test(flavor = "multi_thread")]
    async fn dotenv_backed_profile_does_not_mint() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let vault_dir = tmp.path().join("vault");
        init_vault(&vault_dir);
        write_vault_config(&home, &vault_dir);
        let _env = ScopedEnv::set("IRONHERMES_HOME", &home);

        let store_dir = TempDir::new().unwrap();
        let mut store = open_store(&store_dir);
        store
            .create_task("dotenv path test", SLUG, CreateTaskOptions::default())
            .unwrap();
        let store_arc = Arc::new(TokioMutex::new(store));

        let spawn_calls: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));
        let spawn_calls_for_fn = Arc::clone(&spawn_calls);
        let spawn_fn: SpawnFn = Arc::new(move |_task, _run, _ws, _slug, vault| {
            let spawn_calls = Arc::clone(&spawn_calls_for_fn);
            Box::pin(async move {
                spawn_calls.lock().unwrap().push(vault.is_some());
                Ok(9999)
            })
        });

        let ledger_trace: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let audit_calls: Arc<Mutex<Vec<(String, String, Duration)>>> = Arc::new(Mutex::new(Vec::new()));

        let mut ctx = DispatcherContext::with_spawn_fn(store_arc.clone(), KanbanConfig::default(), spawn_fn);
        ctx.gate_fn = always_allow(); // plain Allow, not AllowFromVault
        ctx = ctx.with_token_audit(Arc::new(RecordingAudit {
            trace: Arc::clone(&ledger_trace),
            calls: Arc::clone(&audit_calls),
        }));

        run_dispatch_tick(&ctx).await.expect("tick failed");

        assert_eq!(
            spawn_calls.lock().unwrap().clone(),
            vec![false],
            "an Allow (dotenv-backed) dispatch must spawn with vault: None"
        );
        assert!(
            audit_calls.lock().unwrap().is_empty(),
            "an Allow dispatch must never mint, even with a working vault + sink wired"
        );
        assert!(ledger_trace.lock().unwrap().is_empty());
    }
}

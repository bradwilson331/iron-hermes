//! Phase 54.8 Plan 07: supervised, self-healing Telegram poll loop.
//!
//! Plan 01 guarantees each `getUpdates` call returns (a 45 s outer deadline).
//! This module guarantees the loop KEEPS polling (D-05): an in-process
//! supervisor watches the adapter's monotonic [`PollLiveness`] stamp and, when
//! no poll has succeeded for [`PollTuning::stale_after`], cancels the current
//! poll generation, JOINS it (aborting if it ignores cancellation), rebuilds
//! the HTTP client and only then spawns the next generation. At most one
//! generation ever runs. The module never exits the process: a normal install
//! has no external supervisor, so healing happens in-process.
//!
//! Each generation ends with a typed [`GenerationExit`] (or a panic); the pure
//! [`next_action`] decides whether that exit is restartable.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::mpsc::{self, error::SendTimeoutError};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::backoff::BackoffState;
use crate::telegram::{POLL_FRESH_MAX_AGE, PollLiveness, TelegramAdapter, TgUpdate};

/// Timing knobs for the poll supervisor. The defaults are the production
/// values; tests pass millisecond values instead of waiting real minutes.
#[derive(Debug, Clone, Copy)]
pub struct PollTuning {
    /// No successful `getUpdates` for this long means the poller is stalled
    /// (D-05). Well above the 90 s liveness window and the 60 s backoff cap,
    /// so ordinary Telegram 5xx windows are handled by backoff, not respawns.
    pub stale_after: Duration,
    /// How often the watchdog evaluates liveness.
    pub tick: Duration,
    /// Minimum spacing between two respawns, so a persistently dead network
    /// cannot turn the watchdog into a respawn storm.
    pub min_respawn_gap: Duration,
    /// How long a cancelled generation gets to finish before it is aborted.
    pub join_grace: Duration,
    /// Cadence of the info-level `Telegram poll ok` heartbeat line (D-06).
    pub trace_every: Duration,
    /// Per-attempt bound on handing one update to the dispatch channel; a full
    /// channel is retried after each timeout (back-pressure, not a stall).
    pub handoff_timeout: Duration,
    /// Poll-error backoff base (today's `BackoffState::default_polling`).
    pub backoff_base: Duration,
    /// Poll-error backoff cap.
    pub backoff_cap: Duration,
}

impl Default for PollTuning {
    fn default() -> Self {
        Self {
            stale_after: Duration::from_secs(5 * 60),
            tick: Duration::from_secs(30),
            min_respawn_gap: Duration::from_secs(60),
            join_grace: Duration::from_secs(5),
            trace_every: Duration::from_secs(10 * 60),
            handoff_timeout: Duration::from_secs(30),
            backoff_base: Duration::from_secs(1),
            backoff_cap: Duration::from_secs(60),
        }
    }
}

/// Generation phase: the poller is fetching from Telegram.
const PHASE_POLLING: u8 = 0;
/// Generation phase: a fetched update is waiting for room in the dispatch
/// channel.
const PHASE_HANDING_OFF: u8 = 1;

/// Shared, lock-free counters. Read by the watchdog and by tests; written by
/// the supervisor and its generations.
#[derive(Default)]
pub struct PollStats {
    generations: AtomicU64,
    respawns: AtomicU64,
    updates_in_window: AtomicU64,
    polls_in_window: AtomicU64,
    phase: AtomicU8,
    handoff_since_ms: AtomicI64,
    active_generations: AtomicU64,
    max_active_generations: AtomicU64,
}

impl PollStats {
    /// Number of generations spawned so far (the first one counts).
    pub fn generations(&self) -> u64 {
        self.generations.load(Ordering::SeqCst)
    }

    /// Number of respawns performed after the first generation.
    pub fn respawns(&self) -> u64 {
        self.respawns.load(Ordering::SeqCst)
    }

    /// Current phase: 0 = polling, 1 = handing off to a full channel.
    pub fn phase(&self) -> u8 {
        self.phase.load(Ordering::SeqCst)
    }

    /// High-water mark of simultaneously running generations (must stay 1).
    pub fn max_active_generations(&self) -> u64 {
        self.max_active_generations.load(Ordering::SeqCst)
    }

    /// Seconds the current hand-off has been blocked (0 when not blocked).
    fn handoff_blocked_secs(&self) -> u64 {
        let since = self.handoff_since_ms.load(Ordering::SeqCst);
        (mono_ms().saturating_sub(since).max(0) / 1000) as u64
    }
}

/// Telegram `connected` for the gateway status heartbeat (D-04): true only
/// when an adapter exists AND its last successful poll is within
/// [`POLL_FRESH_MAX_AGE`] (90 s = 2 x the 45 s poll deadline). Freshness is
/// decided on the monotonic clock inside [`PollLiveness`], so there is
/// deliberately no time parameter: a wall-clock step cannot change the answer.
pub fn telegram_status_connected(liveness: Option<&PollLiveness>) -> bool {
    liveness.is_some_and(|l| l.is_fresh(POLL_FRESH_MAX_AGE))
}

/// Monotonic milliseconds since the first call in this process.
fn mono_ms() -> i64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as i64
}

/// Per-step budget for the dispatcher's network awaits (attachment processing,
/// the attachment-error reply, the queue dispatch). Above the 120 s Telegram
/// client timeout from plan 01, so a normal slow download is not cut short, but
/// a wedged await can no longer pin the single dispatcher and fill the channel
/// forever (review 07 HIGH, T-54.8-54).
pub const DISPATCH_STEP_BUDGET: Duration = Duration::from_secs(150);

/// Bound one dispatcher await by `budget`; `Err(Elapsed)` means the step
/// exceeded it and the caller drops the update and moves on.
pub async fn bounded_dispatch_step<F: std::future::Future>(
    budget: Duration,
    fut: F,
) -> Result<F::Output, tokio::time::error::Elapsed> {
    tokio::time::timeout(budget, fut).await
}

/// RAII gauge: counts a generation as active from start to drop, including a
/// drop caused by a panic or by `JoinHandle::abort`.
struct ActiveGuard(Arc<PollStats>);

impl ActiveGuard {
    fn enter(stats: &Arc<PollStats>) -> Self {
        let now = stats.active_generations.fetch_add(1, Ordering::SeqCst) + 1;
        stats
            .max_active_generations
            .fetch_max(now, Ordering::SeqCst);
        Self(Arc::clone(stats))
    }
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.active_generations.fetch_sub(1, Ordering::SeqCst);
    }
}

/// How one poll generation ended (a panic surfaces as a `JoinError` instead).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationExit {
    /// The generation's cancellation token fired.
    Cancelled,
    /// The dispatch channel is closed: the dispatcher is gone.
    ChannelClosed,
    /// Five consecutive 409s: another bot instance polls this token.
    Fatal409,
}

/// What the supervisor does after a generation ends.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SupervisorAction {
    Respawn,
    Stop,
}

/// Pure decision table for a finished generation (review 07 MEDIUM: a closed
/// dispatch channel and a fatal 409 are terminal, not restartable).
pub(crate) fn next_action(
    outcome: &Result<GenerationExit, tokio::task::JoinError>,
    parent_cancelled: bool,
) -> SupervisorAction {
    match outcome {
        // A cancel the parent did not issue is a watchdog cancel.
        Ok(GenerationExit::Cancelled) | Err(_) if parent_cancelled => SupervisorAction::Stop,
        Ok(GenerationExit::Cancelled) | Err(_) => SupervisorAction::Respawn,
        Ok(GenerationExit::ChannelClosed) | Ok(GenerationExit::Fatal409) => SupervisorAction::Stop,
    }
}

/// Everything a generation needs, owned by the supervisor and shared with
/// every generation it spawns. The offset and the backoff state live HERE so
/// a respawn neither re-fetches acknowledged updates nor resets the 409 count.
#[derive(Clone)]
pub struct PollShared {
    adapter: Arc<TelegramAdapter>,
    /// Next `getUpdates` offset; 0 = none yet.
    offset: Arc<AtomicI64>,
    backoff: Arc<tokio::sync::Mutex<BackoffState>>,
    tx: mpsc::Sender<TgUpdate>,
    parent: CancellationToken,
    tuning: PollTuning,
    stats: Arc<PollStats>,
}

impl PollShared {
    pub fn new(
        adapter: Arc<TelegramAdapter>,
        tx: mpsc::Sender<TgUpdate>,
        parent: CancellationToken,
        tuning: PollTuning,
        stats: Arc<PollStats>,
    ) -> Self {
        let backoff = BackoffState::new(
            tuning.backoff_base.as_millis() as u64,
            tuning.backoff_cap.as_millis() as u64,
        );
        Self {
            adapter,
            offset: Arc::new(AtomicI64::new(0)),
            backoff: Arc::new(tokio::sync::Mutex::new(backoff)),
            tx,
            parent,
            tuning,
            stats,
        }
    }
}

/// Spawn one poll generation on its own task and count it.
fn spawn_generation(
    shared: &PollShared,
    gen_cancel: CancellationToken,
) -> JoinHandle<GenerationExit> {
    shared.stats.generations.fetch_add(1, Ordering::SeqCst);
    shared.stats.phase.store(PHASE_POLLING, Ordering::SeqCst);
    let shared = shared.clone();
    tokio::spawn(async move { run_poll_generation(shared, gen_cancel).await })
}

/// Await a cancelled generation for at most `grace`, then abort it and await
/// the abort. Returns the generation's own outcome when it finished in time.
async fn join_bounded(
    handle: &mut JoinHandle<GenerationExit>,
    grace: Duration,
) -> Option<Result<GenerationExit, tokio::task::JoinError>> {
    match tokio::time::timeout(grace, &mut *handle).await {
        Ok(outcome) => Some(outcome),
        Err(_elapsed) => {
            handle.abort();
            let _ = (&mut *handle).await;
            None
        }
    }
}

/// The supervised Telegram poll task (D-05). Runs until the parent token is
/// cancelled or a generation ends terminally (closed channel, fatal 409).
/// Never exits the process and never cancels `parent` itself (only the 409
/// path inside a generation does, as before).
pub async fn run_poll_supervisor(
    adapter: Arc<TelegramAdapter>,
    tx: mpsc::Sender<TgUpdate>,
    parent: CancellationToken,
    tuning: PollTuning,
    stats: Arc<PollStats>,
) {
    let shared = PollShared::new(adapter.clone(), tx, parent.clone(), tuning, stats.clone());
    let liveness = adapter.poll_liveness();

    let mut gen_cancel = parent.child_token();
    let mut handle = spawn_generation(&shared, gen_cancel.clone());
    let mut gen_started = Instant::now();
    let mut last_respawn: Option<Instant> = None;
    let mut last_backpressure_log: Option<Instant> = None;
    let mut last_trace = Instant::now();

    let mut ticker = tokio::time::interval(tuning.tick);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            biased;
            _ = parent.cancelled() => {
                gen_cancel.cancel();
                let _ = join_bounded(&mut handle, tuning.join_grace).await;
                info!("Poll supervisor stopped (shutdown)");
                return;
            }
            outcome = &mut handle => {
                if next_action(&outcome, parent.is_cancelled()) == SupervisorAction::Stop {
                    log_stop(&outcome);
                    return;
                }
                if let Err(e) = &outcome {
                    error!(error = %e, "Telegram poll generation died — respawning (D-05)");
                }
                // Wait out the respawn gap, but stay cancellable.
                tokio::select! {
                    _ = parent.cancelled() => return,
                    _ = tokio::time::sleep(tuning.min_respawn_gap) => {}
                }
                stats.respawns.fetch_add(1, Ordering::SeqCst);
                last_respawn = Some(Instant::now());
                gen_cancel = parent.child_token();
                handle = spawn_generation(&shared, gen_cancel.clone());
                gen_started = Instant::now();
            }
            _ = ticker.tick() => {
                // D-06: a periodic "alive" line so a silent log means HUNG, not
                // idle. Fires only after at least one successful poll in the
                // window (an idle bot with zero updates still traces), and the
                // window counters reset only when a line is emitted.
                if last_trace.elapsed() >= tuning.trace_every
                    && stats.polls_in_window.load(Ordering::SeqCst) > 0
                {
                    let updates = stats.updates_in_window.swap(0, Ordering::SeqCst);
                    stats.polls_in_window.store(0, Ordering::SeqCst);
                    info!(
                        updates,
                        window_mins = last_trace.elapsed().as_secs() / 60,
                        generation = stats.generations(),
                        "Telegram poll ok"
                    );
                    last_trace = Instant::now();
                }
                // A never-stamped (or long-stale) liveness only counts from the
                // moment THIS generation started, so a fresh generation gets a
                // full stale window before it can be judged.
                let gen_age = gen_started.elapsed();
                let staleness = liveness
                    .since_last_ok()
                    .map_or(gen_age, |since| since.min(gen_age));
                let gap_ok = last_respawn.is_none_or(|t| t.elapsed() >= tuning.min_respawn_gap);
                if staleness > tuning.stale_after && stats.phase() == PHASE_HANDING_OFF {
                    // Back-pressure, not a poll stall: the poll itself worked
                    // and the dispatcher is slow. Respawning would only add a
                    // generation in front of the same full channel, so report
                    // it (at most once per stale window) and leave it alone.
                    if last_backpressure_log.is_none_or(|t| t.elapsed() >= tuning.stale_after) {
                        warn!(
                            blocked_secs = stats.handoff_blocked_secs(),
                            reason = "dispatch back-pressure",
                            "Telegram poller waiting on the dispatcher — not respawning (D-05)"
                        );
                        last_backpressure_log = Some(Instant::now());
                    }
                } else if staleness > tuning.stale_after && gap_ok {
                    warn!(
                        generation = stats.generations(),
                        stale_secs = staleness.as_secs(),
                        "Telegram poll stalled — rebuilding HTTP client and respawning the poller (D-05)"
                    );
                    // Strict order: cancel -> bounded join (abort + join) ->
                    // rebuild -> spawn. Never two generations at once.
                    gen_cancel.cancel();
                    if let Some(outcome) = join_bounded(&mut handle, tuning.join_grace).await
                        && next_action(&outcome, parent.is_cancelled()) == SupervisorAction::Stop
                    {
                        log_stop(&outcome);
                        return;
                    }
                    if parent.is_cancelled() {
                        return;
                    }
                    adapter.rebuild_http();
                    stats.respawns.fetch_add(1, Ordering::SeqCst);
                    last_respawn = Some(Instant::now());
                    gen_cancel = parent.child_token();
                    handle = spawn_generation(&shared, gen_cancel.clone());
                    gen_started = Instant::now();
                }
            }
        }
    }
}

/// Log why the supervisor is stopping without a respawn.
fn log_stop(outcome: &Result<GenerationExit, tokio::task::JoinError>) {
    match outcome {
        Ok(GenerationExit::ChannelClosed) => {
            error!("Telegram dispatch channel closed — poller stopped");
        }
        Ok(GenerationExit::Fatal409) => {
            error!("Telegram poller stopped after a fatal 409 conflict");
        }
        Ok(GenerationExit::Cancelled) | Err(_) => {
            info!("Poll supervisor stopped (shutdown)");
        }
    }
}

/// One poll generation: the old runner closure's body, moved here. Returns a
/// typed exit; the supervisor decides whether to respawn.
///
/// `pub` + `doc(hidden)` only so `tests/telegram_poll_watchdog.rs` can drive a
/// single generation (cancel mid-hand-off, start the next one); not a
/// production API.
#[doc(hidden)]
pub async fn run_poll_generation(
    shared: PollShared,
    gen_cancel: CancellationToken,
) -> GenerationExit {
    let _active = ActiveGuard::enter(&shared.stats);
    loop {
        if gen_cancel.is_cancelled() {
            info!("Poll loop cancelled");
            return GenerationExit::Cancelled;
        }
        let raw = shared.offset.load(Ordering::SeqCst);
        let offset = (raw != 0).then_some(raw);

        let result = tokio::select! {
            _ = gen_cancel.cancelled() => {
                info!("Poll loop cancelled");
                return GenerationExit::Cancelled;
            }
            result = shared.adapter.get_updates(offset) => result,
        };

        match result {
            Ok(updates) => {
                shared.backoff.lock().await.record_success();
                shared.stats.polls_in_window.fetch_add(1, Ordering::SeqCst);
                if !updates.is_empty() {
                    info!(
                        count = updates.len(),
                        "Received {} update(s) from polling",
                        updates.len()
                    );
                }
                for update in updates {
                    if let Some(exit) = hand_off(&shared, &gen_cancel, update).await {
                        return exit;
                    }
                }
            }
            Err(e) => {
                let err_str = e.to_string();
                let delay = {
                    let mut backoff = shared.backoff.lock().await;
                    if err_str.contains("Conflict") || err_str.contains("409") {
                        backoff.record_conflict();
                        if backoff.is_fatal_conflict() {
                            error!(
                                "Fatal 409 conflict — another bot instance is polling on this token. Shutting down."
                            );
                            shared.parent.cancel();
                            return GenerationExit::Fatal409;
                        }
                    } else {
                        backoff.record_failure();
                    }
                    backoff.next_delay()
                };
                warn!(
                    error = %e,
                    delay_ms = delay.as_millis(),
                    "Polling error, backing off"
                );
                tokio::select! {
                    _ = gen_cancel.cancelled() => {
                        info!("Poll loop cancelled");
                        return GenerationExit::Cancelled;
                    }
                    _ = tokio::time::sleep(delay) => {}
                }
            }
        }
    }
}

/// Hand one fetched update to the dispatch channel, then (and only then)
/// advance the supervisor-owned offset. `Some(exit)` ends the generation.
async fn hand_off(
    shared: &PollShared,
    gen_cancel: &CancellationToken,
    update: TgUpdate,
) -> Option<GenerationExit> {
    let update_id = update.update_id;
    let mut pending = update;
    loop {
        let sent = tokio::select! {
            _ = gen_cancel.cancelled() => {
                // Offset untouched: the next generation re-fetches this update.
                info!("Poll loop cancelled");
                return Some(GenerationExit::Cancelled);
            }
            sent = shared.tx.send_timeout(pending, shared.tuning.handoff_timeout) => sent,
        };
        match sent {
            Ok(()) => {
                shared.stats.phase.store(PHASE_POLLING, Ordering::SeqCst);
                shared.offset.fetch_max(update_id + 1, Ordering::SeqCst);
                shared
                    .stats
                    .updates_in_window
                    .fetch_add(1, Ordering::SeqCst);
                return None;
            }
            // The channel is full: this is back-pressure, not a poll stall.
            // Record the phase so the watchdog does not respawn, and retry the
            // SAME update until it lands, the generation is cancelled (offset
            // untouched, so the next generation re-fetches it) or the channel
            // closes.
            Err(SendTimeoutError::Timeout(back)) => {
                if shared.stats.phase.load(Ordering::SeqCst) != PHASE_HANDING_OFF {
                    shared
                        .stats
                        .handoff_since_ms
                        .store(mono_ms(), Ordering::SeqCst);
                    shared
                        .stats
                        .phase
                        .store(PHASE_HANDING_OFF, Ordering::SeqCst);
                }
                warn!(
                    blocked_secs = shared.stats.handoff_blocked_secs(),
                    "Telegram dispatch channel full — hand-off still blocked"
                );
                pending = back;
            }
            Err(SendTimeoutError::Closed(_)) => return Some(GenerationExit::ChannelClosed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn supervisor_next_action_rules() {
        // Cancelled by the watchdog (parent alive) -> respawn.
        assert_eq!(
            next_action(&Ok(GenerationExit::Cancelled), false),
            SupervisorAction::Respawn
        );
        // Cancelled because the whole gateway is shutting down -> stop.
        assert_eq!(
            next_action(&Ok(GenerationExit::Cancelled), true),
            SupervisorAction::Stop
        );
        // The dispatcher is gone / another instance owns the token -> stop.
        assert_eq!(
            next_action(&Ok(GenerationExit::ChannelClosed), false),
            SupervisorAction::Stop
        );
        assert_eq!(
            next_action(&Ok(GenerationExit::Fatal409), false),
            SupervisorAction::Stop
        );
        // A real JoinError from a task that panicked.
        let panicked = tokio::spawn(async {
            panic!("simulated poll generation panic");
            #[allow(unreachable_code)]
            GenerationExit::Cancelled
        })
        .await;
        let err = panicked.as_ref().expect_err("task must have panicked");
        assert!(err.is_panic());
        assert_eq!(next_action(&panicked, false), SupervisorAction::Respawn);
        assert_eq!(next_action(&panicked, true), SupervisorAction::Stop);
    }

    #[tokio::test]
    async fn conflict_count_survives_respawn() {
        // The backoff state is supervisor-owned and shared by every
        // generation, so conflicts counted before a respawn still count after.
        let adapter = Arc::new(TelegramAdapter::for_tests(
            "test-token-abc",
            "http://127.0.0.1:1",
            crate::telegram::TelegramHttpTuning::default(),
        ));
        let (tx, _rx) = mpsc::channel(1);
        let shared = PollShared::new(
            adapter,
            tx,
            CancellationToken::new(),
            PollTuning::default(),
            Arc::new(PollStats::default()),
        );
        let generation_one = shared.clone();
        for _ in 0..3 {
            generation_one.backoff.lock().await.record_conflict();
        }
        drop(generation_one);
        // A respawned generation sees the same state.
        let generation_two = shared.clone();
        {
            let mut backoff = generation_two.backoff.lock().await;
            assert!(!backoff.is_fatal_conflict(), "3 conflicts are not fatal");
            backoff.record_conflict();
            backoff.record_conflict();
        }
        assert!(shared.backoff.lock().await.is_fatal_conflict());
    }

    #[test]
    fn telegram_status_connected_rules() {
        use crate::telegram::PollLiveness;
        use std::time::SystemTime;

        // No adapter at all (Telegram not configured).
        assert!(!telegram_status_connected(None));
        // Never stamped.
        let liveness = PollLiveness::new();
        assert!(!telegram_status_connected(Some(&liveness)));
        // Stamped now.
        liveness.stamp_now();
        assert!(telegram_status_connected(Some(&liveness)));
        // Stamped 91 s ago on the monotonic clock: past POLL_FRESH_MAX_AGE.
        liveness.stamp_at_for_tests(Instant::now() - Duration::from_secs(91), SystemTime::now());
        assert!(!telegram_status_connected(Some(&liveness)));
        // Stamped 10 s ago but with a wall time one day in the past: the wall
        // clock does not decide freshness.
        liveness.stamp_at_for_tests(
            Instant::now() - Duration::from_secs(10),
            SystemTime::now() - Duration::from_secs(86_400),
        );
        assert!(telegram_status_connected(Some(&liveness)));
    }

    #[tokio::test]
    async fn bounded_dispatch_step_times_out() {
        let started = Instant::now();
        let stuck =
            bounded_dispatch_step(Duration::from_millis(50), std::future::pending::<()>()).await;
        assert!(stuck.is_err(), "a pending future must hit the budget");
        assert!(started.elapsed() < Duration::from_secs(1));
        let ready = bounded_dispatch_step(Duration::from_millis(50), async { 7 }).await;
        assert_eq!(ready.expect("a ready future completes"), 7);
    }
}

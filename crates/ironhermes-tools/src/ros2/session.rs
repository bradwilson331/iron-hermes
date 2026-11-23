//! Persistent per-process rosbridge connection (Phase 49.8 planning decision,
//! recorded in `49.8-01-PLAN.md`'s "Planning decisions" section).
//!
//! `client_for` already reconnects whenever the stored client's `is_closed()`
//! is true (below) — 49.8-02 adds the reconnect test
//! (`ros2_session_reconnects_after_drop`) proving it against a mock server
//! that drops the first connection; no behavior change was needed here.
//!
//! rosbridge's `send_action_goal` capability stores in-progress goals per client
//! connection, keyed by the client-supplied `id`, and `cancel_action_goal` looks
//! the goal up in that same per-connection map. It defines no disconnect cancel.
//! A fresh-per-call socket therefore cannot serve `goal_status`/`cancel_goal`
//! across calls and would orphan running goals; a persistent socket also keeps
//! advertised publishers alive so a final zero-velocity stop is not lost to DDS
//! discovery on a just-created publisher. Reversible — every ros2/rospider call
//! goes through this one seam ([`Ros2Session::client_for`]).
//!
//! 49.8-04 adds motion tracking (D-09/D-10): `motion_topics` records every
//! Twist-family topic this process has driven (49.8-07's `stop_all` reads
//! it), and `active_motion` holds at most one in-flight timed motion —
//! `begin_motion` preempts whatever is currently running before installing a
//! new one, so only one motion loop is ever publishing at a time.
//!
//! 49.8-07 Task 1 adds action-goal tracking (D-06): `goals` records every
//! goal `action_ops::send_goal` has sent (action name/type + owning
//! connection id), read back by `action_ops::cancel_goal` (and, from Task 2,
//! by `stop_all`) to know whether a goal's owning connection is still the
//! one this session currently holds. [`cancel_all_goals_fallback`] is the
//! shared "cancel every goal on this action server" helper
//! (`{action}/_action/cancel_goal`, a zero UUID and zero stamp) for when it
//! is not.
//!
//! 49.8-07 Task 2 makes D-10 real: [`Ros2Session::stop_all`] is the one stop
//! path that zeroes every motion topic AND cancels every unfinished goal —
//! on the goal's own connection when it is still open, otherwise via
//! [`cancel_all_goals_fallback`] after one bounded reconnect. (49.8-18: a
//! goal counts as cancelled only once its OWN terminal status confirms it —
//! see the 49.8-18 module note below; the "cancels" language above describes
//! routing, not a fire-and-forget socket write.)
//! [`stop_all_live`] resolves the URL through [`super::Ros2ConfigSource::Live`];
//! the free fn [`end_session_stop`] is the awaited, time-bounded surface
//! `Ros2Tool::on_session_end` spawns and the `stop` action awaits directly.
//!
//! 49.8-14 (G-49.8-2b): `stop_followups` is a second, sticky per-topic
//! registry beside `motion_topics` — the [`super::motion::StopFollowup`] a
//! motion's caller attached (via [`Ros2Session::begin_motion`]), if any.
//! [`Ros2Session::stop_all`] replays each recorded follow-up (advertise then
//! [`super::motion::publish_followup_burst`]) strictly after that topic's own
//! zero-Twist burst — on every surface's session end, on the reconnected
//! connection after a bridge drop, and whether or not this call's zero-Twist
//! burst itself succeeded (the rest stop is what actually halts a ROSPider;
//! see the design note on [`Ros2Session::stop_all`]). [`send_stop_followup`]
//! is the separate, unconditional send `rospider_tool.rs::do_stop` issues on
//! every explicit `stop` — even with nothing driven, because the robot may
//! still be marching from a process that no longer exists.
//!
//! 49.8-15 (G-49.8-2b, CR-01): before this plan, [`Ros2Session::stop_all`]
//! cancelled the active motion's token without waiting for it, so the
//! cancelled `motion_task`'s own zero-Twist-then-gait-0 burst raced, fully
//! unsynchronized, against `stop_all`'s own redundant burst on the same
//! topic/connection. [`Ros2Session::stop_all`] now waits, bounded by
//! [`PREEMPT_WAIT_MS`] (shared with [`Ros2Session::begin_motion`]'s own
//! preemption wait) via [`await_motion_finish`], for the motion it
//! interrupts before publishing anything of its own. It skips its own
//! zero-Twist burst ONLY for the one topic whose interrupted motion reported
//! [`MotionStatus::Finished`] with `followup_confirmed: true` within the
//! wait, re-reads [`Ros2Session::current_client`] after the wait (an
//! interrupted motion that lost its connection reconnects through
//! [`Ros2Session::emergency_stop`], installing a new client), and — on a
//! timed-out wait — still sends its full stop and names the timeout in the
//! report. Ordering CONCURRENT stop requests against each other (the
//! TUI/REPL session-end fan-out, an explicit stop racing `/new`) is
//! 49.8-16's, not this plan's.
//!
//! 49.8-16 (G-49.8-2b): several stop requests can arrive at once — both
//! `on_session_end` hooks plus the awaited `end_session_stop` at TUI/REPL
//! exit, or an explicit stop racing a gateway `/new`. [`Ros2Session::stop_lock`]
//! (held for all of [`Ros2Session::stop_all_as`] and all of
//! [`Ros2Session::send_stop_followup`]) makes every stop run one at a time;
//! lock order is always `stop_lock` first, then the inner mutex, and never
//! the reverse — `motion_task`, `emergency_stop` and `begin_motion` never
//! take `stop_lock`. `SessionInner` also gains `activity_epoch` (bumped by
//! [`Ros2Session::begin_motion`]/[`Ros2Session::track_goal`]),
//! `stops_completed` and `last_clean_stop`: a caller that queued behind a
//! clean stop which completed while it was already queued, with the epoch
//! unchanged, is answered with that stop's report plus
//! [`STOP_COALESCED_NOTE`] instead of resending (C1, direct callers); the
//! fire-and-forget hooks ([`Ros2Session::session_end_hook_stop`]) coalesce
//! with an unchanged-epoch clean stop regardless of queue order (C2) — safe
//! because every surface that fires them also awaits `end_session_stop`
//! first, so C2 only ever removes a redundant resend of a stop that awaited
//! call already sent. This matters most at CLI one-shot/kanban exit, where
//! the hooks fire after that awaited stop, just before the process exits —
//! a resend cut off between its zero Twist and its gait-0 would leave the
//! ROSPider marching. Any new activity since the recorded clean stop's own
//! snapshot re-arms a full stop, unconditionally, for every caller.
//!
//! 49.8-17 (WR-01, WR-02): two verification gaps closed. WR-01 — the outer
//! session-end bound ([`ROS2_SESSION_END_TIMEOUT_SECS`], widened to 6s) must
//! exceed the worst DEGRADED stop path: the post-interrupt-wait reconnect
//! ([`RECONNECT_STOP_TIMEOUT_SECS`]) can exceed the healthy-path budget, so
//! the bound now covers `PREEMPT_WAIT_MS` + reconnect + the interrupted
//! motion's own stop sequence with ~1s headroom (pinned together by the
//! `ros2_stop_ordering_wait_budget_fits_session_end_bound` test). WR-02 —
//! `stop_in_flight` (49.8-19: `Ros2Session::stop_in_flight`, moved out of
//! `SessionInner`) is set in the same critical section where the stop takes
//! `active_motion` out, and cleared (49.8-19: by `StopInFlightGuard`'s
//! `Drop`, on every exit path — see the 49.8-19 module note below) when it
//! finishes; when it is set, [`Ros2Session::begin_motion`] waits on
//! `stop_lock` (acquire-and-release — never held across its own
//! publish/install) so a motion installed mid-stop can never interleave with
//! the stop's draining burst or trailing gait-0 rest stop.
//!
//! 49.8-18 (VERIFICATION gap #2, CR-01): a goal cancel counts only on
//! terminal evidence. [`classify_cancel_all_response`] maps a cancel-all
//! [`ServiceResponse`] per `action_msgs/srv/CancelGoal.srv`'s four
//! `return_code` values (grounded in rcl_action's cancel-all processing):
//! a refusal, a malformed/unrecognized answer, and a merely-ACCEPTED answer
//! (the goals are CANCELING, not terminal — acceptance is not completion,
//! review 49.8-18) are ALL goal failures that leave the stop unclean and
//! re-arm the next stop's retry; only an answer with no goal left to cancel
//! (`NotCancelable`) is reported without failing the stop. Task 2 extends
//! this discipline to the same-connection path with a terminal-status
//! whitelist and bounds the whole goal-cancel phase by one constant,
//! independent of goal count.
//!
//! 49.8-19 (VERIFICATION gap #1, review 49.8-19): [`ROS2_SESSION_END_TIMEOUT_SECS`]
//! is now sized for [`MAX_MOTION_TOPICS`] rather than derived from a live
//! motion-topic count — a live count read at session end could race a
//! concurrent [`Ros2Session::admit_motion_topic`] call, so the bound is
//! finite BECAUSE the ceiling is finite, not because any read is bounded.
//! `admit_motion_topic` — called by [`super::motion::run_timed_motion`]
//! before [`Ros2Session::begin_motion`], its only production call site —
//! refuses a NEW motion topic once the process has already driven
//! `MAX_MOTION_TOPICS` distinct ones, before any wire work for it; a
//! re-drive of an already-tracked topic is never refused. Every awaited,
//! constant-bounded term of the worst degraded stop (the interrupt wait,
//! one bounded reconnect, every driven topic's own stop sequence, and the
//! 49.8-18 goal-cancel phase) is summed independently, for every count in
//! `0..=MAX_MOTION_TOPICS`, by `ros2_stop_ordering_wait_budget_fits_session_end_bound`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use serde_json::{Value, json};
use tokio::sync::{Mutex, watch};
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;

use super::bridge_client::{Ros2BridgeClient, ServiceResponse};
use super::motion::{StopFollowup, publish_followup_burst};

/// D-10: bound on [`Ros2Session::emergency_stop`]'s reconnect attempt after a
/// mid-motion bridge drop.
pub const RECONNECT_STOP_TIMEOUT_SECS: u64 = 3;

/// 49.8-19 (VERIFICATION gap #1): one motion topic's zero-Twist burst, one
/// [`super::safety::STOP_INTERVAL_MS`] gap, and its follow-up (gait-0) burst
/// — computed from `super::safety`'s primitive constants in const context so
/// it can never silently drift from them. Topics without a follow-up need
/// less than this, so it is an upper bound per topic used by the session-end
/// budget (below) and by [`ros2_stop_ordering_wait_budget_fits_session_end_bound`]
/// in the integration test, which pins this value against the raw arithmetic.
pub const STOP_SEQUENCE_MS: u64 = (super::safety::STOP_REPEATS as u64 - 1)
    * super::safety::STOP_INTERVAL_MS
    + super::safety::STOP_INTERVAL_MS
    + (super::safety::STOP_REPEATS as u64 - 1) * super::safety::STOP_INTERVAL_MS;

/// 49.8-19: headroom in the session-end budget for the awaited terms no
/// constant bounds — WebSocket writes, lock hand-offs, scheduling. It is not
/// a race budget; every other term in the budget is sized to actually cover
/// its own worst case.
pub const SESSION_END_HEADROOM_MS: u64 = 1_000;

/// 49.8-19 (VERIFICATION gap #1, review 49.8-19): the code-only ceiling
/// (like `safety.rs`'s hard caps — D-08/D-11, nothing relaxes it) on how
/// many distinct motion topics one process may drive. `config.yaml`'s
/// declared publish allowlist does NOT bound this count on its own:
/// `Ros2ConfigSource::Live` re-reads `config.yaml` fresh on every call, and
/// `motion_topics` is never pruned — so without this ceiling the
/// session-end bound would have to read a live, racing count at session
/// end. Sizing [`ROS2_SESSION_END_TIMEOUT_SECS`] for this ceiling instead
/// removes that read entirely. The arithmetic (see that constant's doc):
/// `PREEMPT_WAIT_MS` (1500) + `RECONNECT_STOP_TIMEOUT_SECS*1000` (3000) +
/// `MAX_MOTION_TOPICS * STOP_SEQUENCE_MS` (8 * 500 = 4000) +
/// `GOAL_CANCEL_PHASE_MS` (2500) + `SESSION_END_HEADROOM_MS` (1000) =
/// 12000ms, plus the 2s shutdown margin = 14s, inside both deploy units'
/// `TimeoutStopSec=15`. Raising this ceiling requires raising
/// `ROS2_SESSION_END_TIMEOUT_SECS` together with it — the unit files' pin
/// caps `ROS2_SESSION_END_TIMEOUT_SECS` at 13s, so the largest ceiling they
/// allow is 10.
/// [`ros2_stop_ordering_wait_budget_fits_session_end_bound`] pins this
/// value against that arithmetic for every `n` in `0..=MAX_MOTION_TOPICS`.
pub const MAX_MOTION_TOPICS: usize = 8;

/// How long [`Ros2Session::begin_motion`] waits for a preempted motion's
/// finished signal before giving up and installing the new one anyway, AND
/// (49.8-15, CR-01) how long [`Ros2Session::stop_all`] waits for the motion
/// it interrupts before running its own per-topic stop burst — long enough
/// for the interrupted motion's own stop sequence to complete with headroom:
/// the zero-Twist burst alone is about 200ms (`STOP_REPEATS` sends,
/// `STOP_REPEATS - 1` gaps of `STOP_INTERVAL_MS`), or about 500ms when a
/// G-49.8-2b stop follow-up is attached (zero burst + one `STOP_INTERVAL_MS`
/// gap + the follow-up burst, also ~200ms) — well inside this 1500ms budget.
/// Shared, not private to `begin_motion`, because both call sites wait on the
/// exact same class of signal (a motion's own finished notification).
pub const PREEMPT_WAIT_MS: u64 = 1_500;

/// 49.8-16 (G-49.8-2b): appended to a stop's report when [`Ros2Session::stop_all_as`]
/// answers a queued/hook caller with an earlier clean stop's report instead
/// of resending — nothing was driven or sent on the wire for this call.
pub const STOP_COALESCED_NOTE: &str =
    " (coalesced with the last clean stop: nothing driven or sent since)";

/// What an in-flight motion has reported on its `finished_tx` channel.
/// `followup_confirmed` is `true` ONLY when the motion attached a
/// [`StopFollowup`] AND both its zero-Twist burst AND its follow-up burst
/// were confirmed sent, on the primary or the reconnected connection
/// (49.8-15, CR-01). When `true`, [`Ros2Session::stop_all`] skips the
/// redundant zero-Twist burst for that one topic — the motion's own confirmed
/// zero cmd_vel already stopped it, and a second zero Twist sent after that
/// motion's own gait-0 rest stop would restart the march.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionStatus {
    Running,
    Finished { followup_confirmed: bool },
}

struct ActiveMotion {
    id: u64,
    token: CancellationToken,
    finished_rx: watch::Receiver<MotionStatus>,
    topic: String,
}

/// The result of [`await_motion_finish`]'s bounded wait on an interrupted
/// motion's finish signal (49.8-15, CR-01).
enum InterruptWait {
    /// No motion was active — nothing to wait for.
    NoActiveMotion,
    /// The motion finished and confirmed BOTH its zero-Twist burst and its
    /// follow-up (gait-0) burst sent, on the named topic. `stop_all` skips
    /// the redundant zero-Twist burst for this topic only.
    FollowupConfirmed(String),
    /// The motion finished, but without a confirmed follow-up (no follow-up
    /// attached, or its burst failed) — the full stop still runs.
    Finished,
    /// The motion's `finished_tx` was dropped without ever sending (e.g. the
    /// motion task panicked) — returns immediately, full stop still runs.
    SignalDropped,
    /// The wait exceeded [`PREEMPT_WAIT_MS`] — the full stop still runs, and
    /// the caller notes the degraded ordering in its report.
    TimedOut,
}

/// Shared bounded wait for an interrupted (cancelled) motion's finish signal
/// — used by BOTH [`Ros2Session::begin_motion`]'s preemption and
/// [`Ros2Session::stop_all`]'s interrupt wait (49.8-15, CR-01). Consumes
/// `active` (its token must already be cancelled by the caller) and waits up
/// to [`PREEMPT_WAIT_MS`] for its `finished_rx` to report
/// `MotionStatus::Finished { .. }`. The watch-guard's value is copied out
/// (`MotionStatus` is `Copy`) before any further await, so no borrow of the
/// channel's internal lock is ever held across an await point.
async fn await_motion_finish(active: ActiveMotion) -> InterruptWait {
    let ActiveMotion {
        mut finished_rx,
        topic,
        ..
    } = active;
    let waited = tokio::time::timeout(
        Duration::from_millis(PREEMPT_WAIT_MS),
        finished_rx.wait_for(|s| matches!(s, MotionStatus::Finished { .. })),
    )
    .await;
    match waited {
        Ok(Ok(status_ref)) => {
            let status = *status_ref;
            drop(status_ref);
            match status {
                MotionStatus::Finished {
                    followup_confirmed: true,
                } => InterruptWait::FollowupConfirmed(topic),
                _ => InterruptWait::Finished,
            }
        }
        Ok(Err(_)) => InterruptWait::SignalDropped,
        Err(_) => InterruptWait::TimedOut,
    }
}

/// D-06 building block: one action goal `action_ops::send_goal` has sent,
/// tracked so [`Ros2Session::stop_all`]/`action_ops::cancel_goal` know which
/// connection owns it. `connection_id` is [`Ros2BridgeClient::connection_id`]
/// at the moment the goal was sent — after a reconnect the session's current
/// client has a *different* id, which is exactly the signal `stop_all` uses
/// to fall back to [`cancel_all_goals_fallback`] instead of the normal
/// same-connection `cancel_action_goal`.
#[derive(Debug, Clone)]
pub struct TrackedGoal {
    pub action: String,
    pub action_type: String,
    pub connection_id: u64,
}

/// 49.8-16 (G-49.8-2b): the last stop `stop_all_as` completed cleanly — a
/// client existed, no topic/goal/rest-stop failure occurred, and the
/// interrupt wait (if any) did not time out. Read by the coalesce check at
/// the top of every subsequent `stop_all_as` call: a caller queued behind
/// this stop, with the activity epoch unchanged, is answered with `report`
/// (plus [`STOP_COALESCED_NOTE`]) instead of resending.
#[derive(Debug, Clone)]
struct CleanStop {
    seq: u64,
    activity_epoch: u64,
    report: String,
}

/// Which caller is asking `stop_all_as` to stop — controls the coalesce
/// check's queuing rule (49.8-16, G-49.8-2b). `Direct` callers (explicit
/// `stop`, `end_session_stop`/`with_session_end_stop`) coalesce only with a
/// clean stop that completed AFTER they entered (C1) — a caller that arrived
/// well before any stop happened still gets its own real stop. The
/// fire-and-forget `on_session_end` hooks always coalesce with an unchanged-
/// epoch clean stop regardless of entry order (C2) — every surface that
/// fires them also awaits `end_session_stop`, so C2 only ever removes a
/// redundant resend of a stop the caller's own turn already waited for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopCaller {
    Direct,
    SessionEndHook,
}

/// 49.8-19 (VERIFICATION gap #7, review 49.8-19): clears
/// [`Ros2Session::stop_in_flight`] on `Drop` — the ONLY way the flag ever
/// clears, on every exit path of the owned stop task ([`run_stop`]): a
/// normal return, a panic (Rust unwinds through this guard's stack slot
/// like any other local), or runtime teardown (the executor drops a
/// still-pending task's live locals, this one included, without polling it
/// further). Constructed in the take critical section, in the SAME
/// synchronous block (no `.await` in between) as the `stop_in_flight.store(true,
/// ..)` call itself, `last_clean_stop = None`, and the `active_motion` take
/// — so a racing `begin_motion`/stop can never observe the flag set without
/// the take, or the take without the flag set.
struct StopInFlightGuard(Arc<std::sync::atomic::AtomicBool>);

impl Drop for StopInFlightGuard {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// 49.8-19: what the owned stop task ([`run_stop`]) needs from the session —
/// `Arc` clones of the two shared fields, so the task's lifetime (and its
/// hold on `stop_lock`/the in-flight flag) is independent of whatever caller
/// spawned it and may later drop the `JoinHandle`.
struct StopCtx {
    inner: Arc<Mutex<SessionInner>>,
    stop_in_flight: Arc<std::sync::atomic::AtomicBool>,
}

/// 49.8-19: what [`run_stop`] hands back once the whole stop has finished —
/// the two guards it held for its ENTIRE run. Field order is load-bearing:
/// Rust drops struct fields in declaration order, so whenever this struct
/// itself is dropped as a whole (e.g. an abandoned `JoinHandle`'s stored
/// output), `in_flight` (which clears the in-flight flag) drops BEFORE
/// `lock` (which releases `stop_lock`) — the flag is never observably true
/// while `stop_lock` is free. [`Ros2Session::stop_all_as`]'s normal path
/// drops them explicitly in that same order instead, so the caller resumes
/// in the same poll in which the lock is released (49.8-17's WR-02 timing,
/// unchanged by this refactor).
struct StopTaskOutput {
    report: String,
    in_flight: Option<StopInFlightGuard>,
    lock: tokio::sync::OwnedMutexGuard<()>,
}

/// 49.8-19 (review 49.8-19): [`Ros2Session::end_session_stop`]'s reply when
/// the outer [`ROS2_SESSION_END_TIMEOUT_SECS`] bound elapses before the
/// owned stop task ([`run_stop`]) finishes. The awaited task IS the drain —
/// there is no second drain, because waiting longer on a process-exit path
/// would consume the systemd margin [`ROS2_SESSION_END_TIMEOUT_SECS`]'s own
/// doc pins. Unfinished stop work (remaining gait-0 rest stops, goal
/// cancels) continues ONLY while this process keeps running; at process
/// exit, runtime teardown drops any stop task still running at its next
/// `.await`, sending nothing further — the next stop (in this process, if
/// it survives, or in a new one) is a full, non-coalesced stop, because
/// `last_clean_stop` was already invalidated at the take that started this
/// run.
pub const SESSION_END_TIMEOUT_REPORT: &str = "ros2 session-end stop timed out — unfinished stop work continues only while this process keeps running";

/// 49.8-19: [`Ros2Session::stop_all_as`]'s reply when the owned stop task
/// ([`run_stop`]) panics. Unwinding already released every guard it held
/// (`stop_lock`, the in-flight flag) before this value is ever returned, and
/// `last_clean_stop` was already invalidated at take if the panic happened
/// after that point — so nothing here is confirmed stopped, and the next
/// stop re-sends everything.
pub const STOP_TASK_PANICKED_REPORT: &str = "stop_all: the stop task panicked — nothing is confirmed stopped; the next stop re-sends everything";

struct SessionInner {
    url: Option<String>,
    client: Option<Arc<Ros2BridgeClient>>,
    /// D-10/D-16: every Twist-family topic (name -> declared type) this
    /// process has driven at least once — read by 49.8-07's `stop_all`.
    motion_topics: BTreeMap<String, String>,
    /// G-49.8-2b (49.8-14): the [`StopFollowup`] the most recent motion on
    /// each topic attached, if any — sticky and never pruned, exactly like
    /// `motion_topics`. `begin_motion` with `stop_followup: None` never
    /// removes an existing entry (a generic-tool motion on the same topic
    /// must not erase the ROSPider's recorded rest stop). Read by
    /// `stop_all`'s replay.
    stop_followups: BTreeMap<String, StopFollowup>,
    active_motion: Option<ActiveMotion>,
    next_motion_id: u64,
    /// D-06/D-10: every action goal sent in this process, by goal id —
    /// never pruned (matches `motion_topics`'s own never-cleared precedent);
    /// `stop_all` re-checks each one's terminal state via `goal_snapshot`
    /// before deciding whether it still needs cancelling.
    goals: HashMap<String, TrackedGoal>,
    /// 49.8-16 (G-49.8-2b): incremented by `begin_motion` and `track_goal` —
    /// any new activity re-arms a full stop, unconditionally, no matter what
    /// `last_clean_stop` records. Coalescing (C1/C2) requires this value to
    /// be unchanged since the recorded clean stop's own snapshot.
    activity_epoch: u64,
    /// 49.8-16 (G-49.8-2b): total `stop_all_as` calls that have completed
    /// (clean or not) — a monotonically increasing sequence number used by
    /// the Direct-caller coalesce rule (C1) to tell "a stop completed while
    /// I was already queued" from "a stop completed before I ever arrived".
    stops_completed: u64,
    /// 49.8-16 (G-49.8-2b): the most recent CLEAN stop, if any — `None`
    /// whenever the most recent stop had a failure or timed out (never
    /// coalesced with).
    last_clean_stop: Option<CleanStop>,
}

/// One persistent rosbridge connection per process, lazily connected, replaced
/// on the next call once closed or once the URL changes.
pub struct Ros2Session {
    inner: Arc<Mutex<SessionInner>>,
    /// 49.8-16 (G-49.8-2b): serializes every `stop_all_as` and
    /// `send_stop_followup` call so concurrent stop requests (both
    /// `on_session_end` hooks plus the awaited `end_session_stop` at
    /// TUI/REPL exit, or an explicit stop racing a session end) never
    /// interleave on the wire. Lock order is ALWAYS this lock first, then
    /// `inner` — never the reverse. `motion_task` and `emergency_stop`
    /// never take this lock, so waiting on it (while it is held across
    /// `stop_all_as`'s bounded interrupt wait) can never deadlock against
    /// the very motion that wait is waiting on. 49.8-17 (WR-02):
    /// `begin_motion` also takes it, but ONLY as a wait-gate: when
    /// [`Ros2Session::stop_in_flight`] is set it acquires this lock and
    /// immediately releases it — blocking, in effect, until the in-flight
    /// stop has fully finished — and NEVER holds it across its own
    /// publish/install. 49.8-19: `Arc`-shared (not held on this struct's own
    /// stack) so the owned stop task ([`run_stop`]) can hold the guard for
    /// its whole run, independent of whether the caller that spawned it is
    /// still awaiting it.
    stop_lock: Arc<Mutex<()>>,
    /// 49.8-17 (WR-02), 49.8-19 (review 49.8-19: moved out of `SessionInner`
    /// into its own `Arc<AtomicBool>` so the owned stop task can hold it,
    /// via [`StopInFlightGuard`], independent of the caller): `true` while a
    /// stop is mid-flight — set in the same critical section that takes
    /// `active_motion` out, cleared by `StopInFlightGuard`'s `Drop` on every
    /// exit path (normal return, a panic, or runtime teardown), never by a
    /// bare assignment. [`Ros2Session::begin_motion`] reads it to decide
    /// whether it must wait on `stop_lock` before installing a new motion
    /// (see the `stop_lock` field doc). Deliberately a boolean, not a
    /// generation counter: the race to close is "a stop is STILL running
    /// right now", which a sequence number could not distinguish from a stop
    /// that finished long ago. Not a synchronization primitive on its own —
    /// see [`Ros2Session::stop_in_flight`]'s doc.
    stop_in_flight: Arc<std::sync::atomic::AtomicBool>,
}

impl Default for Ros2Session {
    fn default() -> Self {
        Self::new_inner()
    }
}

impl Ros2Session {
    /// A fresh, not-yet-connected session, shared via `Arc` (the registry owns
    /// one and hands the same `Arc` to every ros2-toolset tool it registers).
    pub fn new() -> Arc<Self> {
        Arc::new(Self::new_inner())
    }

    fn new_inner() -> Self {
        Self {
            inner: Arc::new(Mutex::new(SessionInner {
                url: None,
                client: None,
                motion_topics: BTreeMap::new(),
                stop_followups: BTreeMap::new(),
                active_motion: None,
                next_motion_id: 0,
                goals: HashMap::new(),
                activity_epoch: 0,
                stops_completed: 0,
                last_clean_stop: None,
            })),
            stop_lock: Arc::new(Mutex::new(())),
            stop_in_flight: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// 49.8-19 (review 49.8-19): observability for tests; not a
    /// synchronization primitive. Reflects whether the owned stop task
    /// ([`run_stop`]) currently holds [`StopInFlightGuard`] — true only
    /// while that task holds `stop_lock`, and cleared by the guard's `Drop`
    /// on every exit path (normal return, panic, or runtime teardown).
    #[doc(hidden)]
    pub fn stop_in_flight(&self) -> bool {
        self.stop_in_flight
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The client connected to `url`. Reuses the current client when the URL
    /// matches and the connection is not closed; otherwise connects fresh and
    /// replaces it.
    pub async fn client_for(&self, url: &str) -> anyhow::Result<Arc<Ros2BridgeClient>> {
        let mut guard = self.inner.lock().await;
        if let (Some(existing_url), Some(client)) = (guard.url.as_deref(), guard.client.as_ref())
            && existing_url == url
            && !client.is_closed()
        {
            return Ok(client.clone());
        }
        let client = Arc::new(Ros2BridgeClient::connect(url).await?);
        guard.url = Some(url.to_string());
        guard.client = Some(client.clone());
        Ok(client)
    }

    /// The currently held client, if any — never connects. Used by
    /// `on_session_end`/`stop_all` (49.8-04+) to reach an already-open
    /// connection without opening a new one just to send a stop.
    pub async fn current_client(&self) -> Option<Arc<Ros2BridgeClient>> {
        self.inner.lock().await.client.clone()
    }

    /// D-09/D-10: records `topic` (-> `msg_type`) in the motion-topics map,
    /// preempts any currently active motion (cancels its token and waits up
    /// to [`PREEMPT_WAIT_MS`] for its finished signal — so the previous
    /// motion's zero Twist lands on the wire before the new velocity does),
    /// then installs and returns a fresh id/cancellation-token/finished-sender
    /// for the caller's own motion.
    ///
    /// G-49.8-2b (49.8-14): `stop_followup` records (inserts or overwrites)
    /// this topic's entry in `stop_followups` when `Some`. `None` NEVER
    /// removes an existing entry — a generic-tool motion on the same topic
    /// (which attaches no follow-up) must not erase the ROSPider's recorded
    /// rest stop from an earlier motion on that topic.
    ///
    /// 49.8-17 (WR-02): a `begin_motion` that lands while a stop is
    /// mid-flight (`stop_in_flight` set — the stop has already taken
    /// `active_motion` out and is draining the old motion) first waits on
    /// `stop_lock`, which the owned stop task (49.8-19: [`run_stop`], not the
    /// caller that spawned it) holds for its whole run: the
    /// stop's zero-Twist bursts and gait-0 rest stops all land on the wire
    /// (and the interrupt wait completes) BEFORE this motion's first velocity
    /// is ever installed. The gate is an acquire-and-immediately-release — it
    /// never holds `stop_lock` across this call's own publish/install (see
    /// the `stop_lock` field doc). After the gate the stale-motion drain is
    /// re-checked so the gate can never install over a motion a racer
    /// installed while this call was waiting.
    pub async fn begin_motion(
        &self,
        topic: &str,
        msg_type: &str,
        stop_followup: Option<&StopFollowup>,
    ) -> (u64, CancellationToken, watch::Sender<MotionStatus>) {
        let mut guard = self.inner.lock().await;
        guard
            .motion_topics
            .insert(topic.to_string(), msg_type.to_string());
        if let Some(followup) = stop_followup {
            guard
                .stop_followups
                .insert(topic.to_string(), followup.clone());
        }
        // 49.8-16 (G-49.8-2b): new activity re-arms a full stop — any stop
        // that was clean before this motion began no longer coalesces.
        guard.activity_epoch += 1;

        if let Some(active) = guard.active_motion.take() {
            active.token.cancel();
            // Release the lock while waiting so the preempted motion's own
            // `end_motion` call (which needs the same lock) is not deadlocked
            // against this wait. Shared with `stop_all` (49.8-15, CR-01).
            drop(guard);
            let _ = await_motion_finish(active).await;
            guard = self.inner.lock().await;
        }

        // 49.8-17 (WR-02): a stop_all is mid-flight (it set `stop_in_flight`
        // in the same critical section where it took `active_motion` out and
        // is draining the old motion — its interrupt wait may even still be
        // running). Installing a new motion now would put the new velocity on
        // the wire WHILE the stop's zero-Twist bursts / gait-0 rest stops are
        // still going out. The owned stop task holds `stop_lock` for its
        // whole run (49.8-19: [`run_stop`], not the caller that spawned it),
        // so acquire-and-immediately-release it: this blocks until the in-flight
        // stop has fully finished, THEN the install below proceeds. The gate
        // never holds `stop_lock` across the install (frontmatter
        // prohibition), and after it the stale-motion drain is re-checked
        // because a racer may have installed a motion while this call waited
        // (begin_motion is not serialized by `stop_lock`).
        if self
            .stop_in_flight
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            drop(guard);
            let _gate = self.stop_lock.lock().await;
            drop(_gate);
            guard = self.inner.lock().await;
            if let Some(active) = guard.active_motion.take() {
                active.token.cancel();
                drop(guard);
                let _ = await_motion_finish(active).await;
                guard = self.inner.lock().await;
            }
        }

        let id = guard.next_motion_id;
        guard.next_motion_id += 1;
        let token = CancellationToken::new();
        let (finished_tx, finished_rx) = watch::channel(MotionStatus::Running);
        guard.active_motion = Some(ActiveMotion {
            id,
            token: token.clone(),
            finished_rx,
            topic: topic.to_string(),
        });
        (id, token, finished_tx)
    }

    /// Clears the active-motion slot ONLY if `id` is still the current one —
    /// a later [`Ros2Session::begin_motion`] may already have preempted it,
    /// in which case this is a no-op.
    pub async fn end_motion(&self, id: u64) {
        let mut guard = self.inner.lock().await;
        if guard.active_motion.as_ref().map(|a| a.id) == Some(id) {
            guard.active_motion = None;
        }
    }

    /// D-06: records that `goal_id` was sent to `action` (`action_type`) on
    /// `connection_id` — read back by [`Ros2Session::stop_all`] and
    /// `action_ops::cancel_goal`.
    pub async fn track_goal(
        &self,
        goal_id: &str,
        action: &str,
        action_type: &str,
        connection_id: u64,
    ) {
        let mut guard = self.inner.lock().await;
        guard.goals.insert(
            goal_id.to_string(),
            TrackedGoal {
                action: action.to_string(),
                action_type: action_type.to_string(),
                connection_id,
            },
        );
        // 49.8-16 (G-49.8-2b): new activity re-arms a full stop.
        guard.activity_epoch += 1;
    }

    /// Removes `goal_id` from the tracked set — used by `action_ops::send_goal`
    /// when rosbridge rejected the goal outright (`ACTION_OPS_UNSUPPORTED`),
    /// so a goal that never existed on the robot cannot leave every later
    /// [`Ros2Session::stop_all`] reporting a permanent goal failure.
    pub async fn untrack_goal(&self, goal_id: &str) {
        self.inner.lock().await.goals.remove(goal_id);
    }

    /// The tracked entry for `goal_id`, if this process has sent it.
    pub async fn tracked_goal(&self, goal_id: &str) -> Option<TrackedGoal> {
        self.inner.lock().await.goals.get(goal_id).cloned()
    }

    /// Every Twist-family topic driven in this process so far, `(topic,
    /// msg_type)` — read by 49.8-07's `stop_all`.
    pub async fn motion_topics(&self) -> Vec<(String, String)> {
        self.inner
            .lock()
            .await
            .motion_topics
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// 49.8-19 (VERIFICATION gap #1, review 49.8-19, D-08/D-11): the code-only
    /// ceiling call site — [`super::motion::run_timed_motion`] calls this
    /// BEFORE [`Ros2Session::begin_motion`] (its only production call site),
    /// so a refused topic never reaches the wire: nothing is connected,
    /// advertised or published for it. Checks and inserts under ONE inner-
    /// lock critical section, with no `.await` in between — a topic already
    /// tracked in `motion_topics` is always admitted (idempotent; re-driving
    /// a tracked topic is never refused and never removed from
    /// `motion_topics`), a new topic is inserted while
    /// `motion_topics.len() < MAX_MOTION_TOPICS`, otherwise the call returns
    /// a refusal in the [`super::not_permitted`] style naming the ceiling and
    /// that the count resets only when the process restarts. This method
    /// never touches `activity_epoch`, `stop_followups` or `active_motion` —
    /// `begin_motion` still does all of that when the motion actually
    /// begins, and its own `motion_topics` insert stays (a no-op for an
    /// already-admitted topic).
    pub async fn admit_motion_topic(&self, topic: &str, msg_type: &str) -> Result<(), String> {
        let mut guard = self.inner.lock().await;
        if guard.motion_topics.contains_key(topic) {
            return Ok(());
        }
        if guard.motion_topics.len() >= MAX_MOTION_TOPICS {
            return Err(format!(
                "'{topic}' is not permitted — this process has already driven \
                 {MAX_MOTION_TOPICS} distinct motion topics, the most whose session-end stop \
                 is guaranteed to fit its bound. Do not retry; the count resets only when the \
                 process restarts."
            ));
        }
        guard
            .motion_topics
            .insert(topic.to_string(), msg_type.to_string());
        Ok(())
    }

    /// D-10: called when the primary connection is lost (or its own stop
    /// publish fails) mid-motion. Reconnects to `url` bounded by
    /// [`RECONNECT_STOP_TIMEOUT_SECS`] and, on success, publishes a zero
    /// Twist [`super::safety::STOP_REPEATS`] times on `topic`. Returns `true`
    /// iff the reconnect AND every stop publish succeeded. The new connection
    /// becomes this session's current one on success, so a subsequent call
    /// reuses it rather than reconnecting again.
    pub async fn emergency_stop(&self, url: &str, topic: &str, msg_type: &str) -> bool {
        use super::safety::{STOP_INTERVAL_MS, STOP_REPEATS, zero_twist};

        let client = match tokio::time::timeout(
            Duration::from_secs(RECONNECT_STOP_TIMEOUT_SECS),
            Ros2BridgeClient::connect(url),
        )
        .await
        {
            Ok(Ok(c)) => Arc::new(c),
            _ => return false,
        };
        {
            let mut guard = self.inner.lock().await;
            guard.url = Some(url.to_string());
            guard.client = Some(client.clone());
        }

        // D-09: no settle here either — see
        // `Ros2BridgeClient::advertise_no_settle`'s doc comment; the 3x
        // zero-Twist stop sequence below is its own redundant-send.
        if client.advertise_no_settle(topic, msg_type).await.is_err() {
            return false;
        }

        let stamped = super::canonical_type(msg_type) == "geometry_msgs/TwistStamped";
        let mut ok = true;
        for i in 0..STOP_REPEATS {
            if client.publish(topic, zero_twist(stamped)).await.is_err() {
                ok = false;
                break;
            }
            if i + 1 < STOP_REPEATS {
                tokio::time::sleep(Duration::from_millis(STOP_INTERVAL_MS)).await;
            }
        }
        ok
    }

    /// D-10: the one stop path — zeroes every motion topic this process has
    /// driven AND cancels every unfinished action goal it has sent. With no
    /// motion topics, no active motion, and no unfinished goals this returns
    /// `"nothing to stop"` WITHOUT connecting (the `stop_all never opens a
    /// bridge connection...` prohibition) — `current_client()` (never
    /// connects) is enough to answer that question. Otherwise: takes the
    /// active motion out, cancels its token, drops the inner lock, and waits
    /// — bounded by [`PREEMPT_WAIT_MS`], via [`await_motion_finish`] — for
    /// that motion's own finish signal BEFORE publishing anything of its own
    /// (49.8-15, CR-01: the interrupted motion's own zero-Twist-then-gait-0
    /// stop is guaranteed to land on the wire first, and complete, for the
    /// topic it was driving). It then re-reads [`Ros2Session::current_client`]
    /// (an interrupted motion that lost its connection reconnects through
    /// [`Ros2Session::emergency_stop`], installing a new client) or —
    /// closed/absent and `url` is `Some` — reconnects once bounded by
    /// [`RECONNECT_STOP_TIMEOUT_SECS`], publishes a zero Twist
    /// [`super::safety::STOP_REPEATS`] times on every `(topic, type)` in
    /// `motion_topics` (skipping only the topic whose interrupted motion just
    /// confirmed its own zero-Twist-then-follow-up burst sent), and for each
    /// unfinished goal either `cancel_action_goal`s it on its own connection
    /// (still open) or falls back to [`cancel_all_goals_fallback`] on
    /// whatever connection this call does hold. A wait that times out still
    /// runs the full stop afterwards, with the degraded ordering named in the
    /// report.
    ///
    /// G-49.8-2b (49.8-14): for every topic that has a recorded
    /// [`StopFollowup`] (`stop_followups`), that topic's own zero-Twist burst
    /// is followed by [`publish_followup_burst`] — attempted even when the
    /// zero burst itself failed partway, because the follow-up (the
    /// ROSPider's gait-0 rest stop) is what actually halts the robot. This
    /// replay applies only to topics this process actually drove; it never
    /// opens a connection for an idle session (the `nothing to stop` early
    /// return above is unchanged) and never widens the unconditional send —
    /// that lives only in `rospider_tool.rs::do_stop`'s [`send_stop_followup`]
    /// call.
    ///
    /// 49.8-16 (G-49.8-2b): serialized against every other `stop_all`/
    /// `session_end_hook_stop`/`send_stop_followup` call via `stop_lock` —
    /// see [`Ros2Session::stop_all_as`] for the serialization and coalescing
    /// this delegates to (as a `StopCaller::Direct` caller).
    pub async fn stop_all(&self, url: Option<&str>) -> String {
        self.stop_all_as(url, StopCaller::Direct).await
    }

    /// The shared body of [`Ros2Session::stop_all`] and
    /// [`Ros2Session::session_end_hook_stop`] (49.8-16, G-49.8-2b).
    ///
    /// Concurrent stop requests are common: both `on_session_end` hooks fire
    /// at the same moment as the awaited `end_session_stop` at TUI/REPL exit
    /// (`cli main.rs`'s `run_chat`, `tui_rata/event_loop.rs`), or an explicit
    /// stop races a gateway `/new`. Without serialization the first request
    /// waits for the interrupted motion while the others burst immediately —
    /// the same race CR-01 already fixed for a single stop. `stop_lock` (a
    /// `Ros2Session`-level `tokio::sync::Mutex<()>`, held for this whole call
    /// and for all of [`Ros2Session::send_stop_followup`]) makes every stop
    /// run one at a time. Lock order is ALWAYS `stop_lock` first, then the
    /// inner mutex, never the reverse — `motion_task` and `emergency_stop`
    /// never take `stop_lock`, so waiting on it while it is held across the
    /// bounded interrupt wait below can never deadlock against the very
    /// motion that wait is waiting on. 49.8-17 (WR-02) removes the one
    /// remaining unclosed race: `begin_motion` (which never held `stop_lock`
    /// before this plan) could install a new motion while this call was
    /// mid-flight — its interrupt wait drained the old motion while `active_motion`
    /// was already `None`, so `begin_motion` saw an empty slot and
    /// published its new Twist WHILE this stop's bursts were still going out.
    /// This call now sets [`Ros2Session::stop_in_flight`] in the same
    /// critical section that takes `active_motion` out; a racing
    /// `begin_motion` sees the flag and waits on `stop_lock` until this call
    /// has fully finished (see the `stop_lock` field doc), so the stop's
    /// wire sequence always completes before the new motion's first
    /// velocity. (49.8-19: the flag now lives on `Ros2Session` itself, not
    /// `SessionInner`, and this whole call is now spawned as an owned task —
    /// see [`run_stop`] — rather than run on this function's own stack.)
    ///
    /// Coalescing (same session end, one stop): `SessionInner` tracks
    /// `activity_epoch` (bumped by `begin_motion`/`track_goal`),
    /// `stops_completed` (a monotonically increasing sequence number) and
    /// `last_clean_stop` (the most recent CLEAN stop's `seq`/`activity_epoch`/
    /// `report` — a stop is clean when a client existed, no topic/goal/rest-
    /// stop failure occurred, and the interrupt wait did not time out; never
    /// recorded for an unclean or timed-out stop). Two rules apply, checked
    /// BEFORE anything else in this call (even before deciding whether there
    /// is anything to stop):
    /// - (C1) A `Direct` caller (explicit stop, `end_session_stop`/
    ///   `with_session_end_stop`) that queued behind a clean stop which
    ///   completed AFTER this caller entered (`c.seq > entered_after`), with
    ///   the epoch unchanged, is answered with that stop's `report` plus
    ///   [`STOP_COALESCED_NOTE`] — nothing else is touched. A `Direct` caller
    ///   that arrived well before any stop happened still gets its own real
    ///   stop; explicit stops never silently vanish.
    /// - (C2) A `SessionEndHook` caller (the fire-and-forget
    ///   `on_session_end` hooks) coalesces with an unchanged-epoch clean stop
    ///   regardless of entry order. This is safe because every surface that
    ///   fires the hooks also awaits `end_session_stop` first (49.8-08/11) —
    ///   C2 only ever removes a REDUNDANT resend of a stop the caller's own
    ///   turn already waited for. This matters most at CLI one-shot/kanban
    ///   exit: there the hooks fire after that awaited stop, just before the
    ///   process exits, and a resend cut off between its zero Twist and its
    ///   gait-0 would leave the ROSPider marching.
    ///
    /// Any `begin_motion`/`track_goal` since the recorded clean stop's own
    /// snapshot bumps `activity_epoch`, which unconditionally re-arms a full
    /// stop for every subsequent caller, `Direct` or `SessionEndHook` alike —
    /// coalescing never suppresses a stop after new activity.
    ///
    /// 49.8-18 (VERIFICATION gap #1/#2, CR-01): the goal-cancel sub-phase
    /// (after the motion-topic zero-Twist bursts above, whose position and
    /// ordering are unchanged) is ONE concurrent phase bounded by
    /// [`GOAL_CANCEL_PHASE_MS`], independent of how many goals are open. A
    /// same-connection cancel counts only on the goal's own terminal
    /// status (4/5/6 — [`confirm_same_connection_cancel`]); a cancel-all is
    /// sent ONCE per distinct action (deduplicated) and its outcome
    /// ([`classify_cancel_all_response`]) applies to every unfinished goal
    /// of that action. Acceptance is not completion: a refused, merely-
    /// accepted (CANCELING), malformed, ignored, unanswered or non-terminal
    /// answer is a goal failure that re-arms the next stop; only
    /// `NotCancelable` is reported without failing the stop.
    async fn stop_all_as(&self, url: Option<&str>, caller: StopCaller) -> String {
        // Read `entered_after` under a brief inner lock and release it
        // BEFORE acquiring `stop_lock` below — this is what lets (C1) tell
        // "a clean stop completed while I was already queued" (coalesce)
        // from "a clean stop completed before I ever arrived" (still stop).
        let entered_after = self.inner.lock().await.stops_completed;

        // 49.8-19 (VERIFICATION gap #7, review 49.8-19): acquire `stop_lock`
        // as an OWNED guard and, with NO `.await` in between, hand it (plus
        // `Arc` clones of `inner`/`stop_in_flight`) to a spawned task
        // ([`run_stop`]) that runs the WHOLE stop. Awaiting that task's
        // `JoinHandle` — rather than holding the guard on this function's
        // own stack — is what lets the stop survive this call being
        // dropped (an outer timeout, an aborted caller): the task's
        // lifetime, and its hold on `stop_lock` and the in-flight flag, no
        // longer depend on anything still awaiting it.
        let lock = self.stop_lock.clone().lock_owned().await;
        let ctx = StopCtx {
            inner: self.inner.clone(),
            stop_in_flight: self.stop_in_flight.clone(),
        };
        let handle = tokio::spawn(run_stop(
            ctx,
            lock,
            url.map(str::to_string),
            caller,
            entered_after,
        ));

        match handle.await {
            Ok(out) => {
                // Drop `in_flight` before `lock` — mirrors `StopTaskOutput`'s
                // own field order and keeps 49.8-17's WR-02 timing: this
                // caller resumes in the same poll in which `stop_lock` is
                // released, exactly as when this function held the guard on
                // its own stack.
                drop(out.in_flight);
                drop(out.lock);
                out.report
            }
            // The stop task panicked. Unwinding already released every
            // guard it held (`stop_lock`, the in-flight flag) before this
            // arm ever runs, and `last_clean_stop` was already invalidated
            // at take if the panic happened after that point.
            Err(_join_err) => STOP_TASK_PANICKED_REPORT.to_string(),
        }
    }
}

/// 49.8-19 (VERIFICATION gap #7, review 49.8-19): the owned stop task
/// [`Ros2Session::stop_all_as`] spawns and awaits `lock` for. Runs the WHOLE
/// former body of a stop — the coalesce check, the pre-wait snapshot, the
/// interrupt wait, the reconnect, every motion topic's zero-Twist-then-
/// gait-0 sequence, the 49.8-18 goal-cancel phase, and the clean-stop
/// recording — from the moment `lock` (already acquired by the caller) is
/// handed in, to the moment this function returns. Dropping the CALLER (an
/// aborted `JoinHandle`, an outer timeout) changes nothing while the
/// runtime lives: this task keeps running to completion on its own,
/// independent of whether anything is still awaiting it — a stop's caller
/// giving up must never truncate it between a topic's zero Twist and its
/// gait-0, and must never skip the goal-cancel phase (VERIFICATION gap #7).
async fn run_stop(
    ctx: StopCtx,
    lock: tokio::sync::OwnedMutexGuard<()>,
    url: Option<String>,
    caller: StopCaller,
    entered_after: u64,
) -> StopTaskOutput {
    // The coalesce check is its own critical section (no `.await` between
    // reading `last_clean_stop`/`activity_epoch` and deciding), so a racing
    // `begin_motion`/`track_goal` can never straddle the decision itself.
    let coalesced_report: Option<String> = {
        let guard = ctx.inner.lock().await;
        match &guard.last_clean_stop {
            Some(c) if c.activity_epoch == guard.activity_epoch => {
                if matches!(caller, StopCaller::SessionEndHook) || c.seq > entered_after {
                    Some(format!("{}{STOP_COALESCED_NOTE}", c.report))
                } else {
                    None
                }
            }
            _ => None,
        }
    };
    // 49.8-19: the coalesced and `nothing to stop` early returns hand back
    // an output with `in_flight: None` — no wire work was ever attempted,
    // so nothing was ever set that needs clearing.
    if let Some(report) = coalesced_report {
        return StopTaskOutput {
            report,
            in_flight: None,
            lock,
        };
    }

    // WR-02 (review 49.8, carried forward): capture `motion_topics`/
    // `active_motion_present`/`goals_snapshot`/`stop_followups`/
    // `snapshot_epoch`, compute `unfinished`, AND decide the "nothing to
    // stop" early return — ALL while STILL HOLDING this one guard, rather
    // than across two separate lock acquisitions with a window between
    // them. Previously the snapshot was taken (and its guard dropped)
    // BEFORE a second, independent lock acquisition fetched `current`; a
    // task that never takes `stop_lock` (e.g. `begin_motion`/`track_goal`,
    // both of which DO take this same `ctx.inner` lock) could install a
    // brand new motion/goal and bump `activity_epoch` in that window
    // without this call ever observing it, letting "nothing to stop" fire
    // on stale pre-`.await` state while a motion had, in fact, just
    // started. `current.goal_snapshot(id)` below only touches the bridge
    // client's own internal (`std::sync`) mutex, never `ctx.inner` — so
    // holding this guard through that call cannot deadlock — and every
    // step from the snapshot to the decision is synchronous (no `.await`),
    // so no other task can observe or mutate `ctx.inner` state in between.
    let stop_work = {
        let guard = ctx.inner.lock().await;
        let current = guard.client.clone();
        let motion_topics = guard
            .motion_topics
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Vec<(String, String)>>();
        let active_motion_present = guard.active_motion.is_some();
        let goals_snapshot = guard.goals.clone();
        let stop_followups = guard.stop_followups.clone();
        let snapshot_epoch = guard.activity_epoch;

        // A goal is only "done, skip cancelling" when we can PROVE it via a
        // real `result` from its own owning connection — a "bridge
        // disconnected" `error` (set by the reader task on stream end, see
        // `bridge_client.rs`) must NOT count as settled here, or a goal that
        // is still running server-side after its connection dropped would
        // silently never get cancelled (exactly the D-10 failure this
        // function exists to prevent). An id this process never tracked, or
        // whose owning connection we no longer hold, is conservatively
        // treated as unfinished.
        let mut unfinished: Vec<(String, TrackedGoal)> = Vec::new();
        for (id, g) in &goals_snapshot {
            let proven_done = current
                .as_ref()
                .filter(|c| c.connection_id() == g.connection_id)
                .and_then(|c| c.goal_snapshot(id))
                .map(|s| s.result.is_some())
                .unwrap_or(false);
            if !proven_done {
                unfinished.push((id.clone(), g.clone()));
            }
        }

        if motion_topics.is_empty() && !active_motion_present && unfinished.is_empty() {
            None
        } else {
            Some((motion_topics, stop_followups, snapshot_epoch, unfinished))
        }
    };
    let (motion_topics, stop_followups, snapshot_epoch, unfinished) = match stop_work {
        None => {
            return StopTaskOutput {
                report: "nothing to stop".to_string(),
                in_flight: None,
                lock,
            };
        }
        Some(work) => work,
    };

    // 49.8-15 (CR-01): take the active motion out from under the inner
    // lock, cancel its token, then DROP the guard before waiting — the
    // interrupted motion's own `end_motion`/`emergency_stop` need this
    // same lock, so waiting while holding it would deadlock against the
    // very motion this call is waiting on (a frontmatter prohibition).
    // 49.8-17 (WR-02)/49.8-19 (gap #7): set `stop_in_flight`, construct its
    // guard, invalidate `last_clean_stop`, and take `active_motion` — ALL
    // in the SAME critical section, in one synchronous block with no
    // `.await` in between. A racing `begin_motion`/stop can never observe
    // one of these without the others, and a stop that has begun wire work
    // invalidates the previous clean record until it completes cleanly
    // itself and re-records (below).
    let (active_motion, in_flight_guard) = {
        let mut guard = ctx.inner.lock().await;
        ctx.stop_in_flight
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let in_flight_guard = StopInFlightGuard(ctx.stop_in_flight.clone());
        guard.last_clean_stop = None;
        (guard.active_motion.take(), in_flight_guard)
    };
    let interrupt_wait = if let Some(active) = active_motion {
        active.token.cancel();
        await_motion_finish(active).await
    } else {
        InterruptWait::NoActiveMotion
    };

    // Re-read the client AFTER the wait: an interrupted motion that lost
    // its connection reconnects through `emergency_stop`, which installs
    // a new session client — reusing that connection here (instead of
    // the pre-wait `current`) avoids a second, redundant reconnect.
    let current_after_wait = ctx.inner.lock().await.client.clone();

    let client: Option<Arc<Ros2BridgeClient>> = match &current_after_wait {
        Some(c) if !c.is_closed() => Some(c.clone()),
        _ => match &url {
            Some(u) => match tokio::time::timeout(
                Duration::from_secs(RECONNECT_STOP_TIMEOUT_SECS),
                Ros2BridgeClient::connect(u),
            )
            .await
            {
                Ok(Ok(c)) => {
                    let c = Arc::new(c);
                    let mut guard = ctx.inner.lock().await;
                    guard.url = Some(u.to_string());
                    guard.client = Some(c.clone());
                    Some(c)
                }
                _ => None,
            },
            None => None,
        },
    };

    let mut topics_stopped = 0usize;
    let mut topics_failed = 0usize;
    let mut rest_ok = 0usize;
    let mut rest_failed = 0usize;
    let mut rest_attempted = 0usize;
    if let Some(client) = &client {
        for (topic, msg_type) in &motion_topics {
            let stamped = super::canonical_type(msg_type) == "geometry_msgs/TwistStamped";
            let followup = stop_followups.get(topic);

            // 49.8-15 (CR-01): skip ONLY this topic's own redundant
            // zero-Twist burst when the interrupted motion just proved
            // (within the wait above) that ITS OWN zero-Twist-then-
            // follow-up burst was fully confirmed sent on this exact
            // topic. A zero Twist sent after that motion's own gait-0
            // rest stop would restart the march — D-10 is still honored,
            // because that motion's own confirmed zero cmd_vel IS this
            // stop's zero burst for this topic. Every other topic, and
            // every other wait outcome (no active motion, unconfirmed,
            // dropped signal, or timed out), keeps the full burst below —
            // the stop is never skipped.
            let skip_zero_twist = matches!(
                &interrupt_wait,
                InterruptWait::FollowupConfirmed(t) if t == topic
            ) && followup.is_some();

            // G-49.8-2b: pre-advertise the follow-up topic before this
            // topic's own zero-Twist burst — non-fatal on error (mirrors
            // `motion.rs::motion_task`'s pre-advertise), because
            // `publish_followup_burst` below re-advertises anyway.
            if let Some(f) = followup
                && let Err(e) = client.advertise_no_settle(&f.topic, &f.msg_type).await
            {
                tracing::warn!(
                    topic = %f.topic,
                    error = %e,
                    "stop_all: pre-advertise of stop follow-up topic failed (non-fatal, publish_followup_burst re-advertises)"
                );
            }

            let ok = if skip_zero_twist {
                // The interrupted motion's own confirmed zero-Twist burst
                // already stopped this topic — nothing more to publish
                // here. Still counts as stopped (D-10 is satisfied by
                // that motion's own burst), and the topic still gets its
                // gait-0 rest stop below (repeated gait-0 is idempotent).
                true
            } else if client.advertise_no_settle(topic, msg_type).await.is_err() {
                false
            } else {
                let mut zero_ok = true;
                for i in 0..super::safety::STOP_REPEATS {
                    if client
                        .publish(topic, super::safety::zero_twist(stamped))
                        .await
                        .is_err()
                    {
                        zero_ok = false;
                        break;
                    }
                    if i + 1 < super::safety::STOP_REPEATS {
                        tokio::time::sleep(Duration::from_millis(super::safety::STOP_INTERVAL_MS))
                            .await;
                    }
                }
                zero_ok
            };
            if ok {
                topics_stopped += 1;
            } else {
                topics_failed += 1;
            }

            // G-49.8-2b: attempted regardless of `ok` — the rest stop is
            // what actually halts the ROSPider, so a partially-failed
            // zero-Twist burst must not skip it.
            if let Some(f) = followup {
                rest_attempted += 1;
                if publish_followup_burst(client, f).await {
                    rest_ok += 1;
                } else {
                    rest_failed += 1;
                }
            }
        }
    } else {
        topics_failed = motion_topics.len();
        for (topic, _) in &motion_topics {
            if stop_followups.contains_key(topic) {
                rest_attempted += 1;
                rest_failed += 1;
            }
        }
    }

    // 49.8-18 (VERIFICATION gap #1/#2, CR-01, D-06/D-10/D-16, review
    // 49.8-18 HIGH/MEDIUM): the goal-cancel phase is ONE concurrent,
    // deduplicated phase bounded by GOAL_CANCEL_PHASE_MS regardless of
    // goal count — never CANCEL_WAIT_SECS times the goal (or action)
    // count. A same-connection cancel counts only on the goal's OWN
    // terminal status (via `confirm_same_connection_cancel`); a
    // cancel-all's outcome (via `classify_cancel_all_response`) is
    // computed ONCE per distinct action and applied to every unfinished
    // goal of that action. A refused, merely-accepted (CANCELING, not
    // terminal), malformed, ignored, unanswered or non-terminal answer
    // is a goal failure that is never written to `last_clean_stop`, so
    // the next stop re-sends it; only `NotCancelable` is reported
    // without failing the stop.
    let mut goals_cancelled = 0usize;
    let mut goals_not_cancelable = 0usize;
    let mut goals_failed = 0usize;
    let mut used_fallback = false;
    let mut goal_cancel_reasons: BTreeSet<&'static str> = BTreeSet::new();
    const ACCEPTED_REASON: &str =
        "action server accepted the cancel-all; goals are CANCELING, terminal state not observed";

    match &client {
        None => {
            if !unfinished.is_empty() {
                goals_failed += unfinished.len();
                goal_cancel_reasons.insert("no bridge connection");
            }
        }
        Some(client) => {
            let phase_start = tokio::time::Instant::now();
            let send_deadline = phase_start + Duration::from_millis(GOAL_CANCEL_SEND_WINDOW_MS);
            let phase_deadline = phase_start + Duration::from_millis(GOAL_CANCEL_PHASE_MS);

            // Partition: same-connection goals confirm individually;
            // fallback goals are grouped by action so each distinct
            // action gets exactly ONE cancel-all, whose outcome applies
            // to every unfinished goal of that action (deduplication —
            // the fallback cancels EVERY goal on that action server).
            let mut same_connection: Vec<(String, String)> = Vec::new(); // (goal_id, action)
            let mut fallback_counts: BTreeMap<String, usize> = BTreeMap::new(); // action -> count
            for (id, g) in &unfinished {
                if client.connection_id() == g.connection_id {
                    same_connection.push((id.clone(), g.action.clone()));
                } else {
                    *fallback_counts.entry(g.action.clone()).or_insert(0) += 1;
                }
            }
            used_fallback = !fallback_counts.is_empty();
            let fallback_actions: Vec<&String> = fallback_counts.keys().collect();

            let same_connection_futs = same_connection.iter().map(|(id, action)| async {
                match tokio::time::timeout_at(
                    phase_deadline,
                    confirm_same_connection_cancel(client, action, id, send_deadline),
                )
                .await
                {
                    Ok(outcome) => outcome,
                    Err(_) => {
                        GoalCancelOutcome::Unconfirmed("no terminal status within the cancel phase")
                    }
                }
            });
            let fallback_futs = fallback_actions
                .iter()
                .map(|action| confirm_cancel_all(client, action, phase_deadline));

            let (same_connection_outcomes, fallback_outcomes) = futures::future::join(
                futures::future::join_all(same_connection_futs),
                futures::future::join_all(fallback_futs),
            )
            .await;

            for outcome in same_connection_outcomes {
                match outcome {
                    GoalCancelOutcome::TerminalConfirmed | GoalCancelOutcome::FinishedFirst => {
                        goals_cancelled += 1;
                    }
                    // WR-03: an action-ops-unsupported bridge can never
                    // confirm any cancel on any goal — reported the
                    // same way NotCancelable is: settled, not a stop
                    // failure (see the variant's doc comment).
                    GoalCancelOutcome::NotCancelable | GoalCancelOutcome::ActionOpsUnsupported => {
                        goals_not_cancelable += 1;
                    }
                    GoalCancelOutcome::Accepted => {
                        goals_failed += 1;
                        goal_cancel_reasons.insert(ACCEPTED_REASON);
                    }
                    GoalCancelOutcome::Refused(reason) | GoalCancelOutcome::Unconfirmed(reason) => {
                        goals_failed += 1;
                        goal_cancel_reasons.insert(reason);
                    }
                }
            }
            for (action, outcome) in fallback_actions.into_iter().zip(fallback_outcomes) {
                let count = fallback_counts[action];
                match outcome {
                    // classify_cancel_all_response never produces these
                    // three — fail closed on an impossible outcome
                    // rather than silently treating it as success.
                    GoalCancelOutcome::TerminalConfirmed
                    | GoalCancelOutcome::FinishedFirst
                    | GoalCancelOutcome::ActionOpsUnsupported => {
                        goals_failed += count;
                        goal_cancel_reasons
                            .insert("unexpected terminal outcome from a cancel-all classification");
                    }
                    GoalCancelOutcome::NotCancelable => goals_not_cancelable += count,
                    GoalCancelOutcome::Accepted => {
                        goals_failed += count;
                        goal_cancel_reasons.insert(ACCEPTED_REASON);
                    }
                    GoalCancelOutcome::Refused(reason) | GoalCancelOutcome::Unconfirmed(reason) => {
                        goals_failed += count;
                        goal_cancel_reasons.insert(reason);
                    }
                }
            }
        }
    }

    let mut out = format!(
        "stop_all: stopped {topics_stopped}/{} topic(s), cancelled {goals_cancelled} goal(s)",
        motion_topics.len()
    );
    if rest_attempted > 0 {
        out.push_str(&format!(
            ", gait-0 rest stop sent on {rest_ok}/{rest_attempted} topic(s)"
        ));
    }
    if used_fallback {
        out.push_str(" (cancel-all fallback used)");
    }
    if goals_not_cancelable > 0 {
        out.push_str(&format!(
            "; {goals_not_cancelable} goal(s) {GOAL_NOT_CANCELABLE_NOTE}"
        ));
    }
    if topics_failed > 0 || goals_failed > 0 || rest_failed > 0 {
        out.push_str(&format!(
            " — {topics_failed} topic failure(s), {goals_failed} goal failure(s)"
        ));
        if rest_failed > 0 {
            out.push_str(&format!(", {rest_failed} rest-stop failure(s)"));
        }
        if !goal_cancel_reasons.is_empty() {
            let reasons: Vec<&str> = goal_cancel_reasons.iter().take(3).copied().collect();
            out.push_str(&format!(
                " ({GOAL_CANCEL_NOT_CONFIRMED_NOTE}: {})",
                reasons.join("; ")
            ));
        }
    }
    if matches!(interrupt_wait, InterruptWait::TimedOut) {
        out.push_str(&format!(
                " (interrupted motion did not finish its own stop within {PREEMPT_WAIT_MS}ms; full stop sent anyway)"
            ));
    }

    // 49.8-16 (G-49.8-2b): record whether this stop was clean — a client
    // existed, no topic/goal/rest-stop failure occurred, and the wait (if
    // any) did not time out. Only a clean stop is ever coalesced with.
    let clean = client.is_some()
        && topics_failed == 0
        && goals_failed == 0
        && rest_failed == 0
        && !matches!(interrupt_wait, InterruptWait::TimedOut);
    {
        let mut guard = ctx.inner.lock().await;
        guard.stops_completed += 1;
        let seq = guard.stops_completed;
        guard.last_clean_stop = if clean {
            Some(CleanStop {
                seq,
                activity_epoch: snapshot_epoch,
                report: out.clone(),
            })
        } else {
            None
        };
    }

    // 49.8-19 (VERIFICATION gap #7): the in-flight flag is cleared by
    // `StopInFlightGuard`'s `Drop`, never by a bare assignment — the
    // caller ([`Ros2Session::stop_all_as`]) drops `in_flight` (and only
    // then `lock`) once it has this output in hand; if that caller is
    // gone, tokio drops this whole struct — and therefore the guard —
    // as soon as this task's output is dropped.
    StopTaskOutput {
        report: out,
        in_flight: Some(in_flight_guard),
        lock,
    }
}

impl Ros2Session {
    /// G-49.8-2b (49.8-14): the unconditional rest-stop send
    /// `rospider_tool.rs::do_stop` issues on EVERY explicit `stop` — even
    /// with nothing recorded in `stop_followups`, because the robot may
    /// still be marching from a process that no longer exists. Reuses or
    /// opens `url` (via `client_for`), then `advertise`s (WITH its 300ms
    /// settle — no zero-Twist burst precedes this standalone send to give
    /// DDS discovery time; idempotent, so a no-op when the topic is already
    /// advertised on this connection), then [`publish_followup_burst`].
    ///
    /// 49.8-16 (G-49.8-2b): acquires `stop_lock` for the connect, advertise
    /// and burst — serialized against every `stop_all`/`session_end_hook_stop`
    /// call, exactly like them. It does NOT touch `last_clean_stop` or
    /// `activity_epoch` (it is unconditional by design, never coalesced).
    /// `RospiderTool::do_stop` already calls this only after `stop_all`
    /// returned (and released `stop_lock`), so there is no nested
    /// acquisition — keep it that way.
    pub async fn send_stop_followup(
        &self,
        url: &str,
        followup: &StopFollowup,
    ) -> anyhow::Result<()> {
        let _stop_guard = self.stop_lock.lock().await;
        let client = self.client_for(url).await?;
        client
            .advertise(&followup.topic, &followup.msg_type)
            .await?;
        if publish_followup_burst(&client, followup).await {
            Ok(())
        } else {
            Err(anyhow::anyhow!("publish failed on {}", followup.topic))
        }
    }

    /// The form [`crate::ros2::Ros2Tool::on_session_end`] and
    /// [`crate::ros2::RospiderTool::on_session_end`] call (49.8-16,
    /// G-49.8-2b) — see [`Ros2Session::stop_all_as`] for the serialization
    /// and coalescing this delegates to (as a `StopCaller::SessionEndHook`
    /// caller, which additionally coalesces via C2 with an earlier clean
    /// stop whenever nothing has been driven or sent since).
    pub async fn session_end_hook_stop(&self, url: Option<&str>) -> String {
        self.stop_all_as(url, StopCaller::SessionEndHook).await
    }
}

/// D-06/D-10: the ROS 2 "cancel every goal on this action server" fallback —
/// `action_msgs/srv/CancelGoal` with an all-zero 16-byte `goal_id.uuid` and a
/// zero stamp is the documented cancel-ALL request. Used when the caller no
/// longer holds the goal's own owning connection: [`Ros2Session::stop_all`]
/// (after a reconnect) and `action_ops::cancel_goal` (closed-connection
/// case) both call this via whatever connection they currently hold.
pub(crate) async fn cancel_all_goals_fallback(
    client: &Ros2BridgeClient,
    action: &str,
) -> anyhow::Result<ServiceResponse> {
    let service = format!("{action}/_action/cancel_goal");
    let zero_uuid: Vec<Value> = vec![json!(0); 16];
    let args = json!({
        "goal_info": {
            "goal_id": {"uuid": zero_uuid},
            "stamp": {"sec": 0, "nanosec": 0},
        }
    });
    client
        .call_service(
            &service,
            Some(super::action_ops::CANCEL_GOAL_SERVICE_TYPE),
            args,
            Duration::from_secs(super::action_ops::CANCEL_WAIT_SECS),
        )
        .await
}

/// D-10: [`Ros2Session::stop_all`], resolving the bridge URL through
/// [`super::Ros2ConfigSource::Live`] (fresh config load, env override
/// consulted) — the form `Ros2Tool::on_session_end` and the `stop` action
/// both use, so neither has to duplicate URL resolution.
pub async fn stop_all_live(session: Arc<Ros2Session>) -> String {
    let cfg = super::Ros2ConfigSource::Live.load();
    let url = super::Ros2ConfigSource::Live.bridge_url(&cfg);
    session.stop_all(url.as_deref()).await
}

/// 49.8-18: appended to a stop's report when at least one goal's cancel was
/// refused, merely accepted (CANCELING, not terminal), malformed, or never
/// acknowledged — that goal is counted under `goal failure(s)` and the stop
/// is never recorded clean ([`Ros2Session::stop_all_as`]'s clean predicate is
/// `goals_failed == 0`), so the next stop (direct or a `session_end_hook_stop`)
/// re-arms and re-sends the cancel rather than coalescing onto the earlier
/// answer (review 49.8-18, VERIFICATION gap #2 / CR-01).
pub const GOAL_CANCEL_NOT_CONFIRMED_NOTE: &str = "goal cancel not confirmed";

/// 49.8-18: labels goals whose action server's cancel-all answered with no
/// goal entering CANCELING (`return_code` 0 or 3 with an empty
/// `goals_canceling`) — grounded in rcl_action's `rcl_action_process_cancel_request`,
/// which emits exactly that shape for an idle or all-terminal server. This is
/// reported distinctly, never as `cancelled`, but it is NOT counted as a
/// failure: an rclcpp server (Nav2) that declined every cancel instead answers
/// ERROR_REJECTED (a [`GoalCancelOutcome::Refused`] failure), and for an
/// rclpy server that silently drops declined goals a re-send cannot change
/// the answer — failing this case closed would leave every later stop in the
/// process permanently unclean once a goal's owning connection has closed
/// (see the design note on [`classify_cancel_all_response`]).
pub const GOAL_NOT_CANCELABLE_NOTE: &str = "not cancelable per the action server";

/// 49.8-18 (Task 2, review 49.8-18 MEDIUM): the guaranteed per-send
/// acknowledgement window. Writes on one connection serialize through the
/// client's writer mutex (`bridge_client.rs`'s `send_op`), so a later send in
/// a concurrent batch can start late; this bounds that. `call_service`
/// starts its own `CANCEL_WAIT_SECS` timer only after its send completes,
/// and the same-connection confirmation helper waits `CANCEL_WAIT_SECS`
/// after its own send, so any send that completes within this window still
/// gets the full acknowledgement window before the phase deadline below.
pub const GOAL_CANCEL_SEND_WINDOW_MS: u64 = 500;

/// 49.8-18 (Task 2, VERIFICATION gap #1): the single bound on a stop's WHOLE
/// goal-cancel phase, independent of how many goals are open — every
/// same-connection cancel and one cancel-all per action server run
/// concurrently from the phase start, so the phase costs this constant once,
/// never `CANCEL_WAIT_SECS` times the goal (or action) count. A cancel whose
/// send does not complete within [`GOAL_CANCEL_SEND_WINDOW_MS`] is cut by
/// this deadline and reported as a failure — never silently dropped.
pub const GOAL_CANCEL_PHASE_MS: u64 =
    GOAL_CANCEL_SEND_WINDOW_MS + super::action_ops::CANCEL_WAIT_SECS * 1000;

/// 49.8-18: what a same-connection or cancel-all goal-cancel attempt actually
/// proved, distinguishing "the goal is confirmed terminal" from "the action
/// server merely accepted the cancel request" (ERROR_NONE means a goal
/// entered CANCELING, not that it reached CANCELED — review 49.8-18 HIGH,
/// grounded in `action_msgs/srv/CancelGoal.srv` and rcl_action's cancel-all
/// processing). `Refused`/`Unconfirmed` carry a `&'static str` reason
/// deliberately — a peer-controlled `ServiceResponse.values` or status-frame
/// `msg` can never be interpolated into a stop report or a `cancel_goal`
/// reply (T-49.8-63), so building a reason from that text is a type error,
/// not a runtime check. `TerminalConfirmed`/`FinishedFirst` are produced
/// ONLY by [`confirm_same_connection_cancel`] (Task 2) — `NotCancelable`,
/// `Accepted`, `Refused` and `Unconfirmed` are the four cancel-all-shaped
/// outcomes [`classify_cancel_all_response`] produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GoalCancelOutcome {
    /// The goal's own `action_result` reported status 5 (CANCELED) — the
    /// only same-connection outcome that counts as a confirmed cancel.
    TerminalConfirmed,
    /// The goal's own `action_result` reported status 4 (SUCCEEDED) or 6
    /// (ABORTED) — it finished before the cancel landed. Counted the same
    /// as a confirmed cancel: the goal is not running, which is what D-10
    /// requires.
    FinishedFirst,
    /// The action server's cancel-all answered with no goal entering
    /// CANCELING — reported via [`GOAL_NOT_CANCELABLE_NOTE`], not a failure.
    NotCancelable,
    /// The action server accepted the cancel-all: the listed goals are
    /// CANCELING, not terminal. Counted as a goal failure (not confirmed) —
    /// never as cancelled — because the goal's terminal state is not
    /// observable on this (fallback) connection.
    Accepted,
    /// The bridge or action server refused the cancel outright (`result`
    /// not `true`, or `return_code` 1 / ERROR_REJECTED).
    Refused(&'static str),
    /// WR-03 (review 49.8, carried forward): the same-connection cancel
    /// could not be confirmed because the bridge itself does not support
    /// `cancel_action_goal` at all (rosbridge_suite < 2.0.0, D-06) — a goal
    /// whose `send_goal` call raced a concurrent stop's snapshot (tracked
    /// just before the immediate-rejection window, still present in that
    /// snapshot) rather than one already untracked by `send_goal`'s own
    /// `ACTION_OPS_UNSUPPORTED` branch. An action-ops-unsupported bridge
    /// can, by construction, never confirm ANY cancel on ANY goal — a
    /// re-send could never succeed either — so this is reported the same
    /// way [`GoalCancelOutcome::NotCancelable`] is: settled, never a stop
    /// failure. Produced ONLY by [`confirm_same_connection_cancel`] —
    /// [`classify_cancel_all_response`] never produces it (mirrors
    /// `TerminalConfirmed`/`FinishedFirst`).
    ActionOpsUnsupported,
    /// The response was missing, malformed, or named a return_code this
    /// classification does not recognize as any of the above — fails closed
    /// to a goal failure rather than a silent success. Also used for a
    /// same-connection cancel whose goal never reported a terminal status
    /// within the cancel phase (ignored, unanswered, or a present-but-
    /// non-terminal/missing status).
    Unconfirmed(&'static str),
}

/// 49.8-18 (review 49.8-18 HIGH; VERIFICATION gap #2, CR-01): classifies a
/// cancel-all [`ServiceResponse`] per `action_msgs/srv/CancelGoal.srv`'s four
/// `return_code` values and rcl_action's `rcl_action_process_cancel_request`
/// (humble) semantics — grounded in the real ROS 2 sources the robot runs,
/// not an assumed compatibility shape:
///
/// - ERROR_NONE (0) and ERROR_GOAL_TERMINATED (3) both mean "one or more
///   goals transitioned to CANCELING", not CANCELED — with a non-empty
///   `goals_canceling` this is [`GoalCancelOutcome::Accepted`] (not
///   terminal); with an empty `goals_canceling` (an idle or all-terminal
///   server) this is [`GoalCancelOutcome::NotCancelable`], distinct from
///   both Accepted and a failure.
/// - ERROR_REJECTED (1) means every cancelable goal's cancel callback
///   rejected (rclcpp) — [`GoalCancelOutcome::Refused`].
/// - ERROR_UNKNOWN_GOAL_ID (2) is never valid for a cancel-all (it only
///   applies to a specific goal id) — [`GoalCancelOutcome::Unconfirmed`].
/// - Any other integer, or a response whose `result` is not `true`, whose
///   `values` is not a JSON object, whose `return_code` is not an integer
///   (`Value::as_i64`), or whose `goals_canceling` is missing or not an
///   array, fails closed to [`GoalCancelOutcome::Unconfirmed`] or
///   [`GoalCancelOutcome::Refused`] — there is NO success branch for a
///   response missing an integer `return_code` and an array
///   `goals_canceling`; the old planned compatibility rule (treating a
///   missing return_code as success) is withdrawn (frontmatter prohibition
///   "Fail closed").
pub(crate) fn classify_cancel_all_response(resp: &ServiceResponse) -> GoalCancelOutcome {
    if !resp.result {
        // Covers rosbridge's own `status`-error path (bridge_client.rs's
        // `dispatch_inbound` already sets `result: false` there) and the
        // mock's default "service not found" for an unconfigured service.
        return GoalCancelOutcome::Refused("rosbridge refused the cancel-all service call");
    }
    let Some(values) = resp.values.as_object() else {
        return GoalCancelOutcome::Unconfirmed("malformed CancelGoal response");
    };
    let Some(return_code) = values.get("return_code").and_then(Value::as_i64) else {
        return GoalCancelOutcome::Unconfirmed("malformed CancelGoal response");
    };
    let Some(goals_canceling) = values.get("goals_canceling").and_then(Value::as_array) else {
        return GoalCancelOutcome::Unconfirmed("malformed CancelGoal response");
    };
    match return_code {
        // ERROR_NONE (0) / ERROR_GOAL_TERMINATED (3): `action_msgs/srv/CancelGoal.srv`
        // plus rcl_action's `rcl_action_process_cancel_request` (humble) — a
        // cancelable goal moving to CANCELING sets ERROR_NONE; a terminal
        // goal sets ERROR_GOAL_TERMINATED; when nothing was cancelable,
        // `goals_canceling` stays empty and `return_code` keeps its last
        // value (ERROR_NONE for a server that tracks no goal at all). Both
        // codes can therefore answer either shape — the goal LIST, not the
        // code alone, decides Accepted vs. NotCancelable.
        0 | 3 if !goals_canceling.is_empty() => GoalCancelOutcome::Accepted,
        0 | 3 => GoalCancelOutcome::NotCancelable,
        // ERROR_REJECTED (1): rclcpp's `ServerBase::execute_cancel_request_received`
        // sets this when every cancelable goal's user cancel callback
        // rejected. (rclpy's `ActionServer._execute_cancel_request` drops
        // rejected goals without ever setting this code — see the accepted
        // T-49.8-83 residual risk on `GOAL_NOT_CANCELABLE_NOTE`'s doc.)
        1 => GoalCancelOutcome::Refused("action server rejected the cancel (ERROR_REJECTED)"),
        // ERROR_UNKNOWN_GOAL_ID (2) is only ever produced for a specific goal
        // id — never valid for a cancel-all's all-zero request — and any
        // other integer is unrecognized. Both fail closed.
        _ => GoalCancelOutcome::Unconfirmed("unexpected CancelGoal return_code for a cancel-all"),
    }
}

/// 49.8-18 (Task 2, D-06/D-10, review 49.8-18 MEDIUM): waits for ONE
/// same-connection goal's cancel to be confirmed by the goal's OWN terminal
/// status — a socket write alone is never a confirmation. Two bounded
/// stages: the send must complete by `send_deadline` (the shared
/// [`GOAL_CANCEL_SEND_WINDOW_MS`] window every concurrent cancel in this
/// stop shares, since writes on one connection serialize through the
/// client's writer mutex), then the wait for a terminal
/// `action_result`/error is bounded by `CANCEL_WAIT_SECS` from the send —
/// never from the phase start, so a send that lands late in the window still
/// gets its full acknowledgement window. The caller ALSO wraps this call in
/// a `timeout_at(phase_deadline, ..)` backstop; this fn's own bound is
/// slightly tighter (`send_deadline` + `CANCEL_WAIT_SECS` <= `phase_deadline`
/// by construction) so the backstop should rarely fire in practice.
///
/// Never returns [`GoalCancelOutcome::NotCancelable`] or
/// [`GoalCancelOutcome::Accepted`] — those are cancel-all-shaped outcomes
/// only [`classify_cancel_all_response`] produces.
async fn confirm_same_connection_cancel(
    client: &Ros2BridgeClient,
    action: &str,
    goal_id: &str,
    send_deadline: tokio::time::Instant,
) -> GoalCancelOutcome {
    match tokio::time::timeout_at(send_deadline, client.cancel_action_goal(action, goal_id)).await {
        Err(_) => {
            return GoalCancelOutcome::Unconfirmed(
                "cancel could not be written within the cancel send window",
            );
        }
        Ok(Err(_)) => {
            return GoalCancelOutcome::Unconfirmed("cancel_action_goal could not be sent");
        }
        Ok(Ok(())) => {}
    }

    let Some(snapshot) = client
        .wait_goal_result(
            goal_id,
            Duration::from_secs(super::action_ops::CANCEL_WAIT_SECS),
        )
        .await
    else {
        return GoalCancelOutcome::Unconfirmed("goal unknown on this connection");
    };

    // Check `result` before `error` (bridge_client.rs's `action_result` arm
    // stores `result = Some(values)` unconditionally on any `action_result`
    // frame, so "a result is present" only means a frame arrived — the
    // status inside it, not its mere presence, decides confirmation).
    if snapshot.result.is_some() {
        return match snapshot.status {
            Some(5) => GoalCancelOutcome::TerminalConfirmed,
            Some(4) | Some(6) => GoalCancelOutcome::FinishedFirst,
            _ => GoalCancelOutcome::Unconfirmed(
                "goal result carried a missing or non-terminal status",
            ),
        };
    }
    if let Some(err) = &snapshot.error {
        return if err == super::bridge_client::ACTION_OPS_UNSUPPORTED {
            // WR-03: settled, not a failure — see the variant's doc comment.
            GoalCancelOutcome::ActionOpsUnsupported
        } else {
            // Never interpolate `err`'s text (T-49.8-63) — it is set from
            // rosbridge's peer-controlled `status` error `msg`.
            GoalCancelOutcome::Unconfirmed("bridge reported an error for the goal")
        };
    }
    GoalCancelOutcome::Unconfirmed("no terminal status within the cancel phase")
}

/// 49.8-18 (Task 2, VERIFICATION gap #1): one cancel-all for `action`,
/// bounded by `phase_deadline` (not `CANCEL_WAIT_SECS` alone — the fallback
/// goes out concurrently with every other cancel in this stop, so a slow
/// send must still respect the shared phase bound). A timeout or call error
/// fails closed to [`GoalCancelOutcome::Unconfirmed`]; a real response is
/// classified exactly as the direct (non-concurrent) fallback path always
/// was, via [`classify_cancel_all_response`].
async fn confirm_cancel_all(
    client: &Ros2BridgeClient,
    action: &str,
    phase_deadline: tokio::time::Instant,
) -> GoalCancelOutcome {
    match tokio::time::timeout_at(phase_deadline, cancel_all_goals_fallback(client, action)).await {
        Err(_) => {
            GoalCancelOutcome::Unconfirmed("cancel-all got no response within the cancel phase")
        }
        Ok(Err(_)) => GoalCancelOutcome::Unconfirmed("cancel-all service call failed"),
        Ok(Ok(resp)) => classify_cancel_all_response(&resp),
    }
}

/// D-10: time-bounded, so a slow/unreachable bridge can never wedge whatever
/// surface (gateway/web/kanban shutdown, 49.8-08) awaits this. Reads the
/// registry's [`Ros2Session`] under a READ lock, drops the guard, then awaits
/// [`stop_all_live`] — never holds the registry lock across the network call.
///
/// 49.8-19 (VERIFICATION gap #1, review 49.8-19): the ONE bound every
/// surface's [`end_session_stop`] applies. Widened 4s (49.8-11) -> 6s
/// (49.8-17, WR-01, which omitted the goal-cancel term and hardcoded the
/// motion-topic count to 1) -> 12s here, sized for [`MAX_MOTION_TOPICS`] so
/// no live motion-topic count ever has to be read (and raced by concurrent
/// `admit_motion_topic` calls) at session end. Full arithmetic, every term
/// named by its constant: [`PREEMPT_WAIT_MS`] (1500) +
/// [`RECONNECT_STOP_TIMEOUT_SECS`]*1000 (3000) + `MAX_MOTION_TOPICS *
/// STOP_SEQUENCE_MS` (8 * 500 = 4000) + [`GOAL_CANCEL_PHASE_MS`] (2500,
/// 49.8-18 — one constant regardless of goal count) +
/// [`SESSION_END_HEADROOM_MS`] (1000) = 12000ms.
/// `ros2_stop_ordering_wait_budget_fits_session_end_bound` sums every term
/// independently from the primitive constants, for every motion-topic count
/// in `0..=MAX_MOTION_TOPICS`, so neither a constant nor a count can drift
/// this bound back into an overrun.
///
/// What is proven, and what is not (review 49.8-19): the gateway's graceful
/// shutdown (`runner.rs`) awaits [`end_session_stop`] as its first step,
/// before MCP teardown and the task drain, and `end_session_stop` wraps all
/// of its async work in one `tokio::time::timeout` (its only synchronous
/// step is `Ros2ConfigSource::Live.load()`). The budget test additionally
/// pins `ROS2_SESSION_END_TIMEOUT_SECS*1000 + 2000 <= min(TimeoutStopSec)`
/// across both deploy units — that proves systemd cannot SIGKILL the
/// gateway during this awaited stop. It does NOT prove that the MCP
/// teardown/drain which follow fit the remaining margin, that stop work
/// still running after a timed-out `end_session_stop` finishes before the
/// process exits, or anything about `iron_hermes_ui`'s graceful shutdown,
/// which awaits no session-end stop at all.
pub const ROS2_SESSION_END_TIMEOUT_SECS: u64 = 12;

pub async fn end_session_stop(
    registry: &tokio::sync::RwLock<crate::registry::ToolRegistry>,
) -> String {
    let session = registry.read().await.ros2_session();
    match tokio::time::timeout(
        Duration::from_secs(ROS2_SESSION_END_TIMEOUT_SECS),
        stop_all_live(session),
    )
    .await
    {
        Ok(report) => report,
        Err(_) => SESSION_END_TIMEOUT_REPORT.to_string(),
    }
}

/// D-10/D-16 (49.8-11 gap closure): the seam that lets a WHOLE-SESSION
/// future always end in the awaited robot stop — on `Ok`, `Err` or panic.
///
/// (1) `turn` MUST be the WHOLE session: the CLI one-shot `run_single`
/// (which is also the kanban worker's `chat -q` process) or an equivalent
/// single-dispatch surface. It must NEVER wrap one turn of a multi-turn
/// session (gateway, web, voice2voice, ACP, the classic REPL `run_chat`, or
/// the TUI) — 49.8-08's "Session-end semantics per surface" decision lets
/// action goals continue across turns on those surfaces until a real
/// session end, and wrapping a single turn here would cancel every
/// in-flight Nav2 goal at every turn boundary instead of only at the real
/// end.
/// (2) A `?` inside `turn` returns from `turn`'s own `async` block, never
/// from this function — the caller's early-return escape hatch stays fully
/// contained inside the future it wrote.
/// (3) The stop runs whether `turn` resolved `Ok`, resolved `Err`, or
/// panicked.
pub async fn with_session_end_stop<Fut>(
    registry: &tokio::sync::RwLock<crate::registry::ToolRegistry>,
    turn: Fut,
) -> (Fut::Output, String)
where
    Fut: std::future::Future,
{
    match futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(turn)).await {
        Ok(output) => {
            let report = end_session_stop(registry).await;
            (output, report)
        }
        Err(payload) => {
            // Await the stop BEFORE resuming the panic — the whole point of
            // this seam is that the robot gets stopped even when the turn
            // never returns normally. `end_session_stop` is already bounded
            // by `ROS2_SESSION_END_TIMEOUT_SECS`, so no extra timeout here.
            let report = end_session_stop(registry).await;
            tracing::warn!(
                report = %report,
                "turn panicked; ros2 session-end stop ran before resuming the panic"
            );
            std::panic::resume_unwind(payload)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn goal_info() -> Value {
        let uuid: Vec<u8> = vec![1u8; 16];
        json!({
            "goal_id": {"uuid": uuid},
            "stamp": {"sec": 1, "nanosec": 0},
        })
    }

    #[test]
    fn ros2_classify_cancel_all_refusals() {
        let refused_result = ServiceResponse {
            result: false,
            values: json!("service not found"),
        };
        assert_eq!(
            classify_cancel_all_response(&refused_result),
            GoalCancelOutcome::Refused("rosbridge refused the cancel-all service call"),
            "result: false must classify as Refused"
        );

        let rejected = ServiceResponse {
            result: true,
            values: json!({"return_code": 1, "goals_canceling": []}),
        };
        match classify_cancel_all_response(&rejected) {
            GoalCancelOutcome::Refused(reason) => {
                assert!(
                    reason.contains("ERROR_REJECTED"),
                    "ERROR_REJECTED (1) reason must name ERROR_REJECTED; got: {reason}"
                );
            }
            other => panic!("return_code 1 must classify as Refused; got: {other:?}"),
        }
    }

    #[test]
    fn ros2_classify_cancel_all_fails_closed_on_malformed() {
        let malformed_inputs = [
            json!("ok"),
            json!({}),
            json!({"goals_canceling": []}),
            json!({"return_code": null, "goals_canceling": []}),
            json!({"return_code": "0", "goals_canceling": []}),
            json!({"return_code": 0.5, "goals_canceling": []}),
            json!({"return_code": 0}),
            json!({"return_code": 0, "goals_canceling": {}}),
            json!({"return_code": 2, "goals_canceling": []}),
            json!({"return_code": 42, "goals_canceling": []}),
        ];
        for values in malformed_inputs {
            let resp = ServiceResponse {
                result: true,
                values: values.clone(),
            };
            assert!(
                matches!(
                    classify_cancel_all_response(&resp),
                    GoalCancelOutcome::Unconfirmed(_)
                ),
                "malformed cancel-all response must fail closed to Unconfirmed; input: {values:?}"
            );
        }
    }

    #[test]
    fn ros2_classify_cancel_all_acceptance_is_not_terminal() {
        let info = goal_info();

        let accepted_0 = ServiceResponse {
            result: true,
            values: json!({"return_code": 0, "goals_canceling": [info.clone()]}),
        };
        assert_eq!(
            classify_cancel_all_response(&accepted_0),
            GoalCancelOutcome::Accepted
        );

        let accepted_3 = ServiceResponse {
            result: true,
            values: json!({"return_code": 3, "goals_canceling": [info.clone()]}),
        };
        assert_eq!(
            classify_cancel_all_response(&accepted_3),
            GoalCancelOutcome::Accepted
        );

        let not_cancelable_0 = ServiceResponse {
            result: true,
            values: json!({"return_code": 0, "goals_canceling": []}),
        };
        assert_eq!(
            classify_cancel_all_response(&not_cancelable_0),
            GoalCancelOutcome::NotCancelable
        );

        let not_cancelable_3 = ServiceResponse {
            result: true,
            values: json!({"return_code": 3, "goals_canceling": []}),
        };
        assert_eq!(
            classify_cancel_all_response(&not_cancelable_3),
            GoalCancelOutcome::NotCancelable
        );
    }
}

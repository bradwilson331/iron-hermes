//! Timed motion with a guaranteed stop (Phase 49.8 D-09/D-10).
//!
//! [`run_timed_motion`] republishes an already-clamped Twist at a capped rate
//! for a capped duration, then ALWAYS finishes by publishing a zero Twist
//! [`crate::ros2::safety::STOP_REPEATS`] times — even when:
//! - the caller's own future is dropped (a registry timeout DROPS the tool
//!   future rather than cooperatively cancelling it, see
//!   `registry.rs::run_bounded`'s `tokio::time::timeout` wrapper — this is
//!   exactly why the republish/stop loop runs inside a detached
//!   [`tokio::spawn`], guarded by a [`CancellationToken`] rather than living
//!   directly in the awaited future);
//! - a new motion on the same (or any) topic preempts this one
//!   ([`crate::ros2::session::Ros2Session::begin_motion`] cancels the
//!   previous motion's token and waits for its finished signal before handing
//!   out a new one), or an explicit/session-end stop interrupts it
//!   ([`crate::ros2::session::Ros2Session::stop_all`] (49.8-15) waits for
//!   this same finished signal, bounded, before publishing its own stop);
//! - the bridge connection drops mid-motion ([`Ros2Session::emergency_stop`]
//!   reconnects once, bounded, and re-sends the stop on the new connection).
//!
//! G-49.8-2b: when [`MotionRequest::stop_followup`] is attached (the
//! ROSPider's gait-0 rest stop — see `rospider_tool.rs::rospider_stop_followup`),
//! the sequence is: the unchanged zero-Twist burst, one [`STOP_INTERVAL_MS`]
//! gap, then the follow-up burst, [`STOP_REPEATS`] times. The reconnect path
//! re-sends both bursts, in the same order, on the new connection. The
//! engine itself is robot-agnostic — it never builds a follow-up, only
//! publishes whichever one the caller attached.

use std::sync::Arc;

use serde_json::Value;
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;

use super::bridge_client::Ros2BridgeClient;
use super::safety::{STOP_INTERVAL_MS, STOP_REPEATS, zero_twist};
use super::session::{MotionStatus, Ros2Session};

/// A message a caller attaches to a timed motion, published strictly after
/// the zero-Twist burst — for robots whose driver treats a zero Twist as
/// "keep stepping in place" rather than a stop (the Hiwonder ROSPider,
/// Phase 49.8 Plan 13 / G-49.8-2b). The motion engine stays robot-agnostic:
/// it publishes whatever follow-up the caller attached and never builds one
/// itself. Only `rospider_tool.rs::rospider_stop_followup` knows about the
/// ROSPider.
#[derive(Debug, Clone, PartialEq)]
pub struct StopFollowup {
    pub topic: String,
    pub msg_type: String,
    pub msg: Value,
}

/// One timed-motion request. The caller (`ros2_tool.rs::do_motion_publish`)
/// has already clamped `twist` (via `safety::apply_field_limits` +
/// `safety::clamp_twist`), `duration` (via `safety::clamp_duration`), and
/// `hz` (`EffectiveCaps::publish_hz`) before constructing this — this module
/// re-clamps nothing, it only republishes and guarantees the stop.
#[derive(Debug, Clone)]
pub struct MotionRequest {
    pub topic: String,
    pub msg_type: String,
    pub twist: Value,
    pub duration: Duration,
    pub hz: f64,
    pub stamped: bool,
    /// G-49.8-2b: a follow-up published strictly after the zero-Twist burst
    /// (the ROSPider's gait-0 rest stop). `None` when the motion topic has
    /// no such follow-up configured.
    pub stop_followup: Option<StopFollowup>,
}

/// What happened during (and after) one timed motion.
#[derive(Debug, Clone, Default)]
pub struct MotionOutcome {
    /// Non-zero-velocity publishes actually sent before the stop sequence.
    pub published: usize,
    /// Wall-clock time from the first publish attempt to the end of the stop
    /// sequence (or the point the loop gave up on failure).
    pub elapsed: Duration,
    /// The republish loop ended via cancellation (preemption or a dropped
    /// caller future) rather than the duration elapsing naturally.
    pub interrupted: bool,
    /// A publish (or the primary-connection stop attempt) failed, indicating
    /// the bridge connection was lost.
    pub connection_lost: bool,
    /// The zero-Twist stop was confirmed sent — either on the original
    /// connection or after an [`Ros2Session::emergency_stop`] reconnect.
    pub stop_sent: bool,
    /// `Some(true)`/`Some(false)` when [`Ros2Session::emergency_stop`] was
    /// invoked (connection lost, or the primary stop attempt itself failed);
    /// `None` when the primary connection's own stop sequence succeeded and
    /// no reconnect was needed.
    pub reconnect_stop: Option<bool>,
    /// `Some(true)`/`Some(false)` when [`MotionRequest::stop_followup`] was
    /// attached and its burst was confirmed sent / failed; `None` when no
    /// follow-up was attached to this request.
    pub stop_followup_sent: Option<bool>,
    /// 49.8-19 (VERIFICATION gap #1, D-08/D-11): `Some(reply)` when
    /// [`Ros2Session::admit_motion_topic`] refused this request's topic (the
    /// process has already driven `MAX_MOTION_TOPICS` distinct motion
    /// topics) — nothing was connected, advertised or published, and every
    /// other field is at its default. `None` for every admitted motion.
    pub refused: Option<String>,
}

/// Cancels `token` when dropped. Held by [`run_timed_motion`]'s own stack
/// frame (not inside the spawned task) so that dropping the AWAITING future —
/// e.g. `registry.rs::run_bounded`'s `tokio::time::timeout` elapsing, which
/// drops `tool.execute(args)` rather than cooperatively cancelling it — still
/// signals the detached [`tokio::spawn`]'d loop to stop and send the zero
/// Twist. The spawned task itself is NOT aborted by this drop (only
/// `JoinHandle::abort()` would do that) — it keeps running to completion
/// independently of whether anything is still awaiting it, which is the whole
/// point: the stop must go out even when nobody is watching anymore.
struct MotionGuard {
    token: CancellationToken,
}

impl Drop for MotionGuard {
    fn drop(&mut self) {
        self.token.cancel();
    }
}

/// Runs one timed motion to completion and returns its outcome. See the
/// module doc for why the actual work happens inside a detached
/// `tokio::spawn` rather than directly in this function's own future.
pub async fn run_timed_motion(
    session: Arc<Ros2Session>,
    url: String,
    req: MotionRequest,
) -> MotionOutcome {
    // 49.8-19 (VERIFICATION gap #1, D-08/D-11): admit the topic against the
    // MAX_MOTION_TOPICS ceiling BEFORE begin_motion — this is begin_motion's
    // only production call site (every timed motion on every surface goes
    // through it), so a refused topic never connects, advertises or
    // publishes anything.
    if let Err(reply) = session.admit_motion_topic(&req.topic, &req.msg_type).await {
        return MotionOutcome {
            refused: Some(reply),
            ..Default::default()
        };
    }

    // G-49.8-2b (49.8-14): passes this motion's follow-up (if any) into the
    // session's sticky per-topic registry — `stop_all`'s replay (session end
    // on every surface, and the explicit `stop`/`rospider stop` paths) reads
    // it back after this motion has ended.
    let (id, token, finished_tx) = session
        .begin_motion(&req.topic, &req.msg_type, req.stop_followup.as_ref())
        .await;
    let _guard = MotionGuard {
        token: token.clone(),
    };

    let handle = tokio::spawn(motion_task(
        session.clone(),
        url,
        req,
        token,
        finished_tx,
        id,
    ));

    match handle.await {
        Ok(outcome) => outcome,
        Err(_join_err) => {
            // The spawned task panicked. Still release the active-motion
            // slot so the session doesn't wedge on a dead motion id forever.
            session.end_motion(id).await;
            MotionOutcome {
                interrupted: true,
                ..Default::default()
            }
        }
    }
}

/// The detached loop: republish `req.twist` at `req.hz` until `token` is
/// cancelled or `req.duration` elapses, then ALWAYS attempt the zero-Twist
/// stop — on the existing connection first, falling back to
/// [`Ros2Session::emergency_stop`] if that connection was lost or the stop
/// itself failed.
async fn motion_task(
    session: Arc<Ros2Session>,
    url: String,
    req: MotionRequest,
    token: CancellationToken,
    finished_tx: tokio::sync::watch::Sender<MotionStatus>,
    id: u64,
) -> MotionOutcome {
    let start = tokio::time::Instant::now();
    let mut outcome = MotionOutcome::default();

    let client = match session.client_for(&url).await {
        Ok(c) => Some(c),
        Err(_) => {
            outcome.connection_lost = true;
            None
        }
    };
    let client = match client {
        // D-09: no advertise settle here — the loop republishes at rate and
        // the stop sequence republishes 3x, so a first message lost to a
        // DDS-discovery race is covered by the next one. See
        // `Ros2BridgeClient::advertise_no_settle`'s doc comment.
        Some(c)
            if c.advertise_no_settle(&req.topic, &req.msg_type)
                .await
                .is_ok() =>
        {
            Some(c)
        }
        _ => {
            outcome.connection_lost = true;
            None
        }
    };

    if let Some(client) = &client {
        // G-49.8-2b: pre-advertise the follow-up topic (the ROSPider's
        // gait-0 rest stop) now, before the republish loop even starts, so
        // DDS discovery has the whole motion's duration to settle and the
        // one-shot follow-up burst at the end isn't lost to a just-created
        // publisher. A failure here is non-fatal — the stop path below
        // re-advertises (`advertise_no_settle` is idempotent per
        // connection).
        if let Some(followup) = &req.stop_followup
            && let Err(e) = client
                .advertise_no_settle(&followup.topic, &followup.msg_type)
                .await
        {
            tracing::warn!(
                topic = %followup.topic,
                error = %e,
                "motion: pre-advertise of stop follow-up topic failed (non-fatal, stop path re-advertises)"
            );
        }

        let interval = Duration::from_secs_f64((1.0 / req.hz.max(0.001)).max(0.001));
        let deadline = tokio::time::Instant::now() + req.duration;

        loop {
            if token.is_cancelled() {
                outcome.interrupted = true;
                break;
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                break;
            }
            match client.publish(&req.topic, req.twist.clone()).await {
                Ok(()) => outcome.published += 1,
                Err(_) => {
                    outcome.connection_lost = true;
                    break;
                }
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let sleep_for = interval.min(remaining);
            if sleep_for.is_zero() {
                break;
            }
            tokio::select! {
                _ = tokio::time::sleep(sleep_for) => {}
                _ = token.cancelled() => {
                    outcome.interrupted = true;
                    break;
                }
            }
        }
    }

    outcome.elapsed = start.elapsed();

    // ALWAYS attempt the stop — on the existing connection first, when we
    // still have one that hasn't been marked lost. G-49.8-2b: when a
    // follow-up is attached (the ROSPider's gait-0 rest stop), it runs
    // strictly after the zero-Twist burst is confirmed sent, on whichever
    // connection actually sent that burst — never before it, and never on a
    // connection the burst itself failed on.
    let mut stop_ok = false;
    // Starts `false`, not `true`: this must default to "not sent" for every
    // path that never actually attempts the follow-up burst (e.g. the
    // primary connection is dead AND the reconnect itself fails) — a
    // default of `true` would misreport an unsent follow-up as sent. When
    // `req.stop_followup` is `None` this value is discarded below (`.map`
    // never runs), so its value is irrelevant in that case.
    let mut followup_ok = false;
    if !outcome.connection_lost
        && let Some(client) = &client
    {
        stop_ok = send_stop_repeats(client, &req).await;
        if stop_ok {
            if let Some(followup) = &req.stop_followup {
                followup_ok = publish_followup_burst(client, followup).await;
                if !followup_ok {
                    // The follow-up publish failed: the socket is gone.
                    // Treat the whole stop sequence as failed so the
                    // reconnect path below re-sends BOTH bursts, in order,
                    // on the new connection.
                    stop_ok = false;
                    outcome.connection_lost = true;
                }
            }
        } else {
            outcome.connection_lost = true;
        }
    }

    if !stop_ok {
        let reconnect_ok = session
            .emergency_stop(&url, &req.topic, &req.msg_type)
            .await;
        outcome.reconnect_stop = Some(reconnect_ok);
        stop_ok = reconnect_ok;
        if stop_ok && let Some(followup) = &req.stop_followup {
            followup_ok = match session.current_client().await {
                Some(reconnect_client) => publish_followup_burst(&reconnect_client, followup).await,
                None => false,
            };
        }
    }
    outcome.stop_sent = stop_ok;
    outcome.stop_followup_sent = req.stop_followup.as_ref().map(|_| followup_ok);

    // 49.8-15 (CR-01): `followup_confirmed` is true ONLY when a follow-up was
    // attached AND both its zero-Twist burst AND its follow-up burst were
    // confirmed sent — the exact condition under which `stop_all`'s interrupt
    // wait may skip its own redundant zero-Twist burst for this topic.
    let _ = finished_tx.send(MotionStatus::Finished {
        followup_confirmed: req.stop_followup.is_some() && stop_ok && followup_ok,
    });
    session.end_motion(id).await;

    outcome
}

async fn send_stop_repeats(client: &Ros2BridgeClient, req: &MotionRequest) -> bool {
    for i in 0..STOP_REPEATS {
        if client
            .publish(&req.topic, zero_twist(req.stamped))
            .await
            .is_err()
        {
            return false;
        }
        if i + 1 < STOP_REPEATS {
            tokio::time::sleep(Duration::from_millis(STOP_INTERVAL_MS)).await;
        }
    }
    true
}

/// G-49.8-2b: publishes `followup.msg` on `followup.topic`
/// [`STOP_REPEATS`] times, [`STOP_INTERVAL_MS`] apart, on `client` —
/// strictly after the caller has already confirmed the zero-Twist burst
/// sent on this same client. Advertises the follow-up topic first
/// (idempotent per connection — a no-op if the motion's own pre-advertise
/// already covered it, and a fresh advertise on a reconnected client
/// otherwise). Sleeps [`STOP_INTERVAL_MS`] after that advertise before the
/// first publish: DDS does not guarantee delivery order across two
/// different topics, so this gap — plus the redundant repeats — makes sure
/// the last message the robot processes is the follow-up, not a straggling
/// zero Twist that would restart the motion. Returns `false` on the first
/// advertise or publish error (the connection is gone).
pub(crate) async fn publish_followup_burst(
    client: &Ros2BridgeClient,
    followup: &StopFollowup,
) -> bool {
    if client
        .advertise_no_settle(&followup.topic, &followup.msg_type)
        .await
        .is_err()
    {
        return false;
    }
    tokio::time::sleep(Duration::from_millis(STOP_INTERVAL_MS)).await;
    for i in 0..STOP_REPEATS {
        if client
            .publish(&followup.topic, followup.msg.clone())
            .await
            .is_err()
        {
            return false;
        }
        if i + 1 < STOP_REPEATS {
            tokio::time::sleep(Duration::from_millis(STOP_INTERVAL_MS)).await;
        }
    }
    true
}

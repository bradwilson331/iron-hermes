//! Phase 49.8 Plan 15 (G-49.8-2b, CR-01): deterministic proofs that every
//! stop path which interrupts an in-flight motion waits for that motion's
//! own zero-Twist-then-gait-0 stop sequence to finish before publishing
//! anything of its own — and never suppresses the stop, even when the
//! interrupted motion is stuck or its finish signal is dropped.
//!
//! Unlike `ros2_rospider_rest_stop.rs`'s pre-49.8-15
//! `ros2_rest_stop_ends_rospider_stop_mid_walk` (which only polls), the
//! tests here assert over the FINAL, complete frame sequence recorded by the
//! mock — deterministic because `Ros2BridgeClient`'s writer is an async
//! mutex over one WebSocket sink, so per-connection write order equals call
//! order, and every test either awaits every sender to completion or drives
//! a fully controllable fake motion instead of racing a real timer.

#[path = "support/mock_rosbridge.rs"]
mod mock_rosbridge;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ironhermes_core::Config;
use ironhermes_core::config::Ros2AllowEntry;
use ironhermes_tools::browser_vision::VisionClientHandle;
use ironhermes_tools::registry::ToolRegistry;
use ironhermes_tools::ros2::Ros2BridgeClient;
use ironhermes_tools::ros2::Ros2ConfigSource;
use ironhermes_tools::ros2::action_ops::CANCEL_WAIT_SECS;
use ironhermes_tools::ros2::motion::StopFollowup;
use ironhermes_tools::ros2::rospider_tool::{ROSPIDER_TRAVELING_TYPE, rospider_rest_stop_msg};
use ironhermes_tools::ros2::safety::{STOP_INTERVAL_MS, STOP_REPEATS, zero_twist};
use ironhermes_tools::ros2::session::{
    GOAL_CANCEL_PHASE_MS, GOAL_CANCEL_SEND_WINDOW_MS, GOAL_NOT_CANCELABLE_NOTE, MAX_MOTION_TOPICS,
    MotionStatus, PREEMPT_WAIT_MS, RECONNECT_STOP_TIMEOUT_SECS, ROS2_SESSION_END_TIMEOUT_SECS,
    Ros2Session, SESSION_END_HEADROOM_MS, STOP_COALESCED_NOTE, STOP_SEQUENCE_MS,
};
use ironhermes_tools::ros2::{end_session_stop, with_session_end_stop};
use mock_rosbridge::{MockConfig, MockRosbridge};
use serde_json::{Value, json};
use std::collections::HashMap;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

fn fixed_config(bridge_url: Option<&str>) -> Config {
    let mut cfg = Config::default();
    cfg.ros2.bridge_url = bridge_url.map(|s| s.to_string());
    cfg
}

fn allow_entry(name: &str, msg_type: &str) -> Ros2AllowEntry {
    Ros2AllowEntry {
        name: name.to_string(),
        msg_type: msg_type.to_string(),
        limits: std::collections::BTreeMap::new(),
    }
}

fn registry_with_rospider(
    cfg: Config,
    vision: Option<Arc<dyn VisionClientHandle>>,
) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register_ros2_tools(Ros2ConfigSource::Fixed(Arc::new(cfg)), vision);
    registry
}

/// `advertise`/`publish` are fire-and-forget rosbridge ops — poll with a
/// short deadline instead of asserting immediately after dispatch (same race
/// `49.8-01-SUMMARY.md` through `49.8-14-SUMMARY.md` documented).
async fn wait_until(mut cond: impl FnMut() -> bool, deadline: Duration) {
    let start = std::time::Instant::now();
    loop {
        if cond() || start.elapsed() > deadline {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn is_zero_twist_msg(msg: &Value) -> bool {
    for group in ["linear", "angular"] {
        for axis in ["x", "y", "z"] {
            let v = msg
                .get(group)
                .and_then(|g| g.get(axis))
                .and_then(Value::as_f64)
                .unwrap_or(f64::NAN);
            if v != 0.0 {
                return false;
            }
        }
    }
    true
}

/// One publish frame on either the cmd_vel or the traveling topic, classified
/// by content/topic — the vocabulary every assertion in this file is stated
/// over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    NonZeroTwist,
    ZeroTwist,
    RestStop,
}

/// Maps every `publish` frame recorded on `cmd_vel` or `traveling`, in mock
/// arrival order, to a [`FrameKind`]. A `cmd_vel` frame is `NonZeroTwist` or
/// `ZeroTwist` by content; a `traveling` frame is always `RestStop` (the only
/// message ever sent on it in these tests).
fn stop_frame_kinds(mock: &MockRosbridge, cmd_vel: &str, traveling: &str) -> Vec<FrameKind> {
    mock.frames_with_op("publish")
        .into_iter()
        .filter_map(|f| {
            let topic = f.value.get("topic").and_then(Value::as_str)?;
            if topic == traveling {
                Some(FrameKind::RestStop)
            } else if topic == cmd_vel {
                let msg = f.value.get("msg").cloned().unwrap_or(Value::Null);
                Some(if is_zero_twist_msg(&msg) {
                    FrameKind::ZeroTwist
                } else {
                    FrameKind::NonZeroTwist
                })
            } else {
                None
            }
        })
        .collect()
}

/// Behavior variants for the controllable fake motion the session-level
/// tests drive directly against `Ros2Session::stop_all` (bypassing
/// `motion.rs::run_timed_motion` entirely, so the test controls exactly what
/// the "interrupted motion" does and when).
enum FakeMotionKind {
    /// Sleeps `slow_ms` after cancellation (a deliberately slow stop),
    /// publishes 3 zero Twists 100ms apart, waits 100ms, publishes 3 rest
    /// stops 100ms apart, then signals
    /// `Finished { followup_confirmed: true }`.
    Confirmed { slow_ms: u64 },
    /// Sleeps `slow_ms` after cancellation, publishes only 3 zero Twists (a
    /// simulated failed follow-up), then signals
    /// `Finished { followup_confirmed: false }`.
    Unconfirmed { slow_ms: u64 },
    /// Never sends a finish signal and publishes nothing after cancellation
    /// — holds `finished_tx` alive (awaiting `release`) so the channel is
    /// neither satisfied nor dropped during the bounded wait.
    Stuck {
        release: tokio::sync::oneshot::Receiver<()>,
    },
    /// Drops `finished_tx` immediately after cancellation without sending
    /// anything — simulates a panicked motion task.
    DroppedSignal,
}

/// A fake motion: advertises both topics, publishes one non-zero Twist,
/// awaits cancellation, then behaves per `kind`. Never uses
/// `motion.rs::motion_task` — this IS the "controllable fake motion" the
/// plan calls for, driving `Ros2Session::stop_all`'s interrupt-wait directly.
///
/// 49.8-17 (WR-02): `cancelled` (when `Some`) is flipped immediately after
/// the cancellation await returns — for the `Confirmed`/`Unconfirmed` arms.
/// That is the deterministic "the stop has taken the active motion and is
/// mid-interrupt-wait" window-entry signal the WR-02 race test waits on. The
/// existing call sites pass `None` and are behaviorally unchanged.
#[allow(clippy::too_many_arguments)]
async fn run_fake_motion(
    session: Arc<Ros2Session>,
    client: Arc<Ros2BridgeClient>,
    cmd_vel: String,
    cmd_vel_type: String,
    traveling: String,
    traveling_type: String,
    token: CancellationToken,
    finished_tx: tokio::sync::watch::Sender<MotionStatus>,
    id: u64,
    done: Arc<AtomicBool>,
    cancelled: Option<Arc<AtomicBool>>,
    kind: FakeMotionKind,
) {
    let _ = client.advertise_no_settle(&cmd_vel, &cmd_vel_type).await;
    let _ = client
        .advertise_no_settle(&traveling, &traveling_type)
        .await;

    let non_zero = json!({
        "linear": {"x": 0.2, "y": 0.0, "z": 0.0},
        "angular": {"x": 0.0, "y": 0.0, "z": 0.0},
    });
    let _ = client.publish(&cmd_vel, non_zero).await;

    token.cancelled().await;

    // 49.8-17 (WR-02): at this point the stopping session has taken the
    // active motion out (active_motion is None) — this is the deterministic
    // interrupt-wait-window entry signal.
    if let Some(c) = &cancelled {
        c.store(true, Ordering::SeqCst);
    }

    match kind {
        FakeMotionKind::Confirmed { slow_ms } => {
            tokio::time::sleep(Duration::from_millis(slow_ms)).await;
            for i in 0..STOP_REPEATS {
                let _ = client.publish(&cmd_vel, zero_twist(false)).await;
                if i + 1 < STOP_REPEATS {
                    tokio::time::sleep(Duration::from_millis(STOP_INTERVAL_MS)).await;
                }
            }
            tokio::time::sleep(Duration::from_millis(STOP_INTERVAL_MS)).await;
            let rest_msg = rospider_rest_stop_msg();
            for i in 0..STOP_REPEATS {
                let _ = client.publish(&traveling, rest_msg.clone()).await;
                if i + 1 < STOP_REPEATS {
                    tokio::time::sleep(Duration::from_millis(STOP_INTERVAL_MS)).await;
                }
            }
            done.store(true, Ordering::SeqCst);
            let _ = finished_tx.send(MotionStatus::Finished {
                followup_confirmed: true,
            });
            session.end_motion(id).await;
        }
        FakeMotionKind::Unconfirmed { slow_ms } => {
            tokio::time::sleep(Duration::from_millis(slow_ms)).await;
            for i in 0..STOP_REPEATS {
                let _ = client.publish(&cmd_vel, zero_twist(false)).await;
                if i + 1 < STOP_REPEATS {
                    tokio::time::sleep(Duration::from_millis(STOP_INTERVAL_MS)).await;
                }
            }
            done.store(true, Ordering::SeqCst);
            let _ = finished_tx.send(MotionStatus::Finished {
                followup_confirmed: false,
            });
            session.end_motion(id).await;
        }
        FakeMotionKind::Stuck { release } => {
            let _ = release.await;
            session.end_motion(id).await;
        }
        FakeMotionKind::DroppedSignal => {
            drop(finished_tx);
        }
    }
}

// ---------------------------------------------------------------------------
// 49.8-15: stop_all awaits the motion it interrupts (G-49.8-2b, CR-01)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_stop_ordering_rospider_stop_mid_walk_sends_no_zero_twist_after_rest_stop() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url));
    let registry = Arc::new(registry_with_rospider(cfg, None));

    let cmd_vel = "/controller/cmd_vel";
    let traveling = "/controller/traveling";

    let registry_walk = registry.clone();
    let walk_handle = tokio::spawn(async move {
        registry_walk
            .dispatch(
                "rospider",
                json!({"action": "walk", "direction": "forward", "duration_secs": 5.0}),
            )
            .await
    });

    wait_until(
        || {
            mock.published_on(cmd_vel)
                .iter()
                .filter(|m| !is_zero_twist_msg(m))
                .count()
                >= 2
        },
        Duration::from_secs(3),
    )
    .await;
    assert!(
        mock.published_on(cmd_vel)
            .iter()
            .filter(|m| !is_zero_twist_msg(m))
            .count()
            >= 2,
        "expected at least 2 non-zero Twist publishes before stopping — the walk never started"
    );

    let stop_result = registry
        .dispatch("rospider", json!({"action": "stop"}))
        .await
        .expect("stop dispatch must not error");

    let walk_output = tokio::time::timeout(Duration::from_secs(3), walk_handle)
        .await
        .expect("walk must finish within 3s")
        .expect("walk task must not panic");
    walk_output.expect("walk dispatch must not error");

    wait_until(
        || mock.published_on(traveling).len() >= 9,
        Duration::from_secs(3),
    )
    .await;

    let kinds = stop_frame_kinds(&mock, cmd_vel, traveling);

    let first_rest_idx = kinds
        .iter()
        .position(|k| *k == FrameKind::RestStop)
        .expect("expected at least one RestStop frame");
    assert!(
        kinds[first_rest_idx..]
            .iter()
            .all(|k| *k == FrameKind::RestStop),
        "no cmd_vel frame of any kind may follow the first RestStop; got {kinds:?}"
    );

    let last_non_zero_idx = kinds
        .iter()
        .rposition(|k| *k == FrameKind::NonZeroTwist)
        .expect("expected at least one NonZeroTwist frame");
    let zero_twists_after = kinds[last_non_zero_idx + 1..]
        .iter()
        .filter(|k| **k == FrameKind::ZeroTwist)
        .count();
    assert_eq!(
        zero_twists_after, 3,
        "expected exactly 3 ZeroTwist frames after the last NonZeroTwist; got {kinds:?}"
    );

    let rest_stop_count = kinds.iter().filter(|k| **k == FrameKind::RestStop).count();
    assert_eq!(
        rest_stop_count, 9,
        "expected exactly 9 RestStop frames; got {kinds:?}"
    );

    assert_eq!(
        kinds.last(),
        Some(&FrameKind::RestStop),
        "expected the last frame to be a RestStop; got {kinds:?}"
    );

    assert!(
        stop_result.contains("gait-0 rest stop sent on 1/1 topic(s)"),
        "got: {stop_result}"
    );
    assert!(
        stop_result.contains("; gait-0 rest stop sent on /controller/traveling"),
        "got: {stop_result}"
    );
    assert!(
        !stop_result.contains("did not finish its own stop"),
        "got: {stop_result}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_stop_ordering_stop_all_awaits_interrupted_motion() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let session = Ros2Session::new();
    let client = session
        .client_for(&mock.url)
        .await
        .expect("client_for must succeed");

    let cmd_vel = "/controller/cmd_vel".to_string();
    let cmd_vel_type = "geometry_msgs/msg/Twist".to_string();
    let traveling = "/controller/traveling".to_string();
    let traveling_type = ROSPIDER_TRAVELING_TYPE.to_string();
    let followup = StopFollowup {
        topic: traveling.clone(),
        msg_type: traveling_type.clone(),
        msg: rospider_rest_stop_msg(),
    };

    let (id, token, finished_tx) = session
        .begin_motion(&cmd_vel, &cmd_vel_type, Some(&followup))
        .await;
    let done = Arc::new(AtomicBool::new(false));

    tokio::spawn(run_fake_motion(
        session.clone(),
        client.clone(),
        cmd_vel.clone(),
        cmd_vel_type.clone(),
        traveling.clone(),
        traveling_type.clone(),
        token,
        finished_tx,
        id,
        done.clone(),
        None,
        FakeMotionKind::Confirmed { slow_ms: 400 },
    ));

    wait_until(
        || !mock.published_on(&cmd_vel).is_empty(),
        Duration::from_secs(2),
    )
    .await;
    assert!(
        !mock.published_on(&cmd_vel).is_empty(),
        "expected the fake motion's NonZeroTwist to be recorded"
    );

    let report = session.stop_all(Some(&mock.url)).await;

    assert!(
        done.load(Ordering::SeqCst),
        "stop_all must not return before the interrupted motion finished"
    );

    wait_until(
        || mock.published_on(&traveling).len() >= 6,
        Duration::from_secs(3),
    )
    .await;

    let kinds = stop_frame_kinds(&mock, &cmd_vel, &traveling);
    let expected = vec![
        FrameKind::NonZeroTwist,
        FrameKind::ZeroTwist,
        FrameKind::ZeroTwist,
        FrameKind::ZeroTwist,
        FrameKind::RestStop,
        FrameKind::RestStop,
        FrameKind::RestStop,
        FrameKind::RestStop,
        FrameKind::RestStop,
        FrameKind::RestStop,
    ];
    assert_eq!(kinds, expected, "unexpected frame sequence: {kinds:?}");

    assert!(report.contains("stopped 1/1 topic(s)"), "got: {report}");
    assert!(
        report.contains("gait-0 rest stop sent on 1/1 topic(s)"),
        "got: {report}"
    );
    assert!(
        !report.contains("did not finish its own stop"),
        "got: {report}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_stop_ordering_unconfirmed_motion_gets_full_stop_after_it() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let session = Ros2Session::new();
    let client = session
        .client_for(&mock.url)
        .await
        .expect("client_for must succeed");

    let cmd_vel = "/controller/cmd_vel".to_string();
    let cmd_vel_type = "geometry_msgs/msg/Twist".to_string();
    let traveling = "/controller/traveling".to_string();
    let traveling_type = ROSPIDER_TRAVELING_TYPE.to_string();
    let followup = StopFollowup {
        topic: traveling.clone(),
        msg_type: traveling_type.clone(),
        msg: rospider_rest_stop_msg(),
    };

    let (id, token, finished_tx) = session
        .begin_motion(&cmd_vel, &cmd_vel_type, Some(&followup))
        .await;
    let done = Arc::new(AtomicBool::new(false));

    tokio::spawn(run_fake_motion(
        session.clone(),
        client.clone(),
        cmd_vel.clone(),
        cmd_vel_type.clone(),
        traveling.clone(),
        traveling_type.clone(),
        token,
        finished_tx,
        id,
        done.clone(),
        None,
        FakeMotionKind::Unconfirmed { slow_ms: 400 },
    ));

    wait_until(
        || !mock.published_on(&cmd_vel).is_empty(),
        Duration::from_secs(2),
    )
    .await;
    assert!(
        !mock.published_on(&cmd_vel).is_empty(),
        "expected the fake motion's NonZeroTwist to be recorded"
    );

    let report = session.stop_all(Some(&mock.url)).await;

    assert!(
        done.load(Ordering::SeqCst),
        "stop_all must not return before the interrupted motion finished"
    );

    wait_until(
        || mock.published_on(&traveling).len() >= 3,
        Duration::from_secs(3),
    )
    .await;

    let kinds = stop_frame_kinds(&mock, &cmd_vel, &traveling);
    let expected = vec![
        FrameKind::NonZeroTwist,
        FrameKind::ZeroTwist,
        FrameKind::ZeroTwist,
        FrameKind::ZeroTwist,
        FrameKind::ZeroTwist,
        FrameKind::ZeroTwist,
        FrameKind::ZeroTwist,
        FrameKind::RestStop,
        FrameKind::RestStop,
        FrameKind::RestStop,
    ];
    assert_eq!(kinds, expected, "unexpected frame sequence: {kinds:?}");
    assert!(
        !report.contains("did not finish its own stop"),
        "got: {report}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_stop_ordering_stuck_motion_bounded_wait_full_stop_never_skipped() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let session = Ros2Session::new();
    let client = session
        .client_for(&mock.url)
        .await
        .expect("client_for must succeed");

    let cmd_vel = "/controller/cmd_vel".to_string();
    let cmd_vel_type = "geometry_msgs/msg/Twist".to_string();
    let traveling = "/controller/traveling".to_string();
    let traveling_type = ROSPIDER_TRAVELING_TYPE.to_string();
    let followup = StopFollowup {
        topic: traveling.clone(),
        msg_type: traveling_type.clone(),
        msg: rospider_rest_stop_msg(),
    };

    let (id, token, finished_tx) = session
        .begin_motion(&cmd_vel, &cmd_vel_type, Some(&followup))
        .await;
    let done = Arc::new(AtomicBool::new(false));
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();

    tokio::spawn(run_fake_motion(
        session.clone(),
        client.clone(),
        cmd_vel.clone(),
        cmd_vel_type.clone(),
        traveling.clone(),
        traveling_type.clone(),
        token,
        finished_tx,
        id,
        done.clone(),
        None,
        FakeMotionKind::Stuck {
            release: release_rx,
        },
    ));

    wait_until(
        || !mock.published_on(&cmd_vel).is_empty(),
        Duration::from_secs(2),
    )
    .await;
    assert!(
        !mock.published_on(&cmd_vel).is_empty(),
        "expected the fake motion's NonZeroTwist to be recorded"
    );

    let t0 = std::time::Instant::now();
    let report = session.stop_all(Some(&mock.url)).await;
    let elapsed = t0.elapsed();

    assert!(
        elapsed >= Duration::from_millis(PREEMPT_WAIT_MS),
        "elapsed {elapsed:?} must be at least PREEMPT_WAIT_MS ({PREEMPT_WAIT_MS}ms)"
    );
    assert!(
        elapsed < Duration::from_millis(PREEMPT_WAIT_MS) + Duration::from_millis(2000),
        "elapsed {elapsed:?} must be under PREEMPT_WAIT_MS + 2000ms"
    );

    wait_until(
        || mock.published_on(&traveling).len() >= 3,
        Duration::from_secs(3),
    )
    .await;

    let cmd_vel_frames: Vec<_> = mock
        .frames_with_op("publish")
        .into_iter()
        .filter(|f| f.value.get("topic").and_then(Value::as_str) == Some(cmd_vel.as_str()))
        .collect();
    let first_zero = cmd_vel_frames
        .iter()
        .find(|f| is_zero_twist_msg(f.value.get("msg").unwrap_or(&Value::Null)))
        .expect("expected a ZeroTwist frame on cmd_vel");
    assert!(
        first_zero.at >= t0 + Duration::from_millis(PREEMPT_WAIT_MS),
        "the first ZeroTwist frame must arrive at least PREEMPT_WAIT_MS after t0"
    );

    let kinds = stop_frame_kinds(&mock, &cmd_vel, &traveling);
    let expected = vec![
        FrameKind::NonZeroTwist,
        FrameKind::ZeroTwist,
        FrameKind::ZeroTwist,
        FrameKind::ZeroTwist,
        FrameKind::RestStop,
        FrameKind::RestStop,
        FrameKind::RestStop,
    ];
    assert_eq!(kinds, expected, "unexpected frame sequence: {kinds:?}");

    assert!(
        report.contains(&format!(
            "did not finish its own stop within {PREEMPT_WAIT_MS}ms"
        )),
        "got: {report}"
    );

    let _ = release_tx.send(());
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_stop_ordering_dropped_motion_signal_does_not_wait() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let session = Ros2Session::new();
    let client = session
        .client_for(&mock.url)
        .await
        .expect("client_for must succeed");

    let cmd_vel = "/controller/cmd_vel".to_string();
    let cmd_vel_type = "geometry_msgs/msg/Twist".to_string();
    let traveling = "/controller/traveling".to_string();
    let traveling_type = ROSPIDER_TRAVELING_TYPE.to_string();
    let followup = StopFollowup {
        topic: traveling.clone(),
        msg_type: traveling_type.clone(),
        msg: rospider_rest_stop_msg(),
    };

    let (id, token, finished_tx) = session
        .begin_motion(&cmd_vel, &cmd_vel_type, Some(&followup))
        .await;
    let done = Arc::new(AtomicBool::new(false));

    tokio::spawn(run_fake_motion(
        session.clone(),
        client.clone(),
        cmd_vel.clone(),
        cmd_vel_type.clone(),
        traveling.clone(),
        traveling_type.clone(),
        token,
        finished_tx,
        id,
        done.clone(),
        None,
        FakeMotionKind::DroppedSignal,
    ));

    wait_until(
        || !mock.published_on(&cmd_vel).is_empty(),
        Duration::from_secs(2),
    )
    .await;
    assert!(
        !mock.published_on(&cmd_vel).is_empty(),
        "expected the fake motion's NonZeroTwist to be recorded"
    );

    let t0 = std::time::Instant::now();
    let report = session.stop_all(Some(&mock.url)).await;
    let elapsed = t0.elapsed();

    assert!(
        elapsed < Duration::from_millis(1000),
        "elapsed {elapsed:?} must be under 1000ms — a dropped finish signal must not wait"
    );

    wait_until(
        || mock.published_on(&traveling).len() >= 3,
        Duration::from_secs(3),
    )
    .await;

    let kinds = stop_frame_kinds(&mock, &cmd_vel, &traveling);
    let expected = vec![
        FrameKind::NonZeroTwist,
        FrameKind::ZeroTwist,
        FrameKind::ZeroTwist,
        FrameKind::ZeroTwist,
        FrameKind::RestStop,
        FrameKind::RestStop,
        FrameKind::RestStop,
    ];
    assert_eq!(kinds, expected, "unexpected frame sequence: {kinds:?}");
    assert!(!report.contains("did not finish"), "got: {report}");
}

// ---------------------------------------------------------------------------
// 49.8-16: concurrent and redundant stop requests never interleave and never
// resend a stop the robot already got (G-49.8-2b, CR-01 concurrent half).
// ---------------------------------------------------------------------------

/// The CLI one-shot/kanban exit shape: `with_session_end_stop` wraps a whole
/// turn that dispatches a walk and returns (dropping it, via `MotionGuard`)
/// before it finishes. Regression coverage — this may already pass after
/// 49.8-15, since only one stop request is involved here.
#[tokio::test(flavor = "multi_thread")]
async fn ros2_stop_ordering_with_session_end_stop_mid_walk() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url));
    let registry = tokio::sync::RwLock::new(registry_with_rospider(cfg, None));

    let cmd_vel = "/controller/cmd_vel";
    let traveling = "/controller/traveling";

    let turn = async {
        let guard = registry.read().await;
        let walk_fut = guard.dispatch(
            "rospider",
            json!({"action": "walk", "direction": "forward", "duration_secs": 5.0}),
        );
        tokio::pin!(walk_fut);
        tokio::select! {
            _ = &mut walk_fut => {},
            _ = wait_until(
                || {
                    mock.published_on(cmd_vel)
                        .iter()
                        .filter(|m| !is_zero_twist_msg(m))
                        .count()
                        >= 2
                },
                Duration::from_secs(3),
            ) => {},
        }
        assert!(
            mock.published_on(cmd_vel)
                .iter()
                .filter(|m| !is_zero_twist_msg(m))
                .count()
                >= 2,
            "expected at least 2 non-zero Twist publishes before the turn returned — the walk never started"
        );
    };

    let (_, report) = with_session_end_stop(&registry, turn).await;

    wait_until(
        || mock.published_on(traveling).len() >= 6,
        Duration::from_secs(3),
    )
    .await;

    let kinds = stop_frame_kinds(&mock, cmd_vel, traveling);

    let first_rest_idx = kinds
        .iter()
        .position(|k| *k == FrameKind::RestStop)
        .expect("expected at least one RestStop frame");
    assert!(
        kinds[first_rest_idx..]
            .iter()
            .all(|k| *k == FrameKind::RestStop),
        "no cmd_vel frame of any kind may follow the first RestStop; got {kinds:?}"
    );

    let last_non_zero_idx = kinds
        .iter()
        .rposition(|k| *k == FrameKind::NonZeroTwist)
        .expect("expected at least one NonZeroTwist frame");
    let zero_twists_after = kinds[last_non_zero_idx + 1..]
        .iter()
        .filter(|k| **k == FrameKind::ZeroTwist)
        .count();
    assert_eq!(
        zero_twists_after, 3,
        "expected exactly 3 ZeroTwist frames after the last NonZeroTwist; got {kinds:?}"
    );

    let rest_stop_count = kinds.iter().filter(|k| **k == FrameKind::RestStop).count();
    assert_eq!(
        rest_stop_count, 6,
        "expected exactly 6 RestStop frames; got {kinds:?}"
    );

    assert!(
        report.contains("gait-0 rest stop sent on 1/1 topic(s)"),
        "got: {report}"
    );
    assert!(!report.contains("did not finish"), "got: {report}");
}

/// The TUI/REPL exit shape: both `on_session_end` hooks fan out at the same
/// moment as the awaited `end_session_stop`. Without serialization the two
/// hooks burst concurrently (RED: 9 ZeroTwist); with `stop_lock` and
/// coalescing exactly one full stop happens (motion's own confirmed burst
/// plus the winning call's own gait-0 rest stop — 3 ZeroTwist, 6 RestStop
/// total), regardless of which of the three callers wins the race, because a
/// coalesced report is a verbatim copy of the winner's report plus the note.
#[tokio::test(flavor = "multi_thread")]
async fn ros2_stop_ordering_session_end_fanout_sends_one_stop() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url));
    let registry = Arc::new(tokio::sync::RwLock::new(registry_with_rospider(cfg, None)));

    let cmd_vel = "/controller/cmd_vel";
    let traveling = "/controller/traveling";

    let registry_walk = registry.clone();
    let walk_handle = tokio::spawn(async move {
        registry_walk
            .read()
            .await
            .dispatch(
                "rospider",
                json!({"action": "walk", "direction": "forward", "duration_secs": 5.0}),
            )
            .await
    });

    wait_until(
        || {
            mock.published_on(cmd_vel)
                .iter()
                .filter(|m| !is_zero_twist_msg(m))
                .count()
                >= 2
        },
        Duration::from_secs(3),
    )
    .await;
    assert!(
        mock.published_on(cmd_vel)
            .iter()
            .filter(|m| !is_zero_twist_msg(m))
            .count()
            >= 2,
        "expected at least 2 non-zero Twist publishes before session end — the walk never started"
    );

    // Exactly as tui_rata/event_loop.rs does it: fire the fan-out hooks,
    // then await the session-end stop.
    registry.read().await.call_session_end_hooks();
    let report = end_session_stop(&registry).await;

    let walk_output = tokio::time::timeout(Duration::from_secs(3), walk_handle)
        .await
        .expect("walk must finish within 3s")
        .expect("walk task must not panic");
    walk_output.expect("walk dispatch must not error");

    wait_until(
        || mock.published_on(traveling).len() >= 6,
        Duration::from_secs(3),
    )
    .await;

    let kinds = stop_frame_kinds(&mock, cmd_vel, traveling);

    let last_non_zero_idx = kinds
        .iter()
        .rposition(|k| *k == FrameKind::NonZeroTwist)
        .expect("expected at least one NonZeroTwist frame");
    let zero_twists_after = kinds[last_non_zero_idx + 1..]
        .iter()
        .filter(|k| **k == FrameKind::ZeroTwist)
        .count();
    assert_eq!(
        zero_twists_after, 3,
        "expected exactly 3 ZeroTwist frames after the last NonZeroTwist; got {kinds:?}"
    );

    let rest_stop_count = kinds.iter().filter(|k| **k == FrameKind::RestStop).count();
    assert_eq!(
        rest_stop_count, 6,
        "expected exactly 6 RestStop frames; got {kinds:?}"
    );

    let first_rest_idx = kinds
        .iter()
        .position(|k| *k == FrameKind::RestStop)
        .expect("expected at least one RestStop frame");
    assert!(
        kinds[first_rest_idx..]
            .iter()
            .all(|k| *k == FrameKind::RestStop),
        "no cmd_vel frame may follow the first RestStop; got {kinds:?}"
    );

    assert!(
        report.contains("gait-0 rest stop sent on 1/1"),
        "got: {report}"
    );
}

/// An explicit `rospider stop` racing a session end (an equally plausible
/// `/new` race). Both are Direct callers of `stop_all_as`, serialized by
/// `stop_lock`; whichever loses coalesces (C1). `do_stop`'s unconditional
/// follow-up still runs regardless of which one that caller was — it always
/// contributes its own 3 RestStop on top of the motion's own 3 and the
/// winning stop_all's own 3, for 9 total.
#[tokio::test(flavor = "multi_thread")]
async fn ros2_stop_ordering_explicit_stop_racing_session_end_serialized() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url));
    let registry = Arc::new(tokio::sync::RwLock::new(registry_with_rospider(cfg, None)));

    let cmd_vel = "/controller/cmd_vel";
    let traveling = "/controller/traveling";

    let registry_walk = registry.clone();
    let walk_handle = tokio::spawn(async move {
        registry_walk
            .read()
            .await
            .dispatch(
                "rospider",
                json!({"action": "walk", "direction": "forward", "duration_secs": 5.0}),
            )
            .await
    });

    wait_until(
        || {
            mock.published_on(cmd_vel)
                .iter()
                .filter(|m| !is_zero_twist_msg(m))
                .count()
                >= 2
        },
        Duration::from_secs(3),
    )
    .await;
    assert!(
        mock.published_on(cmd_vel)
            .iter()
            .filter(|m| !is_zero_twist_msg(m))
            .count()
            >= 2,
        "expected at least 2 non-zero Twist publishes before racing the stop — the walk never started"
    );

    let (stop_report, end_report) = tokio::join!(
        async {
            registry
                .read()
                .await
                .dispatch("rospider", json!({"action": "stop"}))
                .await
                .expect("stop dispatch must not error")
        },
        end_session_stop(&registry),
    );

    let walk_output = tokio::time::timeout(Duration::from_secs(3), walk_handle)
        .await
        .expect("walk must finish within 3s")
        .expect("walk task must not panic");
    walk_output.expect("walk dispatch must not error");

    wait_until(
        || mock.published_on(traveling).len() >= 9,
        Duration::from_secs(3),
    )
    .await;

    let kinds = stop_frame_kinds(&mock, cmd_vel, traveling);

    let last_non_zero_idx = kinds
        .iter()
        .rposition(|k| *k == FrameKind::NonZeroTwist)
        .expect("expected at least one NonZeroTwist frame");
    let zero_twists_after = kinds[last_non_zero_idx + 1..]
        .iter()
        .filter(|k| **k == FrameKind::ZeroTwist)
        .count();
    assert_eq!(
        zero_twists_after, 3,
        "expected exactly 3 ZeroTwist frames; got {kinds:?}"
    );

    let rest_stop_count = kinds.iter().filter(|k| **k == FrameKind::RestStop).count();
    assert_eq!(
        rest_stop_count, 9,
        "expected exactly 9 RestStop frames (motion 3, one stop_all 3, do_stop 3); got {kinds:?}"
    );

    let first_rest_idx = kinds
        .iter()
        .position(|k| *k == FrameKind::RestStop)
        .expect("expected at least one RestStop frame");
    assert!(
        kinds[first_rest_idx..]
            .iter()
            .all(|k| *k == FrameKind::RestStop),
        "no cmd_vel frame may follow the first RestStop; got {kinds:?}"
    );
    assert_eq!(
        kinds.last(),
        Some(&FrameKind::RestStop),
        "expected the last frame to be a RestStop; got {kinds:?}"
    );

    let coalesced_count = [stop_report.as_str(), end_report.as_str()]
        .iter()
        .filter(|r| r.contains(STOP_COALESCED_NOTE))
        .count();
    assert_eq!(
        coalesced_count, 1,
        "exactly one of the two stop reports must contain STOP_COALESCED_NOTE; stop={stop_report}, end={end_report}"
    );
}

/// The CLI exit hazard: a hook stop right after a clean stop with nothing
/// driven since must send nothing (half 1); new activity since that clean
/// stop must force a full resend (half 2).
#[tokio::test(flavor = "multi_thread")]
async fn ros2_stop_ordering_hook_after_clean_stop_sends_nothing() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url));
    let registry = Arc::new(tokio::sync::RwLock::new(registry_with_rospider(cfg, None)));
    let session = registry.read().await.ros2_session();

    let cmd_vel = "/controller/cmd_vel";
    let traveling = "/controller/traveling";

    // Half 1: a walk that finishes on its own, then the awaited session-end
    // stop (the CLI one-shot/kanban shape). D-10 still requires the
    // session-end stop to publish zero Twists even though the walk already
    // finished (the "Idle session end on a ROSPider" residual, documented in
    // Task 2) — so this is 6 RestStop, not 3.
    registry
        .read()
        .await
        .dispatch(
            "rospider",
            json!({"action": "walk", "direction": "forward", "duration_secs": 0.3}),
        )
        .await
        .expect("first walk dispatch must not error");

    let report1 = end_session_stop(&registry).await;
    assert!(!report1.contains("did not finish"), "got: {report1}");

    wait_until(
        || mock.published_on(traveling).len() >= 6,
        Duration::from_secs(3),
    )
    .await;
    assert!(
        mock.published_on(traveling).len() >= 6,
        "expected the natural-finish burst plus the awaited session-end stop to both land"
    );

    let baseline = mock.frames_with_op("publish").len();

    let hook_report1 = session.session_end_hook_stop(Some(&mock.url)).await;
    assert!(
        hook_report1.contains(STOP_COALESCED_NOTE),
        "a hook stop right after a clean stop with nothing driven since must coalesce; got: {hook_report1}"
    );

    let rospider_tool = registry
        .read()
        .await
        .get_arc("rospider")
        .expect("rospider tool must be registered");
    let ros2_tool = registry
        .read()
        .await
        .get_arc("ros2")
        .expect("ros2 tool must be registered");
    rospider_tool.on_session_end();
    ros2_tool.on_session_end();
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert_eq!(
        mock.frames_with_op("publish").len(),
        baseline,
        "a coalesced hook stop, and hooks that also coalesce, must send nothing on the wire"
    );

    // Half 2: new activity since the last clean stop forces a full resend —
    // deliver the second walk's own 3 rest stops (9 in total) BEFORE
    // capturing the marker (the mock records frames asynchronously, so the
    // marker must be taken only after every sender has returned).
    registry
        .read()
        .await
        .dispatch(
            "rospider",
            json!({"action": "walk", "direction": "forward", "duration_secs": 0.3}),
        )
        .await
        .expect("second walk dispatch must not error");

    wait_until(
        || mock.published_on(traveling).len() >= 9,
        Duration::from_secs(3),
    )
    .await;
    assert!(
        mock.published_on(traveling).len() >= 9,
        "expected the second walk's own 3 rest stops to be delivered before capturing the marker"
    );

    let marker2 = std::time::Instant::now();
    let report2 = session.session_end_hook_stop(Some(&mock.url)).await;

    wait_until(
        || {
            mock.frames_with_op("publish")
                .iter()
                .any(|f| f.at > marker2)
        },
        Duration::from_secs(2),
    )
    .await;

    assert!(
        !report2.contains(STOP_COALESCED_NOTE),
        "new activity since the last clean stop must force a full resend; got: {report2}"
    );
    assert!(report2.contains("stopped 1/1 topic(s)"), "got: {report2}");

    let after_marker: Vec<_> = mock
        .frames_with_op("publish")
        .into_iter()
        .filter(|f| f.at > marker2)
        .collect();
    assert!(
        after_marker.iter().any(|f| {
            f.value.get("topic").and_then(Value::as_str) == Some(cmd_vel)
                && is_zero_twist_msg(f.value.get("msg").unwrap_or(&Value::Null))
        }),
        "expected at least one ZeroTwist recorded after the marker; got {after_marker:?}"
    );
    assert!(
        after_marker
            .iter()
            .any(|f| f.value.get("topic").and_then(Value::as_str) == Some(traveling)),
        "expected at least one RestStop recorded after the marker; got {after_marker:?}"
    );
}

#[test]
fn ros2_stop_ordering_wait_budget_fits_session_end_bound() {
    let stop_seq_ms = (STOP_REPEATS as u64 - 1) * STOP_INTERVAL_MS
        + STOP_INTERVAL_MS
        + (STOP_REPEATS as u64 - 1) * STOP_INTERVAL_MS;
    assert!(
        PREEMPT_WAIT_MS >= 2 * stop_seq_ms,
        "PREEMPT_WAIT_MS ({PREEMPT_WAIT_MS}) must be at least 2x stop_seq_ms ({stop_seq_ms})"
    );
    assert!(
        PREEMPT_WAIT_MS + stop_seq_ms < ROS2_SESSION_END_TIMEOUT_SECS * 1000,
        "the healthy interrupt path (PREEMPT_WAIT_MS + stop_seq_ms) must fit inside the session-end bound"
    );
    // 49.8-17 (WR-01): the DEGRADED path — the post-interrupt-wait reconnect
    // branch can additionally burn RECONNECT_STOP_TIMEOUT_SECS before its own
    // burst — must also fit inside the session-end bound, or the outer
    // timeout fires mid-stop and the report degrades to an uninformative
    // "timed out". The three constants are pinned together here so they can
    // never silently drift back into an overrun.
    assert!(
        PREEMPT_WAIT_MS + RECONNECT_STOP_TIMEOUT_SECS * 1000 + stop_seq_ms
            < ROS2_SESSION_END_TIMEOUT_SECS * 1000,
        "the degraded reconnect path (PREEMPT_WAIT_MS + RECONNECT_STOP_TIMEOUT_SECS*1000 + one stop sequence) must fit inside the session-end bound"
    );

    // 49.8-19 (VERIFICATION gap #1): stop_seq_ms, computed from the same raw
    // constants above, must equal the pub const the session-end budget
    // itself derives from — so this test's own arithmetic can never
    // silently diverge from `session.rs`'s.
    assert_eq!(
        stop_seq_ms, STOP_SEQUENCE_MS,
        "stop_seq_ms must equal the STOP_SEQUENCE_MS constant"
    );

    // 49.8-19: the goal-cancel phase term (49.8-18) is one constant,
    // independent of goal count — pin it against its own primitives so a
    // drift in either can never silently widen or shrink the budget below.
    assert_eq!(
        GOAL_CANCEL_PHASE_MS,
        GOAL_CANCEL_SEND_WINDOW_MS + CANCEL_WAIT_SECS * 1000,
        "GOAL_CANCEL_PHASE_MS must equal GOAL_CANCEL_SEND_WINDOW_MS + CANCEL_WAIT_SECS*1000"
    );

    // 49.8-19: parse TimeoutStopSec from both deploy units — the same repo
    // files systemd actually reads — so the pin below tracks the real
    // deployment, not a hardcoded assumption about it.
    fn parse_timeout_stop_sec(unit: &str) -> Option<u64> {
        unit.lines()
            .find_map(|l| l.trim().strip_prefix("TimeoutStopSec="))
            .and_then(|v| v.trim().parse::<u64>().ok())
    }
    let gateway_unit = include_str!("../../../scripts/deploy/ironhermes-gateway.service");
    let web_unit = include_str!("../../../scripts/deploy/ironhermes-web.service");
    let gateway_timeout_stop_sec = parse_timeout_stop_sec(gateway_unit)
        .expect("ironhermes-gateway.service must declare TimeoutStopSec");
    let web_timeout_stop_sec = parse_timeout_stop_sec(web_unit)
        .expect("ironhermes-web.service must declare TimeoutStopSec");
    let stop_budget_ms = gateway_timeout_stop_sec.min(web_timeout_stop_sec) * 1000;

    // 49.8-19 (VERIFICATION gap #1, review 49.8-19): every awaited,
    // constant-bounded term of the worst degraded stop, summed independently
    // from the primitive constants, for EVERY motion-topic count the
    // session can reach (0..=MAX_MOTION_TOPICS is the exact set — this is
    // the test's only per-count iteration). Neither a constant nor a count
    // can drift the bound back into an overrun without failing here first.
    for n in 0..=MAX_MOTION_TOPICS as u64 {
        let worst_n = PREEMPT_WAIT_MS
            + RECONNECT_STOP_TIMEOUT_SECS * 1000
            + n * stop_seq_ms
            + GOAL_CANCEL_PHASE_MS;
        assert!(
            worst_n + SESSION_END_HEADROOM_MS <= ROS2_SESSION_END_TIMEOUT_SECS * 1000,
            "worst({n}) = PREEMPT_WAIT_MS({PREEMPT_WAIT_MS}) + RECONNECT_STOP_TIMEOUT_SECS*1000({}) + \
             {n}*stop_seq_ms({stop_seq_ms}) + GOAL_CANCEL_PHASE_MS({GOAL_CANCEL_PHASE_MS}) = {worst_n}, \
             plus SESSION_END_HEADROOM_MS({SESSION_END_HEADROOM_MS}) = {}, must fit inside \
             ROS2_SESSION_END_TIMEOUT_SECS*1000 ({})",
            RECONNECT_STOP_TIMEOUT_SECS * 1000,
            worst_n + SESSION_END_HEADROOM_MS,
            ROS2_SESSION_END_TIMEOUT_SECS * 1000
        );
    }

    // 49.8-19: pins ONLY the gateway's awaited first shutdown step
    // (runner.rs's graceful shutdown awaits `end_session_stop` before MCP
    // teardown/drain) — not MCP teardown, not the post-timeout residual, and
    // not the web unit's shutdown (which awaits no session-end stop).
    assert!(
        ROS2_SESSION_END_TIMEOUT_SECS * 1000 + 2000 <= stop_budget_ms,
        "ROS2_SESSION_END_TIMEOUT_SECS*1000 + 2000 must fit inside min(TimeoutStopSec)*1000 ({stop_budget_ms}); \
         gateway TimeoutStopSec={gateway_timeout_stop_sec}, web TimeoutStopSec={web_timeout_stop_sec} — this \
         pins only the gateway's awaited first shutdown step"
    );

    // 49.8-19: the ceiling can never be satisfied by refusing all motion.
    const {
        assert!(
            MAX_MOTION_TOPICS >= 1,
            "MAX_MOTION_TOPICS must be at least 1"
        )
    };
}

/// 49.8-19 (VERIFICATION gap #1, D-08/D-11, review 49.8-19): a process
/// drives at most `MAX_MOTION_TOPICS` distinct motion topics, whatever
/// config.yaml declares and however many are admitted concurrently — the
/// ceiling `ROS2_SESSION_END_TIMEOUT_SECS` is sized against, with no live
/// count ever read (and raced) at session end.
#[tokio::test(flavor = "multi_thread")]
async fn ros2_stop_ordering_motion_topic_ceiling_bounds_session_end_stop() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let mut cfg = fixed_config(Some(&mock.url));
    for i in 0..=MAX_MOTION_TOPICS {
        cfg.ros2.publish.push(allow_entry(
            &format!("/ceiling_{i}"),
            "geometry_msgs/msg/Twist",
        ));
    }
    let registry = registry_with_rospider(cfg, None);
    let session = registry.ros2_session();

    for i in 0..MAX_MOTION_TOPICS {
        let topic = format!("/ceiling_{i}");
        let reply = registry
            .dispatch(
                "ros2",
                json!({
                    "action": "publish",
                    "topic": topic,
                    "type": "geometry_msgs/msg/Twist",
                    "msg": {"linear": {"x": 0.05}},
                    "duration_secs": 0.1
                }),
            )
            .await
            .unwrap_or_else(|e| panic!("ceiling motion dispatch for {topic} must not error: {e}"));
        assert!(
            !reply.contains("is not permitted"),
            "topic {topic} within the ceiling must be admitted; got: {reply}"
        );
    }

    let over_topic = format!("/ceiling_{MAX_MOTION_TOPICS}");
    let refused = registry
        .dispatch(
            "ros2",
            json!({
                "action": "publish",
                "topic": over_topic,
                "type": "geometry_msgs/msg/Twist",
                "msg": {"linear": {"x": 0.05}},
                "duration_secs": 0.1
            }),
        )
        .await
        .expect("over-ceiling dispatch must not error");
    assert!(refused.contains("is not permitted"), "got: {refused}");
    assert!(
        refused.contains(&format!("{MAX_MOTION_TOPICS} distinct motion topics")),
        "got: {refused}"
    );
    assert!(
        !mock
            .frames()
            .iter()
            .any(|f| f.value.get("topic").and_then(Value::as_str) == Some(over_topic.as_str())),
        "no recorded mock frame of any op must ever have named the refused topic"
    );
    assert_eq!(
        session.motion_topics().await.len(),
        MAX_MOTION_TOPICS,
        "the refused topic must not have been inserted into motion_topics"
    );

    // Re-driving an already-admitted topic is never refused.
    let redrive = registry
        .dispatch(
            "ros2",
            json!({
                "action": "publish",
                "topic": "/ceiling_0",
                "type": "geometry_msgs/msg/Twist",
                "msg": {"linear": {"x": 0.05}},
                "duration_secs": 0.1
            }),
        )
        .await
        .expect("re-drive dispatch must not error");
    assert!(!redrive.contains("is not permitted"), "got: {redrive}");

    let report = tokio::time::timeout(
        Duration::from_secs(ROS2_SESSION_END_TIMEOUT_SECS),
        session.stop_all(Some(&mock.url)),
    )
    .await
    .expect("stop_all must return within ROS2_SESSION_END_TIMEOUT_SECS");
    assert!(
        report.contains(&format!(
            "stopped {MAX_MOTION_TOPICS}/{MAX_MOTION_TOPICS} topic(s)"
        )),
        "got: {report}"
    );

    // Atomicity: on a fresh session, MAX_MOTION_TOPICS + 4 concurrent admits
    // on distinct new topics must yield exactly MAX_MOTION_TOPICS successes.
    let fresh = Ros2Session::new();
    let mut handles = Vec::new();
    for i in 0..MAX_MOTION_TOPICS + 4 {
        let fresh = fresh.clone();
        handles.push(tokio::spawn(async move {
            fresh
                .admit_motion_topic(&format!("/atomic_{i}"), "geometry_msgs/msg/Twist")
                .await
        }));
    }
    let mut ok_count = 0usize;
    for h in handles {
        if h.await.expect("admit task must not panic").is_ok() {
            ok_count += 1;
        }
    }
    assert_eq!(
        ok_count, MAX_MOTION_TOPICS,
        "exactly MAX_MOTION_TOPICS concurrent admits on distinct new topics must succeed"
    );
    assert_eq!(
        fresh.motion_topics().await.len(),
        MAX_MOTION_TOPICS,
        "motion_topics must have exactly MAX_MOTION_TOPICS entries after the concurrent race"
    );
}

/// 49.8-19 (VERIFICATION gap #1, tracer regression guard): the real degraded
/// session-end path — a reconnect, a ROSPider rest-stop topic, and cancel
/// services on three action servers that never answer — returns the real
/// stop report within the bound, never the timeout report. May already pass
/// after 49.8-18 (the goal-cancel phase became count-independent there);
/// this is a regression guard for the composed path this plan's budget test
/// proves in isolation.
#[tokio::test(flavor = "multi_thread")]
async fn ros2_stop_ordering_session_end_degraded_path_reports_within_bound() {
    let mock = MockRosbridge::start_with(MockConfig {
        unanswered_services: vec![
            "/alpha/_action/cancel_goal".to_string(),
            "/beta/_action/cancel_goal".to_string(),
            "/gamma/_action/cancel_goal".to_string(),
        ],
        action_result_delay_ms: 10_000,
        ..Default::default()
    })
    .await;
    let mut cfg = fixed_config(Some(&mock.url));
    for action in ["/alpha", "/beta", "/gamma"] {
        cfg.ros2
            .actions
            .push(allow_entry(action, "example_interfaces/action/Fibonacci"));
    }
    let registry = Arc::new(tokio::sync::RwLock::new(registry_with_rospider(cfg, None)));

    let traveling = "/controller/traveling";

    registry
        .read()
        .await
        .dispatch(
            "rospider",
            json!({"action": "walk", "direction": "forward", "duration_secs": 0.3}),
        )
        .await
        .expect("walk dispatch must not error");

    wait_until(
        || mock.published_on(traveling).len() >= 3,
        Duration::from_secs(3),
    )
    .await;
    assert!(
        mock.published_on(traveling).len() >= 3,
        "expected the walk's own 3 rest stops before dispatching the goals"
    );

    for action in ["/alpha", "/beta", "/gamma"] {
        registry
            .read()
            .await
            .dispatch(
                "ros2",
                json!({"action": "send_goal", "action_name": action, "goal": {}}),
            )
            .await
            .unwrap_or_else(|e| panic!("send_goal for {action} must not error: {e}"));
    }

    let session = registry.read().await.ros2_session();
    session
        .current_client()
        .await
        .expect("a client must be connected after the walk + three send_goal calls above")
        .close();

    // Deviation (matches 49.8-18-SUMMARY.md's documented Rule 1 fix):
    // `end_session_stop`/`stop_all_live` always resolve the bridge URL
    // through `Ros2ConfigSource::Live` (T-49.8-08 — a test process must
    // never have its reconnect driven by a real robot URL), so after
    // closing the connection above, `end_session_stop`'s own reconnect
    // would resolve to whatever `Config::load()` finds on the real host
    // (`None` here) instead of ever reaching the degraded path this test
    // exercises. `session.stop_all(Some(&mock.url))`, wrapped in the same
    // bound `end_session_stop` applies, is the exact call the sibling
    // reconnect test (`ros2_session_end_after_drop_reconnects_and_cancels_all`
    // in `ros2_actions.rs`) already uses to force this identical path.
    let start = std::time::Instant::now();
    let report = tokio::time::timeout(
        Duration::from_secs(ROS2_SESSION_END_TIMEOUT_SECS),
        session.stop_all(Some(&mock.url)),
    )
    .await
    .expect("the degraded path must report the real result within the bound, not time out");
    let elapsed = start.elapsed();

    assert!(report.contains("3 goal failure(s)"), "got: {report}");
    assert!(
        report.contains("gait-0 rest stop sent on 1/1 topic(s)"),
        "got: {report}"
    );
    assert!(
        report.contains("(cancel-all fallback used)"),
        "got: {report}"
    );
    assert!(
        elapsed < Duration::from_secs(ROS2_SESSION_END_TIMEOUT_SECS),
        "elapsed {elapsed:?} must be under ROS2_SESSION_END_TIMEOUT_SECS"
    );
}

// ---------------------------------------------------------------------------
// 49.8-17: begin_motion racing a stop's interrupt-wait window (WR-02)
// ---------------------------------------------------------------------------

/// A `begin_motion` that lands while `stop_all` is mid-interrupt-wait —
/// `active_motion` is already `None` (the stop took it) and the stop is
/// draining the old motion — must wait for the stop to finish before
/// installing the new motion (49.8-17, WR-02). The test enters the window
/// deterministically via the shared `run_fake_motion` `cancelled` flag
/// (hard-asserted, so the race is never vacuous), then checks that the
/// racing `begin_motion` does not return before the stop task recorded
/// completion, and that the old motion's trailing gait-0 and the new
/// motion's Twist are strictly ordered on the final frame sequence.
///
/// RED behavior (no `stop_in_flight` gate): the racing `begin_motion` sees
/// `active_motion == None` and installs immediately — `stop_done` is false
/// when it returns, and motion B's Twist interleaves with the draining stop.
#[tokio::test(flavor = "multi_thread")]
async fn ros2_stop_ordering_begin_motion_racing_stop_interrupt_wait_serialized() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let session = Ros2Session::new();
    let client = session
        .client_for(&mock.url)
        .await
        .expect("client_for must succeed");

    let cmd_vel = "/controller/cmd_vel";
    let cmd_vel_type = "geometry_msgs/msg/Twist";
    let traveling = "/controller/traveling";
    let followup = StopFollowup {
        topic: traveling.to_string(),
        msg_type: ROSPIDER_TRAVELING_TYPE.to_string(),
        msg: rospider_rest_stop_msg(),
    };

    // Motion A: a controlled slow-finishing fake motion. The harness's
    // post-cancel `cancelled` flag is the deterministic window-entry signal.
    let (id_a, token_a, finished_tx_a) = session
        .begin_motion(cmd_vel, cmd_vel_type, Some(&followup))
        .await;
    let done_a = Arc::new(AtomicBool::new(false));
    let cancelled = Arc::new(AtomicBool::new(false));

    tokio::spawn(run_fake_motion(
        session.clone(),
        client.clone(),
        cmd_vel.to_string(),
        cmd_vel_type.to_string(),
        traveling.to_string(),
        ROSPIDER_TRAVELING_TYPE.to_string(),
        token_a,
        finished_tx_a,
        id_a,
        done_a.clone(),
        Some(cancelled.clone()),
        FakeMotionKind::Confirmed { slow_ms: 400 },
    ));

    wait_until(
        || !mock.published_on(cmd_vel).is_empty(),
        Duration::from_secs(2),
    )
    .await;
    assert!(
        !mock.published_on(cmd_vel).is_empty(),
        "expected the fake motion A's NonZeroTwist to be recorded"
    );

    let stop_done = Arc::new(AtomicBool::new(false));
    let stop_session = session.clone();
    let stop_done_task = stop_done.clone();
    let bridge_url = mock.url.clone();
    let stop_task = tokio::spawn(async move {
        let report = stop_session.stop_all(Some(&bridge_url)).await;
        stop_done_task.store(true, Ordering::SeqCst);
        report
    });

    // Deterministically enter the WR-02 window: the stop has taken the
    // active motion (active_motion is now None) and is mid-interrupt-wait,
    // draining motion A. This is the dominant real-world interleaving — the
    // stop started BEFORE the racing begin_motion entered — which a bare
    // generation counter cannot distinguish from a long-finished stop.
    wait_until(|| cancelled.load(Ordering::SeqCst), Duration::from_secs(2)).await;
    assert!(
        cancelled.load(Ordering::SeqCst),
        "expected to enter the stop's interrupt-wait window (cancelled flag set) — the race never opened"
    );

    // Motion B: the racing begin_motion. With the WR-02 gate it must not
    // return until stop_all has fully finished (stop_done true) — RED: it
    // returns mid-interrupt-wait, long before stop_done.
    let (id_b, token_b, finished_tx_b) = session
        .begin_motion(cmd_vel, cmd_vel_type, Some(&followup))
        .await;
    assert!(
        stop_done.load(Ordering::SeqCst),
        "the racing begin_motion must not return before the in-flight stop finished"
    );

    // Drive motion B to completion: its own Twist on the wire, then a clean
    // finish signal (as motion_task would send), then release the slot.
    let non_zero_b = json!({
        "linear": {"x": 0.3, "y": 0.0, "z": 0.0},
        "angular": {"x": 0.0, "y": 0.0, "z": 0.0},
    });
    let _ = client.publish(cmd_vel, non_zero_b).await;
    token_b.cancel();
    let _ = finished_tx_b.send(MotionStatus::Finished {
        followup_confirmed: true,
    });
    session.end_motion(id_b).await;

    let report = stop_task.await.expect("stop task must not panic");

    // Delivery-wait for the full expected publish count: motion A's Twist +
    // its 3 zero Twists + its 3 rest stops, the stop's own 3 rest stops
    // (cmd_vel's zero burst is skipped — the interrupted motion confirmed its
    // own), and motion B's Twist.
    wait_until(
        || mock.published_on(cmd_vel).len() >= 5 && mock.published_on(traveling).len() >= 6,
        Duration::from_secs(3),
    )
    .await;

    let kinds = stop_frame_kinds(&mock, cmd_vel, traveling);

    let last_rest_idx = kinds
        .iter()
        .rposition(|k| *k == FrameKind::RestStop)
        .expect("expected at least one RestStop frame");
    // Motion A's own Twist is the first NonZeroTwist; motion B's is the
    // last (B publishes exactly one). With the WR-02 gate nothing sent while
    // the stop drained may interleave with B's stream.
    let b_twist_idx = kinds
        .iter()
        .rposition(|k| *k == FrameKind::NonZeroTwist)
        .expect("expected motion B's NonZeroTwist frame");
    assert!(
        b_twist_idx > last_rest_idx,
        "motion B's first NonZeroTwist must land after the stop's LAST RestStop; got {kinds:?}"
    );
    assert!(
        kinds[last_rest_idx + 1..]
            .iter()
            .all(|k| *k == FrameKind::NonZeroTwist),
        "no frame of any kind may follow the stop's last RestStop except motion B's own Twist; got {kinds:?}"
    );
    let zero_twist_count = kinds.iter().filter(|k| **k == FrameKind::ZeroTwist).count();
    assert_eq!(
        zero_twist_count, 3,
        "expected exactly 3 ZeroTwist frames (motion A's own confirmed burst; the stop skips its redundant zero burst); got {kinds:?}"
    );
    let rest_stop_count = kinds.iter().filter(|k| **k == FrameKind::RestStop).count();
    assert_eq!(
        rest_stop_count, 6,
        "expected exactly 6 RestStop frames (motion A's 3 plus the stop's 3); got {kinds:?}"
    );

    assert!(
        done_a.load(Ordering::SeqCst),
        "motion A must have completed its own stop sequence"
    );
    assert!(
        report.contains("gait-0 rest stop sent on 1/1 topic(s)"),
        "got: {report}"
    );
    assert!(
        !report.contains("did not finish its own stop"),
        "got: {report}"
    );
}

// ---------------------------------------------------------------------------
// 49.8-19 (VERIFICATION gap #7): every stop runs in one owned task — a
// dropped caller never truncates it, never releases its serialization
// early, never skips its goal phase, and runtime teardown leaves no stale
// state.
// ---------------------------------------------------------------------------

/// Shared setup for the two caller-drop tests below (49.8-19). Builds a mock
/// that gates connection 1's WebSocket handshake, a session with a tracked
/// motion topic (a follow-up, no live motion) and one tracked goal whose
/// cancel-all answer is `NotCancelable` (never a hard error, so the ONLY way
/// this test can tell a real stop ran is the frame sequence and the
/// GOAL_NOT_CANCELABLE_NOTE — not a goal failure masking a truncated stop),
/// then closes the session's only connection, spawns a first stop attempt,
/// waits until it is provably parked mid-reconnect (connection 1 accepted at
/// TCP level, held at the gate), and aborts its caller. Returns the mock,
/// session and gate — the run_stop task the aborted caller spawned is still
/// running, holding `stop_lock` and the in-flight flag.
async fn dropped_stop_caller_shared_setup() -> (MockRosbridge, Arc<Ros2Session>, Arc<Notify>) {
    let gate = Arc::new(Notify::new());
    let mut service_responses = HashMap::new();
    service_responses.insert(
        "/alpha/_action/cancel_goal".to_string(),
        json!({"return_code": 3, "goals_canceling": []}),
    );
    let mock = MockRosbridge::start_with(MockConfig {
        gate_connection: Some((1, gate.clone())),
        action_result_delay_ms: 10_000,
        service_responses,
        ..Default::default()
    })
    .await;

    let session = Ros2Session::new();
    let client0 = session
        .client_for(&mock.url)
        .await
        .expect("client_for must succeed (connection 0)");

    let cmd_vel = "/controller/cmd_vel";
    let cmd_vel_type = "geometry_msgs/msg/Twist";
    let traveling = "/controller/traveling";
    let followup = StopFollowup {
        topic: traveling.to_string(),
        msg_type: ROSPIDER_TRAVELING_TYPE.to_string(),
        msg: rospider_rest_stop_msg(),
    };
    let (id, _token, finished_tx) = session
        .begin_motion(cmd_vel, cmd_vel_type, Some(&followup))
        .await;
    let _ = finished_tx.send(MotionStatus::Finished {
        followup_confirmed: false,
    });
    session.end_motion(id).await;

    let goal_id = client0
        .send_action_goal("/alpha", "example_interfaces/action/Fibonacci", json!({}))
        .await
        .expect("send_action_goal must not error");
    session
        .track_goal(
            &goal_id,
            "/alpha",
            "example_interfaces/action/Fibonacci",
            client0.connection_id(),
        )
        .await;

    client0.close();

    let stop_session = session.clone();
    let bridge_url = mock.url.clone();
    let h = tokio::spawn(async move { stop_session.stop_all(Some(&bridge_url)).await });

    wait_until(|| mock.connection_count() >= 2, Duration::from_secs(2)).await;
    assert!(
        mock.connection_count() >= 2,
        "expected the first stop's reconnect attempt to have opened connection 1"
    );

    h.abort();
    let aborted = h.await;
    assert!(
        aborted.is_err(),
        "expected the aborted stop's JoinHandle to report a cancellation"
    );

    (mock, session, gate)
}

/// 49.8-19 (VERIFICATION gap #7, review 49.8-19): a stop whose caller is
/// aborted while the stop is parked mid-reconnect keeps stop_lock and the
/// in-flight flag held — a following hook stop waits, then coalesces onto
/// the aborted stop's own completed record (which ran its full goal-cancel
/// phase), proving the goal phase was never skipped.
///
/// RED (before the owned-task rewrite): aborting the caller drops the
/// parked reconnect, the flag leaks true, `stop_lock` is released, and the
/// hook runs its own stop immediately — the negative-window assertion
/// (`!hook.is_finished()`) fails.
#[tokio::test(flavor = "multi_thread")]
async fn ros2_stop_ordering_dropped_stop_caller_keeps_serialized_full_cleanup() {
    let (mock, session, gate) = dropped_stop_caller_shared_setup().await;
    let cmd_vel = "/controller/cmd_vel";
    let traveling = "/controller/traveling";

    let hook_session = session.clone();
    let bridge_url = mock.url.clone();
    let hook =
        tokio::spawn(async move { hook_session.session_end_hook_stop(Some(&bridge_url)).await });

    // Negative window, guaranteed by the held gate: the hook must still be
    // waiting on `stop_lock`, and the in-flight flag must still be set.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !hook.is_finished(),
        "expected the hook to still be waiting on stop_lock while the gate is held"
    );
    assert!(
        session.stop_in_flight(),
        "expected the in-flight flag to still be set while the gate is held"
    );

    gate.notify_one();
    let hook_report =
        tokio::time::timeout(Duration::from_secs(ROS2_SESSION_END_TIMEOUT_SECS), hook)
            .await
            .expect("hook must return within ROS2_SESSION_END_TIMEOUT_SECS")
            .expect("hook task must not panic");

    assert!(
        hook_report.contains(STOP_COALESCED_NOTE),
        "got: {hook_report}"
    );
    assert!(
        hook_report.contains("(cancel-all fallback used)"),
        "got: {hook_report}"
    );
    assert!(
        hook_report.contains(GOAL_NOT_CANCELABLE_NOTE),
        "got: {hook_report}"
    );

    wait_until(
        || mock.published_on(traveling).len() >= 3,
        Duration::from_secs(3),
    )
    .await;

    let kinds = stop_frame_kinds(&mock, cmd_vel, traveling);
    assert_eq!(
        kinds,
        vec![
            FrameKind::ZeroTwist,
            FrameKind::ZeroTwist,
            FrameKind::ZeroTwist,
            FrameKind::RestStop,
            FrameKind::RestStop,
            FrameKind::RestStop,
        ],
        "expected exactly 3 ZeroTwist then 3 RestStop on connection 1 (the aborted stop's own run); got {kinds:?}"
    );

    let cancel_call = mock
        .frames_with_op("call_service")
        .into_iter()
        .find(|f| {
            f.value.get("service").and_then(Value::as_str) == Some("/alpha/_action/cancel_goal")
        })
        .expect("expected the cancel-all call_service for /alpha");
    let last_rest_stop_at = mock
        .frames_with_op("publish")
        .into_iter()
        .rfind(|f| f.value.get("topic").and_then(Value::as_str) == Some(traveling))
        .expect("expected at least one RestStop publish")
        .at;
    assert!(
        cancel_call.at > last_rest_stop_at,
        "the cancel-all must be sent after the rest stops"
    );
    assert!(
        !mock
            .frames_with_op("publish")
            .iter()
            .any(|f| f.at > cancel_call.at),
        "no publish frame may follow the cancel-all — the hook sent nothing"
    );

    assert!(
        !session.stop_in_flight(),
        "expected the in-flight flag to be clear once the aborted stop's own run finished"
    );
}

/// 49.8-19 (VERIFICATION gap #7, review 49.8-19): a `begin_motion` racing a
/// caller-dropped stop must wait for that stop's own cleanup — immediately,
/// without waiting for its tail — before installing the new motion.
///
/// RED: the racing `begin_motion` sees the leaked flag, finds `stop_lock`
/// free, and returns immediately — the negative-window assertion
/// (`!m_returned`) fails.
#[tokio::test(flavor = "multi_thread")]
async fn ros2_stop_ordering_motion_racing_dropped_stop_waits_for_its_cleanup() {
    let (mock, session, gate) = dropped_stop_caller_shared_setup().await;
    let cmd_vel = "/controller/cmd_vel";
    let traveling = "/controller/traveling";

    let racing_session = session.clone();
    let m_returned = Arc::new(AtomicBool::new(false));
    let m_returned_task = m_returned.clone();
    let racing = tokio::spawn(async move {
        let result = racing_session
            .begin_motion(cmd_vel, "geometry_msgs/msg/Twist", None)
            .await;
        m_returned_task.store(true, Ordering::SeqCst);
        result
    });

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !m_returned.load(Ordering::SeqCst),
        "expected the racing begin_motion to still be waiting on stop_lock while the gate is held"
    );
    assert!(
        session.stop_in_flight(),
        "expected the in-flight flag to still be set while the gate is held"
    );

    gate.notify_one();
    let (id, token, finished_tx) =
        tokio::time::timeout(Duration::from_secs(ROS2_SESSION_END_TIMEOUT_SECS), racing)
            .await
            .expect("racing begin_motion must return within ROS2_SESSION_END_TIMEOUT_SECS")
            .expect("racing begin_motion task must not panic");

    let client1 = session
        .current_client()
        .await
        .expect("connection 1 must be current after the aborted stop's own reconnect");
    let marker = json!({
        "linear": {"x": 0.4, "y": 0.0, "z": 0.0},
        "angular": {"x": 0.0, "y": 0.0, "z": 0.0},
    });
    client1
        .publish(cmd_vel, marker)
        .await
        .expect("marker publish must not error");
    token.cancel();
    let _ = finished_tx.send(MotionStatus::Finished {
        followup_confirmed: false,
    });
    session.end_motion(id).await;

    // Completion boundary: this same-connection call_service resolves only
    // after the mock has recorded every earlier frame on connection 1.
    client1
        .call_service("/rosapi/topics", None, json!({}), Duration::from_secs(2))
        .await
        .expect("completion-boundary call_service must succeed");

    let kinds = stop_frame_kinds(&mock, cmd_vel, traveling);
    let last_rest_idx = kinds
        .iter()
        .rposition(|k| *k == FrameKind::RestStop)
        .expect("expected at least one RestStop frame");
    assert!(
        kinds[last_rest_idx + 1..]
            .iter()
            .all(|k| *k == FrameKind::NonZeroTwist),
        "nothing but the marker NonZeroTwist may follow the stop's last RestStop; got {kinds:?}"
    );
    assert_eq!(
        kinds[last_rest_idx + 1..]
            .iter()
            .filter(|k| **k == FrameKind::NonZeroTwist)
            .count(),
        1,
        "expected exactly the marker NonZeroTwist after the last RestStop; got {kinds:?}"
    );

    let cancel_call = mock
        .frames_with_op("call_service")
        .into_iter()
        .find(|f| {
            f.value.get("service").and_then(Value::as_str) == Some("/alpha/_action/cancel_goal")
        })
        .expect("expected the cancel-all call_service for /alpha");
    let marker_frame = mock
        .frames_with_op("publish")
        .into_iter()
        .rfind(|f| {
            f.value.get("topic").and_then(Value::as_str) == Some(cmd_vel)
                && !is_zero_twist_msg(f.value.get("msg").unwrap_or(&Value::Null))
        })
        .expect("expected the marker NonZeroTwist frame");
    assert!(
        marker_frame.at > cancel_call.at,
        "the marker must land strictly after the cancel-all"
    );
}

/// 49.8-19 (VERIFICATION gap #7, review 49.8-19): tearing down the runtime a
/// stop task is running on, mid-stop, leaves no stale state — the in-flight
/// flag clears, `stop_lock` is free (not wedged), and the invalidated clean
/// record forces the next stop to be a full one. A torn-down stop's
/// remaining frames are never sent, but a stop that completed BEFORE
/// teardown loses nothing.
///
/// RED: the flag leaks true after `drop(rt1)` (the assertion after it
/// fails), and the next hook stop coalesces onto S1's stale clean record.
#[test]
fn ros2_stop_ordering_runtime_teardown_mid_stop_leaves_no_stale_state() {
    let mock_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("mock_rt build must succeed");
    let gate = Arc::new(Notify::new());
    let mock = mock_rt.block_on(MockRosbridge::start_with(MockConfig {
        gate_connection: Some((1, gate.clone())),
        ..Default::default()
    }));
    let session = Ros2Session::new();

    let cmd_vel = "/controller/cmd_vel".to_string();
    let cmd_vel_type = "geometry_msgs/msg/Twist".to_string();
    let traveling = "/controller/traveling".to_string();
    let followup = StopFollowup {
        topic: traveling.clone(),
        msg_type: ROSPIDER_TRAVELING_TYPE.to_string(),
        msg: rospider_rest_stop_msg(),
    };
    let url = mock.url.clone();

    let rt1 = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("rt1 build must succeed");
    rt1.block_on(async {
        let (id, _token, finished_tx) = session
            .begin_motion(&cmd_vel, &cmd_vel_type, Some(&followup))
            .await;
        let _ = finished_tx.send(MotionStatus::Finished {
            followup_confirmed: false,
        });
        session.end_motion(id).await;

        let s1_report = session.stop_all(Some(&url)).await;
        assert!(
            !s1_report.contains("failure"),
            "S1 must be a clean stop; got: {s1_report}"
        );

        session
            .current_client()
            .await
            .expect("a client must be connected after S1")
            .close();

        let s2_session = session.clone();
        let s2_url = url.clone();
        tokio::spawn(async move {
            let _ = s2_session.stop_all(Some(&s2_url)).await;
        });

        wait_until(|| mock.connection_count() >= 2, Duration::from_secs(2)).await;
        assert!(
            mock.connection_count() >= 2,
            "expected S2's reconnect to have opened connection 1"
        );
        assert!(
            session.stop_in_flight(),
            "expected the in-flight flag to be set while S2 is parked mid-reconnect"
        );
    });

    // Real runtime teardown while S2 is parked mid-stop (its client-side
    // `connect()` await, gated on the mock side). Dropping the Runtime
    // drops S2's task's live locals, including its StopInFlightGuard and
    // OwnedMutexGuard, exactly like any other future dropped mid-poll.
    drop(rt1);
    gate.notify_one();

    assert!(
        !session.stop_in_flight(),
        "expected the torn-down task's guard to have released the in-flight flag"
    );

    let rt2 = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("rt2 build must succeed");
    let hook_report = rt2.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(ROS2_SESSION_END_TIMEOUT_SECS),
            session.session_end_hook_stop(Some(&url)),
        )
        .await
        .expect(
            "hook must return within ROS2_SESSION_END_TIMEOUT_SECS — stop_lock must not be wedged",
        )
    });
    assert!(
        !hook_report.contains(STOP_COALESCED_NOTE),
        "S2's take must have invalidated S1's clean record, forcing a full resend; got: {hook_report}"
    );
    drop(rt2);

    mock_rt.block_on(wait_until(
        || {
            mock.frames()
                .iter()
                .filter(|f| {
                    f.conn == 2 && f.value.get("op").and_then(Value::as_str) == Some("publish")
                })
                .count()
                >= 6
        },
        Duration::from_secs(3),
    ));

    let conn1_publishes = mock
        .frames()
        .iter()
        .filter(|f| f.conn == 1 && f.value.get("op").and_then(Value::as_str) == Some("publish"))
        .count();
    assert_eq!(
        conn1_publishes, 0,
        "the torn-down stop (S2) must have sent nothing further on connection 1"
    );

    let conn2_kinds: Vec<FrameKind> = mock
        .frames()
        .into_iter()
        .filter(|f| f.conn == 2 && f.value.get("op").and_then(Value::as_str) == Some("publish"))
        .filter_map(|f| {
            let topic = f.value.get("topic").and_then(Value::as_str)?;
            if topic == traveling {
                Some(FrameKind::RestStop)
            } else if topic == cmd_vel {
                let msg = f.value.get("msg").cloned().unwrap_or(Value::Null);
                Some(if is_zero_twist_msg(&msg) {
                    FrameKind::ZeroTwist
                } else {
                    FrameKind::NonZeroTwist
                })
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        conn2_kinds,
        vec![
            FrameKind::ZeroTwist,
            FrameKind::ZeroTwist,
            FrameKind::ZeroTwist,
            FrameKind::RestStop,
            FrameKind::RestStop,
            FrameKind::RestStop,
        ],
        "connection 2 (the hook's full, non-coalesced stop) must show exactly 3 ZeroTwist then 3 RestStop; got {conn2_kinds:?}"
    );
}

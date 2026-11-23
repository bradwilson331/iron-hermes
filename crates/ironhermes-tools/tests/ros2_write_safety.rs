//! Phase 49.8 Plan 04: allowlisted `call_service`, declared-entry
//! field-limit clamping (Task 1, D-04/D-08), and timed motion with a
//! guaranteed stop through cancellation, preemption, and a mid-motion bridge
//! drop (Task 2, D-09/D-10/D-11) — against the in-process mock rosbridge
//! server (D-17).

#[path = "support/mock_rosbridge.rs"]
mod mock_rosbridge;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use ironhermes_core::Config;
use ironhermes_core::config::{Ros2AllowEntry, Ros2Caps};
use ironhermes_tools::registry::ToolRegistry;
use ironhermes_tools::ros2::Ros2ConfigSource;
use mock_rosbridge::{MockConfig, MockRosbridge};
use serde_json::json;

fn allow_entry(name: &str, msg_type: &str) -> Ros2AllowEntry {
    Ros2AllowEntry {
        name: name.to_string(),
        msg_type: msg_type.to_string(),
        limits: BTreeMap::new(),
    }
}

fn allow_entry_with_limits(
    name: &str,
    msg_type: &str,
    limits: BTreeMap<String, [f64; 2]>,
) -> Ros2AllowEntry {
    Ros2AllowEntry {
        name: name.to_string(),
        msg_type: msg_type.to_string(),
        limits,
    }
}

fn fixed_config(
    bridge_url: Option<&str>,
    publish: Vec<Ros2AllowEntry>,
    services: Vec<Ros2AllowEntry>,
) -> Config {
    let mut cfg = Config::default();
    cfg.ros2.bridge_url = bridge_url.map(|s| s.to_string());
    cfg.ros2.publish = publish;
    cfg.ros2.services = services;
    cfg
}

fn motion_config(bridge_url: &str, publish: Vec<Ros2AllowEntry>, caps: Ros2Caps) -> Config {
    let mut cfg = Config::default();
    cfg.ros2.bridge_url = Some(bridge_url.to_string());
    cfg.ros2.publish = publish;
    cfg.ros2.caps = caps;
    cfg
}

fn registry_with_ros2(cfg: Config) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register_ros2_tools(Ros2ConfigSource::Fixed(Arc::new(cfg)), None);
    registry
}

/// `advertise`/`publish` are fire-and-forget rosbridge ops — poll with a short
/// deadline instead of asserting immediately after dispatch (same race
/// `49.8-01-SUMMARY.md`/`49.8-02-SUMMARY.md` documented).
async fn wait_for_frame_count(
    mock: &MockRosbridge,
    op: &str,
    count: usize,
    deadline: std::time::Duration,
) {
    let start = std::time::Instant::now();
    loop {
        if mock.frames_with_op(op).len() >= count || start.elapsed() > deadline {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

// ---------------------------------------------------------------------------
// test_ros2_undeclared_service_blocked (D-04)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn test_ros2_undeclared_service_blocked() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url), vec![], vec![]);
    let registry = registry_with_ros2(cfg);

    let result = registry
        .dispatch(
            "ros2",
            json!({"action": "call_service", "service": "/not_declared"}),
        )
        .await
        .expect("call_service dispatch must not error");

    assert!(
        result.contains("is not permitted"),
        "undeclared call_service must be refused with an 'is not permitted' answer; got: {result}"
    );
    assert_eq!(
        mock.connection_count(),
        0,
        "undeclared call_service must never open a WebSocket connection (D-04 allowlist-before-I/O)"
    );
}

// ---------------------------------------------------------------------------
// ros2_call_service_allowlisted_roundtrip (D-04)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_call_service_allowlisted_roundtrip() {
    let mut service_responses = HashMap::new();
    service_responses.insert("/set_mode".to_string(), json!({"success": true}));
    let mock = MockRosbridge::start_with(MockConfig {
        service_responses,
        ..Default::default()
    })
    .await;

    let mut limits = BTreeMap::new();
    limits.insert("data".to_string(), [0.0, 1.0]);
    let cfg = fixed_config(
        Some(&mock.url),
        vec![],
        vec![allow_entry_with_limits(
            "/set_mode",
            "std_srvs/srv/SetBool",
            limits,
        )],
    );
    let registry = registry_with_ros2(cfg);

    let result = registry
        .dispatch(
            "ros2",
            json!({
                "action": "call_service",
                "service": "/set_mode",
                "type": "std_srvs/srv/SetBool",
                "args": {"data": true}
            }),
        )
        .await
        .expect("call_service dispatch must not error");

    assert!(
        result.contains("result=true") && result.contains("success"),
        "tool result must contain the mock's result and values; got: {result}"
    );

    wait_for_frame_count(&mock, "call_service", 1, std::time::Duration::from_secs(2)).await;
    let frames = mock.frames_with_op("call_service");
    assert_eq!(
        frames.len(),
        1,
        "expected exactly one call_service frame; got: {frames:?}"
    );
    assert_eq!(
        frames[0].value.get("service").and_then(|v| v.as_str()),
        Some("/set_mode")
    );
    assert_eq!(
        frames[0].value.get("type").and_then(|v| v.as_str()),
        Some("std_srvs/srv/SetBool")
    );
    assert!(
        frames[0].value.get("args").is_some(),
        "the call_service frame must carry the args; got: {:?}",
        frames[0].value
    );
}

// ---------------------------------------------------------------------------
// ros2_publish_applies_field_limits (D-08)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_publish_applies_field_limits() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;

    let mut limits = BTreeMap::new();
    limits.insert("position.z".to_string(), [100.0, 160.0]);
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry_with_limits(
            "/pose",
            "geometry_msgs/msg/Pose",
            limits,
        )],
        vec![],
    );
    let registry = registry_with_ros2(cfg);

    let result = registry
        .dispatch(
            "ros2",
            json!({
                "action": "publish",
                "topic": "/pose",
                "type": "geometry_msgs/msg/Pose",
                "msg": {"position": {"z": 300.0}}
            }),
        )
        .await
        .expect("publish dispatch must not error");

    assert!(
        result.contains("Clamped"),
        "the result must name the clamp; got: {result}"
    );

    wait_for_frame_count(&mock, "publish", 1, std::time::Duration::from_secs(2)).await;
    let published = mock.published_on("/pose");
    assert_eq!(
        published.len(),
        1,
        "expected exactly one publish frame; got: {published:?}"
    );
    assert_eq!(
        published[0].get("position").and_then(|p| p.get("z")),
        Some(&json!(160.0)),
        "position.z must be clamped to 160 on the wire; got: {:?}",
        published[0]
    );
}

// ---------------------------------------------------------------------------
// Task 2: timed motion with a guaranteed stop (D-09/D-10/D-11)
// ---------------------------------------------------------------------------

fn linear_x(f: &mock_rosbridge::RecordedFrame) -> Option<f64> {
    f.value
        .get("msg")
        .and_then(|m| m.get("linear"))
        .and_then(|l| l.get("x"))
        .and_then(|v| v.as_f64())
}

fn is_zero_twist(f: &mock_rosbridge::RecordedFrame) -> bool {
    let get = |group: &str, axis: &str| {
        f.value
            .get("msg")
            .and_then(|m| m.get(group))
            .and_then(|g| g.get(axis))
            .and_then(|v| v.as_f64())
            .unwrap_or(f64::NAN)
    };
    for group in ["linear", "angular"] {
        for axis in ["x", "y", "z"] {
            if get(group, axis) != 0.0 {
                return false;
            }
        }
    }
    true
}

async fn wait_until(mut cond: impl FnMut() -> bool, deadline: Duration) {
    let start = std::time::Instant::now();
    loop {
        if cond() || start.elapsed() > deadline {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_ros2_timed_motion_auto_stops() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = motion_config(
        &mock.url,
        vec![allow_entry("/cmd_vel", "geometry_msgs/msg/Twist")],
        Ros2Caps::default(),
    );
    let registry = registry_with_ros2(cfg);

    let result = registry
        .dispatch(
            "ros2",
            json!({
                "action": "publish",
                "topic": "/cmd_vel",
                "type": "geometry_msgs/msg/Twist",
                "msg": {"linear": {"x": 0.5}},
                "duration_secs": 1.0
            }),
        )
        .await
        .expect("publish dispatch must not error");
    assert!(
        result.contains("stop"),
        "result must mention the stop; got: {result}"
    );

    // The dispatch call returns once the client-side `.publish().await` calls
    // for the 3 stop frames complete — that only proves the writes were
    // queued, not that the mock's connection-handler task has read and
    // recorded them yet (same fire-and-forget race documented in
    // 49.8-01/49.8-02). Poll until the last 3 recorded frames are the zero
    // stop before asserting on them.
    wait_until(
        || {
            let f = mock.frames_with_op("publish");
            f.len() >= 3 && f[f.len() - 3..].iter().all(is_zero_twist)
        },
        Duration::from_secs(2),
    )
    .await;

    let frames = mock.frames_with_op("publish");
    assert!(
        frames.len() >= 3,
        "expected at least 3 publish frames; got {}",
        frames.len()
    );

    let non_zero: Vec<_> = frames.iter().filter(|f| linear_x(f) != Some(0.0)).collect();
    assert!(
        (7..=13).contains(&non_zero.len()),
        "expected 7-13 non-zero frames; got {} ({:?})",
        non_zero.len(),
        non_zero.iter().map(|f| &f.value).collect::<Vec<_>>()
    );
    for f in &non_zero {
        assert_eq!(
            linear_x(f),
            Some(0.10),
            "every non-zero frame must be clamped to 0.10; got: {:?}",
            f.value
        );
    }

    let last_three = &frames[frames.len() - 3..];
    for f in last_three {
        assert!(
            is_zero_twist(f),
            "the last 3 frames must be zero Twists; got: {:?}",
            f.value
        );
    }

    let span = last_three[2].at.duration_since(frames[0].at);
    assert!(
        span >= Duration::from_millis(900) && span <= Duration::from_millis(1800),
        "span from first to last zero frame must be within 0.9-1.8s; got {span:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_motion_duration_capped_by_config() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let caps = Ros2Caps {
        max_motion_duration_secs: 0.5,
        ..Ros2Caps::default()
    };
    let cfg = motion_config(
        &mock.url,
        vec![allow_entry("/cmd_vel", "geometry_msgs/msg/Twist")],
        caps,
    );
    let registry = registry_with_ros2(cfg);

    let started = std::time::Instant::now();
    let result = registry
        .dispatch(
            "ros2",
            json!({
                "action": "publish",
                "topic": "/cmd_vel",
                "type": "geometry_msgs/msg/Twist",
                "msg": {"linear": {"x": 0.05}},
                "duration_secs": 60
            }),
        )
        .await
        .expect("publish dispatch must not error");
    let elapsed = started.elapsed();

    assert!(
        elapsed <= Duration::from_millis(900),
        "call must return within 0.9s once config caps duration to 0.5s; took {elapsed:?}"
    );
    assert!(
        result.contains("duration_secs") && result.contains("0.5"),
        "result must name the clamped duration; got: {result}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_motion_cancelled_future_still_stops() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = motion_config(
        &mock.url,
        vec![allow_entry("/cmd_vel", "geometry_msgs/msg/Twist")],
        Ros2Caps::default(),
    );
    let registry = registry_with_ros2(cfg);

    let dispatch_fut = registry.dispatch(
        "ros2",
        json!({
            "action": "publish",
            "topic": "/cmd_vel",
            "type": "geometry_msgs/msg/Twist",
            "msg": {"linear": {"x": 0.5}},
            "duration_secs": 5.0
        }),
    );
    // The outer future is dropped when this timeout elapses — mirrors
    // registry.rs::run_bounded dropping tool.execute(args) on a registry
    // timeout rather than cooperatively cancelling it.
    let _ = tokio::time::timeout(Duration::from_millis(300), dispatch_fut).await;

    tokio::time::sleep(Duration::from_millis(600)).await;

    let frames = mock.frames_with_op("publish");
    assert!(
        !frames.is_empty(),
        "expected at least the stop frames to have been sent"
    );
    let last = frames.last().unwrap();
    assert!(
        is_zero_twist(last),
        "the last recorded /cmd_vel frame must be a zero Twist; got: {:?}",
        last.value
    );

    let last_nonzero_idx = frames.iter().rposition(|f| !is_zero_twist(f));
    if let Some(idx) = last_nonzero_idx {
        assert!(
            frames[idx + 1..].iter().all(is_zero_twist),
            "no non-zero frame may follow the last zero frame"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_motion_preempt_stops_previous() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = motion_config(
        &mock.url,
        vec![allow_entry("/cmd_vel", "geometry_msgs/msg/Twist")],
        Ros2Caps::default(),
    );
    let registry = Arc::new(registry_with_ros2(cfg));

    let registry1 = registry.clone();
    let first = tokio::spawn(async move {
        registry1
            .dispatch(
                "ros2",
                json!({
                    "action": "publish",
                    "topic": "/cmd_vel",
                    "type": "geometry_msgs/msg/Twist",
                    "msg": {"linear": {"x": 0.05}},
                    "duration_secs": 2.0
                }),
            )
            .await
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

    let second_result = registry
        .dispatch(
            "ros2",
            json!({
                "action": "publish",
                "topic": "/cmd_vel",
                "type": "geometry_msgs/msg/Twist",
                "msg": {"linear": {"x": -0.05}},
                "duration_secs": 0.5
            }),
        )
        .await
        .expect("second publish dispatch must not error");
    assert!(
        second_result.contains("stop"),
        "second motion result must mention the stop; got: {second_result}"
    );

    let first_result = first
        .await
        .expect("first dispatch task panicked")
        .expect("first publish dispatch must not error");
    assert!(
        first_result.contains("interrupted") || first_result.contains("stop"),
        "first (preempted) motion result must mention interruption/stop; got: {first_result}"
    );

    let frames = mock.frames_with_op("publish");
    let first_motion_last_nonzero = frames
        .iter()
        .enumerate()
        .filter(|(_, f)| linear_x(f).map(|v| v > 0.0).unwrap_or(false))
        .map(|(i, _)| i)
        .max()
        .expect("first motion must have published at least one positive-x frame");
    let second_motion_first = frames
        .iter()
        .enumerate()
        .find(|(_, f)| linear_x(f).map(|v| v < 0.0).unwrap_or(false))
        .map(|(i, _)| i)
        .expect("second motion must have published at least one negative-x frame");

    assert!(
        first_motion_last_nonzero < second_motion_first,
        "the first motion's last non-zero frame must precede the second motion's first frame; a={first_motion_last_nonzero} b={second_motion_first}"
    );
    let between_zero = frames[first_motion_last_nonzero + 1..second_motion_first]
        .iter()
        .any(is_zero_twist);
    assert!(
        between_zero,
        "at least one zero Twist frame must sit between the two motions; got: {:?}",
        frames[first_motion_last_nonzero..=second_motion_first]
            .iter()
            .map(|f| &f.value)
            .collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_motion_bridge_drop_mid_motion_attempts_stop() {
    let mock = MockRosbridge::start_with(MockConfig {
        drop_after_frames: Some(4),
        ..Default::default()
    })
    .await;
    let cfg = motion_config(
        &mock.url,
        vec![allow_entry("/cmd_vel", "geometry_msgs/msg/Twist")],
        Ros2Caps::default(),
    );
    let registry = registry_with_ros2(cfg);

    let result = registry
        .dispatch(
            "ros2",
            json!({
                "action": "publish",
                "topic": "/cmd_vel",
                "type": "geometry_msgs/msg/Twist",
                "msg": {"linear": {"x": 0.05}},
                "duration_secs": 1.0
            }),
        )
        .await
        .expect("publish dispatch must not error");

    assert!(
        result.contains("connection lost"),
        "result must report the connection loss; got: {result}"
    );

    wait_until(
        || {
            mock.frames_with_op("publish")
                .into_iter()
                .any(|f| f.conn == 1)
        },
        Duration::from_secs(3),
    )
    .await;
    let conn1_publishes: Vec<_> = mock
        .frames_with_op("publish")
        .into_iter()
        .filter(|f| f.conn == 1)
        .collect();
    assert!(
        conn1_publishes.iter().any(is_zero_twist),
        "the mock's connection index 1 must have recorded a zero Twist on /cmd_vel; got: {:?}",
        conn1_publishes.iter().map(|f| &f.value).collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_ros2_yolo_does_not_relax_caps() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let mut cfg = motion_config(
        &mock.url,
        vec![allow_entry("/cmd_vel", "geometry_msgs/msg/Twist")],
        Ros2Caps::default(),
    );
    cfg.autonomous.yolo = true;
    let registry = registry_with_ros2(cfg);

    let undeclared = registry
        .dispatch(
            "ros2",
            json!({
                "action": "publish",
                "topic": "/not_declared",
                "type": "std_msgs/msg/String",
                "msg": {"data": "x"}
            }),
        )
        .await
        .expect("dispatch must not error");
    assert!(
        undeclared.contains("is not permitted"),
        "yolo must not relax the allowlist; got: {undeclared}"
    );
    assert_eq!(
        mock.connection_count(),
        0,
        "yolo must not relax allowlist-before-I/O; a connection was opened"
    );

    let result = registry
        .dispatch(
            "ros2",
            json!({
                "action": "publish",
                "topic": "/cmd_vel",
                "type": "geometry_msgs/msg/Twist",
                "msg": {"linear": {"x": 0.5}},
                "duration_secs": 0.3
            }),
        )
        .await
        .expect("publish dispatch must not error");
    assert!(
        result.contains("stop"),
        "result must mention the stop; got: {result}"
    );

    let frames = mock.frames_with_op("publish");
    assert!(!frames.is_empty());
    let over_cap = frames
        .iter()
        .any(|f| linear_x(f).map(|v| v > 0.10 + 1e-9).unwrap_or(false));
    assert!(
        !over_cap,
        "yolo must not relax the velocity cap; got: {frames:?}"
    );
    let last = frames.last().unwrap();
    assert!(
        is_zero_twist(last),
        "motion must still end with a zero Twist under yolo; got: {:?}",
        last.value
    );
}

// ---------------------------------------------------------------------------
// Gap closure 49.8-12: explicit-zero caps (D-08)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_zero_linear_cap_publishes_only_zero_linear() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let caps = Ros2Caps {
        max_linear_velocity: 0.0,
        ..Ros2Caps::default()
    };
    let cfg = motion_config(
        &mock.url,
        vec![allow_entry("/cmd_vel", "geometry_msgs/msg/Twist")],
        caps,
    );
    let registry = registry_with_ros2(cfg);

    let result = registry
        .dispatch(
            "ros2",
            json!({
                "action": "publish",
                "topic": "/cmd_vel",
                "type": "geometry_msgs/msg/Twist",
                "msg": {"linear": {"x": 0.3}, "angular": {"z": 0.2}},
                "duration_secs": 0.3
            }),
        )
        .await
        .expect("publish dispatch must not error");
    assert!(
        result.contains("linear.x"),
        "result must name the linear.x clamp; got: {result}"
    );

    wait_for_frame_count(&mock, "publish", 1, Duration::from_secs(2)).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let frames = mock.frames_with_op("publish");
    assert!(
        !frames.is_empty(),
        "expected at least one /cmd_vel publish frame"
    );
    assert!(
        frames.iter().all(|f| linear_x(f) == Some(0.0)),
        "every /cmd_vel frame's linear.x must be 0.0 with a zero linear cap; got: {:?}",
        frames.iter().map(|f| &f.value).collect::<Vec<_>>()
    );
    let angular_z = |f: &mock_rosbridge::RecordedFrame| -> Option<f64> {
        f.value
            .get("msg")
            .and_then(|m| m.get("angular"))
            .and_then(|a| a.get("z"))
            .and_then(|v| v.as_f64())
    };
    assert!(
        frames.iter().any(|f| angular_z(f) == Some(0.2)),
        "at least one frame must still carry the requested angular.z 0.2; got: {:?}",
        frames.iter().map(|f| &f.value).collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_zero_subscribe_cap_refuses_without_connecting() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let caps = Ros2Caps {
        max_subscribe_messages: 0,
        ..Ros2Caps::default()
    };
    let cfg = motion_config(&mock.url, vec![], caps);
    let registry = registry_with_ros2(cfg);

    let result = registry
        .dispatch("ros2", json!({"action": "subscribe", "topic": "/chatter"}))
        .await
        .expect("subscribe dispatch must not error");

    assert!(
        result.contains("is not permitted") && result.contains("max_subscribe_messages"),
        "got: {result}"
    );
    assert_eq!(
        mock.connection_count(),
        0,
        "a zero subscribe cap must refuse before any WebSocket connection opens"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rospider_zero_subscribe_cap_refuses_sensor_read() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let caps = Ros2Caps {
        max_subscribe_messages: 0,
        ..Ros2Caps::default()
    };
    let cfg = motion_config(&mock.url, vec![], caps);
    let registry = registry_with_ros2(cfg);

    let result = registry
        .dispatch("rospider", json!({"action": "read_imu"}))
        .await
        .expect("read_imu dispatch must not error");

    assert!(result.contains("is not permitted"), "got: {result}");
    assert_eq!(
        mock.connection_count(),
        0,
        "a zero subscribe cap must refuse rospider sensor reads before any WebSocket connection opens"
    );
}

//! Phase 49.8 Plan 13: G-49.8-2b — a timed ROSPider walk/turn, or a generic
//! `ros2` timed motion on the ROSPider's own `cmd_vel_topic`, ends on the
//! wire with the unchanged zero-Twist burst followed by the gait-0
//! `kinematics_msgs/msg/Traveling` rest stop on the configurable
//! `ros2.rospider.traveling_topic`. On real ROSPider firmware a zero Twist
//! alone leaves the legs stepping in place forever; the rest stop is the
//! actual stop (`move_controller.set_traveling_callback` ->
//! `step_controller.stop_running()`).
//!
//! Mirrors `ros2_rospider.rs`'s own harness (`fixed_config`,
//! `registry_with_rospider`, `wait_until`, `is_zero_twist_msg`) against the
//! in-process mock rosbridge (D-17).

#[path = "support/mock_rosbridge.rs"]
mod mock_rosbridge;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use ironhermes_core::Config;
use ironhermes_core::config::Ros2AllowEntry;
use ironhermes_tools::browser_vision::VisionClientHandle;
use ironhermes_tools::registry::ToolRegistry;
use ironhermes_tools::ros2::Ros2ConfigSource;
use mock_rosbridge::{MockConfig, MockRosbridge};
use serde_json::{Value, json};

fn fixed_config(bridge_url: Option<&str>) -> Config {
    let mut cfg = Config::default();
    cfg.ros2.bridge_url = bridge_url.map(|s| s.to_string());
    cfg
}

fn allow_entry(name: &str, msg_type: &str) -> Ros2AllowEntry {
    Ros2AllowEntry {
        name: name.to_string(),
        msg_type: msg_type.to_string(),
        limits: BTreeMap::new(),
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
/// `49.8-01-SUMMARY.md` through `49.8-12-SUMMARY.md` documented).
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

/// The gait-0 rest stop: `gait == 0` and `time == 1.0`
/// ([`ROSPIDER_REST_STOP_GAIT`]/[`ROSPIDER_REST_STOP_POSE_SECS`] in
/// `rospider_tool.rs`).
fn is_rest_stop(msg: &Value) -> bool {
    msg.get("gait").and_then(Value::as_i64) == Some(0)
        && msg.get("time").and_then(Value::as_f64) == Some(1.0)
}

/// Asserts the last `n` publishes recorded on `topic` are all zero Twists.
fn assert_last_n_zero_twists(mock: &MockRosbridge, topic: &str, n: usize) {
    let publishes = mock.published_on(topic);
    assert!(
        publishes.len() >= n,
        "expected at least {n} publishes on {topic}; got {}",
        publishes.len()
    );
    for msg in &publishes[publishes.len() - n..] {
        assert!(
            is_zero_twist_msg(msg),
            "expected a zero Twist on {topic}; got {msg:?}"
        );
    }
}

/// Asserts exactly `expected` publishes were recorded on `topic`, each a
/// gait-0 rest stop.
fn assert_rest_stops(mock: &MockRosbridge, topic: &str, expected: usize) {
    let publishes = mock.published_on(topic);
    assert_eq!(
        publishes.len(),
        expected,
        "expected {expected} rest-stop publishes on {topic}; got {publishes:?}"
    );
    for msg in &publishes {
        assert!(
            is_rest_stop(msg),
            "expected a gait-0 rest stop on {topic}; got {msg:?}"
        );
    }
}

/// Asserts every publish frame on `traveling_topic` arrives (in
/// `frames_with_op("publish")` order) strictly after the LAST publish frame
/// on `cmd_vel_topic`.
fn assert_traveling_after_cmd_vel(
    mock: &MockRosbridge,
    cmd_vel_topic: &str,
    traveling_topic: &str,
) {
    let publish_frames = mock.frames_with_op("publish");
    let last_cmd_vel_idx = publish_frames
        .iter()
        .rposition(|f| f.value.get("topic").and_then(Value::as_str) == Some(cmd_vel_topic))
        .unwrap_or_else(|| panic!("expected at least one publish frame on {cmd_vel_topic}"));
    let traveling_indices: Vec<usize> = publish_frames
        .iter()
        .enumerate()
        .filter(|(_, f)| f.value.get("topic").and_then(Value::as_str) == Some(traveling_topic))
        .map(|(i, _)| i)
        .collect();
    assert!(
        !traveling_indices.is_empty(),
        "expected at least one publish frame on {traveling_topic}"
    );
    assert!(
        traveling_indices.iter().all(|&i| i > last_cmd_vel_idx),
        "every {traveling_topic} publish must come after the last {cmd_vel_topic} publish; \
         last {cmd_vel_topic} at index {last_cmd_vel_idx}, {traveling_topic} at {traveling_indices:?}"
    );
}

/// Asserts an `advertise` frame for `traveling_topic`/`traveling_type` sits
/// (in overall `frames()` arrival order) BEFORE the first `publish` frame on
/// `cmd_vel_topic` — the traveling topic is pre-advertised before the
/// motion's own republish loop begins.
fn assert_traveling_advertised_before_first_cmd_vel_publish(
    mock: &MockRosbridge,
    cmd_vel_topic: &str,
    traveling_topic: &str,
    traveling_type: &str,
) {
    let frames = mock.frames();
    let advertise_idx = frames
        .iter()
        .position(|f| {
            f.value.get("op").and_then(Value::as_str) == Some("advertise")
                && f.value.get("topic").and_then(Value::as_str) == Some(traveling_topic)
                && f.value.get("type").and_then(Value::as_str) == Some(traveling_type)
        })
        .unwrap_or_else(|| panic!("expected an advertise frame for {traveling_topic}"));
    let first_publish_idx = frames
        .iter()
        .position(|f| {
            f.value.get("op").and_then(Value::as_str) == Some("publish")
                && f.value.get("topic").and_then(Value::as_str) == Some(cmd_vel_topic)
        })
        .unwrap_or_else(|| panic!("expected at least one publish frame on {cmd_vel_topic}"));
    assert!(
        advertise_idx < first_publish_idx,
        "traveling advertise (idx {advertise_idx}) must precede the first {cmd_vel_topic} publish (idx {first_publish_idx})"
    );
}

/// Asserts the first `traveling_topic` publish's arrival time is at least
/// `min_ms` after the last `cmd_vel_topic` publish's arrival time.
fn assert_first_traveling_at_least_ms_after_last_cmd_vel(
    mock: &MockRosbridge,
    cmd_vel_topic: &str,
    traveling_topic: &str,
    min_ms: u64,
) {
    let publish_frames = mock.frames_with_op("publish");
    let last_cmd_vel = publish_frames
        .iter()
        .rev()
        .find(|f| f.value.get("topic").and_then(Value::as_str) == Some(cmd_vel_topic))
        .unwrap_or_else(|| panic!("expected at least one publish frame on {cmd_vel_topic}"));
    let first_traveling = publish_frames
        .iter()
        .find(|f| f.value.get("topic").and_then(Value::as_str) == Some(traveling_topic))
        .unwrap_or_else(|| panic!("expected at least one publish frame on {traveling_topic}"));
    assert!(
        first_traveling.at.duration_since(last_cmd_vel.at) >= Duration::from_millis(min_ms),
        "expected at least {min_ms}ms between the last {cmd_vel_topic} publish and the first {traveling_topic} publish"
    );
}

// ---------------------------------------------------------------------------
// 49.8-13: timed-motion rest stop (G-49.8-2b)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rest_stop_follows_zero_twists_after_rospider_walk() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url));
    let registry = registry_with_rospider(cfg, None);

    let result = registry
        .dispatch(
            "rospider",
            json!({"action": "walk", "direction": "forward", "duration_secs": 0.3}),
        )
        .await
        .expect("walk dispatch must not error");

    wait_until(
        || mock.published_on("/controller/traveling").len() >= 3,
        Duration::from_secs(3),
    )
    .await;

    // (a)
    assert_traveling_advertised_before_first_cmd_vel_publish(
        &mock,
        "/controller/cmd_vel",
        "/controller/traveling",
        "kinematics_msgs/msg/Traveling",
    );
    // (b)
    assert_last_n_zero_twists(&mock, "/controller/cmd_vel", 3);
    // (c)
    assert_rest_stops(&mock, "/controller/traveling", 3);
    // (d)
    assert_traveling_after_cmd_vel(&mock, "/controller/cmd_vel", "/controller/traveling");
    // (e)
    assert_first_traveling_at_least_ms_after_last_cmd_vel(
        &mock,
        "/controller/cmd_vel",
        "/controller/traveling",
        50,
    );
    // (f)
    assert!(result.contains("gait-0 rest stop"), "got: {result}");
    assert!(result.contains("/controller/traveling"), "got: {result}");
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rest_stop_follows_zero_twists_after_rospider_turn() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url));
    let registry = registry_with_rospider(cfg, None);

    let _result = registry
        .dispatch(
            "rospider",
            json!({"action": "turn", "direction": "left", "duration_secs": 0.3}),
        )
        .await
        .expect("turn dispatch must not error");

    wait_until(
        || mock.published_on("/controller/traveling").len() >= 3,
        Duration::from_secs(3),
    )
    .await;

    // (b)
    assert_last_n_zero_twists(&mock, "/controller/cmd_vel", 3);
    // (c)
    assert_rest_stops(&mock, "/controller/traveling", 3);
    // (d)
    assert_traveling_after_cmd_vel(&mock, "/controller/cmd_vel", "/controller/traveling");
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rest_stop_topic_configurable_and_disableable() {
    // (i) configurable target
    {
        let mock = MockRosbridge::start_with(MockConfig::default()).await;
        let mut cfg = fixed_config(Some(&mock.url));
        cfg.ros2.rospider.traveling_topic = "/robot1/traveling".to_string();
        let registry = registry_with_rospider(cfg, None);

        let _result = registry
            .dispatch(
                "rospider",
                json!({"action": "walk", "direction": "forward", "duration_secs": 0.2}),
            )
            .await
            .expect("walk dispatch must not error");

        wait_until(
            || mock.published_on("/robot1/traveling").len() >= 3,
            Duration::from_secs(3),
        )
        .await;

        assert_rest_stops(&mock, "/robot1/traveling", 3);
        assert!(
            mock.published_on("/controller/traveling").is_empty(),
            "expected nothing published on the default traveling topic when it's overridden"
        );
    }

    // (ii) disabled
    {
        let mock = MockRosbridge::start_with(MockConfig::default()).await;
        let mut cfg = fixed_config(Some(&mock.url));
        cfg.ros2.rospider.traveling_topic = "".to_string();
        let registry = registry_with_rospider(cfg, None);

        let result = registry
            .dispatch(
                "rospider",
                json!({"action": "walk", "direction": "forward", "duration_secs": 0.2}),
            )
            .await
            .expect("walk dispatch must not error");

        wait_until(
            || {
                let msgs = mock.published_on("/controller/cmd_vel");
                !msgs.is_empty() && is_zero_twist_msg(msgs.last().unwrap())
            },
            Duration::from_secs(3),
        )
        .await;

        assert!(
            mock.frames().iter().all(|f| {
                f.value
                    .get("topic")
                    .and_then(Value::as_str)
                    .map(|t| !t.contains("traveling"))
                    .unwrap_or(true)
            }),
            "expected no advertise or publish frame naming any topic containing 'traveling'"
        );
        assert_last_n_zero_twists(&mock, "/controller/cmd_vel", 3);
        assert!(result.contains("rest stop disabled"), "got: {result}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rest_stop_on_generic_motion_on_rospider_cmd_vel() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let mut cfg = fixed_config(Some(&mock.url));
    cfg.ros2.publish = vec![allow_entry(
        "/controller/cmd_vel",
        "geometry_msgs/msg/Twist",
    )];
    let registry = registry_with_rospider(cfg, None);

    let result = registry
        .dispatch(
            "ros2",
            json!({
                "action": "publish",
                "topic": "/controller/cmd_vel",
                "type": "geometry_msgs/msg/Twist",
                "msg": {"linear": {"x": 0.05}},
                "duration_secs": 0.3
            }),
        )
        .await
        .expect("publish dispatch must not error");

    wait_until(
        || mock.published_on("/controller/traveling").len() >= 3,
        Duration::from_secs(3),
    )
    .await;

    // (b)
    assert_last_n_zero_twists(&mock, "/controller/cmd_vel", 3);
    // (c)
    assert_rest_stops(&mock, "/controller/traveling", 3);
    // (d)
    assert_traveling_after_cmd_vel(&mock, "/controller/cmd_vel", "/controller/traveling");
    assert!(result.contains("gait-0 rest stop"), "got: {result}");
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rest_stop_absent_for_generic_motion_on_other_topics() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let mut cfg = fixed_config(Some(&mock.url));
    cfg.ros2.publish = vec![allow_entry("/cmd_vel", "geometry_msgs/msg/Twist")];
    let registry = registry_with_rospider(cfg, None);

    let _result = registry
        .dispatch(
            "ros2",
            json!({
                "action": "publish",
                "topic": "/cmd_vel",
                "type": "geometry_msgs/msg/Twist",
                "msg": {"linear": {"x": 0.05}},
                "duration_secs": 0.3
            }),
        )
        .await
        .expect("publish dispatch must not error");

    wait_until(
        || {
            let msgs = mock.published_on("/cmd_vel");
            !msgs.is_empty() && is_zero_twist_msg(msgs.last().unwrap())
        },
        Duration::from_secs(3),
    )
    .await;

    assert!(
        mock.frames()
            .iter()
            .all(|f| f.value.get("topic").and_then(Value::as_str) != Some("/controller/traveling")),
        "expected no advertise or publish frame on /controller/traveling — existing generic behavior is unchanged"
    );

    let publish_frames = mock.frames_with_op("publish");
    let last = publish_frames
        .last()
        .expect("expected at least one publish frame");
    assert!(
        is_zero_twist_msg(last.value.get("msg").unwrap_or(&Value::Null)),
        "expected the last publish frame overall to be a zero Twist; got {:?}",
        last.value
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rest_stop_resent_after_mid_walk_bridge_drop() {
    let mock = MockRosbridge::start_with(MockConfig {
        drop_after_frames: Some(5),
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(Some(&mock.url));
    let registry = registry_with_rospider(cfg, None);

    let result = registry
        .dispatch(
            "rospider",
            json!({"action": "walk", "direction": "forward", "duration_secs": 1.0}),
        )
        .await
        .expect("walk dispatch must not error");

    assert!(result.contains("connection lost"), "got: {result}");

    wait_until(
        || {
            mock.frames_with_op("publish").into_iter().any(|f| {
                f.conn == 1
                    && f.value.get("topic").and_then(Value::as_str) == Some("/controller/traveling")
            })
        },
        Duration::from_secs(3),
    )
    .await;

    let all_publishes = mock.frames_with_op("publish");
    let conn1: Vec<_> = all_publishes.iter().filter(|f| f.conn == 1).collect();

    assert!(
        conn1.iter().any(|f| {
            f.value.get("topic").and_then(Value::as_str) == Some("/controller/cmd_vel")
                && is_zero_twist_msg(f.value.get("msg").unwrap_or(&Value::Null))
        }),
        "expected connection index 1 to record a zero Twist on /controller/cmd_vel; got: {:?}",
        conn1.iter().map(|f| &f.value).collect::<Vec<_>>()
    );
    assert!(
        conn1.iter().any(|f| {
            f.value.get("topic").and_then(Value::as_str) == Some("/controller/traveling")
                && is_rest_stop(f.value.get("msg").unwrap_or(&Value::Null))
        }),
        "expected connection index 1 to record a gait-0 rest stop on /controller/traveling; got: {:?}",
        conn1.iter().map(|f| &f.value).collect::<Vec<_>>()
    );

    let last_conn1_cmd_vel_idx = conn1
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            f.value.get("topic").and_then(Value::as_str) == Some("/controller/cmd_vel")
        })
        .map(|(i, _)| i)
        .next_back()
        .expect("expected at least one conn-1 cmd_vel publish");
    let traveling_conn1_indices: Vec<usize> = conn1
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            f.value.get("topic").and_then(Value::as_str) == Some("/controller/traveling")
        })
        .map(|(i, _)| i)
        .collect();
    assert!(
        traveling_conn1_indices
            .iter()
            .all(|&i| i > last_conn1_cmd_vel_idx),
        "every conn-1 traveling publish must come after the last conn-1 cmd_vel publish"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rest_stop_topic_not_writable_via_generic_publish() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    // Default config: ros2.publish is empty (deny-by-default, D-04).
    let cfg = fixed_config(Some(&mock.url));
    let registry = registry_with_rospider(cfg, None);

    let result = registry
        .dispatch(
            "ros2",
            json!({
                "action": "publish",
                "topic": "/controller/traveling",
                "type": "kinematics_msgs/msg/Traveling",
                "msg": {"gait": 10}
            }),
        )
        .await
        .expect("publish dispatch must not error");

    assert!(result.contains("is not permitted"), "got: {result}");
    assert_eq!(
        mock.connection_count(),
        0,
        "an undeclared write must never open a socket (D-04)"
    );
}

// ---------------------------------------------------------------------------
// 49.8-14: stop and session-end rest stop (G-49.8-2b)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rest_stop_ends_rospider_stop_mid_walk() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url));
    let registry = Arc::new(registry_with_rospider(cfg, None));

    // 49.8-15 test-determinism repair: `ToolRegistry::dispatch` is a lazy
    // `async fn` — a pinned but unpolled future never starts, so the
    // original version of this test's stop ran with nothing driven and
    // never actually interrupted a walk. Spawning it over an `Arc`'d
    // registry, then hard-asserting a non-zero Twist was recorded, proves
    // the walk was really running before the stop.
    let registry_walk = registry.clone();
    let walk_handle = tokio::spawn(async move {
        registry_walk
            .dispatch(
                "rospider",
                json!({"action": "walk", "direction": "forward", "duration_secs": 2.0}),
            )
            .await
    });

    wait_until(
        || {
            mock.published_on("/controller/cmd_vel")
                .iter()
                .any(|m| !is_zero_twist_msg(m))
        },
        Duration::from_secs(2),
    )
    .await;
    assert!(
        mock.published_on("/controller/cmd_vel")
            .iter()
            .any(|m| !is_zero_twist_msg(m)),
        "expected at least one non-zero Twist recorded before stopping — the walk never started"
    );

    let stop_result = registry
        .dispatch("rospider", json!({"action": "stop"}))
        .await
        .expect("stop dispatch must not error");

    let _ = tokio::time::timeout(Duration::from_secs(3), walk_handle).await;

    wait_until(
        || {
            mock.frames_with_op("publish")
                .last()
                .map(|f| {
                    f.value.get("topic").and_then(Value::as_str) == Some("/controller/traveling")
                        && is_rest_stop(f.value.get("msg").unwrap_or(&Value::Null))
                })
                .unwrap_or(false)
        },
        Duration::from_secs(3),
    )
    .await;

    let publish_frames = mock.frames_with_op("publish");
    let last = publish_frames
        .last()
        .expect("expected at least one publish frame");
    assert_eq!(
        last.value.get("topic").and_then(Value::as_str),
        Some("/controller/traveling"),
        "expected the last publish frame overall to be on /controller/traveling; got {:?}",
        last.value
    );
    assert!(
        is_rest_stop(last.value.get("msg").unwrap_or(&Value::Null)),
        "expected the last publish frame to be a gait-0 rest stop; got {:?}",
        last.value
    );

    let last_cmd_vel = publish_frames
        .iter()
        .rev()
        .find(|f| f.value.get("topic").and_then(Value::as_str) == Some("/controller/cmd_vel"))
        .expect("expected at least one /controller/cmd_vel publish");
    assert!(
        is_zero_twist_msg(last_cmd_vel.value.get("msg").unwrap_or(&Value::Null)),
        "expected the last /controller/cmd_vel publish to be a zero Twist; got {:?}",
        last_cmd_vel.value
    );
    assert!(
        last.at.duration_since(last_cmd_vel.at) >= Duration::from_millis(50),
        "expected the rest stop to arrive at least 50ms after the last cmd_vel publish"
    );

    assert!(
        stop_result.contains("gait-0 rest stop sent"),
        "got: {stop_result}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rest_stop_sent_by_rospider_stop_with_nothing_driven() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url));
    let registry = registry_with_rospider(cfg, None);

    let result = registry
        .dispatch("rospider", json!({"action": "stop"}))
        .await
        .expect("stop dispatch must not error");

    assert!(result.contains("nothing to stop"), "got: {result}");
    assert!(result.contains("gait-0 rest stop sent"), "got: {result}");

    wait_until(
        || {
            mock.published_on("/controller/traveling")
                .iter()
                .any(is_rest_stop)
        },
        Duration::from_secs(2),
    )
    .await;

    assert_eq!(mock.connection_count(), 1);
    assert!(
        mock.frames().iter().any(|f| {
            f.value.get("op").and_then(Value::as_str) == Some("advertise")
                && f.value.get("topic").and_then(Value::as_str) == Some("/controller/traveling")
                && f.value.get("type").and_then(Value::as_str)
                    == Some("kinematics_msgs/msg/Traveling")
        }),
        "expected an advertise frame for /controller/traveling"
    );
    assert!(
        mock.published_on("/controller/traveling")
            .iter()
            .any(is_rest_stop),
        "expected at least one rest-stop publish on /controller/traveling"
    );
    assert!(
        mock.published_on("/controller/cmd_vel").is_empty(),
        "expected zero /controller/cmd_vel publishes when nothing was driven"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rest_stop_replayed_by_rospider_on_session_end() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url));
    let registry = registry_with_rospider(cfg, None);

    registry
        .dispatch(
            "rospider",
            json!({"action": "walk", "direction": "forward", "duration_secs": 0.3}),
        )
        .await
        .expect("walk dispatch must not error");

    // The walk's own natural stop (zero-Twist burst then follow-up burst)
    // already completed by the time the dispatch above returned, but the
    // mock's connection-handler task may not have finished RECORDING every
    // frame yet (the same fire-and-forget race other tests in this file
    // document). Wait for it to fully land before capturing `marker`, or a
    // straggling frame from THIS walk's own stop could arrive after the
    // marker and be mistaken for on_session_end's replay.
    wait_until(
        || mock.published_on("/controller/traveling").len() >= 3,
        Duration::from_secs(3),
    )
    .await;

    let marker = std::time::Instant::now();
    let tool = registry
        .get_arc("rospider")
        .expect("rospider tool must be registered");
    tool.on_session_end();

    wait_until(
        || {
            mock.frames_with_op("publish").iter().any(|f| {
                f.at > marker
                    && f.value.get("topic").and_then(Value::as_str) == Some("/controller/traveling")
                    && is_rest_stop(f.value.get("msg").unwrap_or(&Value::Null))
            })
        },
        Duration::from_secs(2),
    )
    .await;

    let post_marker: Vec<_> = mock
        .frames_with_op("publish")
        .into_iter()
        .filter(|f| f.at > marker)
        .collect();

    assert!(
        post_marker.iter().any(|f| {
            f.value.get("topic").and_then(Value::as_str) == Some("/controller/cmd_vel")
                && is_zero_twist_msg(f.value.get("msg").unwrap_or(&Value::Null))
        }),
        "expected a post-marker zero Twist on /controller/cmd_vel; got: {:?}",
        post_marker.iter().map(|f| &f.value).collect::<Vec<_>>()
    );

    let last_cmd_vel_idx = post_marker
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            f.value.get("topic").and_then(Value::as_str) == Some("/controller/cmd_vel")
        })
        .map(|(i, _)| i)
        .next_back()
        .expect("expected at least one post-marker cmd_vel publish");
    let last_traveling_idx = post_marker
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            f.value.get("topic").and_then(Value::as_str) == Some("/controller/traveling")
        })
        .map(|(i, _)| i)
        .next_back()
        .expect("expected at least one post-marker traveling publish");
    assert!(
        last_traveling_idx > last_cmd_vel_idx,
        "expected the last post-marker traveling publish to come after the last post-marker cmd_vel publish"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rest_stop_replayed_by_generic_on_session_end() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url));
    let registry = registry_with_rospider(cfg, None);

    registry
        .dispatch(
            "rospider",
            json!({"action": "walk", "direction": "forward", "duration_secs": 0.3}),
        )
        .await
        .expect("walk dispatch must not error");

    // See the identical comment in
    // `ros2_rest_stop_replayed_by_rospider_on_session_end` above: wait for
    // the walk's own natural stop to fully land (mock-side recording, not
    // just client-side send) before capturing `marker`.
    wait_until(
        || mock.published_on("/controller/traveling").len() >= 3,
        Duration::from_secs(3),
    )
    .await;

    let marker = std::time::Instant::now();
    let tool = registry
        .get_arc("ros2")
        .expect("ros2 tool must be registered");
    tool.on_session_end();

    wait_until(
        || {
            mock.frames_with_op("publish").iter().any(|f| {
                f.at > marker
                    && f.value.get("topic").and_then(Value::as_str) == Some("/controller/traveling")
                    && is_rest_stop(f.value.get("msg").unwrap_or(&Value::Null))
            })
        },
        Duration::from_secs(2),
    )
    .await;

    let post_marker: Vec<_> = mock
        .frames_with_op("publish")
        .into_iter()
        .filter(|f| f.at > marker)
        .collect();
    assert!(
        post_marker.iter().any(|f| {
            f.value.get("topic").and_then(Value::as_str) == Some("/controller/cmd_vel")
                && is_zero_twist_msg(f.value.get("msg").unwrap_or(&Value::Null))
        }),
        "expected a post-marker zero Twist on /controller/cmd_vel"
    );
    assert!(
        post_marker.iter().any(|f| {
            f.value.get("topic").and_then(Value::as_str) == Some("/controller/traveling")
                && is_rest_stop(f.value.get("msg").unwrap_or(&Value::Null))
        }),
        "expected a post-marker rest stop on /controller/traveling; this proves the generic \
         session end invokes the ROSPider rest stop through the shared session"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rest_stop_reported_by_end_session_stop() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url));
    let registry = registry_with_rospider(cfg, None);

    registry
        .dispatch(
            "rospider",
            json!({"action": "walk", "direction": "forward", "duration_secs": 0.3}),
        )
        .await
        .expect("walk dispatch must not error");

    let marker = std::time::Instant::now();
    let rw = tokio::sync::RwLock::new(registry);
    let report = ironhermes_tools::ros2::end_session_stop(&rw).await;

    assert!(report.contains("stopped 1/1 topic(s)"), "got: {report}");
    assert!(
        report.contains("gait-0 rest stop sent on 1/1"),
        "got: {report}"
    );

    wait_until(
        || {
            mock.frames_with_op("publish").iter().any(|f| {
                f.at > marker
                    && f.value.get("topic").and_then(Value::as_str) == Some("/controller/traveling")
                    && is_rest_stop(f.value.get("msg").unwrap_or(&Value::Null))
            })
        },
        Duration::from_secs(2),
    )
    .await;

    let _ = rw.into_inner();
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rest_stop_replayed_on_reconnect_after_drop() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url));
    let registry = registry_with_rospider(cfg, None);

    registry
        .dispatch(
            "rospider",
            json!({"action": "walk", "direction": "forward", "duration_secs": 0.3}),
        )
        .await
        .expect("walk dispatch must not error");

    let session = registry.ros2_session();
    session
        .current_client()
        .await
        .expect("a client must be connected after the walk above")
        .close();

    let report = session.stop_all(Some(&mock.url)).await;
    assert!(report.contains("gait-0 rest stop sent"), "got: {report}");

    wait_until(
        || {
            mock.frames_with_op("publish").into_iter().any(|f| {
                f.conn == 1
                    && f.value.get("topic").and_then(Value::as_str) == Some("/controller/traveling")
            })
        },
        Duration::from_secs(2),
    )
    .await;

    let all_publishes = mock.frames_with_op("publish");
    let conn1: Vec<_> = all_publishes.iter().filter(|f| f.conn == 1).collect();

    assert!(
        conn1.iter().any(|f| {
            f.value.get("topic").and_then(Value::as_str) == Some("/controller/cmd_vel")
                && is_zero_twist_msg(f.value.get("msg").unwrap_or(&Value::Null))
        }),
        "expected connection index 1 to record a zero Twist on /controller/cmd_vel; got: {:?}",
        conn1.iter().map(|f| &f.value).collect::<Vec<_>>()
    );
    assert!(
        conn1.iter().any(|f| {
            f.value.get("topic").and_then(Value::as_str) == Some("/controller/traveling")
                && is_rest_stop(f.value.get("msg").unwrap_or(&Value::Null))
        }),
        "expected connection index 1 to record a gait-0 rest stop on /controller/traveling; got: {:?}",
        conn1.iter().map(|f| &f.value).collect::<Vec<_>>()
    );

    let last_conn1_cmd_vel_idx = conn1
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            f.value.get("topic").and_then(Value::as_str) == Some("/controller/cmd_vel")
        })
        .map(|(i, _)| i)
        .next_back()
        .expect("expected at least one conn-1 cmd_vel publish");
    let traveling_conn1_indices: Vec<usize> = conn1
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            f.value.get("topic").and_then(Value::as_str) == Some("/controller/traveling")
        })
        .map(|(i, _)| i)
        .collect();
    assert!(
        traveling_conn1_indices
            .iter()
            .all(|&i| i > last_conn1_cmd_vel_idx),
        "every conn-1 traveling publish must come after the last conn-1 cmd_vel publish"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rest_stop_absent_at_session_end_for_generic_only_motion() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let mut cfg = fixed_config(Some(&mock.url));
    cfg.ros2.publish = vec![allow_entry("/cmd_vel", "geometry_msgs/msg/Twist")];
    let registry = registry_with_rospider(cfg, None);

    registry
        .dispatch(
            "ros2",
            json!({
                "action": "publish",
                "topic": "/cmd_vel",
                "type": "geometry_msgs/msg/Twist",
                "msg": {"linear": {"x": 0.05}},
                "duration_secs": 0.3
            }),
        )
        .await
        .expect("publish dispatch must not error");

    let rw = tokio::sync::RwLock::new(registry);
    let report = ironhermes_tools::ros2::end_session_stop(&rw).await;

    assert!(!report.contains("rest stop"), "got: {report}");
    assert!(
        mock.frames()
            .iter()
            .all(|f| f.value.get("topic").and_then(Value::as_str) != Some("/controller/traveling")),
        "expected no advertise or publish frame on /controller/traveling"
    );

    let _ = rw.into_inner();
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rest_stop_session_end_noop_when_unused() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url));
    let registry = registry_with_rospider(cfg, None);

    let tool = registry
        .get_arc("rospider")
        .expect("rospider tool must be registered");
    tool.on_session_end();

    tokio::time::sleep(Duration::from_millis(300)).await;

    assert_eq!(
        mock.connection_count(),
        0,
        "an idle session's on_session_end must never open a connection"
    );
}

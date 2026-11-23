//! Phase 49.8 Plan 09: ROSPider verb tests — motion/body/camera-pan (Task 1)
//! and sensor/camera-snapshot reads (Task 2) — against the in-process mock
//! rosbridge server (D-17), mirroring `ros2_actions.rs`/`ros2_image.rs`'s own
//! harness patterns exactly (same `wait_until` fire-and-forget-op poll, same
//! `MockVision` shape).

#[path = "support/mock_rosbridge.rs"]
mod mock_rosbridge;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use ironhermes_core::Config;
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
/// `49.8-01-SUMMARY.md` through `49.8-08-SUMMARY.md` documented).
async fn wait_until(mut cond: impl FnMut() -> bool, deadline: Duration) -> bool {
    let start = std::time::Instant::now();
    loop {
        if cond() {
            return true;
        }
        if start.elapsed() > deadline {
            return false;
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

/// `(prompt, image_data_url)` pairs recorded by a [`MockVision`].
type RecordedCalls = Arc<Mutex<Vec<(String, String)>>>;

struct MockVision {
    calls: RecordedCalls,
    answer: String,
}

#[async_trait]
impl VisionClientHandle for MockVision {
    async fn vision_call(&self, prompt: String, image_data_url: String) -> anyhow::Result<String> {
        self.calls
            .lock()
            .expect("calls mutex poisoned")
            .push((prompt, image_data_url));
        Ok(self.answer.clone())
    }
}

fn mock_vision(answer: &str) -> (Arc<MockVision>, RecordedCalls) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    (
        Arc::new(MockVision {
            calls: calls.clone(),
            answer: answer.to_string(),
        }),
        calls,
    )
}

fn rgb8_image_msg(width: u32, height: u32, fill: [u8; 3]) -> Value {
    let mut data = Vec::with_capacity((width * height * 3) as usize);
    for _ in 0..(width * height) {
        data.extend_from_slice(&fill);
    }
    json!({
        "width": width,
        "height": height,
        "step": width * 3,
        "encoding": "rgb8",
        "data": base64::engine::general_purpose::STANDARD.encode(&data),
    })
}

fn small_jpeg_compressed_msg(width: u32, height: u32) -> Value {
    let mut jpeg_bytes = Vec::new();
    let img = image::RgbImage::from_fn(width, height, |_, _| image::Rgb([30, 60, 90]));
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_bytes, 90)
        .encode_image(&img)
        .expect("test-fixture JPEG encode must succeed");
    json!({
        "format": "jpeg",
        "data": base64::engine::general_purpose::STANDARD.encode(&jpeg_bytes),
    })
}

// ---------------------------------------------------------------------------
// Task 1 — motion / body / camera-pan
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rospider_walk_timed_and_capped() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url));
    let registry = registry_with_rospider(cfg, None);

    let result = registry
        .dispatch(
            "rospider",
            json!({"action": "walk", "direction": "forward", "speed": 1.0, "duration_secs": 0.5}),
        )
        .await
        .expect("walk dispatch must not error");

    wait_until(
        || !mock.published_on("/controller/cmd_vel").is_empty(),
        Duration::from_secs(3),
    )
    .await;
    let frames = mock.published_on("/controller/cmd_vel");
    assert!(
        !frames.is_empty(),
        "expected at least one published Twist; got: {frames:?}"
    );
    assert_eq!(
        frames[0]["linear"]["x"],
        json!(0.10),
        "default ros2.caps.max_linear_velocity (0.10) should clamp requested speed 1.0; got: {:?}",
        frames[0]
    );

    wait_until(
        || {
            let f = mock.published_on("/controller/cmd_vel");
            f.len() >= 3 && f.iter().rev().take(3).all(is_zero_twist_msg)
        },
        Duration::from_secs(2),
    )
    .await;
    let frames = mock.published_on("/controller/cmd_vel");
    let last3 = &frames[frames.len().saturating_sub(3)..];
    assert_eq!(
        last3.len(),
        3,
        "expected the trailing 3 zero-Twist stop frames; got: {frames:?}"
    );
    assert!(last3.iter().all(is_zero_twist_msg), "got: {last3:?}");

    assert!(
        result.contains("Clamped"),
        "expected a clamp note; got: {result}"
    );
    assert!(
        result.contains("stop"),
        "expected the result to mention stop; got: {result}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rospider_walk_directions() {
    for direction in ["backward", "left", "right"] {
        let mock = MockRosbridge::start_with(MockConfig::default()).await;
        let cfg = fixed_config(Some(&mock.url));
        let registry = registry_with_rospider(cfg, None);

        registry
            .dispatch(
                "rospider",
                json!({"action": "walk", "direction": direction, "speed": 0.05, "duration_secs": 0.2}),
            )
            .await
            .expect("walk dispatch must not error");

        wait_until(
            || !mock.published_on("/controller/cmd_vel").is_empty(),
            Duration::from_secs(3),
        )
        .await;
        let frames = mock.published_on("/controller/cmd_vel");
        assert!(
            !frames.is_empty(),
            "direction {direction}: expected at least one frame"
        );
        let first = &frames[0];
        let x = first["linear"]["x"].as_f64().unwrap_or(0.0);
        let y = first["linear"]["y"].as_f64().unwrap_or(0.0);
        match direction {
            "backward" => assert!(
                x < 0.0,
                "direction backward: expected linear.x < 0; got {first:?}"
            ),
            "left" => assert!(
                y > 0.0,
                "direction left: expected linear.y > 0; got {first:?}"
            ),
            "right" => assert!(
                y < 0.0,
                "direction right: expected linear.y < 0; got {first:?}"
            ),
            _ => unreachable!(),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rospider_turn_left_positive_z() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url));
    let registry = registry_with_rospider(cfg, None);
    registry
        .dispatch(
            "rospider",
            json!({"action": "turn", "direction": "left", "rate": 0.3, "duration_secs": 0.2}),
        )
        .await
        .expect("turn dispatch must not error");
    wait_until(
        || !mock.published_on("/controller/cmd_vel").is_empty(),
        Duration::from_secs(3),
    )
    .await;
    let frames = mock.published_on("/controller/cmd_vel");
    assert!(!frames.is_empty());
    assert_eq!(
        frames[0]["angular"]["z"],
        json!(0.3),
        "got: {:?}",
        frames[0]
    );

    let mock2 = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg2 = fixed_config(Some(&mock2.url));
    let registry2 = registry_with_rospider(cfg2, None);
    registry2
        .dispatch(
            "rospider",
            json!({"action": "turn", "direction": "right", "rate": 0.3, "duration_secs": 0.2}),
        )
        .await
        .expect("turn dispatch must not error");
    wait_until(
        || !mock2.published_on("/controller/cmd_vel").is_empty(),
        Duration::from_secs(3),
    )
    .await;
    let frames2 = mock2.published_on("/controller/cmd_vel");
    assert!(!frames2.is_empty());
    assert_eq!(
        frames2[0]["angular"]["z"],
        json!(-0.3),
        "got: {:?}",
        frames2[0]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rospider_set_pose_clamped() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url));
    let registry = registry_with_rospider(cfg, None);

    let result = registry
        .dispatch(
            "rospider",
            json!({"action": "set_pose", "height": 300, "roll": 1.0, "yaw": -1.0}),
        )
        .await
        .expect("set_pose dispatch must not error");

    wait_until(
        || !mock.published_on("/controller/set_pose_euler").is_empty(),
        Duration::from_secs(2),
    )
    .await;

    let advertise = mock.frames_with_op("advertise");
    assert!(
        advertise.iter().any(|f| {
            f.value.get("topic").and_then(|v| v.as_str()) == Some("/controller/set_pose_euler")
                && f.value.get("type").and_then(|v| v.as_str()) == Some("kinematics_msgs/msg/Pose")
        }),
        "expected an advertise for /controller/set_pose_euler [kinematics_msgs/msg/Pose]; got: {advertise:?}"
    );

    let published = mock.published_on("/controller/set_pose_euler");
    assert_eq!(
        published.len(),
        1,
        "expected exactly one publish; got: {published:?}"
    );
    assert_eq!(published[0]["position"]["z"], json!(160.0));
    assert_eq!(published[0]["orientation"]["roll"], json!(0.2));
    assert_eq!(published[0]["orientation"]["yaw"], json!(-0.4));
    assert!(result.contains("Clamped"), "got: {result}");
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rospider_body_height() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url));
    let registry = registry_with_rospider(cfg, None);

    registry
        .dispatch("rospider", json!({"action": "body_height", "height": 50}))
        .await
        .expect("body_height dispatch must not error");

    wait_until(
        || !mock.published_on("/controller/set_pose_euler").is_empty(),
        Duration::from_secs(2),
    )
    .await;
    let published = mock.published_on("/controller/set_pose_euler");
    assert_eq!(published.len(), 1, "got: {published:?}");
    assert_eq!(published[0]["position"]["z"], json!(100.0));
    for axis in ["roll", "pitch", "yaw"] {
        assert_eq!(published[0]["orientation"][axis], json!(0.0), "axis {axis}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rospider_gait_cmd_param() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url));
    let registry = registry_with_rospider(cfg, None);

    registry
        .dispatch(
            "rospider",
            json!({"action": "gait", "gait": "tripod", "step_height": 99, "period": 5, "pose": "SLAM_POSE"}),
        )
        .await
        .expect("gait dispatch must not error");

    wait_until(
        || !mock.published_on("/step_controller/cmd_param").is_empty(),
        Duration::from_secs(2),
    )
    .await;
    let published = mock.published_on("/step_controller/cmd_param");
    assert_eq!(published.len(), 1, "got: {published:?}");
    assert_eq!(published[0]["pose"], json!("SLAM_POSE"));
    assert_eq!(published[0]["gait"], json!(2));
    assert_eq!(published[0]["height"], json!(40));
    assert_eq!(published[0]["period"], json!(2.0));

    let advertise = mock.frames_with_op("advertise");
    assert!(
        advertise.iter().any(
            |f| f.value.get("type").and_then(|v| v.as_str()) == Some("interfaces/msg/CmdParam")
        )
    );

    // Unknown pose returns an Error and publishes nothing.
    let mock2 = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg2 = fixed_config(Some(&mock2.url));
    let registry2 = registry_with_rospider(cfg2, None);
    let result2 = registry2
        .dispatch("rospider", json!({"action": "gait", "pose": "NOT_A_POSE"}))
        .await
        .expect("gait dispatch must not error");
    assert!(result2.starts_with("Error:"), "got: {result2}");
    assert!(mock2.published_on("/step_controller/cmd_param").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rospider_builtin_pose_service() {
    let mut service_responses = HashMap::new();
    service_responses.insert(
        "/controller/set_pose_1".to_string(),
        json!({"result": 0, "message": "ok"}),
    );
    let mock = MockRosbridge::start_with(MockConfig {
        service_responses,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(Some(&mock.url));
    let registry = registry_with_rospider(cfg, None);

    let result = registry
        .dispatch(
            "rospider",
            json!({"action": "builtin_pose", "pose": "DEFAULT_POSE", "duration": 9}),
        )
        .await
        .expect("builtin_pose dispatch must not error");
    assert!(result.contains("result="), "got: {result}");

    let calls = mock.frames_with_op("call_service");
    let call = calls
        .iter()
        .find(|f| f.value.get("service").and_then(|v| v.as_str()) == Some("/controller/set_pose_1"))
        .expect("expected a call_service frame for /controller/set_pose_1");
    assert_eq!(
        call.value.get("type").and_then(|v| v.as_str()),
        Some("kinematics_msgs/srv/SetPose1")
    );
    let args = &call.value["args"];
    assert_eq!(args["pose"], json!("DEFAULT_POSE"));
    assert_eq!(
        args["duration"],
        json!(3.0),
        "duration 9 must clamp to the 3.0s ceiling"
    );
    assert_eq!(args["interrupt"], json!(true));
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rospider_camera_pan_servo_guard() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url));
    let registry = registry_with_rospider(cfg, None);

    registry
        .dispatch("rospider", json!({"action": "camera_pan", "degrees": 100}))
        .await
        .expect("camera_pan dispatch must not error");

    wait_until(
        || !mock.published_on("/servo_controller").is_empty(),
        Duration::from_secs(2),
    )
    .await;
    let published = mock.published_on("/servo_controller");
    assert_eq!(published.len(), 1, "got: {published:?}");
    assert_eq!(published[0]["position_unit"], json!("pulse"));
    let positions = published[0]["position"]
        .as_array()
        .expect("position must be an array");
    assert_eq!(positions.len(), 1);
    assert_eq!(positions[0]["id"], json!(19));
    assert_eq!(
        positions[0]["position"],
        json!(800),
        "100deg clamped to 72deg -> pulse 800; got: {positions:?}"
    );

    // A leg-servo override refuses before any I/O — no new publish on this
    // (or any) topic from the second registry.
    let mut cfg2 = Config::default();
    cfg2.ros2.bridge_url = Some(mock.url.clone());
    cfg2.ros2.rospider.camera_pan_servo_id = 5;
    let registry2 = registry_with_rospider(cfg2, None);
    let result2 = registry2
        .dispatch("rospider", json!({"action": "camera_pan", "degrees": 10}))
        .await
        .expect("camera_pan dispatch must not error");
    assert!(result2.starts_with("Error:"), "got: {result2}");
    assert!(result2.contains("leg servo"), "got: {result2}");
    assert_eq!(
        mock.published_on("/servo_controller").len(),
        1,
        "leg-servo override must not publish anything new"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rospider_targets_config_override() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let mut cfg = Config::default();
    cfg.ros2.bridge_url = Some(mock.url.clone());
    cfg.ros2.rospider.cmd_vel_topic = "/robot1/cmd_vel".to_string();
    let registry = registry_with_rospider(cfg, None);

    registry
        .dispatch(
            "rospider",
            json!({"action": "walk", "direction": "forward", "duration_secs": 0.2}),
        )
        .await
        .expect("walk dispatch must not error");
    wait_until(
        || !mock.published_on("/robot1/cmd_vel").is_empty(),
        Duration::from_secs(3),
    )
    .await;
    assert!(
        !mock.published_on("/robot1/cmd_vel").is_empty(),
        "expected a publish on the overridden topic"
    );

    let mock2 = MockRosbridge::start_with(MockConfig::default()).await;
    let mut cfg2 = Config::default();
    cfg2.ros2.bridge_url = Some(mock2.url.clone());
    cfg2.ros2.rospider.cmd_vel_topic = String::new();
    let registry2 = registry_with_rospider(cfg2, None);
    let result2 = registry2
        .dispatch(
            "rospider",
            json!({"action": "walk", "direction": "forward"}),
        )
        .await
        .expect("walk dispatch must not error");
    assert!(result2.starts_with("Error:"), "got: {result2}");
    assert!(
        result2.contains("ros2.rospider.cmd_vel_topic"),
        "got: {result2}"
    );
    assert_eq!(
        mock2.connection_count(),
        0,
        "must not connect when the target topic is empty"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rospider_does_not_widen_generic_allowlist() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let mut cfg = Config::default();
    cfg.ros2.bridge_url = Some(mock.url.clone());
    cfg.ros2.publish = vec![];
    let registry = registry_with_rospider(cfg, None);

    let result = registry
        .dispatch(
            "ros2",
            json!({
                "action": "publish",
                "topic": "/controller/cmd_vel",
                "type": "geometry_msgs/msg/Twist",
                "msg": {}
            }),
        )
        .await
        .expect("publish dispatch must not error");
    assert!(result.contains("is not permitted"), "got: {result}");
}

/// VERIFICATION gap #5 / WR-02: the mid-walk stop test must actually exercise
/// the stop — the walk future is polled concurrently with the readiness
/// wait (never merely pinned and ignored), the walk's own outcome is
/// asserted `interrupted`, and the final-frame check runs only after a
/// same-connection completion boundary proves every earlier frame is
/// recorded. See 49.8-21-PLAN.md "Design decisions" for why a completion
/// boundary (not polling for the last frame) and the deterministic mutation
/// (disabling the stop's `token.cancel()`) are the right proofs here.
#[tokio::test(flavor = "multi_thread")]
async fn ros2_rospider_stop_stops_all() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url));
    let registry = registry_with_rospider(cfg, None);

    let dispatch_fut = registry.dispatch(
        "rospider",
        json!({"action": "walk", "direction": "forward", "duration_secs": 2.0}),
    );
    tokio::pin!(dispatch_fut);

    // Drive the walk concurrently with the readiness wait — a pinned but
    // never-polled future would let this test pass on the standalone stop
    // burst alone, which is exactly the vacuous-test defect WR-02 names.
    let walk_published = tokio::select! {
        _ = &mut dispatch_fut => {
            panic!("walk completed before stop");
        }
        published = wait_until(|| !mock.published_on("/controller/cmd_vel").is_empty(), Duration::from_secs(2)) => published,
    };
    assert!(
        walk_published,
        "walk never published a frame — the stop would be vacuous"
    );
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
    assert!(
        stop_result.contains("gait-0 rest stop sent on"),
        "expected the gait-0 rest stop to be sent; got: {stop_result}"
    );
    assert!(
        !stop_result.contains("did not finish"),
        "expected the stop's interrupt-wait not to time out; got: {stop_result}"
    );

    let walk_result = tokio::time::timeout(Duration::from_secs(3), &mut dispatch_fut)
        .await
        .expect("the walk must complete within 3s of the stop — it did not finish")
        .expect("walk dispatch must not error");
    assert!(
        walk_result.contains("interrupted before its full duration elapsed"),
        "expected the walk to report it was interrupted; got: {walk_result}"
    );

    // Completion boundary (49.8-21 review, cbdd1d79): both producers (walk
    // and stop) have now returned, so `run_timed_motion`'s awaited
    // JoinHandle and `do_stop`'s awaited `stop_all`+`send_stop_followup`
    // guarantee every frame either wrote is already recorded on the mock's
    // ONE shared connection. A call made only now, on the session's current
    // client, proves that — the mock records each inbound frame before it
    // answers the next request on that same connection.
    let client = registry
        .ros2_session()
        .current_client()
        .await
        .expect("the session must still hold a live client after walk+stop");
    client
        .call_service("/rosapi/topics", None, json!({}), Duration::from_secs(2))
        .await
        .expect("the completion-boundary /rosapi/topics call must succeed");

    let boundary_conn = mock
        .frames_with_op("call_service")
        .into_iter()
        .rfind(|f| f.value.get("service").and_then(Value::as_str) == Some("/rosapi/topics"))
        .expect("the completion-boundary call_service frame must be recorded")
        .conn;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum StopTestFrame {
        NonZeroTwist,
        ZeroTwist,
        RestStop,
    }

    // No further polling below this line — every frame the boundary covers
    // is already in `mock.frames()`.
    let motion_frames: Vec<(usize, StopTestFrame)> = mock
        .frames_with_op("publish")
        .into_iter()
        .filter_map(|f| {
            let topic = f.value.get("topic").and_then(Value::as_str)?;
            if topic == "/controller/traveling" {
                Some((f.conn, StopTestFrame::RestStop))
            } else if topic == "/controller/cmd_vel" {
                let msg = f.value.get("msg").cloned().unwrap_or(Value::Null);
                let kind = if is_zero_twist_msg(&msg) {
                    StopTestFrame::ZeroTwist
                } else {
                    StopTestFrame::NonZeroTwist
                };
                Some((f.conn, kind))
            } else {
                None
            }
        })
        .collect();

    assert!(
        !motion_frames.is_empty(),
        "expected at least one recorded motion frame"
    );
    assert!(
        motion_frames.iter().all(|(conn, _)| *conn == boundary_conn),
        "every motion frame must be on the same connection as the completion boundary call \
         (boundary_conn={boundary_conn}); got: {motion_frames:?}"
    );

    let first_travel_idx = motion_frames
        .iter()
        .position(|(_, k)| *k == StopTestFrame::RestStop)
        .unwrap_or(motion_frames.len());
    assert!(
        motion_frames[..first_travel_idx]
            .iter()
            .any(|(_, k)| *k == StopTestFrame::NonZeroTwist),
        "expected at least one non-zero Twist before the first traveling frame; got: {motion_frames:?}"
    );

    assert_eq!(
        motion_frames.last().map(|(_, k)| *k),
        Some(StopTestFrame::RestStop),
        "the final motion frame must be the gait-0 rest stop on /controller/traveling — no Twist \
         may follow it; got: {motion_frames:?}"
    );

    let last_cmd_vel = motion_frames
        .iter()
        .rev()
        .find(|(_, k)| *k != StopTestFrame::RestStop);
    assert_eq!(
        last_cmd_vel.map(|(_, k)| *k),
        Some(StopTestFrame::ZeroTwist),
        "the last recorded /controller/cmd_vel frame must be a zero Twist; got: {motion_frames:?}"
    );
}

// ---------------------------------------------------------------------------
// Task 2 — sensor / camera-snapshot reads
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rospider_sensor_reads() {
    let mut topic_messages = HashMap::new();
    topic_messages.insert(
        "/imu".to_string(),
        vec![json!({
            "orientation": {"x": 0.0, "y": 0.0, "z": 0.0, "w": 1.0},
            "angular_velocity": {"x": 0.0, "y": 0.0, "z": 0.0},
            "linear_acceleration": {"x": 0.0, "y": 0.0, "z": 9.8}
        })],
    );
    topic_messages.insert(
        "/ros_robot_controller/battery".to_string(),
        vec![json!({"data": 11100})],
    );
    topic_messages.insert(
        "/odom".to_string(),
        vec![json!({
            "pose": {"pose": {
                "position": {"x": 1.0, "y": 2.0, "z": 0.0},
                "orientation": {"x": 0.0, "y": 0.0, "z": 0.0, "w": 1.0}
            }},
            "twist": {"twist": {
                "linear": {"x": 0.1, "y": 0.0, "z": 0.0},
                "angular": {"x": 0.0, "y": 0.0, "z": 0.05}
            }}
        })],
    );
    topic_messages.insert(
        "/scan".to_string(),
        vec![json!({
            "angle_min": -std::f64::consts::PI,
            "angle_increment": std::f64::consts::PI / 2.0,
            "range_min": 0.1,
            "range_max": 10.0,
            "ranges": [1.0, 2.0, 3.0, 4.0]
        })],
    );
    let mock = MockRosbridge::start_with(MockConfig {
        topic_messages,
        publish_interval_ms: 10,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(Some(&mock.url));
    let registry = registry_with_rospider(cfg, None);

    let imu = registry
        .dispatch("rospider", json!({"action": "read_imu"}))
        .await
        .expect("read_imu must not error");
    assert!(imu.contains("IMU:"), "got: {imu}");

    let battery = registry
        .dispatch("rospider", json!({"action": "read_battery"}))
        .await
        .expect("read_battery must not error");
    assert!(battery.contains("Battery: 11.10 V"), "got: {battery}");

    let odom = registry
        .dispatch("rospider", json!({"action": "read_odom"}))
        .await
        .expect("read_odom must not error");
    assert!(odom.contains("Odometry:"), "got: {odom}");

    let lidar = registry
        .dispatch("rospider", json!({"action": "lidar_summary"}))
        .await
        .expect("lidar_summary must not error");
    assert!(
        lidar.contains("LaserScan:") && lidar.contains("front="),
        "got: {lidar}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rospider_sensor_timeout() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url));
    let registry = registry_with_rospider(cfg, None);

    let result = registry
        .dispatch("rospider", json!({"action": "read_imu"}))
        .await
        .expect("read_imu must not error");
    assert!(result.starts_with("Error:"), "got: {result}");
    assert!(result.contains("/imu"), "got: {result}");
}

/// VERIFICATION gap #4 / WR-01: `sample_one` must route through the same
/// `clamp_subscribe_window` the generic `ros2` tool uses, so an operator's
/// `ros2.caps.max_subscribe_secs` binds ROSPider sensor/camera reads too —
/// never just `SENSOR_WINDOW_SECS` (2.0s) hardcoded.
#[tokio::test(flavor = "multi_thread")]
async fn ros2_rospider_sensor_window_honors_max_subscribe_secs() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let mut cfg = fixed_config(Some(&mock.url));
    cfg.ros2.caps.max_subscribe_secs = 0.3;
    let registry = registry_with_rospider(cfg, None);

    let start = std::time::Instant::now();
    let result = registry
        .dispatch("rospider", json!({"action": "read_imu"}))
        .await
        .expect("read_imu must not error");
    let elapsed = start.elapsed();

    assert!(result.starts_with("Error:"), "got: {result}");
    assert!(result.contains("/imu"), "got: {result}");
    assert!(
        result.contains("0.3s"),
        "expected the effective window (0.3s) named in the error; got: {result}"
    );
    assert!(
        elapsed < Duration::from_millis(1500),
        "expected the read to honor the 0.3s cap, not the default 2s window; elapsed: {elapsed:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rospider_camera_snapshot_prefers_compressed() {
    let compressed_msg = small_jpeg_compressed_msg(32, 24);
    let mock = MockRosbridge::start_with(MockConfig {
        topic_messages: HashMap::from([(
            "/depth_cam/rgb/image_compressed".to_string(),
            vec![compressed_msg],
        )]),
        publish_interval_ms: 20,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(Some(&mock.url));
    let (vision, calls) = mock_vision("a hallway with a chair");
    let registry = registry_with_rospider(cfg, Some(vision));

    let result = registry
        .dispatch("rospider", json!({"action": "camera_snapshot"}))
        .await
        .expect("camera_snapshot dispatch must not error");

    assert!(result.contains("a hallway with a chair"), "got: {result}");
    assert!(
        !result.contains("data:image") && !result.contains("base64"),
        "got: {result}"
    );

    let recorded = calls.lock().expect("calls mutex poisoned");
    assert_eq!(
        recorded.len(),
        1,
        "vision must be called exactly once; got: {recorded:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_rospider_camera_snapshot_falls_back_to_raw() {
    let raw_msg = rgb8_image_msg(64, 48, [10, 20, 30]);
    let mock = MockRosbridge::start_with(MockConfig {
        topic_messages: HashMap::from([("/depth_cam/rgb/image_raw".to_string(), vec![raw_msg])]),
        publish_interval_ms: 20,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(Some(&mock.url));
    let (vision, calls) = mock_vision("a red ball on the floor");
    let registry = registry_with_rospider(cfg, Some(vision));

    let result = registry
        .dispatch("rospider", json!({"action": "camera_snapshot"}))
        .await
        .expect("camera_snapshot dispatch must not error");

    assert!(result.contains("a red ball on the floor"), "got: {result}");
    let recorded = calls.lock().expect("calls mutex poisoned");
    assert_eq!(
        recorded.len(),
        1,
        "vision must be called exactly once; got: {recorded:?}"
    );
}

//! Phase 49.8 Plan 10, Task 1 (D-17 local layer, D-06): an ignored,
//! opt-in end-to-end smoke test against a REAL rosbridge — the apt Humble
//! build launched by `docker/ros2-humble-harness/smoke.sh`, never the
//! in-process mock server the rest of this crate's `ros2_*` tests use.
//!
//! Only a real rosbridge proves the apt Humble build's action ops and QoS
//! behave (D-06); the mock server proves protocol logic, not the real
//! binary. This test is deliberately excluded from the default `cargo test`
//! gate — it requires Docker, a network pull of `ros:humble-ros-base`, and
//! several minutes to build.
//!
//! Run it via `docker/ros2-humble-harness/smoke.sh`, or manually:
//! ```bash
//! IRONHERMES_ROS2_LIVE_URL=ws://127.0.0.1:9090 \
//!   cargo test -p ironhermes-tools --test ros2_live_bridge -- --ignored --test-threads=1 --nocapture
//! ```

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use ironhermes_core::Config;
use ironhermes_core::config::Ros2AllowEntry;
use ironhermes_tools::registry::ToolRegistry;
use ironhermes_tools::ros2::{Ros2ConfigSource, end_session_stop};
use serde_json::{Value, json};

fn allow_entry(name: &str, msg_type: &str) -> Ros2AllowEntry {
    Ros2AllowEntry {
        name: name.to_string(),
        msg_type: msg_type.to_string(),
        limits: BTreeMap::new(),
    }
}

/// Pulls a top-level numeric field out of a `turtlesim/msg/Pose` sample
/// line rendered into the `subscribe` action's text result (the message is
/// flat JSON — `x`, `y`, `theta`, `linear_velocity`, `angular_velocity` are
/// all top-level keys, not nested).
fn extract_f64_field(rendered: &str, field: &str) -> Option<f64> {
    // The rendered sample embeds the message as compact JSON on its own
    // line; find the `"field":` key and parse the following number.
    let needle = format!("\"{field}\":");
    let idx = rendered.find(&needle)?;
    let after = &rendered[idx + needle.len()..];
    let end = after
        .find(|c: char| c == ',' || c == '}' || c.is_whitespace() && false)
        .unwrap_or(after.len());
    let num_str: String = after[..end]
        .chars()
        .take_while(|c| {
            c.is_ascii_digit() || *c == '.' || *c == '-' || *c == 'e' || *c == 'E' || *c == '+'
        })
        .collect();
    num_str.trim().parse::<f64>().ok()
}

/// VERIFICATION gap #6 / WR-03: `require_f64_field` parses the rendered
/// sample lines as JSON and requires EXACTLY ONE sample carrying a
/// JSON-number `field` — never a text scan that can silently turn a
/// missing, malformed, or ambiguous field into a passing `0.0` settled
/// reading. Skips the first (header) line per `read_ops.rs`'s
/// `subscribe_action` rendering: header, then one compact-JSON sample per
/// line.
///
/// Panics, each naming the field and echoing the full rendered sample:
///
///   - no sample carries the field (including when the only occurrence is
///     in non-JSON header text) → "settled pose is missing a numeric
///     {field} field: {rendered}";
///   - the value is not a JSON number (null, string, bool, object) →
///     "settled pose has a non-numeric {field} field: {rendered}";
///   - more than one sample carries it → "settled pose carries {field} in
///     more than one sample: {rendered}".
fn require_f64_field(rendered: &str, field: &str) -> f64 {
    let mut found: Vec<f64> = Vec::new();
    for line in rendered.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(obj) = value.as_object() else {
            continue;
        };
        let Some(field_value) = obj.get(field) else {
            continue;
        };
        match field_value.as_f64() {
            Some(f) => found.push(f),
            None => panic!("settled pose has a non-numeric {field} field: {rendered}"),
        }
    }
    match found.len() {
        0 => panic!("settled pose is missing a numeric {field} field: {rendered}"),
        1 => found[0],
        _ => panic!("settled pose carries {field} in more than one sample: {rendered}"),
    }
}

#[test]
fn ros2_live_bridge_required_field_is_parsed() {
    let rendered = "Received 1/1 message(s) on /turtle1/pose [turtlesim/msg/Pose] in 0.1s\n{\"x\":1.0,\"linear_velocity\":0.0}";
    assert_eq!(require_f64_field(rendered, "linear_velocity"), 0.0);

    let rendered_neg = "Received 1/1 message(s) on /turtle1/pose [turtlesim/msg/Pose] in 0.1s\n{\"x\":1.0,\"linear_velocity\":-2.5e-3}";
    assert_eq!(require_f64_field(rendered_neg, "linear_velocity"), -0.0025);
}

#[test]
#[should_panic(expected = "missing a numeric linear_velocity field")]
fn ros2_live_bridge_missing_required_field_fails_loudly() {
    let rendered =
        "Received 1/1 message(s) on /turtle1/pose [turtlesim/msg/Pose] in 0.1s\n{\"x\":1.0}";
    require_f64_field(rendered, "linear_velocity");
}

#[test]
fn ros2_live_bridge_malformed_required_field_fails_loudly() {
    let header = "Received 1/1 message(s) on /turtle1/pose [turtlesim/msg/Pose] in 0.1s";

    // null value.
    let null_value = format!("{header}\n{{\"x\":1.0,\"linear_velocity\":null}}");
    assert!(
        std::panic::catch_unwind(|| require_f64_field(&null_value, "linear_velocity")).is_err()
    );

    // string value.
    let string_value = format!("{header}\n{{\"x\":1.0,\"linear_velocity\":\"0.0\"}}");
    assert!(
        std::panic::catch_unwind(|| require_f64_field(&string_value, "linear_velocity")).is_err()
    );

    // malformed JSON sample line.
    let malformed = format!("{header}\n{{\"x\":1.0,\"linear_velocity\":0.0x}}");
    assert!(std::panic::catch_unwind(|| require_f64_field(&malformed, "linear_velocity")).is_err());

    // the field text appears only in the (skipped) header line, above a
    // sample that never carries it.
    let prose_header = format!("{header} \"linear_velocity\":0.0\n{{\"x\":1.0}}");
    assert!(
        std::panic::catch_unwind(|| require_f64_field(&prose_header, "linear_velocity")).is_err()
    );

    // two sample lines both carry the field — ambiguous.
    let two_samples = format!(
        "{header}\n{{\"x\":1.0,\"linear_velocity\":0.0}}\n{{\"x\":2.0,\"linear_velocity\":0.0}}"
    );
    assert!(
        std::panic::catch_unwind(|| require_f64_field(&two_samples, "linear_velocity")).is_err()
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs docker/ros2-humble-harness/smoke.sh"]
async fn ros2_live_bridge_smoke() {
    let url = std::env::var("IRONHERMES_ROS2_LIVE_URL").unwrap_or_else(|_| {
        panic!(
            "IRONHERMES_ROS2_LIVE_URL is not set — this test needs a real rosbridge. \
             Run docker/ros2-humble-harness/smoke.sh, which sets it for you, or set it \
             yourself to ws://127.0.0.1:9090 after starting the harness manually."
        )
    });

    let mut cfg = Config::default();
    cfg.ros2.bridge_url = Some(url.clone());
    cfg.ros2.publish = vec![allow_entry("/turtle1/cmd_vel", "geometry_msgs/msg/Twist")];
    cfg.ros2.actions = vec![allow_entry(
        "/turtle1/rotate_absolute",
        "turtlesim/action/RotateAbsolute",
    )];

    let mut registry = ToolRegistry::new();
    registry.register_ros2_tools(Ros2ConfigSource::Fixed(Arc::new(cfg)), None);

    // 1. list_topics contains /turtle1/pose and /turtle1/cmd_vel.
    let topics = registry
        .dispatch("ros2", json!({"action": "list_topics"}))
        .await
        .expect("list_topics dispatch must not error");
    println!("== list_topics ==\n{topics}\n");
    assert!(
        topics.contains("/turtle1/pose"),
        "expected /turtle1/pose; got: {topics}"
    );
    assert!(
        topics.contains("/turtle1/cmd_vel"),
        "expected /turtle1/cmd_vel; got: {topics}"
    );

    // 2. subscribe on /turtle1/pose yields an x.
    let pose1 = registry
        .dispatch(
            "ros2",
            json!({
                "action": "subscribe",
                "topic": "/turtle1/pose",
                "type": "turtlesim/msg/Pose",
                "max_messages": 1,
                "timeout_secs": 5,
            }),
        )
        .await
        .expect("subscribe dispatch must not error");
    println!("== subscribe /turtle1/pose (before motion) ==\n{pose1}\n");
    let x_before = extract_f64_field(&pose1, "x")
        .unwrap_or_else(|| panic!("expected an 'x' field in the pose sample; got: {pose1}"));

    // 3. publish linear.x: 0.5 for 1.0s reports a clamp to 0.10 and stop.
    let motion = registry
        .dispatch(
            "ros2",
            json!({
                "action": "publish",
                "topic": "/turtle1/cmd_vel",
                "type": "geometry_msgs/msg/Twist",
                "msg": {"linear": {"x": 0.5}},
                "duration_secs": 1.0,
            }),
        )
        .await
        .expect("publish dispatch must not error");
    println!("== publish /turtle1/cmd_vel (0.5 m/s requested) ==\n{motion}\n");
    assert!(
        motion.contains("0.10") || motion.to_lowercase().contains("clamp"),
        "expected a clamp to 0.10 m/s noted; got: {motion}"
    );
    assert!(
        motion.to_lowercase().contains("stop"),
        "expected the motion result to mention the stop; got: {motion}"
    );

    // 4. second pose sample shows x increased by more than 0.05.
    let pose2 = registry
        .dispatch(
            "ros2",
            json!({
                "action": "subscribe",
                "topic": "/turtle1/pose",
                "type": "turtlesim/msg/Pose",
                "max_messages": 1,
                "timeout_secs": 5,
            }),
        )
        .await
        .expect("subscribe dispatch must not error");
    println!("== subscribe /turtle1/pose (after motion) ==\n{pose2}\n");
    let x_after = extract_f64_field(&pose2, "x")
        .unwrap_or_else(|| panic!("expected an 'x' field in the pose sample; got: {pose2}"));
    assert!(
        x_after - x_before > 0.05,
        "expected the turtle to have moved forward by more than 0.05; before={x_before} after={x_after}"
    );

    // 5. a third sample, 500ms later, shows linear_velocity 0 (the auto-stop
    // actually reached zero, not just a reported claim).
    tokio::time::sleep(Duration::from_millis(500)).await;
    let pose3 = registry
        .dispatch(
            "ros2",
            json!({
                "action": "subscribe",
                "topic": "/turtle1/pose",
                "type": "turtlesim/msg/Pose",
                "max_messages": 1,
                "timeout_secs": 5,
            }),
        )
        .await
        .expect("subscribe dispatch must not error");
    println!("== subscribe /turtle1/pose (settled) ==\n{pose3}\n");
    let lv = require_f64_field(&pose3, "linear_velocity");
    assert!(
        lv.abs() < 0.01,
        "expected linear_velocity to have settled near 0 after the auto-stop; got: {pose3}"
    );

    // 6. send_goal {theta: 1.57} then goal_status {wait_secs: 10} reports
    // SUCCEEDED — proves action ops work end to end on a REAL rosbridge
    // (D-06), not just the mock.
    let sent = registry
        .dispatch(
            "ros2",
            json!({
                "action": "send_goal",
                "action_name": "/turtle1/rotate_absolute",
                "type": "turtlesim/action/RotateAbsolute",
                "goal": {"theta": 1.57},
            }),
        )
        .await
        .expect("send_goal dispatch must not error");
    println!("== send_goal /turtle1/rotate_absolute (theta=1.57) ==\n{sent}\n");
    let goal_id = sent
        .split_whitespace()
        .nth(1)
        .unwrap_or_else(|| panic!("expected a goal id in the response; got: {sent}"))
        .to_string();

    let status = registry
        .dispatch(
            "ros2",
            json!({"action": "goal_status", "goal_id": goal_id, "wait_secs": 10}),
        )
        .await
        .expect("goal_status dispatch must not error");
    println!("== goal_status (first rotate) ==\n{status}\n");
    assert!(
        status.contains("SUCCEEDED"),
        "expected SUCCEEDED on a real Humble rosbridge; got: {status}"
    );

    // 7. a second goal followed immediately by cancel_goal ends terminal
    // (CANCELED or SUCCEEDED — a fast rotation may complete before the
    // cancel lands, which is still a valid terminal outcome).
    let sent2 = registry
        .dispatch(
            "ros2",
            json!({
                "action": "send_goal",
                "action_name": "/turtle1/rotate_absolute",
                "type": "turtlesim/action/RotateAbsolute",
                "goal": {"theta": -1.57},
            }),
        )
        .await
        .expect("send_goal dispatch must not error");
    println!("== send_goal /turtle1/rotate_absolute (theta=-1.57) ==\n{sent2}\n");
    let goal_id2 = sent2
        .split_whitespace()
        .nth(1)
        .unwrap_or_else(|| panic!("expected a goal id in the response; got: {sent2}"))
        .to_string();

    let cancelled = registry
        .dispatch(
            "ros2",
            json!({"action": "cancel_goal", "goal_id": goal_id2}),
        )
        .await
        .expect("cancel_goal dispatch must not error");
    println!("== cancel_goal (second rotate) ==\n{cancelled}\n");
    assert!(
        cancelled.contains("CANCELED") || cancelled.contains("SUCCEEDED"),
        "expected a terminal status (CANCELED or SUCCEEDED); got: {cancelled}"
    );

    // 8. end_session_stop returns a report.
    let registry_lock = tokio::sync::RwLock::new(registry);
    let report = end_session_stop(&registry_lock).await;
    println!("== end_session_stop ==\n{report}\n");
    assert!(
        !report.is_empty(),
        "expected a non-empty end_session_stop report"
    );
}

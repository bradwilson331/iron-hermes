//! Phase 49.8 Plan 01 (tracer, `tdd="true"`): "list the robot's topics and
//! publish one declared message" end to end over a real WebSocket against an
//! in-process mock rosbridge server (D-17) — plus the deny-by-default write
//! allowlist (D-04) and the fail-closed motion refusal (D-08/D-09) that make
//! the rest of Phase 49.8 safe to build on.
//!
//! `test_ros2_undeclared_publish_blocked` is this plan's RED/GREEN target test
//! (see `49.8-01-SUMMARY.md`'s TDD Gate Compliance section for the recorded
//! RED evidence).

#[path = "support/mock_rosbridge.rs"]
mod mock_rosbridge;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use ironhermes_core::Config;
use ironhermes_core::config::Ros2AllowEntry;
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

fn fixed_config(bridge_url: Option<&str>, publish: Vec<Ros2AllowEntry>) -> Config {
    let mut cfg = Config::default();
    cfg.ros2.bridge_url = bridge_url.map(|s| s.to_string());
    cfg.ros2.publish = publish;
    cfg
}

fn registry_with_ros2(cfg: Config) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register_ros2_tools(Ros2ConfigSource::Fixed(Arc::new(cfg)), None);
    registry
}

/// `advertise`/`publish` are fire-and-forget rosbridge ops — the client's
/// `.send().await` returning only proves the local write was queued, not that
/// the mock's own connection-handler task has scheduled, read, and recorded
/// the frame yet. Poll with a short deadline instead of asserting immediately,
/// so this test is deterministic rather than a localhost-latency race.
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
// ros2_tracer_list_topics_and_publish_end_to_end
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_tracer_list_topics_and_publish_end_to_end() {
    let mock = MockRosbridge::start_with(MockConfig {
        topics: vec![
            ("/chatter".to_string(), "std_msgs/msg/String".to_string()),
            (
                "/cmd_vel".to_string(),
                "geometry_msgs/msg/Twist".to_string(),
            ),
        ],
        service_responses: HashMap::new(),
        ..Default::default()
    })
    .await;

    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry("/chatter", "std_msgs/msg/String")],
    );
    let registry = registry_with_ros2(cfg);

    let defs = registry.get_definitions(None);
    assert!(
        defs.iter().any(|d| d.function.name == "ros2"),
        "ros2 tool must appear in get_definitions when a bridge_url resolves; got: {:?}",
        defs.iter().map(|d| &d.function.name).collect::<Vec<_>>()
    );

    let list_result = registry
        .dispatch("ros2", json!({"action": "list_topics"}))
        .await
        .expect("list_topics dispatch must not error");
    assert!(
        list_result.contains("/chatter") && list_result.contains("std_msgs/msg/String"),
        "list_topics output must name the mock's topics and types; got: {list_result}"
    );

    let publish_result = registry
        .dispatch(
            "ros2",
            json!({
                "action": "publish",
                "topic": "/chatter",
                "type": "std_msgs/msg/String",
                "msg": {"data": "hi"}
            }),
        )
        .await
        .expect("publish dispatch must not error");
    assert!(
        publish_result.contains("Published"),
        "declared publish must succeed; got: {publish_result}"
    );

    wait_for_frame_count(&mock, "advertise", 1, std::time::Duration::from_secs(2)).await;
    wait_for_frame_count(&mock, "publish", 1, std::time::Duration::from_secs(2)).await;
    let advertise_frames = mock.frames_with_op("advertise");
    let publish_frames = mock.frames_with_op("publish");
    assert_eq!(
        advertise_frames.len(),
        1,
        "expected exactly one advertise frame; got: {advertise_frames:?}"
    );
    assert_eq!(
        publish_frames.len(),
        1,
        "expected exactly one publish frame; got: {publish_frames:?}"
    );
    assert_eq!(
        advertise_frames[0]
            .value
            .get("topic")
            .and_then(|v| v.as_str()),
        Some("/chatter")
    );
    assert_eq!(
        advertise_frames[0]
            .value
            .get("type")
            .and_then(|v| v.as_str()),
        Some("std_msgs/msg/String")
    );
    assert_eq!(
        publish_frames[0]
            .value
            .get("topic")
            .and_then(|v| v.as_str()),
        Some("/chatter")
    );
    assert_eq!(
        publish_frames[0]
            .value
            .get("msg")
            .and_then(|v| v.get("data"))
            .and_then(|v| v.as_str()),
        Some("hi")
    );
    assert!(
        advertise_frames[0].at <= publish_frames[0].at,
        "advertise must be recorded before publish"
    );
}

// ---------------------------------------------------------------------------
// test_ros2_undeclared_publish_blocked (RED/GREEN target test, D-04)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn test_ros2_undeclared_publish_blocked() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url), vec![]);
    let registry = registry_with_ros2(cfg);

    let result = registry
        .dispatch(
            "ros2",
            json!({
                "action": "publish",
                "topic": "/not_declared",
                "type": "std_msgs/msg/String",
                "msg": {"data": "hi"}
            }),
        )
        .await
        .expect("publish dispatch must not error");

    assert!(
        result.contains("is not permitted"),
        "undeclared publish must be refused with an 'is not permitted' answer; got: {result}"
    );
    assert_eq!(
        mock.connection_count(),
        0,
        "undeclared publish must never open a WebSocket connection (D-04 allowlist-before-I/O)"
    );
}

// ---------------------------------------------------------------------------
// ros2_type_mismatch_blocked (D-04)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_type_mismatch_blocked() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry("/chatter", "std_msgs/msg/String")],
    );
    let registry = registry_with_ros2(cfg);

    let result = registry
        .dispatch(
            "ros2",
            json!({
                "action": "publish",
                "topic": "/chatter",
                "type": "std_msgs/msg/Int32",
                "msg": {"data": 1}
            }),
        )
        .await
        .expect("publish dispatch must not error");

    assert!(
        result.contains("is not permitted"),
        "a declared-type mismatch must be refused; got: {result}"
    );
    assert!(
        mock.published_on("/chatter").is_empty(),
        "no publish frame may reach the wire on a type mismatch"
    );
}

// ---------------------------------------------------------------------------
// ros2_tool_hidden_without_bridge_url (D-14)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_tool_hidden_without_bridge_url() {
    let cfg = fixed_config(None, vec![]);
    let registry = registry_with_ros2(cfg);

    let defs = registry.get_definitions(None);
    assert!(
        !defs.iter().any(|d| d.function.name == "ros2"),
        "ros2 tool must be hidden when no bridge_url resolves; got: {:?}",
        defs.iter().map(|d| &d.function.name).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// ros2_motion_publish_now_times_and_stops (D-08/D-09, supersedes this
// tracer's original fail-closed placeholder — 49.8-04 lands the timed-motion
// engine, so a declared Twist-family publish now completes with a guaranteed
// stop instead of being refused. The full auto-stop timing/count/preemption
// behavior is proven in `tests/ros2_write_safety.rs`; this test only pins
// that the tracer's own end-to-end path (this file) still works.)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_motion_publish_now_times_and_stops() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry("/cmd_vel", "geometry_msgs/msg/Twist")],
    );
    let registry = registry_with_ros2(cfg);

    let result = registry
        .dispatch(
            "ros2",
            json!({
                "action": "publish",
                "topic": "/cmd_vel",
                "type": "geometry_msgs/msg/Twist",
                "msg": {"linear": {"x": 0.1}},
                "duration_secs": 0.2
            }),
        )
        .await
        .expect("publish dispatch must not error");

    assert!(
        result.contains("stop"),
        "a declared Twist-family motion publish must now complete with a guaranteed stop; got: {result}"
    );

    wait_for_frame_count(&mock, "publish", 1, std::time::Duration::from_secs(2)).await;
    assert!(
        !mock.published_on("/cmd_vel").is_empty(),
        "the timed-motion engine must actually publish on the wire"
    );
}

// ---------------------------------------------------------------------------
// ros2_unknown_action_blocked (hexapod D-16 pattern)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_unknown_action_blocked() {
    let cfg = fixed_config(Some("ws://127.0.0.1:1"), vec![]);
    let registry = registry_with_ros2(cfg);

    let result = registry
        .dispatch("ros2", json!({"action": "launch_missiles"}))
        .await
        .expect("dispatch must not error");

    assert_eq!(
        result,
        "Action 'launch_missiles' is blocked — not permitted via ros2. Never send this command."
    );
}

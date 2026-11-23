//! D-04/D-12/D-13 integration tests for the `subscribe` bounded sample and
//! the rosapi introspection actions (Phase 49.8 Plan 05).
//!
//! `test_ros2_bounded_subscribe_caps` is this plan's Task 1 RED/GREEN target
//! test — see `49.8-05-SUMMARY.md`'s TDD Gate Compliance section for the
//! recorded RED evidence. Task 2's LaserScan-summarized-subscribe test
//! (`ros2_subscribe_laserscan_is_summarized`) is appended by that task.

#[path = "support/mock_rosbridge.rs"]
mod mock_rosbridge;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use ironhermes_core::Config;
use ironhermes_core::config::Ros2AllowEntry;
use ironhermes_tools::registry::ToolRegistry;
use ironhermes_tools::ros2::Ros2ConfigSource;
use mock_rosbridge::{MockConfig, MockRosbridge};
use serde_json::json;

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

/// `subscribe`/`unsubscribe` are fire-and-forget rosbridge ops with no
/// client-side ack — the client's `.send().await` returning only proves the
/// local write was queued, not that the mock's own connection-handler task
/// has scheduled, read, and recorded the frame yet (same race
/// `49.8-01-SUMMARY.md` documented for advertise/publish). Poll with a short
/// deadline instead of asserting immediately.
async fn wait_for_frame_count(mock: &MockRosbridge, op: &str, count: usize, deadline: Duration) {
    let start = std::time::Instant::now();
    loop {
        if mock.frames_with_op(op).len() >= count || start.elapsed() > deadline {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// ---------------------------------------------------------------------------
// test_ros2_bounded_subscribe_caps (D-12, RED/GREEN target)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn test_ros2_bounded_subscribe_caps() {
    let mock = MockRosbridge::start_with(MockConfig {
        topic_messages: HashMap::from([(
            "/chatter".to_string(),
            vec![
                json!({"data": "a"}),
                json!({"data": "b"}),
                json!({"data": "c"}),
                json!({"data": "d"}),
                json!({"data": "e"}),
            ],
        )]),
        publish_interval_ms: 20,
        topic_types: HashMap::from([("/chatter".to_string(), "std_msgs/msg/String".to_string())]),
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(Some(&mock.url), vec![]);
    let registry = registry_with_ros2(cfg);

    let result = registry
        .dispatch(
            "ros2",
            json!({
                "action": "subscribe",
                "topic": "/chatter",
                "type": "std_msgs/msg/String",
                "max_messages": 100,
                "timeout_secs": 30
            }),
        )
        .await
        .expect("subscribe dispatch must not error");

    // D-12: caps.max_subscribe_messages is 20 (Ros2Caps::default()) — a
    // requested 100 must be clamped down to exactly 20 messages, not the
    // requested 100.
    assert!(
        result.contains("Received 20/20"),
        "expected the bounded sample to clamp to 20/20 messages; got: {result}"
    );

    wait_for_frame_count(&mock, "subscribe", 1, Duration::from_secs(2)).await;
    let sub_frames = mock.frames_with_op("subscribe");
    assert_eq!(
        sub_frames.len(),
        1,
        "expected exactly one subscribe frame; got: {sub_frames:?}"
    );
    assert!(
        sub_frames[0]
            .value
            .get("throttle_rate")
            .and_then(|v| v.as_u64())
            .is_some(),
        "subscribe frame must carry a numeric throttle_rate (RESEARCH.md Pitfall 4): {:?}",
        sub_frames[0].value
    );
    assert!(
        sub_frames[0]
            .value
            .get("queue_length")
            .and_then(|v| v.as_u64())
            .is_some(),
        "subscribe frame must carry a numeric queue_length: {:?}",
        sub_frames[0].value
    );

    wait_for_frame_count(&mock, "unsubscribe", 1, Duration::from_secs(2)).await;
    let unsub_frames = mock.frames_with_op("unsubscribe");
    assert_eq!(
        unsub_frames.len(),
        1,
        "expected exactly one unsubscribe frame; got: {unsub_frames:?}"
    );
    assert_eq!(
        sub_frames[0].value.get("id"),
        unsub_frames[0].value.get("id"),
        "unsubscribe must reuse the subscribe frame's own id"
    );
}

// ---------------------------------------------------------------------------
// ros2_subscribe_timeout_returns_partial (D-12)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_subscribe_timeout_returns_partial() {
    let mock = MockRosbridge::start_with(MockConfig {
        topic_types: HashMap::from([("/empty".to_string(), "std_msgs/msg/String".to_string())]),
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(Some(&mock.url), vec![]);
    let registry = registry_with_ros2(cfg);

    let start = std::time::Instant::now();
    let result = registry
        .dispatch(
            "ros2",
            json!({
                "action": "subscribe",
                "topic": "/empty",
                "type": "std_msgs/msg/String",
                "max_messages": 1,
                "timeout_secs": 0.5
            }),
        )
        .await
        .expect("subscribe dispatch must not error");
    let elapsed = start.elapsed();

    assert!(
        result.contains("Received 0/1"),
        "expected a zero-message partial sample; got: {result}"
    );
    assert!(
        elapsed < Duration::from_millis(900),
        "subscribe with a 0.5s window took too long: {elapsed:?}"
    );

    wait_for_frame_count(&mock, "unsubscribe", 1, Duration::from_secs(2)).await;
    assert_eq!(
        mock.frames_with_op("unsubscribe").len(),
        1,
        "a timed-out (zero-message) subscribe must still unsubscribe"
    );
}

// ---------------------------------------------------------------------------
// ros2_subscribe_resolves_type_via_rosapi (D-12)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_subscribe_resolves_type_via_rosapi() {
    let mock = MockRosbridge::start_with(MockConfig {
        topic_types: HashMap::from([("/chatter".to_string(), "std_msgs/msg/String".to_string())]),
        topic_messages: HashMap::from([("/chatter".to_string(), vec![json!({"data": "hi"})])]),
        publish_interval_ms: 20,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(Some(&mock.url), vec![]);
    let registry = registry_with_ros2(cfg);

    let result = registry
        .dispatch(
            "ros2",
            json!({"action": "subscribe", "topic": "/chatter", "max_messages": 1, "timeout_secs": 1.0}),
        )
        .await
        .expect("subscribe dispatch must not error");
    assert!(result.contains("Received"), "got: {result}");

    wait_for_frame_count(&mock, "subscribe", 1, Duration::from_secs(2)).await;
    let call_service_frames = mock.frames_with_op("call_service");
    let topic_type_call = call_service_frames
        .iter()
        .find(|f| f.value.get("service").and_then(|v| v.as_str()) == Some("/rosapi/topic_type"));
    assert!(
        topic_type_call.is_some(),
        "expected a /rosapi/topic_type call_service frame when type is omitted; got: {call_service_frames:?}"
    );
    let sub_frames = mock.frames_with_op("subscribe");
    assert_eq!(sub_frames.len(), 1, "got: {sub_frames:?}");
    assert!(
        topic_type_call.unwrap().at <= sub_frames[0].at,
        "the /rosapi/topic_type call must be recorded before the subscribe frame"
    );
    assert_eq!(
        sub_frames[0].value.get("type").and_then(|v| v.as_str()),
        Some("std_msgs/msg/String"),
        "the subscribe frame must carry the resolved type"
    );

    // An unresolvable topic: no topic_types entry — /rosapi/topic_type
    // answers an empty type string, which resolve_topic_type treats as
    // unknown. No subscribe frame should be sent for this call.
    let subscribe_count_before = mock.frames_with_op("subscribe").len();
    let result2 = registry
        .dispatch(
            "ros2",
            json!({"action": "subscribe", "topic": "/nope", "max_messages": 1, "timeout_secs": 1.0}),
        )
        .await
        .expect("subscribe dispatch must not error");
    assert!(
        result2.contains("unknown type"),
        "an unresolvable topic must answer 'unknown type'; got: {result2}"
    );
    assert_eq!(
        mock.frames_with_op("subscribe").len(),
        subscribe_count_before,
        "an unresolvable topic must send no subscribe frame"
    );
}

// ---------------------------------------------------------------------------
// ros2_subscribe_any_topic_no_allowlist (D-04)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_subscribe_any_topic_no_allowlist() {
    let mock = MockRosbridge::start_with(MockConfig {
        topic_types: HashMap::from([("/chatter".to_string(), "std_msgs/msg/String".to_string())]),
        ..Default::default()
    })
    .await;
    // Empty ros2.publish/services/actions (Config::default() already leaves
    // services/actions empty; fixed_config only sets publish) — D-04 reads
    // must succeed regardless.
    let cfg = fixed_config(Some(&mock.url), vec![]);
    assert!(cfg.ros2.services.is_empty());
    assert!(cfg.ros2.actions.is_empty());
    let registry = registry_with_ros2(cfg);

    let result = registry
        .dispatch(
            "ros2",
            json!({
                "action": "subscribe",
                "topic": "/chatter",
                "type": "std_msgs/msg/String",
                "max_messages": 1,
                "timeout_secs": 0.5
            }),
        )
        .await
        .expect("subscribe dispatch must not error");

    assert!(
        !result.starts_with("Error:") && !result.contains("is not permitted"),
        "subscribe on an undeclared topic must succeed — reads are open (D-04); got: {result}"
    );
    assert!(result.contains("Received"), "got: {result}");
}

// ---------------------------------------------------------------------------
// ros2_introspection_actions (D-04)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_introspection_actions() {
    let mock = MockRosbridge::start_with(MockConfig {
        nodes: vec!["/talker".to_string(), "/listener".to_string()],
        services: vec![("/set_mode".to_string(), "std_srvs/srv/SetBool".to_string())],
        topic_types: HashMap::from([("/chatter".to_string(), "std_msgs/msg/String".to_string())]),
        message_details: HashMap::from([(
            "std_msgs/msg/String".to_string(),
            json!([{"name": "data", "type": "string"}]),
        )]),
        // action_servers left at its Default (empty) — list_actions must
        // answer Error:, not a silent empty list.
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(Some(&mock.url), vec![]);
    let registry = registry_with_ros2(cfg);

    let list_services = registry
        .dispatch("ros2", json!({"action": "list_services"}))
        .await
        .expect("list_services dispatch must not error");
    assert!(list_services.contains("/set_mode"), "got: {list_services}");

    let list_nodes = registry
        .dispatch("ros2", json!({"action": "list_nodes"}))
        .await
        .expect("list_nodes dispatch must not error");
    assert!(
        list_nodes.contains("/talker") && list_nodes.contains("/listener"),
        "got: {list_nodes}"
    );

    let list_actions = registry
        .dispatch("ros2", json!({"action": "list_actions"}))
        .await
        .expect("list_actions dispatch must not error");
    assert!(
        list_actions.starts_with("Error:") && list_actions.contains("action_servers"),
        "an unconfigured action_servers list must render an Error: naming action_servers; got: {list_actions}"
    );

    let topic_type = registry
        .dispatch("ros2", json!({"action": "topic_type", "topic": "/chatter"}))
        .await
        .expect("topic_type dispatch must not error");
    assert!(
        topic_type.contains("std_msgs/msg/String"),
        "got: {topic_type}"
    );

    let service_type = registry
        .dispatch(
            "ros2",
            json!({"action": "service_type", "service": "/set_mode"}),
        )
        .await
        .expect("service_type dispatch must not error");
    assert!(
        service_type.contains("std_srvs/srv/SetBool"),
        "got: {service_type}"
    );

    let message_details = registry
        .dispatch(
            "ros2",
            json!({"action": "message_details", "msg_type_name": "std_msgs/msg/String"}),
        )
        .await
        .expect("message_details dispatch must not error");
    assert!(message_details.contains("data"), "got: {message_details}");
}

// ---------------------------------------------------------------------------
// ros2_subscribe_laserscan_is_summarized (D-13, Task 2 RED/GREEN target)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_subscribe_laserscan_is_summarized() {
    let ranges: Vec<serde_json::Value> = (0..360).map(|_| json!(1.5)).collect();
    let laserscan = json!({
        "angle_min": -std::f64::consts::PI,
        "angle_increment": 1.0_f64.to_radians(),
        "range_min": 0.1,
        "range_max": 12.0,
        "ranges": ranges,
    });

    let mock = MockRosbridge::start_with(MockConfig {
        topic_types: HashMap::from([(
            "/scan".to_string(),
            "sensor_msgs/msg/LaserScan".to_string(),
        )]),
        topic_messages: HashMap::from([("/scan".to_string(), vec![laserscan])]),
        publish_interval_ms: 20,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(Some(&mock.url), vec![]);
    let registry = registry_with_ros2(cfg);

    let result = registry
        .dispatch(
            "ros2",
            json!({"action": "subscribe", "topic": "/scan", "max_messages": 1, "timeout_secs": 1.0}),
        )
        .await
        .expect("subscribe dispatch must not error");

    assert!(result.contains("LaserScan:"), "got: {result}");
    assert!(result.contains("front="), "got: {result}");
    assert!(
        result.len() < 2_000,
        "a summarized LaserScan result must be short (<2000 chars); got {} chars: {result}",
        result.len()
    );
}

// ---------------------------------------------------------------------------
// ros2_subscribe_laserscan_hostile_angles_is_summarized (CR-02, Task 1 RED/GREEN target)
// ---------------------------------------------------------------------------

/// A plain `#[test]` (not `#[tokio::test]`): the whole scenario — mock,
/// registry, subscribe dispatch — runs on its own `std::thread` with its own
/// multi-thread tokio runtime, so a reintroduced synchronous unbounded loop
/// in `normalize_angle` cannot pre-empt-block via a tokio timeout. The test
/// thread waits on `recv_timeout`; the abandoned worker thread (if any) ends
/// only when the test process exits, never hanging `cargo test` itself
/// (review 49.8-20).
#[test]
fn ros2_subscribe_laserscan_hostile_angles_is_summarized() {
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");
        let result = runtime.block_on(async {
            let ranges: Vec<serde_json::Value> = (0..36).map(|_| json!(1.5)).collect();
            let laserscan = json!({
                "angle_min": 1e308,
                "angle_increment": 1e308,
                "range_min": 0.1,
                "range_max": 12.0,
                "ranges": ranges,
            });

            let mock = MockRosbridge::start_with(MockConfig {
                topic_types: HashMap::from([(
                    "/scan".to_string(),
                    "sensor_msgs/msg/LaserScan".to_string(),
                )]),
                topic_messages: HashMap::from([("/scan".to_string(), vec![laserscan])]),
                publish_interval_ms: 20,
                ..Default::default()
            })
            .await;
            let cfg = fixed_config(Some(&mock.url), vec![]);
            let registry = registry_with_ros2(cfg);

            registry
                .dispatch(
                    "ros2",
                    json!({"action": "subscribe", "topic": "/scan", "max_messages": 1, "timeout_secs": 1.0}),
                )
                .await
                .expect("subscribe dispatch must not error")
        });
        let _ = tx.send(result);
    });

    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(result) => {
            assert!(result.contains("LaserScan:"), "got: {result}");
            assert!(result.contains("36 beams"), "got: {result}");
            assert!(result.contains("non-finite scan angles"), "got: {result}");
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            panic!(
                "ros2_subscribe_laserscan_hostile_angles_is_summarized: normalize_angle did not \
                 return within 10s — CR-02 regression (a tokio timeout cannot pre-empt a \
                 synchronous unbounded loop)"
            );
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => match handle.join() {
            Ok(_) => panic!(
                "ros2_subscribe_laserscan_hostile_angles_is_summarized: worker thread ended \
                 without sending a result"
            ),
            Err(panic_payload) => std::panic::resume_unwind(panic_payload),
        },
    }
}

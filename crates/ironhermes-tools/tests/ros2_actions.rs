//! Phase 49.8 Plan 07: ROS 2 action goals — send/poll/cancel with a watchdog
//! (Task 1, D-06/D-04/D-08) and the one stop path — `stop_all`,
//! `on_session_end`, the `stop` action, and the awaited `end_session_stop`,
//! including after the bridge dropped (Task 2, D-10) — against the
//! in-process mock rosbridge server (D-17).

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
use serde_json::{Value, json};

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
    actions: Vec<Ros2AllowEntry>,
    publish: Vec<Ros2AllowEntry>,
) -> Config {
    let mut cfg = Config::default();
    cfg.ros2.bridge_url = bridge_url.map(|s| s.to_string());
    cfg.ros2.actions = actions;
    cfg.ros2.publish = publish;
    cfg
}

fn config_with_caps(bridge_url: &str, actions: Vec<Ros2AllowEntry>, caps: Ros2Caps) -> Config {
    let mut cfg = fixed_config(Some(bridge_url), actions, vec![]);
    cfg.ros2.caps = caps;
    cfg
}

fn registry_with_ros2(cfg: Config) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register_ros2_tools(Ros2ConfigSource::Fixed(Arc::new(cfg)), None);
    registry
}

/// `advertise`/`publish`/`send_action_goal` are fire-and-forget rosbridge ops
/// — poll with a short deadline instead of asserting immediately after
/// dispatch (same race `49.8-01-SUMMARY.md` through `49.8-06-SUMMARY.md`
/// documented).
async fn wait_until(mut cond: impl FnMut() -> bool, deadline: Duration) {
    let start = std::time::Instant::now();
    loop {
        if cond() || start.elapsed() > deadline {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
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

// ---------------------------------------------------------------------------
// Task 1: send_goal / goal_status / cancel_goal, allowlist + watchdog
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn test_ros2_action_goal_roundtrip() {
    let mock = MockRosbridge::start_with(MockConfig {
        action_feedback: Some(json!({"progress": 0.5})),
        action_result_values: json!({"result_ok": true}),
        action_result_delay_ms: 100,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
        )],
        vec![],
    );
    let registry = registry_with_ros2(cfg);

    let sent = registry
        .dispatch(
            "ros2",
            json!({
                "action": "send_goal",
                "action_name": "/navigate_to_pose",
                "type": "nav2_msgs/action/NavigateToPose",
                "goal": {"pose": {"pose": {"position": {"x": 1.0}}}}
            }),
        )
        .await
        .expect("send_goal dispatch must not error");
    assert!(
        sent.starts_with("Goal "),
        "expected a goal id in the response; got: {sent}"
    );
    let goal_id = sent
        .split_whitespace()
        .nth(1)
        .expect("response must contain the goal id as the second word")
        .to_string();

    let status = registry
        .dispatch(
            "ros2",
            json!({"action": "goal_status", "goal_id": goal_id, "wait_secs": 2}),
        )
        .await
        .expect("goal_status dispatch must not error");
    assert!(
        status.contains("SUCCEEDED"),
        "expected SUCCEEDED; got: {status}"
    );
    assert!(
        status.contains("progress"),
        "expected the last feedback; got: {status}"
    );
    assert!(
        status.contains("result_ok"),
        "expected the result values; got: {status}"
    );

    let frames = mock.frames_with_op("send_action_goal");
    assert_eq!(
        frames.len(),
        1,
        "expected exactly one send_action_goal frame; got: {frames:?}"
    );
    assert_eq!(frames[0].value.get("feedback"), Some(&json!(true)));
    assert_eq!(
        frames[0].value.get("action_type").and_then(|v| v.as_str()),
        Some("nav2_msgs/action/NavigateToPose")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_undeclared_action_blocked() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url), vec![], vec![]);
    let registry = registry_with_ros2(cfg);

    let result = registry
        .dispatch(
            "ros2",
            json!({"action": "send_goal", "action_name": "/not_declared", "goal": {}}),
        )
        .await
        .expect("send_goal dispatch must not error");
    assert!(
        result.contains("is not permitted"),
        "undeclared send_goal must be refused; got: {result}"
    );
    assert_eq!(
        mock.connection_count(),
        0,
        "undeclared send_goal must never open a WebSocket connection (D-04 allowlist-before-I/O)"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_cancel_goal_roundtrip() {
    let mock = MockRosbridge::start_with(MockConfig {
        action_result_delay_ms: 10_000,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
        )],
        vec![],
    );
    let registry = registry_with_ros2(cfg);

    let sent = registry
        .dispatch(
            "ros2",
            json!({"action": "send_goal", "action_name": "/navigate_to_pose", "goal": {}}),
        )
        .await
        .expect("send_goal dispatch must not error");
    let goal_id = sent.split_whitespace().nth(1).unwrap().to_string();

    let cancelled = registry
        .dispatch("ros2", json!({"action": "cancel_goal", "goal_id": goal_id}))
        .await
        .expect("cancel_goal dispatch must not error");
    assert!(
        cancelled.contains("CANCELED"),
        "expected CANCELED; got: {cancelled}"
    );

    wait_until(
        || {
            mock.frames_with_op("cancel_action_goal")
                .iter()
                .any(|f| f.value.get("id").and_then(|v| v.as_str()) == Some(goal_id.as_str()))
        },
        Duration::from_secs(2),
    )
    .await;
    let cancel_frames: Vec<_> = mock
        .frames_with_op("cancel_action_goal")
        .into_iter()
        .filter(|f| f.value.get("id").and_then(|v| v.as_str()) == Some(goal_id.as_str()))
        .collect();
    assert_eq!(
        cancel_frames.len(),
        1,
        "expected exactly one cancel_action_goal frame for this goal; got: {cancel_frames:?}"
    );
    assert_eq!(
        cancel_frames[0]
            .value
            .get("action")
            .and_then(|v| v.as_str()),
        Some("/navigate_to_pose")
    );
}

/// 49.8-18 (Task 3, CR-01's `cancel_goal` half): an ignored (unacknowledged)
/// same-connection cancel must never be reported CANCELED.
#[tokio::test(flavor = "multi_thread")]
async fn ros2_cancel_goal_unacknowledged_is_not_reported_canceled() {
    let mock = MockRosbridge::start_with(MockConfig {
        ignore_cancel_action_goal: true,
        action_result_delay_ms: 10_000,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
        )],
        vec![],
    );
    let registry = registry_with_ros2(cfg);

    let sent = registry
        .dispatch(
            "ros2",
            json!({"action": "send_goal", "action_name": "/navigate_to_pose", "goal": {}}),
        )
        .await
        .expect("send_goal dispatch must not error");
    let goal_id = sent.split_whitespace().nth(1).unwrap().to_string();

    let reply = registry
        .dispatch("ros2", json!({"action": "cancel_goal", "goal_id": goal_id}))
        .await
        .expect("cancel_goal dispatch must not error");
    assert!(
        reply.contains("not confirmed"),
        "expected 'not confirmed'; got: {reply}"
    );
    assert!(
        !reply.contains("status=CANCELED"),
        "must never claim CANCELED without evidence; got: {reply}"
    );
}

/// 49.8-18 (Task 3, review 49.8-18 MEDIUM): a present-but-non-terminal (or
/// missing) cancel status must be reported as an explicit error, never
/// CANCELED.
#[tokio::test(flavor = "multi_thread")]
async fn ros2_cancel_goal_nonterminal_status_is_not_confirmed() {
    let mock = MockRosbridge::start_with(MockConfig {
        honoured_cancel_status: Some(json!(3)),
        action_result_delay_ms: 10_000,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
        )],
        vec![],
    );
    let registry = registry_with_ros2(cfg);

    let sent = registry
        .dispatch(
            "ros2",
            json!({"action": "send_goal", "action_name": "/navigate_to_pose", "goal": {}}),
        )
        .await
        .expect("send_goal dispatch must not error");
    let goal_id = sent.split_whitespace().nth(1).unwrap().to_string();

    let reply = registry
        .dispatch("ros2", json!({"action": "cancel_goal", "goal_id": goal_id}))
        .await
        .expect("cancel_goal dispatch must not error");
    assert!(
        reply.starts_with("Error:"),
        "expected an Error reply; got: {reply}"
    );
    assert!(
        reply.contains("not confirmed"),
        "expected 'not confirmed'; got: {reply}"
    );
    assert!(
        reply.contains("CANCELING"),
        "expected the observed CANCELING status named; got: {reply}"
    );
    assert!(
        !reply.contains("status=CANCELED"),
        "must never claim CANCELED without evidence; got: {reply}"
    );
}

/// 49.8-18 (Task 3, CR-01's `cancel_goal` half): a refused fallback cancel-all
/// (the goal's connection has closed, so cancel_goal reconnects and uses the
/// fallback) must be an explicit error, never CANCELED.
#[tokio::test(flavor = "multi_thread")]
async fn ros2_cancel_goal_fallback_refusal_is_an_error() {
    let mock = MockRosbridge::start_with(MockConfig {
        action_result_delay_ms: 10_000,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
        )],
        vec![],
    );
    let registry = registry_with_ros2(cfg);

    let sent = registry
        .dispatch(
            "ros2",
            json!({"action": "send_goal", "action_name": "/navigate_to_pose", "goal": {}}),
        )
        .await
        .expect("send_goal dispatch must not error");
    let goal_id = sent.split_whitespace().nth(1).unwrap().to_string();

    registry
        .ros2_session()
        .current_client()
        .await
        .expect("a client must be connected after send_goal above")
        .close();

    let reply = registry
        .dispatch("ros2", json!({"action": "cancel_goal", "goal_id": goal_id}))
        .await
        .expect("cancel_goal dispatch must not error");
    assert!(
        reply.starts_with("Error:"),
        "expected an Error reply; got: {reply}"
    );
    assert!(
        reply.contains("refused"),
        "expected 'refused'; got: {reply}"
    );
}

/// 49.8-18 (Task 3): a fallback cancel-all whose action server rejected the
/// cancel (ERROR_REJECTED) must be an explicit error naming the code.
#[tokio::test(flavor = "multi_thread")]
async fn ros2_cancel_goal_fallback_rejected_return_code_is_an_error() {
    let mut service_responses = HashMap::new();
    service_responses.insert(
        "/navigate_to_pose/_action/cancel_goal".to_string(),
        json!({"return_code": 1, "goals_canceling": []}),
    );
    let mock = MockRosbridge::start_with(MockConfig {
        action_result_delay_ms: 10_000,
        service_responses,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
        )],
        vec![],
    );
    let registry = registry_with_ros2(cfg);

    let sent = registry
        .dispatch(
            "ros2",
            json!({"action": "send_goal", "action_name": "/navigate_to_pose", "goal": {}}),
        )
        .await
        .expect("send_goal dispatch must not error");
    let goal_id = sent.split_whitespace().nth(1).unwrap().to_string();

    registry
        .ros2_session()
        .current_client()
        .await
        .expect("a client must be connected after send_goal above")
        .close();

    let reply = registry
        .dispatch("ros2", json!({"action": "cancel_goal", "goal_id": goal_id}))
        .await
        .expect("cancel_goal dispatch must not error");
    assert!(
        reply.starts_with("Error:"),
        "expected an Error reply; got: {reply}"
    );
    assert!(
        reply.contains("ERROR_REJECTED"),
        "expected 'ERROR_REJECTED'; got: {reply}"
    );
}

/// 49.8-18 (Task 3, review 49.8-18 HIGH): a fallback cancel-all the action
/// server merely ACCEPTED (goals CANCELING, not terminal) must say so —
/// never CANCELED, and never a hard error (the caller can retry).
#[tokio::test(flavor = "multi_thread")]
async fn ros2_cancel_goal_fallback_accepted_reports_canceling() {
    let goal_info: serde_json::Value = {
        let uuid: Vec<u8> = vec![9u8; 16];
        json!({"goal_id": {"uuid": uuid}, "stamp": {"sec": 4, "nanosec": 0}})
    };
    let mut service_responses = HashMap::new();
    service_responses.insert(
        "/navigate_to_pose/_action/cancel_goal".to_string(),
        json!({"return_code": 0, "goals_canceling": [goal_info]}),
    );
    let mock = MockRosbridge::start_with(MockConfig {
        action_result_delay_ms: 10_000,
        service_responses,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
        )],
        vec![],
    );
    let registry = registry_with_ros2(cfg);

    let sent = registry
        .dispatch(
            "ros2",
            json!({"action": "send_goal", "action_name": "/navigate_to_pose", "goal": {}}),
        )
        .await
        .expect("send_goal dispatch must not error");
    let goal_id = sent.split_whitespace().nth(1).unwrap().to_string();

    registry
        .ros2_session()
        .current_client()
        .await
        .expect("a client must be connected after send_goal above")
        .close();

    let reply = registry
        .dispatch("ros2", json!({"action": "cancel_goal", "goal_id": goal_id}))
        .await
        .expect("cancel_goal dispatch must not error");
    assert!(
        reply.contains("accepted"),
        "expected 'accepted'; got: {reply}"
    );
    assert!(
        reply.contains("CANCELING"),
        "expected 'CANCELING'; got: {reply}"
    );
    assert!(
        !reply.starts_with("Error:"),
        "an accepted cancel-all is not a hard error; got: {reply}"
    );
    assert!(
        !reply.contains("status=CANCELED"),
        "must never claim CANCELED without evidence; got: {reply}"
    );
}

/// 49.8-18 (Task 3): a fallback cancel-all with nothing left to cancel
/// (`{0, []}`) must be reported as not-cancelable, never CANCELED and never
/// a hard error.
#[tokio::test(flavor = "multi_thread")]
async fn ros2_cancel_goal_fallback_not_cancelable_is_reported() {
    let mut service_responses = HashMap::new();
    service_responses.insert(
        "/navigate_to_pose/_action/cancel_goal".to_string(),
        json!({"return_code": 0, "goals_canceling": []}),
    );
    let mock = MockRosbridge::start_with(MockConfig {
        action_result_delay_ms: 10_000,
        service_responses,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
        )],
        vec![],
    );
    let registry = registry_with_ros2(cfg);

    let sent = registry
        .dispatch(
            "ros2",
            json!({"action": "send_goal", "action_name": "/navigate_to_pose", "goal": {}}),
        )
        .await
        .expect("send_goal dispatch must not error");
    let goal_id = sent.split_whitespace().nth(1).unwrap().to_string();

    registry
        .ros2_session()
        .current_client()
        .await
        .expect("a client must be connected after send_goal above")
        .close();

    let reply = registry
        .dispatch("ros2", json!({"action": "cancel_goal", "goal_id": goal_id}))
        .await
        .expect("cancel_goal dispatch must not error");
    assert!(
        reply.contains(ironhermes_tools::ros2::session::GOAL_NOT_CANCELABLE_NOTE),
        "expected the not-cancelable note; got: {reply}"
    );
    assert!(
        !reply.starts_with("Error:"),
        "a not-cancelable answer is not a hard error; got: {reply}"
    );
    assert!(
        !reply.contains("status=CANCELED"),
        "must never claim CANCELED without evidence; got: {reply}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_goal_watchdog_cancels_after_max_goal_secs() {
    let mock = MockRosbridge::start_with(MockConfig {
        action_result_delay_ms: 10_000,
        ..Default::default()
    })
    .await;
    let caps = Ros2Caps {
        max_goal_secs: 0.5,
        ..Ros2Caps::default()
    };
    let cfg = config_with_caps(
        &mock.url,
        vec![allow_entry(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
        )],
        caps,
    );
    let registry = registry_with_ros2(cfg);

    let sent = registry
        .dispatch(
            "ros2",
            json!({"action": "send_goal", "action_name": "/navigate_to_pose", "goal": {}}),
        )
        .await
        .expect("send_goal dispatch must not error");
    let goal_id = sent.split_whitespace().nth(1).unwrap().to_string();

    wait_until(
        || {
            mock.frames_with_op("cancel_action_goal")
                .iter()
                .any(|f| f.value.get("id").and_then(|v| v.as_str()) == Some(goal_id.as_str()))
        },
        Duration::from_millis(1_500),
    )
    .await;
    assert!(
        mock.frames_with_op("cancel_action_goal").iter().any(|f| f
            .value
            .get("id")
            .and_then(|v| v.as_str())
            == Some(goal_id.as_str())),
        "watchdog must cancel the goal within 1.5s of max_goal_secs=0.5"
    );

    let status = registry
        .dispatch(
            "ros2",
            json!({"action": "goal_status", "goal_id": goal_id, "wait_secs": 1}),
        )
        .await
        .expect("goal_status dispatch must not error");
    assert!(
        status.contains("CANCELED"),
        "expected CANCELED after the watchdog fired; got: {status}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_action_ops_unsupported_message() {
    let mock = MockRosbridge::start_with(MockConfig {
        reject_action_ops: true,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
        )],
        vec![],
    );
    let registry = registry_with_ros2(cfg);

    let started = std::time::Instant::now();
    let result = registry
        .dispatch(
            "ros2",
            json!({"action": "send_goal", "action_name": "/navigate_to_pose", "goal": {}}),
        )
        .await
        .expect("send_goal dispatch must not error");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "must return within 1s"
    );
    assert!(
        result.contains("rosbridge_suite >= 2.0.0"),
        "expected the unsupported-ops message; got: {result}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_goal_limits_clamp() {
    let mock = MockRosbridge::start_with(MockConfig {
        action_result_delay_ms: 50,
        ..Default::default()
    })
    .await;
    let mut limits = BTreeMap::new();
    limits.insert("pose.pose.position.x".to_string(), [-2.0, 2.0]);
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry_with_limits(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
            limits,
        )],
        vec![],
    );
    let registry = registry_with_ros2(cfg);

    let result = registry
        .dispatch(
            "ros2",
            json!({
                "action": "send_goal",
                "action_name": "/navigate_to_pose",
                "goal": {"pose": {"pose": {"position": {"x": 10.0}}}}
            }),
        )
        .await
        .expect("send_goal dispatch must not error");
    assert!(
        result.contains("Clamped"),
        "result must name the clamp; got: {result}"
    );

    wait_until(
        || !mock.frames_with_op("send_action_goal").is_empty(),
        Duration::from_secs(2),
    )
    .await;
    let frames = mock.frames_with_op("send_action_goal");
    assert_eq!(frames.len(), 1);
    assert_eq!(
        frames[0]
            .value
            .get("args")
            .and_then(|a| a.get("pose"))
            .and_then(|p| p.get("pose"))
            .and_then(|p| p.get("position"))
            .and_then(|p| p.get("x")),
        Some(&json!(2.0)),
        "requested 10.0 must be clamped to 2.0 on the wire; got: {:?}",
        frames[0].value
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_goal_status_unknown_id() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
        )],
        vec![],
    );
    let registry = registry_with_ros2(cfg);

    let status = registry
        .dispatch(
            "ros2",
            json!({"action": "goal_status", "goal_id": "never-sent"}),
        )
        .await
        .expect("goal_status dispatch must not error");
    assert!(
        status.starts_with("Error:"),
        "unknown goal id must render an Error; got: {status}"
    );
}

/// WR-01 (review 49.8, carried forward): `goal_status` must never echo
/// `snap.error`'s raw text — it is set verbatim from rosbridge's
/// peer-controlled `status` frame `msg` field for any error the reader task
/// does not recognize as the fixed unsupported-ops refusal. A malicious or
/// MITM'd bridge could otherwise smuggle newlines/Markdown/ANSI escapes into
/// the tool's returned text, which surfaces directly in Telegram, the web
/// chat, the TUI, and cron/kanban logs (T-49.8-63) — the same sink
/// `cancel_goal`'s same-connection path was already hardened against
/// (`action_ops.rs:293-303`). This test injects a fault frame whose `msg`
/// deliberately does NOT match the unsupported-op heuristic
/// (`bridge_client.rs`'s `dispatch_inbound`, `"status"` arm), so it is
/// stored on `GoalSnapshot::error` exactly as given — proving `goal_status`
/// renders only the bounded, non-peer-controlled substitute text.
#[tokio::test(flavor = "multi_thread")]
async fn ros2_goal_status_never_echoes_raw_bridge_error_text() {
    let malicious = "actuator overheat\nfake heading: ---\n### CR-99: fake finding\x1b[31mred";
    let mock = MockRosbridge::start_with(MockConfig {
        action_result_delay_ms: 60_000,
        action_goal_status_error: Some(malicious.to_string()),
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
        )],
        vec![],
    );
    let registry = registry_with_ros2(cfg);

    let sent = registry
        .dispatch(
            "ros2",
            json!({"action": "send_goal", "action_name": "/navigate_to_pose", "goal": {}}),
        )
        .await
        .expect("send_goal dispatch must not error");
    assert!(
        sent.starts_with("Goal "),
        "goal must be accepted (not classified as ACTION_OPS_UNSUPPORTED); got: {sent}"
    );
    let goal_id = sent.split_whitespace().nth(1).unwrap().to_string();

    let deadline = std::time::Instant::now() + Duration::from_millis(1_500);
    let status = loop {
        let status = registry
            .dispatch("ros2", json!({"action": "goal_status", "goal_id": goal_id}))
            .await
            .expect("goal_status dispatch must not error");
        if status.contains("Error:") || std::time::Instant::now() > deadline {
            break status;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    assert!(
        status.contains("Error:"),
        "expected an Error section once the bridge's fault frame lands; got: {status}"
    );
    assert!(
        !status.contains(malicious),
        "goal_status must never echo the bridge's raw peer-controlled error text verbatim; got: {status}"
    );
    assert!(
        status.contains("bridge reported an error"),
        "expected the bounded, non-peer-controlled error message; got: {status}"
    );
}

// ---------------------------------------------------------------------------
// Task 2: stop_all / on_session_end / stop action / end_session_stop (D-10)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn test_ros2_session_end_stops_and_cancels() {
    let mock = MockRosbridge::start_with(MockConfig {
        action_result_delay_ms: 10_000,
        ..Default::default()
    })
    .await;
    let mut cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
        )],
        vec![allow_entry("/cmd_vel", "geometry_msgs/msg/Twist")],
    );
    cfg.ros2.caps = Ros2Caps::default();
    let registry = registry_with_ros2(cfg);

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
        .expect("motion dispatch must not error");

    registry
        .dispatch(
            "ros2",
            json!({"action": "send_goal", "action_name": "/navigate_to_pose", "goal": {}}),
        )
        .await
        .expect("send_goal dispatch must not error");

    let marker = std::time::Instant::now();

    let tool = registry
        .get_arc("ros2")
        .expect("ros2 tool must be registered");
    tool.on_session_end();

    wait_until(
        || {
            mock.frames_with_op("publish")
                .iter()
                .any(|f| f.at > marker && is_zero_twist(f))
                && !mock.frames_with_op("cancel_action_goal").is_empty()
        },
        Duration::from_secs(1),
    )
    .await;

    assert!(
        mock.frames_with_op("publish")
            .iter()
            .any(|f| f.at > marker && is_zero_twist(f)),
        "on_session_end must publish a zero Twist on /cmd_vel after it was called"
    );
    assert!(
        !mock.frames_with_op("cancel_action_goal").is_empty(),
        "on_session_end must cancel the pending goal"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_end_session_stop_awaits_and_reports() {
    let mock = MockRosbridge::start_with(MockConfig {
        action_result_delay_ms: 10_000,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
        )],
        vec![allow_entry("/cmd_vel", "geometry_msgs/msg/Twist")],
    );
    let registry = registry_with_ros2(cfg);

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
        .expect("motion dispatch must not error");
    registry
        .dispatch(
            "ros2",
            json!({"action": "send_goal", "action_name": "/navigate_to_pose", "goal": {}}),
        )
        .await
        .expect("send_goal dispatch must not error");

    let rw = tokio::sync::RwLock::new(registry);
    let report = ironhermes_tools::ros2::end_session_stop(&rw).await;

    assert!(
        report.contains("topic"),
        "report must name the stopped topic count; got: {report}"
    );
    assert!(
        report.contains("cancelled 1 goal"),
        "report must mention cancelling 1 goal; got: {report}"
    );

    let registry = rw.into_inner();
    // `end_session_stop` awaits `stop_all`, which awaits `client.cancel_action_goal(...)`'s
    // own `send_op` — that only proves the WS frame was queued client-side,
    // not that the mock's separate connection-handler task has read and
    // recorded it yet (the same fire-and-forget race 49.8-01 through
    // 49.8-06 documented for advertise/publish/unsubscribe). Poll with a
    // short deadline rather than asserting the instant `end_session_stop`
    // returns.
    wait_until(
        || !mock.frames_with_op("cancel_action_goal").is_empty(),
        Duration::from_secs(1),
    )
    .await;
    let cancel_frames = mock.frames_with_op("cancel_action_goal");
    assert!(
        !cancel_frames.is_empty(),
        "the cancel frame must be recorded shortly after end_session_stop returns"
    );
    let _ = registry; // keep alive to end of test
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_session_end_noop_when_unused() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url), vec![], vec![]);
    let registry = registry_with_ros2(cfg);

    let rw = tokio::sync::RwLock::new(registry);
    let report = ironhermes_tools::ros2::end_session_stop(&rw).await;
    assert!(
        report.contains("nothing to stop"),
        "expected 'nothing to stop'; got: {report}"
    );
    assert_eq!(
        mock.connection_count(),
        0,
        "a no-op stop must never open a connection"
    );
}

/// 49.8-18 (VERIFICATION gap #2, CR-01): a cancel-all the bridge refuses
/// (rosbridge answers `{result: false, values: "service not found"}` — the
/// mock's default for an unconfigured service) must be a reported goal
/// failure, must never be recorded clean, and a following
/// `session_end_hook_stop` must re-send it rather than coalescing. RED
/// against current source: `stop_all_as`'s fallback branch discards the
/// `ServiceResponse` and counts the socket write alone as `cancelled`.
#[tokio::test(flavor = "multi_thread")]
async fn ros2_session_end_refused_cancel_all_is_a_goal_failure_and_not_coalesced() {
    let mock = MockRosbridge::start_with(MockConfig {
        action_result_delay_ms: 10_000,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
        )],
        vec![],
    );
    let registry = registry_with_ros2(cfg);

    registry
        .dispatch(
            "ros2",
            json!({"action": "send_goal", "action_name": "/navigate_to_pose", "goal": {}}),
        )
        .await
        .expect("send_goal dispatch must not error");

    // Force the fallback path: close the current connection so stop_all_as
    // must reconnect and use the cancel-all fallback, not the same-connection
    // cancel_action_goal — `/navigate_to_pose/_action/cancel_goal` is left
    // unconfigured, so the mock refuses it.
    let session = registry.ros2_session();
    session
        .current_client()
        .await
        .expect("a client must be connected after send_goal above")
        .close();

    // Deviation from the plan's literal `end_session_stop(&rw)` call
    // (Rule 1): `end_session_stop`/`stop_all_live` always resolve the bridge
    // URL through `Ros2ConfigSource::Live` (T-49.8-08 — a test process must
    // never have its reconnect driven by a real robot URL), so after the
    // close above it can never reconnect to THIS test's mock server and the
    // reconnect falls through to the "no bridge connection" branch instead
    // of the fallback this test targets. `session.stop_all(Some(&mock.url))`
    // is the same direct call the sibling drop test
    // (`ros2_session_end_after_drop_reconnects_and_cancels_all`) already
    // uses to force the identical reconnect-then-fallback path.
    let report = session.stop_all(Some(&mock.url)).await;
    assert!(
        report.contains("1 goal failure(s)"),
        "a refused cancel-all must be a goal failure; got: {report}"
    );
    assert!(
        report.contains(ironhermes_tools::ros2::session::GOAL_CANCEL_NOT_CONFIRMED_NOTE),
        "report must name the not-confirmed note; got: {report}"
    );
    assert!(
        report.contains("refused"),
        "report must say the cancel was refused; got: {report}"
    );
    assert!(
        !report.contains("cancelled 1 goal"),
        "a refused cancel-all must never be reported as cancelled; got: {report}"
    );

    let hook_report = session.session_end_hook_stop(Some(&mock.url)).await;
    assert!(
        !hook_report.contains(ironhermes_tools::ros2::session::STOP_COALESCED_NOTE),
        "an unclean (refused) stop must never be coalesced onto by the next hook stop; got: {hook_report}"
    );

    wait_until(
        || {
            mock.frames_with_op("call_service")
                .iter()
                .filter(|f| {
                    f.value.get("service").and_then(|v| v.as_str())
                        == Some("/navigate_to_pose/_action/cancel_goal")
                })
                .count()
                >= 2
        },
        Duration::from_secs(2),
    )
    .await;
    let cancel_all_calls = mock
        .frames_with_op("call_service")
        .into_iter()
        .filter(|f| {
            f.value.get("service").and_then(|v| v.as_str())
                == Some("/navigate_to_pose/_action/cancel_goal")
        })
        .count();
    assert!(
        cancel_all_calls >= 2,
        "the next session-end hook stop must re-send the cancel-all rather than coalescing; got {cancel_all_calls} call(s)"
    );
}

fn goal_info_json(byte: u8, sec: i64) -> serde_json::Value {
    let uuid: Vec<u8> = vec![byte; 16];
    json!({
        "goal_id": {"uuid": uuid},
        "stamp": {"sec": sec, "nanosec": 0},
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_session_end_after_drop_reconnects_and_cancels_all() {
    let mut service_responses = HashMap::new();
    service_responses.insert(
        "/navigate_to_pose/_action/cancel_goal".to_string(),
        json!({"return_code": 0, "goals_canceling": [goal_info_json(1, 1)]}),
    );
    let mock = MockRosbridge::start_with(MockConfig {
        action_result_delay_ms: 10_000,
        drop_after_frames: Some(2),
        service_responses,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
        )],
        vec![allow_entry("/cmd_vel", "geometry_msgs/msg/Twist")],
    );
    let registry = registry_with_ros2(cfg);

    registry
        .dispatch(
            "ros2",
            json!({"action": "send_goal", "action_name": "/navigate_to_pose", "goal": {}}),
        )
        .await
        .expect("send_goal dispatch must not error");

    // Also drive a motion topic (in the same process, before the drop) so
    // stop_all has a topic to re-zero on the reconnected connection.
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
        .expect("motion dispatch must not error");

    // `drop_after_frames: Some(2)` makes the mock close connection 0
    // server-side after the goal + advertise frames — but a local TCP
    // socket can keep accepting writes for a while after the remote end
    // closes (it's not a synchronous signal), so relying on that timing
    // alone would make this test flake. Force the client-side state
    // deterministically instead: `Ros2BridgeClient::close()` is the exact
    // signal `Ros2Session::client_for`/`stop_all` already key off of, so
    // this reproduces "session holds a dead connection" without depending on
    // how fast the OS happens to notice the real one dying.
    let session = registry.ros2_session();
    session
        .current_client()
        .await
        .expect("a client must be connected after the goal + motion above")
        .close();

    let report = session.stop_all(Some(&mock.url)).await;
    assert!(
        report.contains("cancel-all fallback used"),
        "expected the cancel-all fallback to have been used; got: {report}"
    );
    // 49.8-18: an ACCEPTED cancel-all (goals CANCELING, not terminal) must be
    // a goal failure, never a success — the goal may still be running.
    assert!(
        report.contains("1 goal failure(s)"),
        "an accepted-but-not-terminal cancel-all must be a goal failure; got: {report}"
    );
    assert!(
        report.contains(ironhermes_tools::ros2::session::GOAL_CANCEL_NOT_CONFIRMED_NOTE),
        "report must name the not-confirmed note; got: {report}"
    );
    assert!(
        report.contains("CANCELING"),
        "report must say the goal is CANCELING, not terminal; got: {report}"
    );
    assert!(
        !report.contains("cancelled 1 goal"),
        "an accepted-but-not-terminal cancel-all must never be reported as cancelled; got: {report}"
    );

    wait_until(
        || {
            mock.frames_with_op("call_service").iter().any(|f| {
                f.conn == 1
                    && f.value.get("service").and_then(|v| v.as_str())
                        == Some("/navigate_to_pose/_action/cancel_goal")
            })
        },
        Duration::from_secs(2),
    )
    .await;
    let cancel_all_frames: Vec<_> = mock
        .frames_with_op("call_service")
        .into_iter()
        .filter(|f| {
            f.conn == 1
                && f.value.get("service").and_then(|v| v.as_str())
                    == Some("/navigate_to_pose/_action/cancel_goal")
        })
        .collect();
    assert_eq!(
        cancel_all_frames.len(),
        1,
        "expected exactly one cancel-all call_service on connection 1; got: {cancel_all_frames:?}"
    );
    assert_eq!(
        cancel_all_frames[0]
            .value
            .get("type")
            .and_then(|v| v.as_str()),
        Some("action_msgs/srv/CancelGoal")
    );
    let uuid = cancel_all_frames[0]
        .value
        .get("args")
        .and_then(|a| a.get("goal_info"))
        .and_then(|g| g.get("goal_id"))
        .and_then(|g| g.get("uuid"))
        .and_then(|v| v.as_array())
        .expect("goal_id.uuid must be an array");
    assert_eq!(uuid.len(), 16, "uuid must be 16 bytes; got: {uuid:?}");
    assert!(
        uuid.iter().all(|v| v == &json!(0)),
        "uuid must be all-zero; got: {uuid:?}"
    );
    let stamp = cancel_all_frames[0]
        .value
        .get("args")
        .and_then(|a| a.get("goal_info"))
        .and_then(|g| g.get("stamp"))
        .expect("goal_info.stamp must be present");
    assert_eq!(stamp.get("sec"), Some(&json!(0)));
    assert_eq!(stamp.get("nanosec"), Some(&json!(0)));

    wait_until(
        || {
            mock.frames_with_op("publish")
                .into_iter()
                .any(|f| f.conn == 1 && is_zero_twist(&f))
        },
        Duration::from_secs(2),
    )
    .await;
    assert!(
        mock.frames_with_op("publish")
            .into_iter()
            .any(|f| f.conn == 1 && is_zero_twist(&f)),
        "connection index 1 must have recorded a zero Twist on /cmd_vel"
    );

    // 49.8-18: the accepted-but-not-terminal cancel-all left the stop
    // unclean, so the next session-end hook stop must retry it rather than
    // coalescing.
    let hook_report = session.session_end_hook_stop(Some(&mock.url)).await;
    assert!(
        !hook_report.contains(ironhermes_tools::ros2::session::STOP_COALESCED_NOTE),
        "an accepted-but-not-terminal (unclean) stop must never be coalesced onto by the next hook stop; got: {hook_report}"
    );
    wait_until(
        || {
            mock.frames_with_op("call_service")
                .iter()
                .filter(|f| {
                    f.conn == 1
                        && f.value.get("service").and_then(|v| v.as_str())
                            == Some("/navigate_to_pose/_action/cancel_goal")
                })
                .count()
                >= 2
        },
        Duration::from_secs(2),
    )
    .await;
    let cancel_all_frames_conn1 = mock
        .frames_with_op("call_service")
        .into_iter()
        .filter(|f| {
            f.conn == 1
                && f.value.get("service").and_then(|v| v.as_str())
                    == Some("/navigate_to_pose/_action/cancel_goal")
        })
        .count();
    assert!(
        cancel_all_frames_conn1 >= 2,
        "the accepted-but-not-terminal cancel must be retried on the next stop; got {cancel_all_frames_conn1} call(s) on connection 1"
    );
}

/// 49.8-18: the cancel-all fallback cancels EVERY goal on an action server in
/// one call, so when the server accepted fewer goals than this process
/// tracks (partial acceptance), the outcome applies to every unfinished goal
/// of that action — all of them are counted not confirmed, none as
/// cancelled.
#[tokio::test(flavor = "multi_thread")]
async fn ros2_session_end_fallback_partial_acceptance_fails_every_goal() {
    let mut service_responses = HashMap::new();
    service_responses.insert(
        "/navigate_to_pose/_action/cancel_goal".to_string(),
        json!({"return_code": 0, "goals_canceling": [goal_info_json(2, 3)]}),
    );
    let mock = MockRosbridge::start_with(MockConfig {
        action_result_delay_ms: 10_000,
        service_responses,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
        )],
        vec![],
    );
    let registry = registry_with_ros2(cfg);

    registry
        .dispatch(
            "ros2",
            json!({"action": "send_goal", "action_name": "/navigate_to_pose", "goal": {}}),
        )
        .await
        .expect("first send_goal dispatch must not error");
    registry
        .dispatch(
            "ros2",
            json!({"action": "send_goal", "action_name": "/navigate_to_pose", "goal": {}}),
        )
        .await
        .expect("second send_goal dispatch must not error");

    let session = registry.ros2_session();
    session
        .current_client()
        .await
        .expect("a client must be connected after the two send_goal calls above")
        .close();

    let report = session.stop_all(Some(&mock.url)).await;
    assert!(
        report.contains("2 goal failure(s)"),
        "the cancel-all's one accepted answer must fail EVERY tracked goal of that action; got: {report}"
    );
    assert!(
        report.contains("CANCELING"),
        "report must say the goals are CANCELING, not terminal; got: {report}"
    );
    assert!(
        !report.contains("cancelled 1 goal"),
        "no goal may be reported cancelled; got: {report}"
    );
    assert!(
        !report.contains("cancelled 2 goal"),
        "no goal may be reported cancelled; got: {report}"
    );
}

/// 49.8-18: a cancel-all whose action server reports no goal entering
/// CANCELING (`{3, []}`, the shape rcl_action emits for an idle or
/// all-terminal server) must be reported distinctly as "not cancelable",
/// never as `cancelled`, and must NOT be treated as a failure — the settled
/// answer lets the coalescing retry loop converge.
#[tokio::test(flavor = "multi_thread")]
async fn ros2_session_end_fallback_not_cancelable_is_reported_not_cancelled() {
    let mut service_responses = HashMap::new();
    service_responses.insert(
        "/navigate_to_pose/_action/cancel_goal".to_string(),
        json!({"return_code": 3, "goals_canceling": []}),
    );
    let mock = MockRosbridge::start_with(MockConfig {
        action_result_delay_ms: 10_000,
        service_responses,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
        )],
        vec![],
    );
    let registry = registry_with_ros2(cfg);

    registry
        .dispatch(
            "ros2",
            json!({"action": "send_goal", "action_name": "/navigate_to_pose", "goal": {}}),
        )
        .await
        .expect("send_goal dispatch must not error");

    let session = registry.ros2_session();
    session
        .current_client()
        .await
        .expect("a client must be connected after send_goal above")
        .close();

    let report = session.stop_all(Some(&mock.url)).await;
    assert!(
        report.contains("cancelled 0 goal(s)"),
        "a not-cancelable answer must never be reported as cancelled; got: {report}"
    );
    assert!(
        report.contains(&format!(
            "1 goal(s) {}",
            ironhermes_tools::ros2::session::GOAL_NOT_CANCELABLE_NOTE
        )),
        "report must name the not-cancelable note; got: {report}"
    );
    assert!(
        !report.contains("goal failure"),
        "a not-cancelable answer must not fail the stop; got: {report}"
    );

    // The settled (not-cancelable) answer IS clean — the next hook stop
    // coalesces rather than resending, proving the retry loop converges once
    // the action server has nothing left to cancel.
    let hook_report = session.session_end_hook_stop(Some(&mock.url)).await;
    assert!(
        hook_report.contains(ironhermes_tools::ros2::session::STOP_COALESCED_NOTE),
        "a clean not-cancelable stop must be coalesced onto by the next hook stop; got: {hook_report}"
    );
}

/// 49.8-18 (Task 2): a same-connection cancel that the mock records but never
/// acts on (`ignore_cancel_action_goal`) must never be reported cancelled —
/// a socket write alone is not confirmation.
#[tokio::test(flavor = "multi_thread")]
async fn ros2_stop_all_unacknowledged_cancel_is_a_goal_failure() {
    let mock = MockRosbridge::start_with(MockConfig {
        ignore_cancel_action_goal: true,
        action_result_delay_ms: 10_000,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
        )],
        vec![],
    );
    let registry = registry_with_ros2(cfg);

    registry
        .dispatch(
            "ros2",
            json!({"action": "send_goal", "action_name": "/navigate_to_pose", "goal": {}}),
        )
        .await
        .expect("send_goal dispatch must not error");

    let session = registry.ros2_session();
    let report = session.stop_all(Some(&mock.url)).await;
    assert!(
        report.contains("1 goal failure(s)"),
        "an ignored (unacknowledged) cancel must be a goal failure; got: {report}"
    );
    assert!(
        report.contains(ironhermes_tools::ros2::session::GOAL_CANCEL_NOT_CONFIRMED_NOTE),
        "report must name the not-confirmed note; got: {report}"
    );
    assert!(
        report.contains("no terminal status"),
        "report must say no terminal status arrived within the cancel phase; got: {report}"
    );

    let hook_report = session.session_end_hook_stop(Some(&mock.url)).await;
    assert!(
        !hook_report.contains(ironhermes_tools::ros2::session::STOP_COALESCED_NOTE),
        "an unconfirmed same-connection cancel must never be coalesced onto by the next hook stop; got: {hook_report}"
    );
    wait_until(
        || mock.frames_with_op("cancel_action_goal").len() >= 2,
        Duration::from_secs(2),
    )
    .await;
    let cancel_frames = mock.frames_with_op("cancel_action_goal").len();
    assert!(
        cancel_frames >= 2,
        "the unconfirmed cancel must be retried on the next stop; got {cancel_frames} frame(s)"
    );
}

/// 49.8-18 (Task 2, review 49.8-18 MEDIUM): `bridge_client.rs`'s
/// `action_result` handler stores `result = Some(values)` unconditionally —
/// a present result with a missing or non-terminal `status` (only 4/5/6 are
/// terminal) must never be confirmed as a cancel.
#[tokio::test(flavor = "multi_thread")]
async fn ros2_stop_all_nonterminal_cancel_result_is_not_confirmed() {
    for honoured_cancel_status in [Some(json!(3)), Some(Value::Null)] {
        let mock = MockRosbridge::start_with(MockConfig {
            action_result_delay_ms: 10_000,
            honoured_cancel_status,
            ..Default::default()
        })
        .await;
        let cfg = fixed_config(
            Some(&mock.url),
            vec![allow_entry(
                "/navigate_to_pose",
                "nav2_msgs/action/NavigateToPose",
            )],
            vec![],
        );
        let registry = registry_with_ros2(cfg);

        registry
            .dispatch(
                "ros2",
                json!({"action": "send_goal", "action_name": "/navigate_to_pose", "goal": {}}),
            )
            .await
            .expect("send_goal dispatch must not error");

        let session = registry.ros2_session();
        let report = session.stop_all(Some(&mock.url)).await;
        assert!(
            report.contains("1 goal failure(s)"),
            "a non-terminal or missing cancel status must be a goal failure; got: {report}"
        );
        assert!(
            report.contains("missing or non-terminal status"),
            "report must say the status was missing or non-terminal; got: {report}"
        );
        assert!(
            !report.contains("cancelled 1 goal"),
            "must never be reported as cancelled; got: {report}"
        );
    }
}

/// 49.8-18 (Task 2, VERIFICATION gap #1): the whole goal-cancel phase costs
/// ONE `GOAL_CANCEL_PHASE_MS`, regardless of goal count, because every
/// unfinished goal's cancel runs concurrently and each distinct action's
/// cancel-all is deduplicated (sent once, its outcome applied to every
/// tracked goal of that action).
#[tokio::test(flavor = "multi_thread")]
async fn ros2_stop_all_goal_cancel_phase_is_bounded_and_deduplicated() {
    let mock = MockRosbridge::start_with(MockConfig {
        action_result_delay_ms: 10_000,
        unanswered_services: vec![
            "/alpha/_action/cancel_goal".to_string(),
            "/beta/_action/cancel_goal".to_string(),
            "/gamma/_action/cancel_goal".to_string(),
        ],
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![
            allow_entry("/alpha", "example_interfaces/action/Fibonacci"),
            allow_entry("/beta", "example_interfaces/action/Fibonacci"),
            allow_entry("/gamma", "example_interfaces/action/Fibonacci"),
        ],
        vec![],
    );
    let registry = registry_with_ros2(cfg);

    for action in ["/alpha", "/alpha", "/beta", "/gamma"] {
        registry
            .dispatch(
                "ros2",
                json!({"action": "send_goal", "action_name": action, "goal": {}}),
            )
            .await
            .unwrap_or_else(|e| panic!("send_goal for {action} must not error: {e}"));
    }

    let session = registry.ros2_session();
    session
        .current_client()
        .await
        .expect("a client must be connected after the four send_goal calls above")
        .close();

    let start = std::time::Instant::now();
    let report = session.stop_all(Some(&mock.url)).await;
    let elapsed = start.elapsed();

    assert!(
        elapsed
            < Duration::from_millis(ironhermes_tools::ros2::session::GOAL_CANCEL_PHASE_MS + 1500),
        "the goal-cancel phase must be bounded by one constant regardless of goal/action count; took {elapsed:?}"
    );
    assert!(
        report.contains("4 goal failure(s)"),
        "all four goals must fail (no cancel-all response within the phase); got: {report}"
    );
    assert!(
        report.contains("cancel-all fallback used"),
        "expected the cancel-all fallback; got: {report}"
    );
    assert!(
        report.contains(ironhermes_tools::ros2::session::GOAL_CANCEL_NOT_CONFIRMED_NOTE),
        "report must name the not-confirmed note; got: {report}"
    );

    let cancel_all_frames = mock
        .frames_with_op("call_service")
        .into_iter()
        .filter(|f| {
            f.conn == 1
                && matches!(
                    f.value.get("service").and_then(|v| v.as_str()),
                    Some("/alpha/_action/cancel_goal")
                        | Some("/beta/_action/cancel_goal")
                        | Some("/gamma/_action/cancel_goal")
                )
        })
        .count();
    assert_eq!(
        cancel_all_frames, 3,
        "expected exactly one cancel-all call per distinct action (deduplicated) on the reconnected connection; got {cancel_all_frames}"
    );
}

/// 49.8-18 (Task 2, review 49.8-18 MEDIUM): a regression guard that 12
/// concurrent same-connection cancels — all serialized through the single
/// connection's writer mutex — still all get confirmed inside the one
/// bounded phase.
#[tokio::test(flavor = "multi_thread")]
async fn ros2_stop_all_concurrent_same_connection_cancels_are_all_confirmed() {
    let mock = MockRosbridge::start_with(MockConfig {
        action_result_delay_ms: 10_000,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
        )],
        vec![],
    );
    let registry = registry_with_ros2(cfg);
    let session = registry.ros2_session();
    let client = session
        .client_for(&mock.url)
        .await
        .expect("client_for must connect to the mock");

    for _ in 0..12 {
        let id = client
            .send_action_goal(
                "/navigate_to_pose",
                "nav2_msgs/action/NavigateToPose",
                json!({}),
            )
            .await
            .expect("send_action_goal must not error");
        session
            .track_goal(
                &id,
                "/navigate_to_pose",
                "nav2_msgs/action/NavigateToPose",
                client.connection_id(),
            )
            .await;
    }

    let start = std::time::Instant::now();
    let report = session.stop_all(Some(&mock.url)).await;
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_millis(ironhermes_tools::ros2::session::GOAL_CANCEL_PHASE_MS),
        "12 concurrent same-connection cancels must all complete within the one bounded phase; took {elapsed:?}"
    );
    assert!(
        report.contains("cancelled 12 goal(s)"),
        "expected all 12 goals confirmed cancelled; got: {report}"
    );
    assert!(
        !report.contains("goal failure"),
        "no goal may fail; got: {report}"
    );

    // `cancel_action_goal` is a fire-and-forget write (bridge_client.rs's
    // `send_op`) — `stop_all` awaiting client-side confirmation via
    // `wait_goal_result` does not guarantee the mock's separate
    // connection-handler task has read and recorded every frame yet. Poll
    // with a short deadline rather than asserting the instant `stop_all`
    // returns (same class of race 49.8-01 through 49.8-17 documented).
    wait_until(
        || mock.frames_with_op("cancel_action_goal").len() >= 12,
        Duration::from_secs(1),
    )
    .await;
    let cancel_frames = mock.frames_with_op("cancel_action_goal").len();
    assert_eq!(
        cancel_frames, 12,
        "expected exactly 12 cancel_action_goal frames; got {cancel_frames}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_stop_action() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![],
        vec![allow_entry("/cmd_vel", "geometry_msgs/msg/Twist")],
    );
    let registry = Arc::new(registry_with_ros2(cfg));

    // 49.8-15 test-determinism repair: `ToolRegistry::dispatch` is a lazy
    // `async fn` — a pinned but unpolled future never starts, so the original
    // version of this test's stop ran with nothing driven and never actually
    // interrupted a motion. Spawning it over an `Arc`'d registry, then
    // hard-asserting a non-zero Twist was recorded, proves the motion was
    // really running before the stop.
    let registry_pub = registry.clone();
    let dispatch_handle = tokio::spawn(async move {
        registry_pub
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

    wait_until(
        || {
            mock.frames_with_op("publish").iter().any(|f| {
                f.value.get("topic").and_then(|v| v.as_str()) == Some("/cmd_vel")
                    && !is_zero_twist(f)
            })
        },
        Duration::from_secs(2),
    )
    .await;
    assert!(
        mock.frames_with_op("publish").iter().any(|f| f
            .value
            .get("topic")
            .and_then(|v| v.as_str())
            == Some("/cmd_vel")
            && !is_zero_twist(f)),
        "expected at least one non-zero Twist recorded before stopping — the motion never started"
    );

    let stop_result = registry
        .dispatch("ros2", json!({"action": "stop"}))
        .await
        .expect("stop dispatch must not error");
    assert!(
        stop_result.contains("stopped") || stop_result.contains("nothing to stop"),
        "expected a stop report; got: {stop_result}"
    );

    let _ = tokio::time::timeout(Duration::from_secs(3), dispatch_handle).await;

    wait_until(
        || {
            let frames = mock.frames_with_op("publish");
            !frames.is_empty() && is_zero_twist(frames.last().unwrap())
        },
        Duration::from_secs(2),
    )
    .await;
    let frames = mock.frames_with_op("publish");
    assert!(!frames.is_empty(), "expected at least one publish frame");
    assert!(
        is_zero_twist(frames.last().unwrap()),
        "the last recorded /cmd_vel frame must be a zero Twist; got: {:?}",
        frames.last().unwrap().value
    );
}

/// 49.8-18/49.8-19 gap (D-06/D-10/D-16, T-49.8-60/61/62/82): `send_goal`
/// calls `session.track_goal` BEFORE the `ACTION_OPS_UNSUPPORTED`
/// immediate-rejection check runs, so a goal sent against a rosbridge
/// without action-ops support stays tracked in `Ros2Session::goals`
/// forever — no watchdog is ever scheduled for it, and nothing prunes it
/// back out. A subsequent `stop_all` can therefore never re-confirm a clean
/// stop: this goal has no owning `action_result`, so it is always
/// "unfinished", and every cancel attempt against it (same-connection or
/// fallback) never lands on a real, tracked action-goal cancel handler
/// (the mock's `reject_action_ops` path never registers one), so the
/// cancel phase always finds it unconfirmed. RED against current source:
/// `stop_all` reports a permanent goal failure instead of "nothing to
/// stop"/a clean stop for the rest of the process's life.
#[tokio::test(flavor = "multi_thread")]
async fn ros2_action_ops_unsupported_goal_never_blocks_stop_all_forever() {
    let mock = MockRosbridge::start_with(MockConfig {
        reject_action_ops: true,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
        )],
        vec![],
    );
    let registry = registry_with_ros2(cfg);

    let sent = registry
        .dispatch(
            "ros2",
            json!({"action": "send_goal", "action_name": "/navigate_to_pose", "goal": {}}),
        )
        .await
        .expect("send_goal dispatch must not error");
    assert!(
        sent.contains("rosbridge_suite >= 2.0.0"),
        "expected the unsupported-ops message; got: {sent}"
    );

    let session = registry.ros2_session();

    // A goal was sent, so `track_goal` ran — but the caller was told the
    // action ops are unsupported, and no further use of this "goal" is
    // possible (no watchdog, no cancel target the mock ever honors). A
    // stop_all with nothing else in play should converge to a clean
    // stop/"nothing to stop", not report a permanent goal failure.
    let report = session.stop_all(Some(&mock.url)).await;
    assert!(
        !report.contains("goal failure"),
        "an ACTION_OPS_UNSUPPORTED goal must never permanently fail every future stop_all; got: {report}"
    );

    // Prove it is permanent, not merely slow to settle: retry once more —
    // real code that recovered (e.g. by pruning the goal) would converge to
    // a clean, coalescible stop on the second call.
    let hook_report = session.session_end_hook_stop(Some(&mock.url)).await;
    assert!(
        hook_report.contains(ironhermes_tools::ros2::session::STOP_COALESCED_NOTE)
            || !hook_report.contains("goal failure"),
        "stop_all must eventually recover a clean stop for this session; got: {hook_report}"
    );
}

/// WR-03 (review 49.8): a `stop_all`/`session_end_hook_stop` that races
/// `send_goal`'s `ACTION_OPS_UNSUPPORTED` confirmation window — landing its
/// goal snapshot after `track_goal` has run but before `send_goal`'s own
/// `untrack_goal` — must classify the still-tracked, action-ops-rejected
/// goal as settled (like `GOAL_NOT_CANCELABLE_NOTE`), never a goal failure:
/// an action-ops-unsupported bridge can, by construction, never confirm ANY
/// cancel on ANY goal, so counting it a failure would mark this one
/// `stop_all` call unclean and surface `GOAL_CANCEL_NOT_CONFIRMED_NOTE` —
/// the exact string `docs/robots/rospider-uat.md`'s checklist instructs an
/// operator to treat as an automatic fail — for a goal that, in reality,
/// was never accepted by the action server at all.
///
/// This constructs the exact state the race would leave behind directly
/// (`client.send_action_goal` + `session.track_goal`, bypassing
/// `action_ops::send_goal`'s own `untrack_goal` entirely) rather than
/// forcing the timing itself — `send_goal`'s immediate-rejection status
/// frame is keyed by goal id, so the client-side `GoalEntry::error` it sets
/// is indistinguishable from what a genuinely-raced `send_goal` call would
/// have left behind by the time a concurrent stop's cancel phase queries it.
#[tokio::test(flavor = "multi_thread")]
async fn ros2_stop_all_racing_send_goal_action_ops_rejection_is_not_a_goal_failure() {
    let mock = MockRosbridge::start_with(MockConfig {
        reject_action_ops: true,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
        )],
        vec![],
    );
    let registry = registry_with_ros2(cfg);
    let session = registry.ros2_session();

    let client = session
        .client_for(&mock.url)
        .await
        .expect("client_for must succeed");

    let goal_id = client
        .send_action_goal(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
            json!({}),
        )
        .await
        .expect("send_action_goal must not error");
    // Track it as `send_goal` would, BEFORE its own `untrack_goal` — this is
    // exactly the state a concurrent stop's snapshot would observe if it
    // landed in that window (action_ops.rs:112-130).
    session
        .track_goal(
            &goal_id,
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
            client.connection_id(),
        )
        .await;

    wait_until(
        || {
            client
                .goal_snapshot(&goal_id)
                .and_then(|s| s.error)
                .is_some()
        },
        Duration::from_secs(1),
    )
    .await;
    assert!(
        client
            .goal_snapshot(&goal_id)
            .and_then(|s| s.error)
            .is_some(),
        "expected the mock's immediate ACTION_OPS_UNSUPPORTED rejection to have landed before stopping"
    );

    let report = session.stop_all(Some(&mock.url)).await;
    assert!(
        !report.contains("goal failure"),
        "an action-ops-unsupported goal must never count as a stop-cancel failure (WR-03); got: {report}"
    );
    assert!(
        !report.contains(ironhermes_tools::ros2::session::GOAL_CANCEL_NOT_CONFIRMED_NOTE),
        "must never surface the operator-facing 'not confirmed' note for a bridge that can never \
         confirm any cancel; got: {report}"
    );
    assert!(
        report.contains(ironhermes_tools::ros2::session::GOAL_NOT_CANCELABLE_NOTE),
        "expected the goal to be reported settled via the not-cancelable note, mirroring how that \
         note already treats a cancel-all with nothing to cancel; got: {report}"
    );
}

#[test]
fn ros2_on_session_end_outside_runtime_is_safe() {
    let mut cfg = Config::default();
    cfg.ros2.bridge_url = Some("ws://127.0.0.1:1".to_string());
    let mut registry = ToolRegistry::new();
    registry.register_ros2_tools(Ros2ConfigSource::Fixed(Arc::new(cfg)), None);

    let tool = registry
        .get_arc("ros2")
        .expect("ros2 tool must be registered");
    // No #[tokio::test] runtime here — must log and return, never panic.
    tool.on_session_end();
}

// ---------------------------------------------------------------------------
// Gap closure 49.8-11: with_session_end_stop (D-10/D-16)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_with_session_end_stop_turn_error_cancels_goal() {
    let mock = MockRosbridge::start_with(MockConfig {
        action_result_delay_ms: 10_000,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
        )],
        vec![allow_entry("/cmd_vel", "geometry_msgs/msg/Twist")],
    );
    let registry = tokio::sync::RwLock::new(registry_with_ros2(cfg));

    let turn = async {
        registry
            .read()
            .await
            .dispatch(
                "ros2",
                json!({"action": "send_goal", "action_name": "/navigate_to_pose", "goal": {}}),
            )
            .await
            .expect("send_goal dispatch must not error");
        Err::<(), anyhow::Error>(anyhow::anyhow!("simulated provider error after send_goal"))
    };

    let (outcome, report) = ironhermes_tools::ros2::with_session_end_stop(&registry, turn).await;

    let err = outcome.expect_err("turn resolved Err and must be returned unchanged");
    assert_eq!(
        err.to_string(),
        "simulated provider error after send_goal",
        "the returned error must be the exact turn error, unchanged"
    );
    assert!(
        report.contains("cancelled 1 goal"),
        "report must mention cancelling 1 goal; got: {report}"
    );

    wait_until(
        || !mock.frames_with_op("cancel_action_goal").is_empty(),
        Duration::from_secs(1),
    )
    .await;
    let cancel_frames = mock.frames_with_op("cancel_action_goal");
    assert_eq!(
        cancel_frames.len(),
        1,
        "exactly one cancel_action_goal frame must be recorded; got {}",
        cancel_frames.len()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_with_session_end_stop_turn_ok_passes_through() {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;
    let cfg = fixed_config(Some(&mock.url), vec![], vec![]);
    let registry = tokio::sync::RwLock::new(registry_with_ros2(cfg));

    let turn = async { 42u32 };

    let (outcome, report) = ironhermes_tools::ros2::with_session_end_stop(&registry, turn).await;

    assert_eq!(
        outcome, 42,
        "a turn that never touches ros2 must pass its value through unchanged"
    );
    assert!(
        report.contains("nothing to stop"),
        "expected 'nothing to stop'; got: {report}"
    );
    assert_eq!(
        mock.connection_count(),
        0,
        "a turn that never touches ros2 must never open a connection"
    );
}

/// A turn future with a declared `()` output, so the diverging `panic!` tail
/// coerces cleanly instead of leaving the compiler to infer `Fut::Output`
/// from a never-typed block with no other constraint.
async fn panicking_turn_after_send_goal(registry: Arc<tokio::sync::RwLock<ToolRegistry>>) {
    registry
        .read()
        .await
        .dispatch(
            "ros2",
            json!({"action": "send_goal", "action_name": "/navigate_to_pose", "goal": {}}),
        )
        .await
        .expect("send_goal dispatch must not error");
    panic!("simulated turn panic after send_goal");
}

#[tokio::test(flavor = "multi_thread")]
async fn ros2_with_session_end_stop_turn_panic_still_cancels() {
    let mock = MockRosbridge::start_with(MockConfig {
        action_result_delay_ms: 10_000,
        ..Default::default()
    })
    .await;
    let cfg = fixed_config(
        Some(&mock.url),
        vec![allow_entry(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
        )],
        vec![allow_entry("/cmd_vel", "geometry_msgs/msg/Twist")],
    );
    let registry = Arc::new(tokio::sync::RwLock::new(registry_with_ros2(cfg)));
    let registry_for_task = registry.clone();

    let handle = tokio::spawn(async move {
        let turn = panicking_turn_after_send_goal(registry_for_task.clone());
        let _ = ironhermes_tools::ros2::with_session_end_stop(&registry_for_task, turn).await;
    });

    let join_result = handle.await;
    assert!(
        join_result.is_err() && join_result.unwrap_err().is_panic(),
        "the spawned task must have panicked"
    );

    wait_until(
        || !mock.frames_with_op("cancel_action_goal").is_empty(),
        Duration::from_secs(1),
    )
    .await;
    assert!(
        !mock.frames_with_op("cancel_action_goal").is_empty(),
        "the cancel frame must be recorded even though the turn panicked"
    );
}

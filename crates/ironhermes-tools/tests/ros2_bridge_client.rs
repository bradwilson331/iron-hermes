//! Client-level integration tests for `Ros2BridgeClient`'s op surface
//! completed in Phase 49.8 Plan 02 — subscribe demux (D-12 building block),
//! the action goal lifecycle with cancel (D-06 building block), rosbridge
//! `status` error frames, non-finite JSON sanitization, the per-connection
//! size cap, close semantics, and `Ros2Session` reconnect — against the
//! in-process mock rosbridge server (D-17).
//!
//! `ros2_client_subscribe_demux_and_unsubscribe` is this plan's RED/GREEN
//! target test — see `49.8-02-SUMMARY.md`'s TDD Gate Compliance section for
//! the recorded RED evidence.

#[path = "support/mock_rosbridge.rs"]
mod mock_rosbridge;

use std::time::Duration;

use ironhermes_tools::ros2::bridge_client::{
    ACTION_OPS_UNSUPPORTED, ConnectOptions, Ros2BridgeClient,
};
use ironhermes_tools::ros2::session::Ros2Session;
use mock_rosbridge::{MockConfig, MockRosbridge};
use serde_json::json;

/// Polls `client.is_closed()` until true or `deadline` elapses.
async fn wait_for_closed(client: &Ros2BridgeClient, deadline: Duration) -> bool {
    let start = std::time::Instant::now();
    loop {
        if client.is_closed() {
            return true;
        }
        if start.elapsed() > deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// `unsubscribe`/`cancel_action_goal` are fire-and-forget rosbridge ops with
/// no client-side ack — the client's `.send().await` returning only proves
/// the local write was queued, not that the mock's own connection-handler
/// task has scheduled, read, and recorded the frame yet (same race
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
// ros2_client_subscribe_demux_and_unsubscribe (D-12 building block, RED/GREEN target)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_client_subscribe_demux_and_unsubscribe() {
    let mock = MockRosbridge::start_with(MockConfig {
        topic_messages: [(
            "/chatter".to_string(),
            vec![json!({"data": "one"}), json!({"data": "two"})],
        )]
        .into_iter()
        .collect(),
        publish_interval_ms: 20,
        ..Default::default()
    })
    .await;

    let client = Ros2BridgeClient::connect(&mock.url).await.expect("connect");

    let mut sub_a = client
        .subscribe("/chatter", Some("std_msgs/msg/String"), 50, 5)
        .await
        .expect("subscribe a");
    let mut sub_b = client
        .subscribe("/chatter", Some("std_msgs/msg/String"), 50, 5)
        .await
        .expect("subscribe b");

    let msg_a = tokio::time::timeout(Duration::from_secs(2), sub_a.rx.recv())
        .await
        .expect("sub_a must receive a message before the deadline")
        .expect("sub_a channel must not be closed");
    let msg_b = tokio::time::timeout(Duration::from_secs(2), sub_b.rx.recv())
        .await
        .expect("sub_b must receive a message before the deadline")
        .expect("sub_b channel must not be closed");
    assert_eq!(
        msg_a,
        json!({"data": "one"}),
        "sub_a must receive the mock's canned message"
    );
    assert_eq!(
        msg_b,
        json!({"data": "one"}),
        "sub_b (same topic) must independently receive it too"
    );

    client.unsubscribe(&sub_a).await.expect("unsubscribe a");
    wait_for_frame_count(&mock, "unsubscribe", 1, Duration::from_secs(2)).await;

    let subscribe_frames = mock.frames_with_op("subscribe");
    assert_eq!(
        subscribe_frames.len(),
        2,
        "expected 2 subscribe frames; got {subscribe_frames:?}"
    );
    for f in &subscribe_frames {
        assert!(
            f.value.get("throttle_rate").is_some(),
            "every subscribe frame must carry throttle_rate (RESEARCH.md Pitfall 4): {f:?}"
        );
        assert!(
            f.value.get("queue_length").is_some(),
            "every subscribe frame must carry queue_length (RESEARCH.md Pitfall 4): {f:?}"
        );
    }

    let unsubscribe_frames = mock.frames_with_op("unsubscribe");
    assert_eq!(
        unsubscribe_frames.len(),
        1,
        "expected 1 unsubscribe frame; got {unsubscribe_frames:?}"
    );
    assert_eq!(
        unsubscribe_frames[0]
            .value
            .get("id")
            .and_then(|v| v.as_str()),
        Some(sub_a.id.as_str()),
        "unsubscribe frame must carry the subscribe frame's id"
    );
}

// ---------------------------------------------------------------------------
// ros2_client_action_goal_feedback_and_result (D-06 building block)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_client_action_goal_feedback_and_result() {
    let mock = MockRosbridge::start_with(MockConfig {
        action_feedback: Some(json!({"progress": 0.5})),
        action_result_values: json!(true),
        action_result_delay_ms: 100,
        ..Default::default()
    })
    .await;

    let client = Ros2BridgeClient::connect(&mock.url).await.expect("connect");
    let goal_id = client
        .send_action_goal(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
            json!({}),
        )
        .await
        .expect("send_action_goal");
    assert!(
        !goal_id.is_empty(),
        "send_action_goal must return a non-empty id"
    );

    let snapshot = client
        .wait_goal_result(&goal_id, Duration::from_secs(3))
        .await
        .expect("goal must resolve before the deadline");

    assert_eq!(
        snapshot.status,
        Some(4),
        "expected SUCCEEDED status; got {snapshot:?}"
    );
    assert_eq!(snapshot.succeeded, Some(true));
    assert_eq!(
        snapshot.result,
        Some(json!(true)),
        "expected the mock's result values"
    );
    assert_eq!(
        snapshot.last_feedback,
        Some(json!({"progress": 0.5})),
        "expected the mock's stored last_feedback"
    );
}

// ---------------------------------------------------------------------------
// ros2_client_cancel_goal_yields_canceled_status (D-06 building block)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_client_cancel_goal_yields_canceled_status() {
    let mock = MockRosbridge::start_with(MockConfig {
        action_result_delay_ms: 5_000,
        ..Default::default()
    })
    .await;

    let client = Ros2BridgeClient::connect(&mock.url).await.expect("connect");
    let goal_id = client
        .send_action_goal(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
            json!({}),
        )
        .await
        .expect("send_action_goal");

    // Give the mock a moment to record the goal before cancelling.
    tokio::time::sleep(Duration::from_millis(50)).await;
    client
        .cancel_action_goal("/navigate_to_pose", &goal_id)
        .await
        .expect("cancel_action_goal");

    let snapshot = client
        .wait_goal_result(&goal_id, Duration::from_secs(3))
        .await
        .expect("goal must resolve before the deadline");
    assert_eq!(
        snapshot.status,
        Some(5),
        "expected CANCELED status; got {snapshot:?}"
    );

    let cancel_frames = mock.frames_with_op("cancel_action_goal");
    assert_eq!(
        cancel_frames.len(),
        1,
        "expected 1 cancel_action_goal frame; got {cancel_frames:?}"
    );
    assert_eq!(
        cancel_frames[0].value.get("id").and_then(|v| v.as_str()),
        Some(goal_id.as_str()),
        "cancel_action_goal frame must carry the goal id"
    );
    assert_eq!(
        cancel_frames[0]
            .value
            .get("action")
            .and_then(|v| v.as_str()),
        Some("/navigate_to_pose"),
        "cancel_action_goal frame must carry the action name"
    );
}

// ---------------------------------------------------------------------------
// ros2_client_unknown_action_op_maps_to_unsupported (D-06/T-49.8-28)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_client_unknown_action_op_maps_to_unsupported() {
    let mock = MockRosbridge::start_with(MockConfig {
        reject_action_ops: true,
        ..Default::default()
    })
    .await;

    let client = Ros2BridgeClient::connect(&mock.url).await.expect("connect");
    let goal_id = client
        .send_action_goal(
            "/navigate_to_pose",
            "nav2_msgs/action/NavigateToPose",
            json!({}),
        )
        .await
        .expect("send_action_goal");

    let snapshot = client
        .wait_goal_result(&goal_id, Duration::from_secs(2))
        .await
        .expect("goal must resolve (with an error) before the deadline");

    assert_eq!(
        snapshot.error.as_deref(),
        Some(ACTION_OPS_UNSUPPORTED),
        "an unsupported action op must set the fixed ACTION_OPS_UNSUPPORTED error; got {snapshot:?}"
    );
}

// ---------------------------------------------------------------------------
// ros2_client_sanitizes_nonfinite_numbers (D-01/T-49.8-13)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_client_sanitizes_nonfinite_numbers() {
    let raw = r#"{"op":"publish","topic":"/imu","msg":{"a":NaN,"b":Infinity,"c":-Infinity,"note":"NaN is a string here"}}"#;
    let mock = MockRosbridge::start_with(MockConfig {
        raw_topic_frames: [("/imu".to_string(), vec![raw.to_string()])]
            .into_iter()
            .collect(),
        ..Default::default()
    })
    .await;

    let client = Ros2BridgeClient::connect(&mock.url).await.expect("connect");
    let mut sub = client
        .subscribe("/imu", None, 0, 1)
        .await
        .expect("subscribe");

    let msg = tokio::time::timeout(Duration::from_secs(2), sub.rx.recv())
        .await
        .expect("must receive the sanitized message before the deadline")
        .expect("channel must not be closed");

    assert_eq!(msg["a"], json!(null), "bare NaN must become null: {msg}");
    assert_eq!(
        msg["b"],
        json!(null),
        "bare Infinity must become null: {msg}"
    );
    assert_eq!(
        msg["c"],
        json!(null),
        "bare -Infinity must become null: {msg}"
    );
    assert_eq!(
        msg["note"],
        json!("NaN is a string here"),
        "a string VALUE containing the text NaN must be unchanged: {msg}"
    );
}

// ---------------------------------------------------------------------------
// ros2_client_marks_closed_and_fails_pending_on_drop (T-49.8-12)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_client_marks_closed_and_fails_pending_on_drop() {
    let mock = MockRosbridge::start_with(MockConfig {
        drop_after_frames: Some(1),
        ..Default::default()
    })
    .await;

    let client = Ros2BridgeClient::connect(&mock.url).await.expect("connect");
    let mut sub = client
        .subscribe("/chatter", None, 50, 5)
        .await
        .expect("subscribe");

    // The subscribe frame above is inbound frame #1 — it triggers the mock's
    // drop_after_frames(1). This call_service is sent after that, so it must
    // fail one way or another (send error or the pending map being cleared).
    let call_result = client
        .call_service("/rosapi/topics", None, json!({}), Duration::from_secs(2))
        .await;
    assert!(
        call_result.is_err(),
        "pending call_service must fail once the connection drops; got {call_result:?}"
    );

    let closed = wait_for_closed(&client, Duration::from_secs(2)).await;
    assert!(
        closed,
        "client must report closed after the mock drops the connection"
    );

    let recv = tokio::time::timeout(Duration::from_secs(2), sub.rx.recv()).await;
    assert!(
        matches!(recv, Ok(None)),
        "subscription receiver must end (None) once the connection drops; got {recv:?}"
    );
}

// ---------------------------------------------------------------------------
// ros2_client_rejects_oversized_frame (T-49.8-09)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_client_rejects_oversized_frame() {
    let mock = MockRosbridge::start_with(MockConfig {
        oversized_frame_bytes: Some(100 * 1024),
        ..Default::default()
    })
    .await;

    let client = Ros2BridgeClient::connect_with(
        &mock.url,
        ConnectOptions {
            max_message_bytes: 65_536,
        },
    )
    .await
    .expect("connect_with");

    let mut sub = client
        .subscribe("/big", None, 0, 1)
        .await
        .expect("subscribe");

    let recv = tokio::time::timeout(Duration::from_secs(2), sub.rx.recv()).await;
    assert!(
        matches!(recv, Ok(None)),
        "subscription receiver must end once the oversized frame closes the connection; got {recv:?}"
    );

    let closed = wait_for_closed(&client, Duration::from_secs(2)).await;
    assert!(
        closed,
        "client must report closed after receiving an oversized frame, without panicking"
    );
}

// ---------------------------------------------------------------------------
// ros2_session_reconnects_after_drop
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_session_reconnects_after_drop() {
    let mock = MockRosbridge::start_with(MockConfig {
        drop_after_frames: Some(1),
        ..Default::default()
    })
    .await;

    let session = Ros2Session::new();
    let client1 = session
        .client_for(&mock.url)
        .await
        .expect("first client_for");
    assert!(
        !client1.is_closed(),
        "freshly connected client must be open"
    );

    // Drive one inbound frame so the mock's drop_after_frames(1) closes the
    // FIRST connection. The call itself is expected to fail — that's the
    // point, not a bug in the test.
    let _ = client1
        .call_service("/rosapi/topics", None, json!({}), Duration::from_secs(2))
        .await;

    let closed = wait_for_closed(&client1, Duration::from_secs(2)).await;
    assert!(
        closed,
        "first connection must be marked closed after the drop"
    );

    let client2 = session
        .client_for(&mock.url)
        .await
        .expect("second client_for");
    assert!(!client2.is_closed(), "reconnected client must be open");
    assert_eq!(
        mock.connection_count(),
        2,
        "expected exactly 2 connections after the reconnect; got {}",
        mock.connection_count()
    );
}

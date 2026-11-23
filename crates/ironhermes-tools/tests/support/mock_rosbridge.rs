#![allow(dead_code)]
//! In-process mock rosbridge v2 JSON WebSocket server for CI (Phase 49.8 D-17).
//!
//! Mirrors `crates/ironhermes-gateway/tests/buzz_relay_roundtrip.rs`'s
//! `LocalRelay` shape exactly: bind an ephemeral loopback port, spawn an accept
//! loop, `Notify`-based shutdown on `Drop`. Every inbound frame is recorded
//! with its connection index and arrival order so tests can assert allowlist
//! enforcement (zero connections on a blocked write) and op ordering
//! (advertise-before-publish). 49.8-01 implemented frame recording plus a
//! `/rosapi/topics` and configured-service `call_service` responder. This
//! plan (49.8-02) extends the responder with subscribe emission (cycling
//! canned messages or verbatim raw frames), the rest of the rosapi
//! introspection surface, the action-goal lifecycle (feedback, result,
//! cancel, and an unsupported-op rejection), a first-connection-only drop,
//! and an oversized-frame injector — every scenario 49.8-04 through 49.8-09
//! configure via [`MockConfig`], never editing this file again.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures::stream::SplitSink;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// One inbound rosbridge JSON frame recorded by the mock server, in arrival
/// order across every connection it has served.
#[derive(Debug, Clone)]
pub struct RecordedFrame {
    pub at: Instant,
    pub conn: usize,
    pub value: Value,
}

/// Configuration for one `MockRosbridge` instance.
#[derive(Debug, Clone, Default)]
pub struct MockConfig {
    /// `(topic, type)` pairs answered by a `call_service` on `/rosapi/topics`.
    pub topics: Vec<(String, String)>,
    /// service name -> `values` payload returned on a successful `call_service`
    /// for any service NOT covered by the built-in rosapi responders below. A
    /// service with no entry here answers `{result: false, values: "service
    /// not found"}`.
    pub service_responses: HashMap<String, Value>,

    /// 49.8-02: topic -> canned message list. On `subscribe`, the mock cycles
    /// through this list, emitting one `{op: publish, topic, msg}` frame every
    /// `publish_interval_ms` until `unsubscribe` (for that subscription id) or
    /// disconnect. Ignored for a topic also present in `raw_topic_frames` or
    /// when `oversized_frame_bytes` is set (those take priority).
    pub topic_messages: HashMap<String, Vec<Value>>,
    /// Interval between cycled `topic_messages` emissions. 0 is treated as 1ms.
    pub publish_interval_ms: u64,
    /// topic -> verbatim text frames sent once, in order, on `subscribe` —
    /// used to prove non-finite JSON sanitization, since these bypass
    /// `serde_json` entirely on the mock side and can contain bare
    /// `NaN`/`Infinity` tokens a normal `Value` could never encode.
    pub raw_topic_frames: HashMap<String, Vec<String>>,

    /// `/rosapi/nodes` response.
    pub nodes: Vec<String>,
    /// `(name, type)` pairs answered by `/rosapi/services` (names only) and
    /// `/rosapi/service_type` (by name).
    pub services: Vec<(String, String)>,
    /// `/rosapi/action_servers` response.
    pub action_servers: Vec<String>,
    /// topic -> type, answered by `/rosapi/topic_type`.
    pub topic_types: HashMap<String, String>,
    /// service -> type, answered by `/rosapi/service_type` (checked after
    /// `services` above — lets a test supply a type without a full services
    /// listing entry).
    pub service_types: HashMap<String, String>,
    /// message/service/action type -> typedefs `Value`, answered by
    /// `/rosapi/message_details`.
    pub message_details: HashMap<String, Value>,

    /// If set, the mock sends one `action_feedback` frame ~50ms after
    /// `send_action_goal` (unless cancelled first).
    pub action_feedback: Option<Value>,
    /// The `values` payload of the `action_result` frame sent on a
    /// successful (non-cancelled) goal completion — mirrors a real
    /// rosbridge's goal-result MESSAGE fields (e.g. `delta` for
    /// turtlesim/action/RotateAbsolute), never a `{status, result}` wrapper
    /// (49.8-10 live-harness finding: `status`/`result` are top-level on
    /// the real wire frame, not nested under `values`).
    pub action_result_values: Value,
    /// Delay (ms) after `send_action_goal` before the mock sends
    /// `action_result` (status 4/SUCCEEDED) unless cancelled first, in which
    /// case `cancel_action_goal` immediately answers with status
    /// 5/CANCELED.
    pub action_result_delay_ms: u64,
    /// When true, `send_action_goal`/`cancel_action_goal` are answered with a
    /// rosbridge `status` error frame naming the op as unknown — reproducing
    /// a pre-2.0.0 rosbridge_suite that lacks action ops (D-06).
    pub reject_action_ops: bool,

    /// Close ONLY the first connection (`conn == 0`) after it has sent this
    /// many inbound frames. Later connections are never affected — this
    /// proves `Ros2Session::client_for` reconnects rather than every
    /// connection this server ever serves being capped.
    pub drop_after_frames: Option<usize>,
    /// On `subscribe`, send one raw text frame of this many bytes instead of
    /// any `topic_messages`/`raw_topic_frames` response — proves the
    /// client-side size cap closes the connection without a panic.
    pub oversized_frame_bytes: Option<usize>,

    /// 49.8-18: when true, a `cancel_action_goal` frame is recorded (as
    /// always) but the goal's cancel flag is never set, so the goal keeps
    /// running until `action_result_delay_ms` elapses — proves an ignored
    /// (unacknowledged) same-connection cancel is a goal failure, never a
    /// silent success. Inert by default (every existing test is unchanged).
    pub ignore_cancel_action_goal: bool,
    /// 49.8-18: a `call_service` frame whose `service` is listed here is
    /// recorded (as always) but never answered — proves a cancel-all that
    /// gets no response within the goal-cancel phase deadline is a reported
    /// failure, not a silent success. Inert by default (empty).
    pub unanswered_services: Vec<String>,
    /// 49.8-18: the `status` field of the `action_result` a HONOURED
    /// same-connection cancel produces. `None` (default) sends `5`
    /// (CANCELED, today's behavior); `Some(Value::Null)` omits the `status`
    /// field entirely (a missing status); `Some(v)` sends `v` verbatim
    /// (e.g. `3`/CANCELING, a non-terminal status). Inert by default.
    pub honoured_cancel_status: Option<Value>,

    /// 49.8-19 (VERIFICATION gap #7, review 49.8-19): a deterministic
    /// barrier — the connection whose index equals `.0` is accepted at TCP
    /// level as always (`connection_count()` already counts it), but
    /// `handle_connection` awaits `.1` (in a `select!` with shutdown)
    /// BEFORE `accept_async` — so the WebSocket handshake itself is held
    /// until the test calls `notify_one()`. Lets a test park a stop task
    /// provably mid-reconnect (well inside `RECONNECT_STOP_TIMEOUT_SECS`)
    /// instead of racing burst timing. Inert by default (`None`).
    pub gate_connection: Option<(usize, Arc<Notify>)>,

    /// WR-01 regression coverage (49.8 review): when set, `send_action_goal`
    /// sends this exact text as a `status` error frame's `msg` (level
    /// `error`, same goal `id`) shortly after tracking the goal — reproduces
    /// a mid-execution fault status frame carrying arbitrary, peer-controlled
    /// text. Distinct from `reject_action_ops`'s canned "Unknown operation"
    /// message, which the client-side classifier (`bridge_client.rs`'s
    /// `dispatch_inbound`) recognizes and maps to the fixed
    /// `ACTION_OPS_UNSUPPORTED` constant instead of storing verbatim — this
    /// field's text does NOT match that heuristic, so it is stored on
    /// `GoalSnapshot::error` exactly as given, letting a test assert the
    /// tool layer never echoes it raw. Inert by default (`None`).
    pub action_goal_status_error: Option<String>,
}

/// A running in-process mock rosbridge server. Dropping this stops the accept
/// loop and every open connection task.
pub struct MockRosbridge {
    pub url: String,
    shutdown: Arc<Notify>,
    frames: Arc<Mutex<Vec<RecordedFrame>>>,
    connection_count: Arc<AtomicUsize>,
}

impl Drop for MockRosbridge {
    fn drop(&mut self) {
        // notify_waiters (not notify_one) so the accept loop AND every still-open
        // per-connection task wake and exit together.
        self.shutdown.notify_waiters();
    }
}

impl MockRosbridge {
    /// Bind on an OS-assigned loopback port and start accepting connections in
    /// the background. Returns once bound.
    pub async fn start_with(config: MockConfig) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("MockRosbridge: bind failed");
        let addr = listener
            .local_addr()
            .expect("MockRosbridge: local_addr failed");
        let url = format!("ws://{addr}");
        let shutdown = Arc::new(Notify::new());
        let frames = Arc::new(Mutex::new(Vec::new()));
        let connection_count = Arc::new(AtomicUsize::new(0));
        let config = Arc::new(config);

        let shutdown_accept = shutdown.clone();
        let frames_accept = frames.clone();
        let connection_count_accept = connection_count.clone();
        let config_accept = config.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_accept.notified() => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break };
                        let conn_idx = connection_count_accept.fetch_add(1, Ordering::SeqCst);
                        let shutdown_conn = shutdown_accept.clone();
                        let frames_conn = frames_accept.clone();
                        let config_conn = config_accept.clone();
                        tokio::spawn(handle_connection(stream, conn_idx, shutdown_conn, frames_conn, config_conn));
                    }
                }
            }
        });

        Self {
            url,
            shutdown,
            frames,
            connection_count,
        }
    }

    /// Every inbound frame recorded so far, in arrival order across every
    /// connection this server has served.
    pub fn frames(&self) -> Vec<RecordedFrame> {
        self.frames.lock().expect("frames mutex poisoned").clone()
    }

    /// Frames whose `op` field equals `op`, in arrival order.
    pub fn frames_with_op(&self, op: &str) -> Vec<RecordedFrame> {
        self.frames()
            .into_iter()
            .filter(|f| f.value.get("op").and_then(|v| v.as_str()) == Some(op))
            .collect()
    }

    /// The `msg` payload of every `publish` frame recorded for `topic`, in
    /// arrival order.
    pub fn published_on(&self, topic: &str) -> Vec<Value> {
        self.frames_with_op("publish")
            .into_iter()
            .filter(|f| f.value.get("topic").and_then(|v| v.as_str()) == Some(topic))
            .map(|f| f.value.get("msg").cloned().unwrap_or(Value::Null))
            .collect()
    }

    /// How many WebSocket connections this server has accepted so far. Zero
    /// proves an allowlist-blocked write never opened a socket (D-04).
    pub fn connection_count(&self) -> usize {
        self.connection_count.load(Ordering::SeqCst)
    }
}

type WsWriter = SplitSink<WebSocketStream<TcpStream>, WsMessage>;

async fn handle_connection(
    stream: TcpStream,
    conn_idx: usize,
    shutdown: Arc<Notify>,
    frames: Arc<Mutex<Vec<RecordedFrame>>>,
    config: Arc<MockConfig>,
) {
    // 49.8-19: hold the WebSocket handshake for the gated connection index
    // until the test releases it — the TCP accept (and connection_count)
    // already happened in the outer accept loop, so this only delays the
    // handshake itself.
    if let Some((gate_idx, gate)) = &config.gate_connection
        && *gate_idx == conn_idx
    {
        tokio::select! {
            _ = shutdown.notified() => return,
            _ = gate.notified() => {}
        }
    }

    let Ok(ws_stream) = tokio_tungstenite::accept_async(stream).await else {
        return;
    };
    let (write, mut read) = ws_stream.split();

    // A dedicated writer task owns the sink so both the main read loop
    // (call_service responses) and background subscribe-emission /
    // action-goal tasks can send frames without fighting over the same
    // `SplitSink`.
    let (out_tx, mut out_rx) = mpsc::channel::<WsMessage>(256);
    let write_task = tokio::spawn(async move {
        let mut write: WsWriter = write;
        while let Some(msg) = out_rx.recv().await {
            if write.send(msg).await.is_err() {
                break;
            }
        }
    });

    // Per-connection subscribe-emission stop signals, keyed by subscribe id.
    let active_subs: Arc<Mutex<HashMap<String, Arc<Notify>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    // Per-connection action-goal cancel flags, keyed by goal id.
    let cancel_flags: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let mut frame_count: usize = 0;

    loop {
        tokio::select! {
            _ = shutdown.notified() => break,
            incoming = read.next() => {
                let Some(Ok(msg)) = incoming else { break };
                let text = match msg {
                    WsMessage::Text(t) => t,
                    WsMessage::Close(_) => break,
                    _ => continue, // ping/pong/binary — ignore, keep connection open
                };
                let Ok(value) = serde_json::from_str::<Value>(text.as_str()) else { continue };
                frames.lock().expect("frames mutex poisoned").push(RecordedFrame {
                    at: Instant::now(),
                    conn: conn_idx,
                    value: value.clone(),
                });
                frame_count += 1;

                let op = value.get("op").and_then(|v| v.as_str()).unwrap_or("");

                match op {
                    "call_service" => {
                        // 49.8-18: a service listed in `unanswered_services`
                        // is recorded (above) but never answered — proves a
                        // cancel-all that gets no response is a reported
                        // failure, not a silent success.
                        let service = value.get("service").and_then(|v| v.as_str()).unwrap_or("");
                        if !config.unanswered_services.iter().any(|s| s == service) {
                            let response = build_service_response(&value, &config);
                            if out_tx.send(WsMessage::Text(response.to_string().into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    "subscribe" => {
                        handle_subscribe(&value, &config, &out_tx, &active_subs);
                    }
                    "unsubscribe" => {
                        let id = value.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        if let Some(stop) = active_subs.lock().expect("active_subs mutex poisoned").remove(id) {
                            stop.notify_waiters();
                        }
                    }
                    "send_action_goal" => {
                        handle_send_action_goal(&value, &config, &out_tx, &cancel_flags);
                    }
                    // 49.8-18: when `ignore_cancel_action_goal` is set, the
                    // frame is still recorded (above) but the cancel flag is
                    // never set — the goal keeps running, proving an ignored
                    // cancel is never confirmed.
                    "cancel_action_goal" if !config.ignore_cancel_action_goal => {
                        let id = value.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        if let Some(flag) = cancel_flags.lock().expect("cancel_flags mutex poisoned").get(id) {
                            flag.store(true, Ordering::SeqCst);
                        }
                    }
                    "cancel_action_goal" => {}
                    _ => {}
                }

                // 49.8-02: close ONLY the first connection after N inbound
                // frames — proves reconnect (a later connection is never
                // affected) rather than every connection being capped.
                if let Some(n) = config.drop_after_frames
                    && conn_idx == 0
                    && frame_count >= n
                {
                    break;
                }
            }
        }
    }

    // Connection ending (shutdown, close, drop_after_frames, or a send
    // failure): stop every background subscribe-emission task for THIS
    // connection so it doesn't keep sending into a dead writer.
    for stop in active_subs
        .lock()
        .expect("active_subs mutex poisoned")
        .drain()
        .map(|(_, s)| s)
    {
        stop.notify_waiters();
    }
    drop(out_tx);
    let _ = write_task.await;
}

fn handle_subscribe(
    value: &Value,
    config: &Arc<MockConfig>,
    out_tx: &mpsc::Sender<WsMessage>,
    active_subs: &Arc<Mutex<HashMap<String, Arc<Notify>>>>,
) {
    let id = value
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let topic = value
        .get("topic")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if let Some(oversized) = config.oversized_frame_bytes {
        let payload = "x".repeat(oversized);
        let out_tx = out_tx.clone();
        tokio::spawn(async move {
            let _ = out_tx.send(WsMessage::Text(payload.into())).await;
        });
        return;
    }

    if let Some(raw_frames) = config.raw_topic_frames.get(&topic).cloned() {
        let out_tx = out_tx.clone();
        tokio::spawn(async move {
            for raw in raw_frames {
                if out_tx.send(WsMessage::Text(raw.into())).await.is_err() {
                    break;
                }
            }
        });
        return;
    }

    if let Some(messages) = config.topic_messages.get(&topic).cloned()
        && !messages.is_empty()
    {
        let stop = Arc::new(Notify::new());
        active_subs
            .lock()
            .expect("active_subs mutex poisoned")
            .insert(id, stop.clone());
        let out_tx = out_tx.clone();
        let interval_ms = config.publish_interval_ms.max(1);
        tokio::spawn(async move {
            let mut i = 0usize;
            loop {
                tokio::select! {
                    _ = stop.notified() => break,
                    _ = tokio::time::sleep(Duration::from_millis(interval_ms)) => {
                        let msg = messages[i % messages.len()].clone();
                        let frame = json!({"op": "publish", "topic": topic, "msg": msg});
                        if out_tx.send(WsMessage::Text(frame.to_string().into())).await.is_err() {
                            break;
                        }
                        i += 1;
                    }
                }
            }
        });
    }
}

fn handle_send_action_goal(
    value: &Value,
    config: &Arc<MockConfig>,
    out_tx: &mpsc::Sender<WsMessage>,
    cancel_flags: &Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
) {
    let id = value
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let action = value
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if config.reject_action_ops {
        let resp = json!({
            "op": "status",
            "level": "error",
            "id": id,
            "msg": "Unknown operation: send_action_goal",
        });
        let out_tx = out_tx.clone();
        tokio::spawn(async move {
            let _ = out_tx.send(WsMessage::Text(resp.to_string().into())).await;
        });
        return;
    }

    if let Some(msg) = &config.action_goal_status_error {
        let resp = json!({
            "op": "status",
            "level": "error",
            "id": id,
            "msg": msg,
        });
        let out_tx2 = out_tx.clone();
        tokio::spawn(async move {
            let _ = out_tx2.send(WsMessage::Text(resp.to_string().into())).await;
        });
    }

    let cancel_flag = Arc::new(AtomicBool::new(false));
    cancel_flags
        .lock()
        .expect("cancel_flags mutex poisoned")
        .insert(id.clone(), cancel_flag.clone());

    let out_tx = out_tx.clone();
    let config = config.clone();
    tokio::spawn(async move {
        if let Some(fb) = &config.action_feedback {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if !cancel_flag.load(Ordering::SeqCst) {
                let frame =
                    json!({"op": "action_feedback", "id": id, "action": action, "values": fb});
                if out_tx
                    .send(WsMessage::Text(frame.to_string().into()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }

        let total_delay = config.action_result_delay_ms;
        let step = 20u64.min(total_delay.max(1));
        let mut waited = 0u64;
        while waited < total_delay {
            if cancel_flag.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(step)).await;
            waited += step;
        }

        let frame = if cancel_flag.load(Ordering::SeqCst) {
            // 49.8-18: `honoured_cancel_status` overrides the `status` field
            // a honoured cancel reports — `None` (default) keeps `5`
            // (CANCELED); `Some(Value::Null)` omits the field entirely (a
            // missing status); `Some(v)` sends `v` verbatim (e.g. a
            // non-terminal `3`/CANCELING).
            let mut frame = json!({"op": "action_result", "id": id, "action": action, "status": 5, "result": false, "values": {}});
            match &config.honoured_cancel_status {
                None => {}
                Some(Value::Null) => {
                    if let Some(map) = frame.as_object_mut() {
                        map.remove("status");
                    }
                }
                Some(v) => {
                    frame["status"] = v.clone();
                }
            }
            frame
        } else {
            json!({
                "op": "action_result",
                "id": id,
                "action": action,
                "status": 4,
                "result": true,
                "values": config.action_result_values,
            })
        };
        let _ = out_tx.send(WsMessage::Text(frame.to_string().into())).await;
    });
}

fn build_service_response(request: &Value, config: &MockConfig) -> Value {
    let service = request
        .get("service")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let args = request.get("args").cloned().unwrap_or(json!({}));

    let values = match service.as_str() {
        "/rosapi/topics" => {
            let topics: Vec<Value> = config.topics.iter().map(|(t, _)| json!(t)).collect();
            let types: Vec<Value> = config.topics.iter().map(|(_, t)| json!(t)).collect();
            Some(json!({"topics": topics, "types": types}))
        }
        "/rosapi/nodes" => Some(json!({"nodes": config.nodes})),
        "/rosapi/services" => {
            let names: Vec<Value> = config.services.iter().map(|(n, _)| json!(n)).collect();
            Some(json!({"services": names}))
        }
        // 49.8-05: an unconfigured (empty) `action_servers` list answers
        // "service not found" (falls through to the default below) rather
        // than an unconditional success — mirrors a rosapi that doesn't
        // implement this introspection call at all (D-06's pre-2.0.0
        // rosbridge_suite concern), which `Ros2Tool`'s `list_actions` must
        // surface as an `Error:`, not a silent empty list.
        "/rosapi/action_servers" if !config.action_servers.is_empty() => {
            Some(json!({"action_servers": config.action_servers}))
        }
        "/rosapi/topic_type" => {
            let topic = args.get("topic").and_then(|v| v.as_str()).unwrap_or("");
            let ty = config.topic_types.get(topic).cloned().unwrap_or_default();
            Some(json!({"type": ty}))
        }
        "/rosapi/service_type" => {
            let svc_name = args.get("service").and_then(|v| v.as_str()).unwrap_or("");
            let ty = config
                .services
                .iter()
                .find(|(n, _)| n == svc_name)
                .map(|(_, t)| t.clone())
                .or_else(|| config.service_types.get(svc_name).cloned())
                .unwrap_or_default();
            Some(json!({"type": ty}))
        }
        "/rosapi/message_details" => {
            let ty = args.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let typedefs = config
                .message_details
                .get(ty)
                .cloned()
                .unwrap_or_else(|| json!([]));
            Some(json!({"typedefs": typedefs}))
        }
        _ => None,
    };

    if let Some(values) = values {
        return json!({
            "op": "service_response",
            "service": service,
            "values": values,
            "result": true,
            "id": id,
        });
    }

    if let Some(values) = config.service_responses.get(&service) {
        return json!({
            "op": "service_response",
            "service": service,
            "values": values,
            "result": true,
            "id": id,
        });
    }

    json!({
        "op": "service_response",
        "service": service,
        "values": "service not found",
        "result": false,
        "id": id,
    })
}

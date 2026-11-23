//! `Ros2BridgeClient` — the single rosbridge JSON op encoder/decoder (Phase
//! 49.8 D-01). 49.8-01 landed `connect`/`call_service`/`advertise`/`publish`;
//! this plan (49.8-02) completes the op surface: bounded subscribe demux
//! (D-12 building block), the action goal lifecycle with cancel (D-06),
//! rosbridge `status` error frames, non-finite JSON sanitization, a
//! per-connection size cap, and close/reconnect semantics.
//!
//! Every send goes through the one private [`Ros2BridgeClient::send_op`] —
//! the single source of truth for wire format, mirroring `hexapod_tcp.rs`'s
//! `CMD_*` constants block. A spawned reader task parses each inbound text
//! frame (after [`sanitize_nonfinite_json`]) and dispatches by `op`:
//! `service_response` frames route to the pending `oneshot` keyed by `id`;
//! `publish` frames fan out to every local [`Subscription`] on that topic;
//! `action_feedback`/`action_result` update the matching goal entry;
//! `status` error frames fail a pending service call or set a goal's error.
//! A frame that fails to parse is skipped, not fatal (CR-03): it is logged as
//! bounded, payload-free metadata only — never the peer-controlled frame text
//! itself — and rate-limited per connection at
//! [`MALFORMED_FRAME_LOG_LIMIT`]. On stream end, error, or an oversized frame
//! it marks the client closed and fails every still-pending request, drops
//! every subscription sender (ending its receiver), and marks every
//! unfinished goal `bridge disconnected`.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures::stream::SplitSink;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::{Mutex as AsyncMutex, Notify, mpsc, oneshot};
use tokio::time::{Duration, timeout};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async_with_config};

/// Caps `WebSocketConfig::max_message_size`/`max_frame_size` — the first line
/// of defense against a malicious/buggy rosbridge peer flooding the reader
/// task (T-49.8-09). [`ConnectOptions::max_message_bytes`] overrides this
/// per-connection; [`Ros2BridgeClient::connect`] uses this default.
pub const MAX_INBOUND_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// Per-connection cap on how many malformed-inbound-frame `warn` lines the
/// reader task emits (CR-03, T-49.8-74): frames `1..=MALFORMED_FRAME_LOG_LIMIT`
/// each log one bounded, payload-free line; later malformed frames on the
/// SAME connection are only counted, and one suppression summary (naming the
/// total and how many were suppressed) is logged when the connection ends.
/// The bound is per connection — reconnect churn is governed by the
/// session's own reconnect paths, not by this constant, so a peer that
/// forces reconnects can still produce roughly `MALFORMED_FRAME_LOG_LIMIT + 1`
/// lines per connection over time.
pub const MALFORMED_FRAME_LOG_LIMIT: u64 = 5;

/// Timeout wrapping the initial WebSocket handshake (T-49.8-12).
pub const CONNECT_TIMEOUT_SECS: u64 = 5;

/// Default `call_service` response timeout when the caller does not override
/// it (T-49.8-12).
pub const DEFAULT_SERVICE_TIMEOUT_SECS: u64 = 5;

/// After the first `advertise` for a topic, wait this long before the first
/// `publish` so the message is not lost to DDS discovery on a just-created
/// publisher (rosbridge protocol note; planning decision recorded in
/// `49.8-01-PLAN.md`).
pub const ADVERTISE_SETTLE_MS: u64 = 300;

/// Bounded channel capacity for every [`Subscription`]'s receiver
/// (T-49.8-09): a full channel drops the message via `try_send` rather than
/// growing memory unboundedly.
pub const SUBSCRIPTION_CHANNEL_CAPACITY: usize = 64;

/// D-06/T-49.8-28: fixed refusal set on a goal when rosbridge answers
/// `send_action_goal`/`cancel_action_goal` with a `status` error naming the
/// op as unknown or unsupported (rosbridge_suite < 2.0.0 on Humble).
pub const ACTION_OPS_UNSUPPORTED: &str = "rosbridge on the robot does not support ROS 2 action ops (rosbridge_suite >= 2.0.0 is \
     required — see docs/robots/rospider.md, D-06)";

/// A rosbridge `service_response` frame's payload.
#[derive(Debug, Clone)]
pub struct ServiceResponse {
    pub result: bool,
    pub values: Value,
}

/// Per-connection size cap override for [`Ros2BridgeClient::connect_with`].
/// `Default` uses [`MAX_INBOUND_MESSAGE_BYTES`] — the same value
/// [`Ros2BridgeClient::connect`] has always used.
#[derive(Debug, Clone, Copy)]
pub struct ConnectOptions {
    pub max_message_bytes: usize,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            max_message_bytes: MAX_INBOUND_MESSAGE_BYTES,
        }
    }
}

/// A live bounded-sample subscription (D-12 building block). Dropping this
/// without calling [`Ros2BridgeClient::unsubscribe`] leaves the local
/// registration in place (the mock/rosbridge server never learns) — callers
/// are expected to always unsubscribe when done, matching the stateless
/// bounded-sample shape 49.8-05 builds on.
pub struct Subscription {
    pub id: String,
    pub topic: String,
    pub rx: mpsc::Receiver<Value>,
}

/// A snapshot of one action goal's state (D-06 building block).
#[derive(Debug, Clone, Default)]
pub struct GoalSnapshot {
    pub action: String,
    pub last_feedback: Option<Value>,
    pub status: Option<i64>,
    pub succeeded: Option<bool>,
    pub result: Option<Value>,
    pub error: Option<String>,
}

/// Internal goal bookkeeping — the reader task mutates this; callers only
/// ever see the immutable [`GoalSnapshot`] copy.
struct GoalEntry {
    action: String,
    last_feedback: Option<Value>,
    status: Option<i64>,
    succeeded: Option<bool>,
    result: Option<Value>,
    error: Option<String>,
    notify: Arc<Notify>,
}

impl GoalEntry {
    fn snapshot(&self) -> GoalSnapshot {
        GoalSnapshot {
            action: self.action.clone(),
            last_feedback: self.last_feedback.clone(),
            status: self.status,
            succeeded: self.succeeded,
            result: self.result.clone(),
            error: self.error.clone(),
        }
    }

    fn is_settled(&self) -> bool {
        self.result.is_some() || self.error.is_some()
    }
}

/// D-06: `action_msgs/msg/GoalStatus` constant names. Unknown/out-of-range
/// values map to `"UNKNOWN"` rather than panicking — a forward-compat
/// rosbridge peer reporting a status this client doesn't recognize should
/// degrade gracefully, not crash the reader task.
pub fn goal_status_name(status: i64) -> &'static str {
    match status {
        0 => "UNKNOWN",
        1 => "ACCEPTED",
        2 => "EXECUTING",
        3 => "CANCELING",
        4 => "SUCCEEDED",
        5 => "CANCELED",
        6 => "ABORTED",
        _ => "UNKNOWN",
    }
}

/// D-01/T-49.8-13: rewrite bare `NaN`, `Infinity` and `-Infinity` tokens that
/// appear OUTSIDE JSON string literals to `null`, so `serde_json::from_str`
/// (strict JSON, no non-finite literal support) can parse a frame a
/// non-finite-emitting ROS bridge produced. A token appearing inside a string
/// (e.g. `"NaN"` as text) is left untouched — string/escape state is tracked
/// while scanning.
pub fn sanitize_nonfinite_json(text: &str) -> Cow<'_, str> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    let mut in_string = false;
    let mut escaped = false;
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }

        if c == '"' {
            in_string = true;
            out.push(c);
            i += 1;
            continue;
        }

        if let Some(len) = matches_nonfinite_token(&chars, i) {
            out.push_str("null");
            changed = true;
            i += len;
            continue;
        }

        out.push(c);
        i += 1;
    }

    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(text)
    }
}

/// Returns the token length (in `chars`) if a bare `NaN`/`Infinity`/
/// `-Infinity` token starts at `i`, bounded so it doesn't match inside a
/// larger identifier (e.g. `NaNny`).
fn matches_nonfinite_token(chars: &[char], i: usize) -> Option<usize> {
    const TOKENS: &[&str] = &["-Infinity", "Infinity", "NaN"];
    for token in TOKENS {
        let token_chars: Vec<char> = token.chars().collect();
        let end = i + token_chars.len();
        if end > chars.len() {
            continue;
        }
        if chars[i..end] != token_chars[..] {
            continue;
        }
        let boundary_before_ok = i == 0 || !is_ident_char(chars[i - 1]);
        let boundary_after_ok = end == chars.len() || !is_ident_char(chars[end]);
        if boundary_before_ok && boundary_after_ok {
            return Some(token_chars.len());
        }
    }
    None
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct SubEntry {
    topic: String,
    tx: mpsc::Sender<Value>,
}

static CONNECTION_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The single rosbridge JSON op encoder/decoder for one WebSocket connection.
pub struct Ros2BridgeClient {
    writer: AsyncMutex<SplitSink<WsStream, WsMessage>>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<ServiceResponse>>>>,
    subscriptions: Arc<Mutex<HashMap<String, SubEntry>>>,
    goals: Arc<Mutex<HashMap<String, GoalEntry>>>,
    closed: Arc<AtomicBool>,
    advertised: Mutex<HashSet<String>>,
    id_counter: AtomicU64,
    connection_id: u64,
}

impl Ros2BridgeClient {
    /// Connect to `url` with the default [`ConnectOptions`] (16 MiB size cap
    /// — [`MAX_INBOUND_MESSAGE_BYTES`]).
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        Self::connect_with(url, ConnectOptions::default()).await
    }

    /// Connect to `url` (already `validate_bridge_url`-checked by the caller —
    /// this fn does not re-validate the scheme), capping inbound
    /// message/frame size at `opts.max_message_bytes` and wrapping the
    /// handshake in [`CONNECT_TIMEOUT_SECS`].
    pub async fn connect_with(url: &str, opts: ConnectOptions) -> anyhow::Result<Self> {
        let ws_config = WebSocketConfig::default()
            .max_message_size(Some(opts.max_message_bytes))
            .max_frame_size(Some(opts.max_message_bytes));

        let connect_fut = connect_async_with_config(url, Some(ws_config), false);
        let (ws_stream, _response) =
            timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS), connect_fut)
                .await
                .map_err(|_| {
                    anyhow::anyhow!(
                        "rosbridge connect to {url} timed out after {CONNECT_TIMEOUT_SECS}s"
                    )
                })?
                .map_err(|e| anyhow::anyhow!("rosbridge connect to {url} failed: {e}"))?;

        let (write, mut read) = ws_stream.split();
        let pending: Arc<Mutex<HashMap<String, oneshot::Sender<ServiceResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let subscriptions: Arc<Mutex<HashMap<String, SubEntry>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let goals: Arc<Mutex<HashMap<String, GoalEntry>>> = Arc::new(Mutex::new(HashMap::new()));
        let closed = Arc::new(AtomicBool::new(false));

        // Allocated BEFORE the reader task is spawned (49.8-20) so every
        // reader log line — including the very first malformed-frame warn —
        // carries this connection's id.
        let connection_id = CONNECTION_ID_COUNTER.fetch_add(1, Ordering::SeqCst);

        let pending_reader = pending.clone();
        let subscriptions_reader = subscriptions.clone();
        let goals_reader = goals.clone();
        let closed_reader = closed.clone();
        tokio::spawn(async move {
            let mut malformed: u64 = 0;
            while let Some(msg) = read.next().await {
                match msg {
                    Ok(WsMessage::Text(text)) => {
                        let sanitized = sanitize_nonfinite_json(text.as_str());
                        match serde_json::from_str::<Value>(sanitized.as_ref()) {
                            Ok(value) => {
                                dispatch_inbound(
                                    &value,
                                    &pending_reader,
                                    &subscriptions_reader,
                                    &goals_reader,
                                );
                            }
                            Err(err) => {
                                // Frame fails to parse: skip it — the
                                // connection stays up, a malformed frame is
                                // not fatal — and log only bounded,
                                // payload-free metadata (CR-03: never
                                // interpolate the peer-controlled frame text,
                                // a prefix of it, or a hash seeded from it).
                                malformed += 1;
                                if malformed <= MALFORMED_FRAME_LOG_LIMIT {
                                    if malformed == MALFORMED_FRAME_LOG_LIMIT {
                                        tracing::warn!(
                                            connection_id,
                                            frame_bytes = text.len(),
                                            parse_error_kind = ?err.classify(),
                                            line = err.line(),
                                            column = err.column(),
                                            malformed_frames = malformed,
                                            "rosbridge: inbound frame failed to parse, skipping \
                                             (payload not logged; further malformed frames on \
                                             this connection are counted, not logged)"
                                        );
                                    } else {
                                        tracing::warn!(
                                            connection_id,
                                            frame_bytes = text.len(),
                                            parse_error_kind = ?err.classify(),
                                            line = err.line(),
                                            column = err.column(),
                                            malformed_frames = malformed,
                                            "rosbridge: inbound frame failed to parse, skipping \
                                             (payload not logged)"
                                        );
                                    }
                                }
                                continue;
                            }
                        }
                    }
                    Ok(WsMessage::Close(_)) => break,
                    Err(_) => break,
                    _ => {}
                }
            }
            if malformed > MALFORMED_FRAME_LOG_LIMIT {
                tracing::warn!(
                    connection_id,
                    malformed_frames = malformed,
                    suppressed = malformed - MALFORMED_FRAME_LOG_LIMIT,
                    "rosbridge: connection closed; malformed inbound frames were suppressed \
                     from the log"
                );
            }
            // Stream ended, errored, or a frame exceeded the size cap: mark
            // closed and fail every still-pending request — dropping each
            // sender completes its receiver with a RecvError, which
            // `call_service` maps into an anyhow error. Every subscription
            // sender is dropped too (ending its receiver), and every
            // unfinished goal is marked `bridge disconnected`.
            closed_reader.store(true, Ordering::SeqCst);
            pending_reader
                .lock()
                .expect("pending mutex poisoned")
                .clear();
            subscriptions_reader
                .lock()
                .expect("subscriptions mutex poisoned")
                .clear();
            let mut goals = goals_reader.lock().expect("goals mutex poisoned");
            for entry in goals.values_mut() {
                if !entry.is_settled() {
                    entry.error = Some("bridge disconnected".to_string());
                }
                entry.notify.notify_waiters();
            }
        });

        Ok(Self {
            writer: AsyncMutex::new(write),
            pending,
            subscriptions,
            goals,
            closed,
            advertised: Mutex::new(HashSet::new()),
            id_counter: AtomicU64::new(0),
            connection_id,
        })
    }

    /// A process-global, monotonically increasing id identifying which
    /// connection this client wraps — lets a caller (e.g. `Ros2Session`)
    /// tell which connection owns a goal across a reconnect.
    pub fn connection_id(&self) -> u64 {
        self.connection_id
    }

    fn next_id(&self, op: &str) -> String {
        let n = self.id_counter.fetch_add(1, Ordering::SeqCst);
        format!("ironhermes:{op}:{n}")
    }

    /// The single source of truth for wire format — every rosbridge op is sent
    /// through this one fn.
    async fn send_op(&self, value: Value) -> anyhow::Result<()> {
        let text = value.to_string();
        let mut writer = self.writer.lock().await;
        writer
            .send(WsMessage::Text(text.into()))
            .await
            .map_err(|e| anyhow::anyhow!("rosbridge send failed: {e}"))
    }

    /// `call_service` — sends the op, then awaits the matching
    /// `service_response` (by `id`) or `timeout_dur`, whichever comes first.
    /// A rosbridge `status` error frame naming this `id` also resolves the
    /// wait, as a failed [`ServiceResponse`] (`result: false`).
    pub async fn call_service(
        &self,
        service: &str,
        service_type: Option<&str>,
        args: Value,
        timeout_dur: Duration,
    ) -> anyhow::Result<ServiceResponse> {
        let id = self.next_id("call_service");
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .expect("pending mutex poisoned")
            .insert(id.clone(), tx);

        let mut op = json!({
            "op": "call_service",
            "service": service,
            "args": args,
            "id": id,
        });
        if let Some(t) = service_type {
            op["type"] = json!(t);
        }

        if let Err(e) = self.send_op(op).await {
            self.pending
                .lock()
                .expect("pending mutex poisoned")
                .remove(&id);
            return Err(e);
        }

        match timeout(timeout_dur, rx).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => {
                self.pending
                    .lock()
                    .expect("pending mutex poisoned")
                    .remove(&id);
                Err(anyhow::anyhow!(
                    "rosbridge connection closed while waiting for '{service}' response"
                ))
            }
            Err(_) => {
                self.pending
                    .lock()
                    .expect("pending mutex poisoned")
                    .remove(&id);
                Err(anyhow::anyhow!(
                    "rosbridge call_service '{service}' timed out"
                ))
            }
        }
    }

    /// `advertise` — idempotent per topic within this connection's lifetime.
    /// On the FIRST advertise for `topic`, sends the op and waits
    /// [`ADVERTISE_SETTLE_MS`] so the first message is not lost to DDS
    /// discovery. Returns `Ok(true)` when this call actually advertised,
    /// `Ok(false)` when `topic` was already advertised (no-op, no wait).
    pub async fn advertise(&self, topic: &str, msg_type: &str) -> anyhow::Result<bool> {
        if self.advertise_no_settle(topic, msg_type).await? {
            tokio::time::sleep(Duration::from_millis(ADVERTISE_SETTLE_MS)).await;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Same idempotence tracking as [`Ros2BridgeClient::advertise`], but
    /// WITHOUT the [`ADVERTISE_SETTLE_MS`] wait (Phase 49.8 D-09). The settle
    /// exists to avoid losing a single one-shot publish to a DDS-discovery
    /// race against a just-created publisher; a caller that republishes many
    /// times over — the timed-motion loop at rate, or the 3x zero-Twist stop
    /// sequence — doesn't need it, since a lost first message is covered by
    /// the next one. `advertise`'s own callers are unaffected: whichever of
    /// the two is called first for a topic on this connection marks it
    /// advertised, so a later call (of either kind) for the same topic is a
    /// no-op regardless of which one ran first.
    pub async fn advertise_no_settle(&self, topic: &str, msg_type: &str) -> anyhow::Result<bool> {
        {
            let mut advertised = self.advertised.lock().expect("advertised mutex poisoned");
            if advertised.contains(topic) {
                return Ok(false);
            }
            advertised.insert(topic.to_string());
        }
        let op = json!({"op": "advertise", "topic": topic, "type": msg_type});
        if let Err(e) = self.send_op(op).await {
            // Roll back the idempotence marker so a retry after a transient
            // send failure re-advertises rather than silently skipping it.
            self.advertised
                .lock()
                .expect("advertised mutex poisoned")
                .remove(topic);
            return Err(e);
        }
        Ok(true)
    }

    /// `publish` — sends one message on an already-advertised topic.
    pub async fn publish(&self, topic: &str, msg: Value) -> anyhow::Result<()> {
        let op = json!({"op": "publish", "topic": topic, "msg": msg});
        self.send_op(op).await
    }

    /// `subscribe` (D-12 building block) — always sends `throttle_rate` and
    /// `queue_length` (RESEARCH.md Pitfall 4: a naive client that omits them
    /// gets an unthrottled firehose). Returns a [`Subscription`] whose bounded
    /// `rx` ([`SUBSCRIPTION_CHANNEL_CAPACITY`]) receives every inbound
    /// `publish` frame's `msg` for `topic` — a full channel drops the message
    /// via `try_send` rather than growing memory. Two subscriptions to the
    /// same topic each get their own channel and both receive every message.
    pub async fn subscribe(
        &self,
        topic: &str,
        msg_type: Option<&str>,
        throttle_rate_ms: u32,
        queue_length: u32,
    ) -> anyhow::Result<Subscription> {
        let id = self.next_id("subscribe");
        let (tx, rx) = mpsc::channel(SUBSCRIPTION_CHANNEL_CAPACITY);
        self.subscriptions
            .lock()
            .expect("subscriptions mutex poisoned")
            .insert(
                id.clone(),
                SubEntry {
                    topic: topic.to_string(),
                    tx,
                },
            );

        let mut op = json!({
            "op": "subscribe",
            "topic": topic,
            "id": id,
            "throttle_rate": throttle_rate_ms,
            "queue_length": queue_length,
        });
        if let Some(t) = msg_type {
            op["type"] = json!(t);
        }

        if let Err(e) = self.send_op(op).await {
            self.subscriptions
                .lock()
                .expect("subscriptions mutex poisoned")
                .remove(&id);
            return Err(e);
        }

        Ok(Subscription {
            id,
            topic: topic.to_string(),
            rx,
        })
    }

    /// `unsubscribe` — sends `{op: unsubscribe, id, topic}` (reusing `sub`'s
    /// own id/topic) and removes the local subscription entry.
    pub async fn unsubscribe(&self, sub: &Subscription) -> anyhow::Result<()> {
        self.subscriptions
            .lock()
            .expect("subscriptions mutex poisoned")
            .remove(&sub.id);
        let op = json!({"op": "unsubscribe", "id": sub.id, "topic": sub.topic});
        self.send_op(op).await
    }

    /// `send_action_goal` (D-06 building block) — sends
    /// `{op: send_action_goal, id, action, action_type, args, feedback: true}`
    /// and records a goal entry keyed by the returned id. The reader task
    /// stores inbound `action_feedback` as [`GoalSnapshot::last_feedback`] and
    /// `action_result` `status`/`result` on the entry, notifying any
    /// [`Ros2BridgeClient::wait_goal_result`] waiter.
    pub async fn send_action_goal(
        &self,
        action: &str,
        action_type: &str,
        args: Value,
    ) -> anyhow::Result<String> {
        let id = self.next_id("send_action_goal");
        self.goals.lock().expect("goals mutex poisoned").insert(
            id.clone(),
            GoalEntry {
                action: action.to_string(),
                last_feedback: None,
                status: None,
                succeeded: None,
                result: None,
                error: None,
                notify: Arc::new(Notify::new()),
            },
        );

        let op = json!({
            "op": "send_action_goal",
            "id": id,
            "action": action,
            "action_type": action_type,
            "args": args,
            "feedback": true,
        });
        if let Err(e) = self.send_op(op).await {
            self.goals.lock().expect("goals mutex poisoned").remove(&id);
            return Err(e);
        }

        Ok(id)
    }

    /// `cancel_action_goal` — sends `{op: cancel_action_goal, id: goal_id, action}`.
    pub async fn cancel_action_goal(&self, action: &str, goal_id: &str) -> anyhow::Result<()> {
        let op = json!({"op": "cancel_action_goal", "id": goal_id, "action": action});
        self.send_op(op).await
    }

    /// An immediate snapshot of `goal_id`'s current state, or `None` if no
    /// such goal is known (never sent, or already evicted).
    pub fn goal_snapshot(&self, goal_id: &str) -> Option<GoalSnapshot> {
        self.goals
            .lock()
            .expect("goals mutex poisoned")
            .get(goal_id)
            .map(GoalEntry::snapshot)
    }

    /// Waits until `goal_id` settles (a `result` or an `error` is recorded)
    /// or `timeout_dur` elapses, whichever comes first. Returns `None` if
    /// `goal_id` is unknown OR the timeout elapses before it settles.
    pub async fn wait_goal_result(
        &self,
        goal_id: &str,
        timeout_dur: Duration,
    ) -> Option<GoalSnapshot> {
        let notify = {
            let goals = self.goals.lock().expect("goals mutex poisoned");
            let entry = goals.get(goal_id)?;
            if entry.is_settled() {
                return Some(entry.snapshot());
            }
            entry.notify.clone()
        };

        let deadline = tokio::time::Instant::now() + timeout_dur;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return self.goal_snapshot(goal_id);
            }
            let notified = notify.notified();
            tokio::pin!(notified);
            if timeout(remaining, notified).await.is_err() {
                return self.goal_snapshot(goal_id);
            }
            let goals = self.goals.lock().expect("goals mutex poisoned");
            if let Some(entry) = goals.get(goal_id)
                && entry.is_settled()
            {
                return Some(entry.snapshot());
            }
            // Spurious/other-goal wakeup (a single connection can have many
            // in-flight goals) — loop and wait again on the remaining budget.
        }
    }

    /// Has the reader task observed the connection close (stream end, error,
    /// or explicit [`Ros2BridgeClient::close`])?
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Mark this client closed so [`super::session::Ros2Session::client_for`]
    /// reconnects on the next call rather than reusing a connection the caller
    /// knows is no longer good.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

/// Dispatches one already-parsed, already-sanitized inbound frame by `op`.
/// Kept as a free fn (not a method) so it only needs `&Arc<Mutex<_>>`
/// references — the reader task owns no `&Ros2BridgeClient`.
fn dispatch_inbound(
    value: &Value,
    pending: &Arc<Mutex<HashMap<String, oneshot::Sender<ServiceResponse>>>>,
    subscriptions: &Arc<Mutex<HashMap<String, SubEntry>>>,
    goals: &Arc<Mutex<HashMap<String, GoalEntry>>>,
) {
    let op = value.get("op").and_then(|v| v.as_str()).unwrap_or("");
    match op {
        "service_response" => {
            let id = value
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let result = value
                .get("result")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let values = value.get("values").cloned().unwrap_or(Value::Null);
            if let Some(tx) = pending.lock().expect("pending mutex poisoned").remove(&id) {
                let _ = tx.send(ServiceResponse { result, values });
            }
        }
        "publish" => {
            let topic = value.get("topic").and_then(|v| v.as_str()).unwrap_or("");
            let msg = value.get("msg").cloned().unwrap_or(Value::Null);
            let subs = subscriptions.lock().expect("subscriptions mutex poisoned");
            for entry in subs.values() {
                if entry.topic == topic {
                    // A full channel drops the message (T-49.8-09) rather
                    // than blocking the reader task or growing memory.
                    let _ = entry.tx.try_send(msg.clone());
                }
            }
        }
        "action_feedback" => {
            let id = value.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let feedback = value.get("values").cloned().unwrap_or(Value::Null);
            let mut goals = goals.lock().expect("goals mutex poisoned");
            if let Some(entry) = goals.get_mut(id) {
                entry.last_feedback = Some(feedback);
                entry.notify.notify_waiters();
            }
        }
        "action_result" => {
            // 49.8-10, live-harness finding: a real rosbridge_suite >= 2.0.0
            // server puts `status` (the action_msgs/GoalStatus code) and
            // `result` (a bare success bool) as TOP-LEVEL fields on the
            // `action_result` frame — `values` carries the actual goal
            // result MESSAGE fields (e.g. `delta` for
            // turtlesim/action/RotateAbsolute), never a `{status, result}`
            // wrapper nested under `values`. [VERIFIED against a live
            // Humble 2.0.8 rosbridge via docker/ros2-humble-harness — the
            // CI mock server previously modeled the wrong (nested) shape,
            // which made every real action_result decode as status=UNKNOWN.]
            let id = value.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let status = value.get("status").and_then(|v| v.as_i64());
            let succeeded = value
                .get("result")
                .and_then(|v| v.as_bool())
                .or_else(|| status.map(|s| s == 4));
            let values = value.get("values").cloned().unwrap_or(Value::Null);
            let mut goals = goals.lock().expect("goals mutex poisoned");
            if let Some(entry) = goals.get_mut(id) {
                entry.status = status;
                entry.succeeded = succeeded;
                entry.result = Some(values);
                entry.notify.notify_waiters();
            }
        }
        "status" => {
            let level = value.get("level").and_then(|v| v.as_str()).unwrap_or("");
            if level != "error" {
                return;
            }
            let id = value
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let msg = value
                .get("msg")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let handled_as_service = {
                let mut pending = pending.lock().expect("pending mutex poisoned");
                if let Some(tx) = pending.remove(&id) {
                    let _ = tx.send(ServiceResponse {
                        result: false,
                        values: json!(msg),
                    });
                    true
                } else {
                    false
                }
            };
            if handled_as_service {
                return;
            }

            let mut goals = goals.lock().expect("goals mutex poisoned");
            if let Some(entry) = goals.get_mut(&id) {
                let unsupported_op = (msg.contains("send_action_goal")
                    || msg.contains("cancel_action_goal"))
                    && (msg.to_lowercase().contains("unknown")
                        || msg.to_lowercase().contains("unsupported"));
                entry.error = Some(if unsupported_op {
                    ACTION_OPS_UNSUPPORTED.to_string()
                } else {
                    msg
                });
                entry.notify.notify_waiters();
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ros2_goal_status_name_maps_action_msgs_enum() {
        assert_eq!(goal_status_name(0), "UNKNOWN");
        assert_eq!(goal_status_name(1), "ACCEPTED");
        assert_eq!(goal_status_name(2), "EXECUTING");
        assert_eq!(goal_status_name(3), "CANCELING");
        assert_eq!(goal_status_name(4), "SUCCEEDED");
        assert_eq!(goal_status_name(5), "CANCELED");
        assert_eq!(goal_status_name(6), "ABORTED");
        assert_eq!(goal_status_name(99), "UNKNOWN");
    }

    #[test]
    fn ros2_sanitize_nonfinite_json_replaces_bare_tokens_outside_strings() {
        let input = r#"{"a":NaN,"b":Infinity,"c":-Infinity,"s":"NaN is text","t":"Infinity too"}"#;
        let out = sanitize_nonfinite_json(input);
        assert_eq!(
            out,
            r#"{"a":null,"b":null,"c":null,"s":"NaN is text","t":"Infinity too"}"#
        );
        assert!(
            serde_json::from_str::<Value>(&out).is_ok(),
            "sanitized output must be valid JSON"
        );
    }

    #[test]
    fn ros2_sanitize_nonfinite_json_is_borrowed_when_unchanged() {
        let input = r#"{"a":1,"b":"NaN"}"#;
        let out = sanitize_nonfinite_json(input);
        assert!(
            matches!(out, Cow::Borrowed(_)),
            "unchanged input must not allocate"
        );
    }
}

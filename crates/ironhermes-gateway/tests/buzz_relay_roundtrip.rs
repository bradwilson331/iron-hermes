#![cfg(feature = "buzz")]
//! In-process Nostr relay harness + end-to-end Buzz behavior tests
//! (Phase 47.6 Plan 01).
//!
//! Every test in this file verifies against `local_relay::LocalRelay` — a
//! minimal in-process NIP-01 relay written for this phase (see the module doc
//! below for why `nostr-relay-builder` could not be used) — so the whole
//! suite runs in CI with no Docker and no external relay process (RESEARCH.md
//! Pitfall 5: Docker is unavailable on this machine). `spawn_mock_relay` and
//! `connected_client` are the reusable harness; the harness self-test
//! (`relay_harness_round_trips_a_published_event`) is the instrument every
//! later assertion in this phase depends on — if it is flaky, nothing
//! downstream can be trusted.

use ironhermes_core::{ChannelTrust, MessageEvent, PlatformGatewayConfig};
use ironhermes_gateway::adapter::{MessageHandler, PlatformAdapter};
use ironhermes_gateway::buzz::{BuzzAdapter, run_buzz_adapter};
use nostr_sdk::prelude::*;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex as TokioMutex;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// local_relay — minimal in-process NIP-01 relay (VERSION-ALIGNMENT FALLBACK)
// ---------------------------------------------------------------------------

/// Minimal in-process NIP-01 relay for tests.
///
/// # Why this exists instead of `nostr-relay-builder`
///
/// `nostr-relay-builder` has exactly two releases on the "0.45" minor line
/// shared with `nostr-sdk`/`nostr` 0.45.0: `0.45.0-alpha.2` and
/// `0.45.0-alpha.3` (the latest). **Both fail to compile** against the
/// published `nostr` 0.45.0 crate — they reference
/// `SingleLetterTag::lowercase(Alphabet::P)` and an `Events` type, neither of
/// which exists in the released `nostr` 0.45.0 API (that API was replaced by
/// `SingleLetterTag::LOWERCASE_P` / `from_char` before the final release
/// shipped). This was verified directly: `cargo check` against a scratch
/// project depending on `nostr-sdk = "0.45.0"` plus each alpha in turn
/// reproduces the identical `E0599`/`E0433` compiler errors both times. Full
/// error text is recorded in this plan's SUMMARY.
///
/// Per this plan's task 1 VERSION-ALIGNMENT FALLBACK instruction — "do not
/// block the tracer... write a small local test-relay wrapper in the test
/// file" — this module is that wrapper. Its job, per the same instruction, is
/// deliberately narrow: accept a WebSocket connection, store/broadcast
/// published events, and serve `REQ` filters back. It does NOT implement
/// NIP-42 AUTH (this phase's own D-10 live-relay smoke test in plan 08 is
/// where real NIP-42 gets exercised against a real Buzz-compatible relay);
/// omitting AUTH here does not weaken any assertion in this file because
/// `nostr_sdk::Client`'s `SignerAuthenticator` only reacts to an AUTH
/// challenge the relay never sends — the client simply proceeds unauthenticated,
/// which is exactly the behavior every `BuzzAdapter` test in this phase needs.
mod local_relay {
    use futures::{SinkExt, StreamExt};
    use nostr_sdk::prelude::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::{Notify, broadcast};
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    /// A running in-process relay. Dropping this stops the accept loop and
    /// every open connection task.
    pub struct LocalRelay {
        pub url: String,
        shutdown: Arc<Notify>,
        /// Every REQ's filter set received across ALL connections, in
        /// arrival order — the fix-verification regression tests use this to
        /// assert the adapter's actual subscription shape (one REQ per
        /// configured channel, each with exactly one `#h` value) without the
        /// relay needing to implement any real channel-scoping logic itself.
        received_reqs: Arc<Mutex<Vec<Vec<Filter>>>>,
    }

    impl Drop for LocalRelay {
        fn drop(&mut self) {
            // notify_waiters (not notify_one) so the accept loop AND every
            // still-open per-connection task wake and exit together.
            self.shutdown.notify_waiters();
        }
    }

    impl LocalRelay {
        /// Bind on an OS-assigned loopback port and start accepting
        /// connections in the background. Returns once bound (not once the
        /// accept loop has started serving — the caller's own `connect()`
        /// call naturally waits for the listener to be ready).
        pub async fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .expect("LocalRelay: bind failed");
            let addr = listener.local_addr().expect("LocalRelay: local_addr failed");
            let url = format!("ws://{addr}");
            let shutdown = Arc::new(Notify::new());
            let received_reqs = Arc::new(Mutex::new(Vec::new()));
            // Broadcast channel fans every published event out to every open
            // connection's own per-connection task, which then applies that
            // connection's own subscription filters before forwarding.
            let (event_tx, _) = broadcast::channel::<Event>(1024);

            let shutdown_accept = shutdown.clone();
            let received_reqs_accept = received_reqs.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = shutdown_accept.notified() => break,
                        accepted = listener.accept() => {
                            let Ok((stream, _)) = accepted else { break };
                            let event_tx = event_tx.clone();
                            let shutdown_conn = shutdown_accept.clone();
                            let received_reqs_conn = received_reqs_accept.clone();
                            tokio::spawn(handle_connection(stream, event_tx, shutdown_conn, received_reqs_conn));
                        }
                    }
                }
            });

            Self { url, shutdown, received_reqs }
        }

        /// Snapshot of every REQ's filter set received so far, in arrival
        /// order, across every connection this relay has served.
        pub fn received_reqs(&self) -> Vec<Vec<Filter>> {
            self.received_reqs.lock().expect("received_reqs mutex poisoned").clone()
        }
    }

    async fn handle_connection(
        stream: TcpStream,
        event_tx: broadcast::Sender<Event>,
        shutdown: Arc<Notify>,
        received_reqs: Arc<Mutex<Vec<Vec<Filter>>>>,
    ) {
        let Ok(ws_stream) = tokio_tungstenite::accept_async(stream).await else {
            return;
        };
        let (mut write, mut read) = ws_stream.split();
        let mut event_rx = event_tx.subscribe();
        // sub_id -> filters registered by THIS connection via REQ.
        let mut subs: HashMap<String, Vec<Filter>> = HashMap::new();

        loop {
            tokio::select! {
                _ = shutdown.notified() => break,
                incoming = read.next() => {
                    let Some(Ok(msg)) = incoming else { break };
                    if !handle_client_message(msg, &mut write, &event_tx, &mut subs, &received_reqs).await {
                        break;
                    }
                }
                broadcasted = event_rx.recv() => {
                    let Ok(event) = broadcasted else { continue };
                    for (sub_id, filters) in &subs {
                        let matches = filters
                            .iter()
                            .any(|f| f.match_event(&event, MatchEventOptions::default()));
                        if matches {
                            let payload = serde_json::json!(["EVENT", sub_id, event]);
                            if write
                                .send(WsMessage::Text(payload.to_string().into()))
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Handle one inbound client frame. Returns `false` when the connection
    /// should close.
    async fn handle_client_message<S>(
        msg: WsMessage,
        write: &mut S,
        event_tx: &broadcast::Sender<Event>,
        subs: &mut HashMap<String, Vec<Filter>>,
        received_reqs: &Arc<Mutex<Vec<Vec<Filter>>>>,
    ) -> bool
    where
        S: futures::Sink<WsMessage> + Unpin,
    {
        let text = match msg {
            WsMessage::Text(t) => t,
            WsMessage::Close(_) => return false,
            _ => return true, // ping/pong/binary — ignore, keep connection open
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(text.as_str()) else {
            return true;
        };
        let Some(arr) = value.as_array() else {
            return true;
        };

        match arr.first().and_then(|v| v.as_str()) {
            Some("EVENT") => {
                if let Some(ev_val) = arr.get(1)
                    && let Ok(event) = serde_json::from_value::<Event>(ev_val.clone())
                {
                    let id = event.id;
                    let _ = event_tx.send(event);
                    let ok = serde_json::json!(["OK", id.to_hex(), true, ""]);
                    let _ = write.send(WsMessage::Text(ok.to_string().into())).await;
                }
            }
            Some("REQ") => {
                if let Some(sub_id) = arr.get(1).and_then(|v| v.as_str()) {
                    let filters: Vec<Filter> = arr[2..]
                        .iter()
                        .filter_map(|v| serde_json::from_value(v.clone()).ok())
                        .collect();
                    received_reqs
                        .lock()
                        .expect("received_reqs mutex poisoned")
                        .push(filters.clone());
                    subs.insert(sub_id.to_string(), filters);
                    let eose = serde_json::json!(["EOSE", sub_id]);
                    let _ = write.send(WsMessage::Text(eose.to_string().into())).await;
                }
            }
            Some("CLOSE") => {
                if let Some(sub_id) = arr.get(1).and_then(|v| v.as_str()) {
                    subs.remove(sub_id);
                }
            }
            _ => {}
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Boot a fresh in-process relay and return it alongside its `ws://` URL. The
/// relay stops accepting/serving when the returned `LocalRelay` is dropped —
/// callers MUST keep it alive for as long as the relay needs to be reachable.
pub async fn spawn_mock_relay() -> (local_relay::LocalRelay, String) {
    let relay = local_relay::LocalRelay::start().await;
    let url = relay.url.clone();
    (relay, url)
}

/// Build a `nostr_sdk::Client` for `keys`, connected to `relay_url`, with a
/// `SignerAuthenticator` installed. This is the current 0.45.0 API shape for
/// what 47.6-RESEARCH.md called `ClientOptions::automatic_authentication`
/// (that field no longer exists on this SDK version; passing an authenticator
/// to `Client::builder()` is the replacement — a correction recorded in this
/// plan's SUMMARY).
pub async fn connected_client(keys: &Keys, relay_url: &str) -> Client {
    let authenticator = SignerAuthenticator::new(keys.clone());
    let client = Client::builder().authenticator(authenticator).build();
    client
        .add_relay(relay_url)
        .await
        .expect("add_relay failed");
    client.connect().await;
    client
}

/// The stream type `Client::notifications()` returns. Named here so every
/// test can hold one without spelling out the `Pin<Box<dyn Stream<...>>>`
/// type at each call site.
pub type Notifications = std::pin::Pin<Box<dyn futures::Stream<Item = ClientNotification> + Send>>;

/// Subscribe to `client`'s notification stream.
///
/// **MUST be called BEFORE the action that is expected to produce a
/// notification (subscribe, publish, etc.) — never after.** This is a real,
/// verified 0.45.0 ordering requirement, not a style preference:
/// `Client::notifications()` subscribes a fresh receiver to an internal
/// broadcast channel, and a `tokio::sync::broadcast` channel only delivers a
/// message to receivers that were already subscribed at send time — a
/// receiver created afterward simply never sees it. Calling this AFTER
/// `send_event`/`subscribe` races the client's own background relay-read
/// task: under a multi-thread Tokio runtime the race is usually won by luck
/// (enough scheduling jitter that the foreground code reaches
/// `.notifications()` first); under `#[tokio::test]`'s default
/// current-thread runtime the race is lost **every time** — the background
/// task runs to completion and drops the notification before the foreground
/// code ever subscribes, and every assertion in this file hangs until its
/// timeout. Verified directly against a scratch reproduction against this
/// exact `nostr-sdk` version; both failure and fix are recorded in this
/// plan's SUMMARY.
pub fn subscribe_notifications(client: &Client) -> Notifications {
    client.notifications()
}

/// Bounded wait for the first notification event satisfying `pred`, or
/// `None` if `timeout` elapses first. Shared by every positive assertion in
/// this file. Takes an already-subscribed `notifications` stream (see
/// [`subscribe_notifications`] — call it before triggering the action).
///
/// Matches on `ClientNotification::Message` + `RelayMessage::Event`, NOT
/// `ClientNotification::Event` — a second real, verified 0.45.0 API behavior
/// this plan's SUMMARY documents as a deviation from 47.6-RESEARCH.md's
/// assumed shape: `ClientNotification::Event`'s own doc comment states it
/// fires only for events NOT sent by this client ("Events sent by this
/// client are not included") — which would make every same-client
/// publish-then-subscribe assertion in this file (the harness self-test,
/// `buzz_own_event_is_not_echoed`, and any test where one `Client` both
/// publishes and is subscribed to its own channel) hang forever waiting for
/// a notification variant that structurally cannot fire for a self-authored
/// event. `ClientNotification::Message` fires for every relay message
/// unconditionally (its own doc: "This notification is sent every time...
/// regardless of whether it has been received before"), which is what a
/// real round-trip assertion needs.
pub async fn wait_for_event<F>(
    notifications: &mut Notifications,
    timeout: Duration,
    pred: F,
) -> Option<Event>
where
    F: Fn(&Event) -> bool,
{
    tokio::time::timeout(timeout, async {
        while let Some(notification) = notifications.next().await {
            if let ClientNotification::Message { message, .. } = notification
                && let RelayMessage::Event { event, .. } = message.as_ref()
                && pred(event)
            {
                return event.clone().into_owned();
            }
        }
        panic!("notification stream closed before a matching event arrived");
    })
    .await
    .ok()
}

/// Settle window for negative assertions (absence of an event). Short enough
/// to keep the suite fast, long enough that a real bug reliably trips it.
pub const SETTLE_WINDOW: Duration = Duration::from_millis(800);

/// Assert that NO notification event satisfying `pred` arrives within
/// `window`. A negative test that merely asserts "no panic" proves nothing —
/// this makes the absence itself the assertion.
pub async fn assert_no_event<F>(notifications: &mut Notifications, window: Duration, pred: F)
where
    F: Fn(&Event) -> bool,
{
    let outcome = wait_for_event(notifications, window, pred).await;
    assert!(
        outcome.is_none(),
        "expected no matching event within the settle window, but one arrived: {outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// Harness self-test (Task 1) — every later test in this phase depends on this
// ---------------------------------------------------------------------------

#[tokio::test]
async fn relay_harness_round_trips_a_published_event() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let keys = Keys::generate();
    let client = connected_client(&keys, &relay_url).await;

    let filter = Filter::new().author(keys.public_key()).kind(Kind::TextNote);
    client.subscribe(filter).await.expect("subscribe failed");

    // MUST subscribe to notifications before publishing (see
    // `subscribe_notifications`'s doc comment) — this ordering IS the fix
    // for a real race this plan's SUMMARY documents.
    let mut notifications = subscribe_notifications(&client);

    let event = EventBuilder::new(Kind::TextNote, "buzz harness self-test")
        .finalize(&keys)
        .expect("event signing failed");
    let published_id = event.id;

    client.send_event(&event).await.expect("send_event failed");

    let received = wait_for_event(&mut notifications, Duration::from_secs(5), |e| {
        e.id == published_id
    })
    .await
    .expect("timed out waiting for the published event to round-trip");

    assert_eq!(received.id, published_id);
    assert_eq!(received.pubkey, keys.public_key());
}

// ---------------------------------------------------------------------------
// Task 2 — end-to-end "@-mention in a Buzz channel gets a signed reply"
// ---------------------------------------------------------------------------

/// Records every `MessageEvent` it receives and, when `respond` is `true`,
/// echoes the content straight back through the adapter it was handed — the
/// stub every positive test in this file uses in place of the real
/// `GatewayMessageHandler` (no LLM in the loop; the agent side is
/// already-proven shared code, per this plan's `<action>`).
struct RecordingHandler {
    events: Arc<TokioMutex<Vec<MessageEvent>>>,
    respond: bool,
}

impl RecordingHandler {
    fn new(respond: bool) -> Arc<Self> {
        Arc::new(Self {
            events: Arc::new(TokioMutex::new(Vec::new())),
            respond,
        })
    }

    async fn call_count(&self) -> usize {
        self.events.lock().await.len()
    }
}

#[async_trait::async_trait]
impl MessageHandler for RecordingHandler {
    async fn handle(
        &self,
        event: &MessageEvent,
        adapter: Arc<dyn PlatformAdapter>,
        _cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        self.events.lock().await.push(event.clone());
        if self.respond {
            adapter
                .send_message(&event.chat_id, &event.content, None)
                .await?;
        }
        Ok(())
    }
}

/// Records which message ids have STARTED handling and which have COMPLETED,
/// separately — the concurrency test needs to observe "started" for a second
/// message while the first is still in flight, well before either completes.
/// A message whose content contains the literal `BLOCK` marker waits on
/// `gate` before completing; every other message completes immediately.
struct ConcurrentHandler {
    started: Arc<TokioMutex<Vec<String>>>,
    completed: Arc<TokioMutex<Vec<String>>>,
    gate: Arc<tokio::sync::Notify>,
}

impl ConcurrentHandler {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            started: Arc::new(TokioMutex::new(Vec::new())),
            completed: Arc::new(TokioMutex::new(Vec::new())),
            gate: Arc::new(tokio::sync::Notify::new()),
        })
    }
}

#[async_trait::async_trait]
impl MessageHandler for ConcurrentHandler {
    async fn handle(
        &self,
        event: &MessageEvent,
        _adapter: Arc<dyn PlatformAdapter>,
        _cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        self.started.lock().await.push(event.message_id.clone());
        if event.content.contains("BLOCK") {
            self.gate.notified().await;
        }
        self.completed.lock().await.push(event.message_id.clone());
        Ok(())
    }
}

/// Build a `PlatformGatewayConfig` for the Buzz platform section with the
/// given channels/whitelist/trust — every other field left at its default.
fn buzz_config(
    channels: Vec<String>,
    whitelist: Vec<String>,
    channel_trust: ChannelTrust,
) -> PlatformGatewayConfig {
    PlatformGatewayConfig {
        enabled: true,
        channels,
        whitelist,
        channel_trust,
        ..Default::default()
    }
}

/// Build (and sign) a NIP-29 channel event: `kind:9`, tagged `h` = `channel`,
/// optionally `p`-tagging `mention` (D-16's mention convention — a tag, never
/// literal `"@name"` text).
fn build_channel_event(
    sender: &Keys,
    channel: &str,
    content: &str,
    mention: Option<PublicKey>,
) -> Event {
    let mut builder =
        EventBuilder::new(Kind::Custom(9), content).tag(Tag::custom("h", [channel.to_string()]));
    if let Some(pk) = mention {
        builder = builder.tag(Tag::public_key(pk));
    }
    builder.finalize(sender).expect("event signing failed")
}

/// Spawn `run_buzz_adapter` for a freshly generated identity ("agent A"),
/// connected to `relay_url`, with `handler` wired in. Returns the adapter
/// (so tests can assert on `pubkey_hex()`), A's `Keys`, the `CancellationToken`
/// controlling the loop (drop/cancel it to stop the spawned task cleanly at
/// the end of the test), and the spawned task's `JoinHandle`.
async fn spawn_agent(
    relay_url: &str,
    channels: Vec<String>,
    whitelist: Vec<String>,
    channel_trust: ChannelTrust,
    handler: Arc<dyn MessageHandler>,
) -> (
    Arc<BuzzAdapter>,
    Keys,
    CancellationToken,
    tokio::task::JoinHandle<()>,
) {
    let keys = Keys::generate();
    let adapter = Arc::new(BuzzAdapter::new(keys.clone(), relay_url.to_string()));
    adapter.connect().await.expect("BuzzAdapter connect failed");

    let config = buzz_config(channels, whitelist, channel_trust);
    let cancel = CancellationToken::new();
    let cancel_for_task = cancel.clone();
    let adapter_for_task = adapter.clone();
    let task = tokio::spawn(async move {
        let _ = run_buzz_adapter(adapter_for_task, config, handler, cancel_for_task).await;
    });

    // Give the adapter's subscribe() a moment to land before the test
    // publishes — the adapter itself does not race (it subscribes to
    // notifications before calling client.subscribe()), but the TEST's own
    // publish, issued from a separate client, has no ordering guarantee
    // relative to the adapter's subscribe() completing on the relay side.
    tokio::time::sleep(Duration::from_millis(50)).await;

    (adapter, keys, cancel, task)
}

/// Connect an "operator" (B) client subscribed to every `kind:9` event on
/// `channel`, so the test can observe both its own published event and any
/// reply A publishes back into the same channel.
async fn spawn_observer(relay_url: &str, channel: &str) -> (Client, Keys, Notifications) {
    let keys = Keys::generate();
    let client = connected_client(&keys, relay_url).await;
    let mut notifications = subscribe_notifications(&client);
    let filter = Filter::new()
        .kind(Kind::Custom(9))
        .custom_tag(SingleLetterTag::LOWERCASE_H, channel.to_string());
    client.subscribe(filter).await.expect("subscribe failed");
    // Drain the observer's own EOSE/OK noise isn't necessary — wait_for_event
    // already filters by predicate, so callers just look for what they need.
    let _ = &mut notifications; // keep alive; returned to caller
    (client, keys, notifications)
}

#[tokio::test]
async fn buzz_mention_in_channel_gets_signed_reply() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let channel = "channel-1";
    let handler = RecordingHandler::new(true);
    let (_adapter, a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![channel.to_string()],
        vec![],
        ChannelTrust::Open,
        handler.clone(),
    )
    .await;
    let (b_client, b_keys, mut b_notifications) = spawn_observer(&relay_url, channel).await;

    let mention_event =
        build_channel_event(&b_keys, channel, "hello agent", Some(a_keys.public_key()));
    let sent = b_client
        .send_event(&mention_event)
        .await
        .expect("send_event failed");
    assert_eq!(*sent.id(), mention_event.id);

    let reply = wait_for_event(&mut b_notifications, Duration::from_secs(5), |e| {
        e.pubkey == a_keys.public_key()
    })
    .await
    .expect("timed out waiting for agent's reply");

    assert_eq!(reply.content, "hello agent");
    assert_eq!(handler.call_count().await, 1);
    cancel.cancel();
}

#[tokio::test]
async fn buzz_channel_message_without_mention_is_ignored() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let channel = "channel-2";
    let handler = RecordingHandler::new(true);
    let (_adapter, a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![channel.to_string()],
        vec![],
        ChannelTrust::Open,
        handler.clone(),
    )
    .await;
    let (b_client, b_keys, mut b_notifications) = spawn_observer(&relay_url, channel).await;

    // No p-tag naming A — the mention gate must drop this.
    let unmentioned_event = build_channel_event(&b_keys, channel, "no mention here", None);
    b_client
        .send_event(&unmentioned_event)
        .await
        .expect("send_event failed");

    assert_no_event(&mut b_notifications, SETTLE_WINDOW, |e| {
        e.pubkey == a_keys.public_key()
    })
    .await;
    assert_eq!(handler.call_count().await, 0);
    cancel.cancel();
}

#[tokio::test]
async fn buzz_own_event_is_not_echoed() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let channel = "channel-3";
    let handler = RecordingHandler::new(false);
    let (adapter, a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![channel.to_string()],
        vec![],
        ChannelTrust::Open,
        handler.clone(),
    )
    .await;

    // A publishes an event authored by itself, p-tagging itself — the
    // self-loop guard must drop this before it ever reaches the handler,
    // regardless of the mention gate passing.
    adapter
        .send_message(channel, "self mention", None)
        .await
        .expect("send_message failed");
    // Publish a genuine self-mention too (send_message doesn't p-tag itself).
    let self_mention = build_channel_event(&a_keys, channel, "self mention 2", Some(a_keys.public_key()));
    adapter
        .send_message(channel, &self_mention.content, None)
        .await
        .expect("send_message failed");

    tokio::time::sleep(SETTLE_WINDOW).await;
    assert_eq!(handler.call_count().await, 0);
    cancel.cancel();
}

#[tokio::test]
async fn buzz_redelivered_event_dispatches_once() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let channel = "channel-4";
    let handler = RecordingHandler::new(false);
    let (_adapter, a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![channel.to_string()],
        vec![],
        ChannelTrust::Open,
        handler.clone(),
    )
    .await;
    let (b_client, b_keys, _b_notifications) = spawn_observer(&relay_url, channel).await;

    let mention_event =
        build_channel_event(&b_keys, channel, "redeliver me", Some(a_keys.public_key()));
    // Publish the SAME already-signed event twice — same event id both
    // times. A relay re-serving stored events after a reconnect looks
    // identical to the adapter: the same event id arriving on the
    // subscription more than once.
    b_client
        .send_event(&mention_event)
        .await
        .expect("first send_event failed");
    b_client
        .send_event(&mention_event)
        .await
        .expect("second send_event failed");

    tokio::time::sleep(SETTLE_WINDOW).await;
    assert_eq!(
        handler.call_count().await,
        1,
        "the same event id must dispatch exactly once"
    );
    cancel.cancel();
}

#[tokio::test]
async fn buzz_concurrent_messages_do_not_block_the_subscription() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let channel = "channel-5";
    let handler = ConcurrentHandler::new();
    let (_adapter, a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![channel.to_string()],
        vec![],
        ChannelTrust::Open,
        handler.clone(),
    )
    .await;
    let (b_client, b_keys, _b_notifications) = spawn_observer(&relay_url, channel).await;

    let blocking_event =
        build_channel_event(&b_keys, channel, "BLOCK first", Some(a_keys.public_key()));
    let second_event =
        build_channel_event(&b_keys, channel, "second unblocked", Some(a_keys.public_key()));

    b_client
        .send_event(&blocking_event)
        .await
        .expect("send_event failed");
    // Give the first event's handler task a moment to start (and block).
    tokio::time::sleep(Duration::from_millis(100)).await;
    b_client
        .send_event(&second_event)
        .await
        .expect("send_event failed");

    // Both must have STARTED even though the first is still blocked.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if handler.started.lock().await.len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("second message never reached the handler while the first was in flight");

    assert_eq!(
        handler.completed.lock().await.len(),
        1,
        "only the non-blocking message should have completed so far"
    );

    // Release the gate — the first (blocked) call now completes too.
    handler.gate.notify_waiters();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if handler.completed.lock().await.len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("blocked message never completed after the gate was released");

    cancel.cancel();
}

#[tokio::test]
async fn whitelist_entry_with_surrounding_whitespace_does_not_authorize() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let channel = "channel-6";
    let handler = RecordingHandler::new(true);
    let b_keys_probe = Keys::generate();
    let sender_hex = b_keys_probe.public_key().to_hex();
    let (_adapter, a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![channel.to_string()],
        vec![format!(" {sender_hex} ")], // whitespace-padded — must NOT match
        ChannelTrust::Closed,
        handler.clone(),
    )
    .await;
    let (b_client, _b_keys, mut b_notifications) = spawn_observer(&relay_url, channel).await;
    // Use the SAME keys the whitelist entry was derived from, so the only
    // difference between sender and whitelist entry is whitespace.
    let mention_event =
        build_channel_event(&b_keys_probe, channel, "should be denied", Some(a_keys.public_key()));
    b_client
        .send_event(&mention_event)
        .await
        .expect("send_event failed");

    assert_no_event(&mut b_notifications, SETTLE_WINDOW, |e| {
        e.pubkey == a_keys.public_key()
    })
    .await;
    assert_eq!(handler.call_count().await, 0);
    cancel.cancel();
}

#[tokio::test]
async fn whitelist_entry_with_different_hex_case_does_not_authorize() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let channel = "channel-7";
    let handler = RecordingHandler::new(true);
    let b_keys_probe = Keys::generate();
    let sender_hex = b_keys_probe.public_key().to_hex();
    let (_adapter, a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![channel.to_string()],
        vec![sender_hex.to_uppercase()], // case-differing — must NOT match
        ChannelTrust::Closed,
        handler.clone(),
    )
    .await;
    let (b_client, _b_keys, mut b_notifications) = spawn_observer(&relay_url, channel).await;
    let mention_event =
        build_channel_event(&b_keys_probe, channel, "should be denied", Some(a_keys.public_key()));
    b_client
        .send_event(&mention_event)
        .await
        .expect("send_event failed");

    assert_no_event(&mut b_notifications, SETTLE_WINDOW, |e| {
        e.pubkey == a_keys.public_key()
    })
    .await;
    assert_eq!(handler.call_count().await, 0);
    cancel.cancel();
}

#[tokio::test]
async fn buzz_closed_trust_empty_whitelist_denies() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let channel = "channel-8";
    let handler = RecordingHandler::new(true);
    let (_adapter, a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![channel.to_string()],
        vec![], // empty whitelist
        ChannelTrust::Closed,
        handler.clone(),
    )
    .await;
    let (b_client, b_keys, mut b_notifications) = spawn_observer(&relay_url, channel).await;
    let mention_event =
        build_channel_event(&b_keys, channel, "denied by empty whitelist", Some(a_keys.public_key()));
    b_client
        .send_event(&mention_event)
        .await
        .expect("send_event failed");

    assert_no_event(&mut b_notifications, SETTLE_WINDOW, |e| {
        e.pubkey == a_keys.public_key()
    })
    .await;
    assert_eq!(handler.call_count().await, 0);
    cancel.cancel();
}

#[tokio::test]
async fn buzz_closed_trust_unlisted_pubkey_denies() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let channel = "channel-9";
    let handler = RecordingHandler::new(true);
    let other_pubkey_hex = Keys::generate().public_key().to_hex();
    let (_adapter, a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![channel.to_string()],
        vec![other_pubkey_hex], // a DIFFERENT pubkey than B's
        ChannelTrust::Closed,
        handler.clone(),
    )
    .await;
    let (b_client, b_keys, mut b_notifications) = spawn_observer(&relay_url, channel).await;
    let mention_event =
        build_channel_event(&b_keys, channel, "denied by unlisted pubkey", Some(a_keys.public_key()));
    b_client
        .send_event(&mention_event)
        .await
        .expect("send_event failed");

    assert_no_event(&mut b_notifications, SETTLE_WINDOW, |e| {
        e.pubkey == a_keys.public_key()
    })
    .await;
    assert_eq!(handler.call_count().await, 0);
    cancel.cancel();
}

#[tokio::test]
async fn buzz_closed_trust_listed_pubkey_replies() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let channel = "channel-10";
    let handler = RecordingHandler::new(true);
    let b_keys_probe = Keys::generate();
    let sender_hex = b_keys_probe.public_key().to_hex();
    let (_adapter, a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![channel.to_string()],
        vec![sender_hex], // B's own pubkey hex, exactly
        ChannelTrust::Closed,
        handler.clone(),
    )
    .await;
    let (b_client, _b_keys, mut b_notifications) = spawn_observer(&relay_url, channel).await;
    let mention_event = build_channel_event(
        &b_keys_probe,
        channel,
        "allowed by listed pubkey",
        Some(a_keys.public_key()),
    );
    b_client
        .send_event(&mention_event)
        .await
        .expect("send_event failed");

    let reply = wait_for_event(&mut b_notifications, Duration::from_secs(5), |e| {
        e.pubkey == a_keys.public_key()
    })
    .await
    .expect("timed out waiting for agent's reply");

    assert_eq!(reply.content, "allowed by listed pubkey");
    assert_eq!(handler.call_count().await, 1);
    cancel.cancel();
}

// ---------------------------------------------------------------------------
// Whitelist deserialization — P0-2/D-05 (pure data tests, no relay needed)
// ---------------------------------------------------------------------------

#[test]
fn whitelist_bare_numbers_deserialize_to_strings() {
    let cfg: PlatformGatewayConfig =
        serde_yaml::from_str("whitelist: [12345, 67890]").expect("deserialize failed");
    assert_eq!(cfg.whitelist, vec!["12345".to_string(), "67890".to_string()]);
}

#[test]
fn whitelist_quoted_strings_deserialize() {
    let hex = "a".repeat(64);
    let yaml = format!("whitelist: [\"{hex}\", \"U012AB3CD\"]");
    let cfg: PlatformGatewayConfig = serde_yaml::from_str(&yaml).expect("deserialize failed");
    assert_eq!(cfg.whitelist, vec![hex, "U012AB3CD".to_string()]);
}

#[test]
fn whitelist_mixed_numbers_and_strings_deserialize() {
    let cfg: PlatformGatewayConfig =
        serde_yaml::from_str("whitelist: [12345, \"U012AB3CD\", 67890]")
            .expect("deserialize failed");
    assert_eq!(
        cfg.whitelist,
        vec![
            "12345".to_string(),
            "U012AB3CD".to_string(),
            "67890".to_string(),
        ]
    );
}

#[test]
fn whitelist_numeric_and_quoted_forms_of_one_id_compare_equal() {
    let cfg: PlatformGatewayConfig =
        serde_yaml::from_str("whitelist: [12345, \"12345\"]").expect("deserialize failed");
    assert_eq!(cfg.whitelist, vec!["12345".to_string(), "12345".to_string()]);
    // Both entries compare equal as strings — the coercion merges identity,
    // it does not mint two distinct authorizations.
    assert!(cfg.whitelist.iter().all(|w| w == "12345"));
}

#[test]
fn whitelist_loading_twice_is_byte_identical() {
    let yaml = "whitelist: [12345, \"U012AB3CD\", 67890]";
    let first: PlatformGatewayConfig = serde_yaml::from_str(yaml).expect("first deserialize");
    let second: PlatformGatewayConfig = serde_yaml::from_str(yaml).expect("second deserialize");
    assert_eq!(first.whitelist, second.whitelist);
}

// ---------------------------------------------------------------------------
// Per-channel subscription shape (47.6-08 fix) — regression coverage for the
// live-UAT finding that the agent never received live channel events. A
// single multi-channel REQ makes the Buzz relay's
// `extract_channel_id_from_filters` (buzz repo:
// `crates/buzz-relay/src/handlers/req.rs:1029`) see more than one distinct
// channel UUID and fall back to registering the subscription as GLOBAL,
// which the relay's `fan_out_scoped` invariant (buzz repo:
// `crates/buzz-relay/src/subscription.rs`) never delivers channel-scoped
// (`kind:9`) events to. These tests pin the fixed shape: one REQ per
// configured channel, each with exactly one `#h` value and a `since` bound;
// the DM gift-wrap filter carries neither.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn buzz_adapter_issues_one_req_per_configured_channel() {
    let (relay, relay_url) = spawn_mock_relay().await;
    let handler = RecordingHandler::new(false);
    let channels = vec![
        "channel-per-1".to_string(),
        "channel-per-2".to_string(),
        "channel-per-3".to_string(),
    ];
    let (_adapter, _a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        channels.clone(),
        vec![],
        ChannelTrust::Open,
        handler.clone(),
    )
    .await;

    let reqs = relay.received_reqs();
    let channel_reqs: Vec<&Vec<Filter>> = reqs
        .iter()
        .filter(|filters| {
            filters
                .iter()
                .any(|f| f.kinds.as_ref().is_some_and(|kinds| kinds.contains(&Kind::Custom(9))))
        })
        .collect();

    assert_eq!(
        channel_reqs.len(),
        channels.len(),
        "expected one REQ per configured channel (one filter each), got: {channel_reqs:?}"
    );

    let mut seen_channels: Vec<String> = Vec::new();
    for filters in &channel_reqs {
        assert_eq!(
            filters.len(),
            1,
            "expected exactly one filter in each channel REQ, got: {filters:?}"
        );
        let filter = &filters[0];
        let h_values = filter
            .generic_tags
            .get(&SingleLetterTag::LOWERCASE_H)
            .expect("channel filter missing #h tag");
        assert_eq!(
            h_values.len(),
            1,
            "expected exactly one #h value per channel filter (never multiple channels in one \
             filter — that shape is exactly what makes the relay treat the subscription as \
             global), got: {h_values:?}"
        );
        seen_channels.push(h_values.iter().next().unwrap().clone());
    }
    seen_channels.sort();
    let mut expected_channels = channels.clone();
    expected_channels.sort();
    assert_eq!(seen_channels, expected_channels);

    cancel.cancel();
}

#[tokio::test]
async fn buzz_channel_filters_carry_since_dm_filter_does_not() {
    let (relay, relay_url) = spawn_mock_relay().await;
    let handler = RecordingHandler::new(false);
    let channels = vec!["channel-since-1".to_string()];
    let (_adapter, _a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        channels.clone(),
        vec![],
        ChannelTrust::Open,
        handler.clone(),
    )
    .await;

    let reqs = relay.received_reqs();

    let channel_filter = reqs
        .iter()
        .flatten()
        .find(|f| f.kinds.as_ref().is_some_and(|kinds| kinds.contains(&Kind::Custom(9))))
        .expect("no channel (kind:9) filter observed in any REQ");
    assert!(
        channel_filter.since.is_some(),
        "channel filter must carry a `since` bound (the 15-minute replay grace that prevents \
         re-answering the entire channel history on every restart)"
    );

    let dm_filter = reqs
        .iter()
        .flatten()
        .find(|f| f.kinds.as_ref().is_some_and(|kinds| kinds.contains(&Kind::GiftWrap)))
        .expect("no DM (kind:1059 gift-wrap) filter observed in any REQ");
    assert!(
        dm_filter.since.is_none(),
        "DM gift-wrap filter must NEVER carry a `since` bound — NIP-17 randomizes created_at by \
         up to ~2 days specifically to defeat timestamp correlation, so bounding it would \
         silently drop legitimate DMs"
    );

    cancel.cancel();
}

// ---------------------------------------------------------------------------
// Phase 47.6 Plan 08 (T-47.6-08-RESTART) — persisted inbound dedupe across
// restart. Live UAT finding: a gateway restart replayed 4 already-answered
// mentions and started 4 agent loops simultaneously, because the in-memory-
// only dedupe set does not survive a process restart.
// ---------------------------------------------------------------------------

/// RAII guard: restores (or clears) `IRONHERMES_HOME` on drop, so a panicking
/// assertion in one of these tests never leaks a mutated env var forward.
/// nextest runs each test in its own process (see `telegram.rs`'s
/// `media_path_guard_tests` for the same convention already established in
/// this crate), so no cross-test synchronization is needed beyond that.
struct IronhermesHomeGuard {
    previous: Option<std::ffi::OsString>,
}

impl IronhermesHomeGuard {
    fn set(path: &std::path::Path) -> Self {
        let previous = std::env::var_os("IRONHERMES_HOME");
        // SAFETY: test-only env var mutation; nextest isolates this test in
        // its own process.
        unsafe {
            std::env::set_var("IRONHERMES_HOME", path);
        }
        Self { previous }
    }
}

impl Drop for IronhermesHomeGuard {
    fn drop(&mut self) {
        // SAFETY: see `set` above.
        unsafe {
            match &self.previous {
                Some(v) => std::env::set_var("IRONHERMES_HOME", v),
                None => std::env::remove_var("IRONHERMES_HOME"),
            }
        }
    }
}

#[tokio::test]
async fn buzz_dedupe_persists_across_adapter_restart() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let _home_guard = IronhermesHomeGuard::set(tempdir.path());

    let (_relay, relay_url) = spawn_mock_relay().await;
    let channel = "channel-restart-1";
    let agent_keys = Keys::generate();
    let config = buzz_config(vec![channel.to_string()], vec![], ChannelTrust::Open);

    // --- First adapter loop: dispatch a mention ---
    let adapter1 = Arc::new(BuzzAdapter::new(agent_keys.clone(), relay_url.clone()));
    adapter1.connect().await.expect("adapter1 connect failed");
    let handler1 = RecordingHandler::new(false);
    let cancel1 = CancellationToken::new();
    let task1 = {
        let adapter1 = adapter1.clone();
        let config = config.clone();
        let handler1: Arc<dyn MessageHandler> = handler1.clone();
        let cancel1 = cancel1.clone();
        tokio::spawn(async move {
            let _ = run_buzz_adapter(adapter1, config, handler1, cancel1).await;
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (b_client, b_keys, _b_notifications) = spawn_observer(&relay_url, channel).await;
    let mention_event = build_channel_event(
        &b_keys,
        channel,
        "restart replay test",
        Some(agent_keys.public_key()),
    );
    b_client
        .send_event(&mention_event)
        .await
        .expect("first send_event failed");

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if handler1.call_count().await == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("first dispatch never happened");

    // Tear down the first adapter loop cleanly before starting the second.
    cancel1.cancel();
    let _ = task1.await;

    // --- Second adapter loop: SAME identity, SAME IRONHERMES_HOME ---
    let adapter2 = Arc::new(BuzzAdapter::new(agent_keys.clone(), relay_url.clone()));
    adapter2.connect().await.expect("adapter2 connect failed");
    let handler2 = RecordingHandler::new(false);
    let cancel2 = CancellationToken::new();
    let task2 = {
        let adapter2 = adapter2.clone();
        let config = config.clone();
        let handler2: Arc<dyn MessageHandler> = handler2.clone();
        let cancel2 = cancel2.clone();
        tokio::spawn(async move {
            let _ = run_buzz_adapter(adapter2, config, handler2, cancel2).await;
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Replay: the SAME already-signed event (same event id), published
    // again — exactly what a relay re-serving stored events after a
    // reconnect looks like to the adapter.
    b_client
        .send_event(&mention_event)
        .await
        .expect("replay send_event failed");

    tokio::time::sleep(SETTLE_WINDOW).await;
    assert_eq!(
        handler2.call_count().await,
        0,
        "persisted dedupe must prevent re-dispatch of an already-answered mention after restart"
    );

    cancel2.cancel();
    let _ = task2.await;
}

#[tokio::test]
async fn buzz_adapter_starts_despite_corrupt_dedupe_file() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let _home_guard = IronhermesHomeGuard::set(tempdir.path());

    // Pre-seed a corrupt persisted dedupe file at the path the adapter will
    // read on startup.
    let state_dir = tempdir.path().join("state");
    std::fs::create_dir_all(&state_dir).expect("create state dir");
    std::fs::write(state_dir.join("buzz_seen_events.json"), b"not valid json{{{")
        .expect("write corrupt dedupe file");

    let (_relay, relay_url) = spawn_mock_relay().await;
    let channel = "channel-corrupt-dedupe-1";
    let handler = RecordingHandler::new(true);
    let (_adapter, a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![channel.to_string()],
        vec![],
        ChannelTrust::Open,
        handler.clone(),
    )
    .await;
    let (b_client, b_keys, mut b_notifications) = spawn_observer(&relay_url, channel).await;

    let mention_event = build_channel_event(
        &b_keys,
        channel,
        "adapter must still work",
        Some(a_keys.public_key()),
    );
    b_client
        .send_event(&mention_event)
        .await
        .expect("send_event failed");

    let reply = wait_for_event(&mut b_notifications, Duration::from_secs(5), |e| {
        e.pubkey == a_keys.public_key()
    })
    .await
    .expect("adapter failed to start/respond despite a corrupt dedupe file on disk");

    assert_eq!(reply.content, "adapter must still work");
    assert_eq!(handler.call_count().await, 1);
    cancel.cancel();
}

// ---------------------------------------------------------------------------
// Phase 47.6 Plan 08 (T-47.6-08-REPLY) — reply-marker + sender p-tag
// threading. Live UAT finding: our channel replies published `kind:9` with
// ONLY an `#h` tag (`"tags":[["h","..."]]`) — Buzz's UI could not thread
// them, and Buzz's own agents reply with `["e", <id>, "", "reply"]` plus a
// `p` tag.
// ---------------------------------------------------------------------------

/// Threads onto `event.thread_id` (falling back to `event.message_id` when
/// absent) — mimics exactly what `GatewayMessageHandler`'s Buzz CHANNEL turn
/// now wires through `StreamConsumer::with_reply_to` (T-47.6-08-REPLY,
/// updated by the live-UAT thread-root fix, 47.6 plan 08 restart:
/// `event.thread_id` is the RESOLVED THREAD ROOT computed by
/// `buzz::resolve_thread_root`, not the triggering message's raw id).
/// `RecordingHandler` intentionally keeps its pre-existing `None` behavior
/// above so none of the earlier tests in this file need to change.
struct ThreadedReplyHandler;

#[async_trait::async_trait]
impl MessageHandler for ThreadedReplyHandler {
    async fn handle(
        &self,
        event: &MessageEvent,
        adapter: Arc<dyn PlatformAdapter>,
        _cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        let thread_id = event
            .thread_id
            .clone()
            .unwrap_or_else(|| event.message_id.clone());
        adapter
            .send_message(&event.chat_id, &event.content, Some(&thread_id))
            .await?;
        Ok(())
    }
}

/// Build a signed `kind:9` channel event that both mentions `mention` (a
/// `p`-tag, so it passes the mention gate regardless of the `e`-tag) AND
/// carries a single raw `e`-tag `["e", root_hex, "", marker]` — used to
/// simulate a triggering message that is itself mid-thread (Slack-style flat
/// threading), so the reply-threading test below can assert the agent's
/// reply threads onto the resolved ROOT, not this message's own id.
fn build_mid_thread_channel_event(
    sender: &Keys,
    channel: &str,
    content: &str,
    mention: PublicKey,
    root_hex: &str,
    marker: &str,
) -> Event {
    EventBuilder::new(Kind::Custom(9), content)
        .tag(Tag::custom("h", [channel.to_string()]))
        .tag(Tag::public_key(mention))
        .tag(Tag::parse(["e", root_hex, "", marker]).expect("failed to build e-tag"))
        .finalize(sender)
        .expect("event signing failed")
}

#[tokio::test]
async fn buzz_channel_reply_carries_reply_marker_and_sender_p_tag() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let channel = "channel-reply-tags-1";
    let handler: Arc<dyn MessageHandler> = Arc::new(ThreadedReplyHandler);
    let (_adapter, a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![channel.to_string()],
        vec![],
        ChannelTrust::Open,
        handler,
    )
    .await;
    let (b_client, b_keys, mut b_notifications) = spawn_observer(&relay_url, channel).await;

    let mention_event = build_channel_event(
        &b_keys,
        channel,
        "please thread your reply",
        Some(a_keys.public_key()),
    );
    let mention_id = mention_event.id;
    b_client
        .send_event(&mention_event)
        .await
        .expect("send_event failed");

    let reply = wait_for_event(&mut b_notifications, Duration::from_secs(5), |e| {
        e.pubkey == a_keys.public_key()
    })
    .await
    .expect("timed out waiting for agent's reply");

    // #h — unchanged pre-existing tag.
    let h_value = reply
        .tags
        .iter()
        .find(|t| t.single_letter_tag() == Some(SingleLetterTag::LOWERCASE_H))
        .and_then(|t| t.content())
        .expect("reply missing #h tag");
    assert_eq!(h_value, channel);

    // ["e", <mention id>, "", "reply"] — NIP-10 reply marker.
    let e_tag = reply
        .tags
        .iter()
        .find(|t| t.single_letter_tag() == Some(SingleLetterTag::LOWERCASE_E))
        .expect("reply missing e tag");
    assert_eq!(
        e_tag.as_slice(),
        &[
            "e".to_string(),
            mention_id.to_hex(),
            String::new(),
            "reply".to_string(),
        ]
    );

    // p tag naming the mention's sender.
    let p_value = reply
        .tags
        .iter()
        .find(|t| t.single_letter_tag() == Some(SingleLetterTag::LOWERCASE_P))
        .and_then(|t| t.content())
        .expect("reply missing p tag");
    assert_eq!(p_value, b_keys.public_key().to_hex());

    cancel.cancel();
}

// ---------------------------------------------------------------------------
// Live UAT fix (47.6 plan 08 restart) — thread-root regression coverage.
// The Buzz relay REJECTED a threaded reply with
// `["OK", <id>, false, "invalid: root tag does not match thread ancestry"]`
// because the outbound e-tag pointed at the triggering message's own id
// (a mid-thread parent) instead of the thread's true root. These tests pin
// the fixed behavior: replying to a mid-thread message threads onto the
// RESOLVED ROOT, never the mid-thread parent.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn buzz_channel_reply_to_mid_thread_message_threads_onto_resolved_root() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let channel = "channel-reply-tags-2";
    let handler: Arc<dyn MessageHandler> = Arc::new(ThreadedReplyHandler);
    let (_adapter, a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![channel.to_string()],
        vec![],
        ChannelTrust::Open,
        handler,
    )
    .await;
    let (b_client, b_keys, mut b_notifications) = spawn_observer(&relay_url, channel).await;

    // The triggering message's own `e`-tag carries a "reply" marker
    // pointing at `root_id` — exactly the Slack-style flat-thread shape
    // from the live UAT finding: the triggering message is ITSELF a reply,
    // and its e-tag value is the thread's true root.
    let root_id = EventBuilder::new(Kind::TextNote, "thread root")
        .finalize(&Keys::generate())
        .expect("sign failed")
        .id;
    let mid_thread_event = build_mid_thread_channel_event(
        &b_keys,
        channel,
        "please thread onto the root, not onto me",
        a_keys.public_key(),
        &root_id.to_hex(),
        "reply",
    );
    let mid_thread_id = mid_thread_event.id;
    b_client
        .send_event(&mid_thread_event)
        .await
        .expect("send_event failed");

    let reply = wait_for_event(&mut b_notifications, Duration::from_secs(5), |e| {
        e.pubkey == a_keys.public_key()
    })
    .await
    .expect("timed out waiting for agent's reply");

    let e_tag = reply
        .tags
        .iter()
        .find(|t| t.single_letter_tag() == Some(SingleLetterTag::LOWERCASE_E))
        .expect("reply missing e tag");
    assert_eq!(
        e_tag.as_slice(),
        &[
            "e".to_string(),
            root_id.to_hex(),
            String::new(),
            "reply".to_string(),
        ],
        "reply must thread onto the RESOLVED ROOT, not the mid-thread message's own id"
    );
    assert_ne!(
        e_tag.content(),
        Some(mid_thread_id.to_hex().as_str()),
        "reply must NOT thread onto the mid-thread triggering message's own id"
    );

    cancel.cancel();
}

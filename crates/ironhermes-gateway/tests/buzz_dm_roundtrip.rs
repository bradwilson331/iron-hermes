#![cfg(feature = "buzz")]
//! NIP-17 gift-wrapped DM tests (Phase 47.6 Plan 05).
//!
//! Every test in this file verifies against an in-process NIP-01 relay
//! harness, mirroring `buzz_relay_roundtrip.rs`'s `local_relay` module
//! (plan 01) — cargo integration test files each compile as an independent
//! binary, so the harness is duplicated here rather than imported; the
//! *behavior* it exercises is not forked, only the plumbing that runs it.

use ironhermes_core::{ChannelTrust, MessageEvent, PlatformGatewayConfig};
use ironhermes_gateway::adapter::{MessageHandler, PlatformAdapter};
use ironhermes_gateway::buzz::{
    BuzzAdapter, BuzzChatTarget, buzz_dm_subscription_filter, parse_buzz_chat_target,
    run_buzz_adapter,
};
use nostr_sdk::prelude::*;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex as TokioMutex;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// local_relay — minimal in-process NIP-01 relay (mirrors plan 01's harness)
// ---------------------------------------------------------------------------

mod local_relay {
    use futures::{SinkExt, StreamExt};
    use nostr_sdk::prelude::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::{Notify, broadcast};
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    /// A running in-process relay. Dropping this stops the accept loop and
    /// every open connection task.
    pub struct LocalRelay {
        pub url: String,
        shutdown: Arc<Notify>,
    }

    impl Drop for LocalRelay {
        fn drop(&mut self) {
            self.shutdown.notify_waiters();
        }
    }

    impl LocalRelay {
        pub async fn start() -> Self {
            Self::start_inner(None).await
        }

        /// CR-02/WR-01 regression coverage: a relay that replies `["OK",
        /// <id>, false, "<reason>"]` to EVERY published event instead of
        /// broadcasting it — simulating a NIP-01 relay-side rejection so
        /// tests can assert `send_dm`/`delete_message` surface it as an
        /// `Err` rather than reporting success (buzz.rs's `output.success`
        /// check).
        pub async fn start_rejecting(reason: impl Into<String>) -> Self {
            Self::start_inner(Some(reason.into())).await
        }

        async fn start_inner(reject_reason: Option<String>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .expect("LocalRelay: bind failed");
            let addr = listener.local_addr().expect("LocalRelay: local_addr failed");
            let url = format!("ws://{addr}");
            let shutdown = Arc::new(Notify::new());
            let (event_tx, _) = broadcast::channel::<Event>(1024);
            let reject_reason = Arc::new(reject_reason);

            let shutdown_accept = shutdown.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = shutdown_accept.notified() => break,
                        accepted = listener.accept() => {
                            let Ok((stream, _)) = accepted else { break };
                            let event_tx = event_tx.clone();
                            let shutdown_conn = shutdown_accept.clone();
                            let reject_reason = reject_reason.clone();
                            tokio::spawn(handle_connection(stream, event_tx, shutdown_conn, reject_reason));
                        }
                    }
                }
            });

            Self { url, shutdown }
        }
    }

    async fn handle_connection(
        stream: TcpStream,
        event_tx: broadcast::Sender<Event>,
        shutdown: Arc<Notify>,
        reject_reason: Arc<Option<String>>,
    ) {
        let Ok(ws_stream) = tokio_tungstenite::accept_async(stream).await else {
            return;
        };
        let (mut write, mut read) = ws_stream.split();
        let mut event_rx = event_tx.subscribe();
        let mut subs: HashMap<String, Vec<Filter>> = HashMap::new();

        loop {
            tokio::select! {
                _ = shutdown.notified() => break,
                incoming = read.next() => {
                    let Some(Ok(msg)) = incoming else { break };
                    if !handle_client_message(msg, &mut write, &event_tx, &mut subs, &reject_reason).await {
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

    async fn handle_client_message<S>(
        msg: WsMessage,
        write: &mut S,
        event_tx: &broadcast::Sender<Event>,
        subs: &mut HashMap<String, Vec<Filter>>,
        reject_reason: &Arc<Option<String>>,
    ) -> bool
    where
        S: futures::Sink<WsMessage> + Unpin,
    {
        let text = match msg {
            WsMessage::Text(t) => t,
            WsMessage::Close(_) => return false,
            _ => return true,
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
                    if let Some(reason) = reject_reason.as_ref() {
                        // Simulated relay-side rejection: do NOT broadcast
                        // the event, reply with `["OK", <id>, false,
                        // "<reason>"]`.
                        let ok = serde_json::json!(["OK", id.to_hex(), false, reason]);
                        let _ = write.send(WsMessage::Text(ok.to_string().into())).await;
                    } else {
                        let _ = event_tx.send(event);
                        let ok = serde_json::json!(["OK", id.to_hex(), true, ""]);
                        let _ = write.send(WsMessage::Text(ok.to_string().into())).await;
                    }
                }
            }
            Some("REQ") => {
                if let Some(sub_id) = arr.get(1).and_then(|v| v.as_str()) {
                    let filters: Vec<Filter> = arr[2..]
                        .iter()
                        .filter_map(|v| serde_json::from_value(v.clone()).ok())
                        .collect();
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

pub async fn spawn_mock_relay() -> (local_relay::LocalRelay, String) {
    let relay = local_relay::LocalRelay::start().await;
    let url = relay.url.clone();
    (relay, url)
}

/// CR-02/WR-01 regression coverage: a relay that answers every publish with
/// `["OK", <id>, false, "<reason>"]` (see `local_relay::LocalRelay::start_rejecting`).
pub async fn spawn_rejecting_relay(reason: &str) -> (local_relay::LocalRelay, String) {
    let relay = local_relay::LocalRelay::start_rejecting(reason).await;
    let url = relay.url.clone();
    (relay, url)
}

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

pub type Notifications = std::pin::Pin<Box<dyn futures::Stream<Item = ClientNotification> + Send>>;

/// MUST be called BEFORE the action expected to produce a notification — see
/// `buzz_relay_roundtrip.rs`'s identical helper for the full root-cause
/// writeup of why (a real `tokio::sync::broadcast` subscribe-after-send race,
/// not a style preference).
pub fn subscribe_notifications(client: &Client) -> Notifications {
    client.notifications()
}

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

pub const SETTLE_WINDOW: Duration = Duration::from_millis(800);

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

/// Records every `MessageEvent` it receives and, when `respond` is `true`,
/// echoes the content straight back through the adapter it was handed — the
/// stub every positive test in this file uses in place of the real
/// `GatewayMessageHandler` (no LLM in the loop; mirrors plan 01's
/// `RecordingHandler`).
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

    async fn events_snapshot(&self) -> Vec<MessageEvent> {
        self.events.lock().await.clone()
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

/// Build a `PlatformGatewayConfig` for the Buzz platform section with the
/// given channels/whitelist/trust.
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

/// Spawn `run_buzz_adapter` for a freshly generated identity ("agent A"),
/// connected to `relay_url`, with the given channel config and `handler`
/// wired in. Mirrors plan 01's `spawn_agent` helper.
async fn spawn_agent_full(
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

    // Give the adapter's subscribe() calls a moment to land before the test
    // publishes — same rationale as plan 01's identical sleep.
    tokio::time::sleep(Duration::from_millis(50)).await;

    (adapter, keys, cancel, task)
}

/// DM-only convenience wrapper over [`spawn_agent_full`] — no channels
/// configured.
async fn spawn_agent(
    relay_url: &str,
    whitelist: Vec<String>,
    channel_trust: ChannelTrust,
    handler: Arc<dyn MessageHandler>,
) -> (
    Arc<BuzzAdapter>,
    Keys,
    CancellationToken,
    tokio::task::JoinHandle<()>,
) {
    spawn_agent_full(relay_url, vec![], whitelist, channel_trust, handler).await
}

/// Connect an "operator" client subscribed to every `kind:9` event on
/// `channel`, so a test can observe a channel-routed `send_message`.
///
/// Sleeps briefly after `subscribe()` returns before handing control back to
/// the caller: `subscribe().await` resolving does NOT itself guarantee the
/// RELAY has processed the `REQ` yet (same cross-connection race
/// `spawn_agent`'s own identical sleep documents) — the publisher here is a
/// separate client/connection from this observer, so there is no ordering
/// guarantee between "this call returned" and "the relay registered the
/// subscription" without it.
async fn spawn_channel_observer(relay_url: &str, channel: &str) -> (Client, Notifications) {
    let keys = Keys::generate();
    let client = connected_client(&keys, relay_url).await;
    let mut notifications = subscribe_notifications(&client);
    let filter = Filter::new()
        .kind(Kind::Custom(9))
        .custom_tag(SingleLetterTag::LOWERCASE_H, channel.to_string());
    client.subscribe(filter).await.expect("subscribe failed");
    tokio::time::sleep(Duration::from_millis(50)).await;
    let _ = &mut notifications;
    (client, notifications)
}

/// Connect a "peer" client subscribed to gift-wrapped DMs addressed to
/// `keys`, so a test can observe an npub-routed `send_message`. See
/// [`spawn_channel_observer`]'s doc comment for why the settle sleep is
/// needed here too.
async fn spawn_dm_observer(relay_url: &str, keys: &Keys) -> (Client, Notifications) {
    let client = connected_client(keys, relay_url).await;
    let mut notifications = subscribe_notifications(&client);
    let filter = buzz_dm_subscription_filter(keys.public_key());
    client.subscribe(filter).await.expect("subscribe failed");
    tokio::time::sleep(Duration::from_millis(50)).await;
    let _ = &mut notifications;
    (client, notifications)
}

/// Build (and sign) a genuine NIP-17 gift-wrapped DM from `sender` to
/// `receiver` — uses the SDK's own `PrivateDirectMessageBuilder`
/// (Don't-Hand-Roll: ephemeral-key wrapping, NIP-44 encryption and padding
/// are never reimplemented here).
fn build_dm_event(sender: &Keys, receiver: PublicKey, content: &str) -> Event {
    PrivateDirectMessageBuilder::new(receiver, content)
        .finalize(sender)
        .expect("gift wrap build failed")
}

/// Build a `kind:1059` event that is NOT valid NIP-44 ciphertext for
/// `receiver` — the SDK's `UnwrappedGift::from_gift_wrap` must fail to
/// decrypt it. Simulates a gift wrap addressed to us that we cannot actually
/// open (a normal occurrence on a public relay).
fn build_undecryptable_gift_wrap(receiver: PublicKey) -> Event {
    EventBuilder::new(Kind::GiftWrap, "not valid nip-44 ciphertext")
        .tag(Tag::public_key(receiver))
        .finalize(&Keys::generate())
        .expect("event signing failed")
}

// ---------------------------------------------------------------------------
// Task 1 — receive and unwrap NIP-17 gift-wrapped DMs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dm_from_whitelisted_pubkey_reaches_the_handler() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let b_keys = Keys::generate();
    let b_hex = b_keys.public_key().to_hex();
    let handler = RecordingHandler::new(false);
    let (adapter, _a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![b_hex],
        ChannelTrust::Closed,
        handler.clone(),
    )
    .await;

    let a_pubkey = PublicKey::from_hex(&adapter.pubkey_hex()).expect("adapter pubkey hex");
    let b_client = connected_client(&b_keys, &relay_url).await;
    let dm_event = build_dm_event(&b_keys, a_pubkey, "hello from B");
    b_client
        .send_event(&dm_event)
        .await
        .expect("send_event failed");

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if handler.call_count().await >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("DM never reached the handler");

    let events = handler.events_snapshot().await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].content, "hello from B");
    assert_eq!(events[0].sender_id, b_keys.public_key().to_hex());
    assert_eq!(events[0].chat_type, "dm");
    cancel.cancel();
}

#[tokio::test]
async fn dm_chat_id_is_the_peer_npub() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let b_keys = Keys::generate();
    let b_hex = b_keys.public_key().to_hex();
    let handler = RecordingHandler::new(false);
    let (adapter, _a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![b_hex],
        ChannelTrust::Closed,
        handler.clone(),
    )
    .await;

    let a_pubkey = PublicKey::from_hex(&adapter.pubkey_hex()).expect("adapter pubkey hex");
    let b_client = connected_client(&b_keys, &relay_url).await;
    let dm_event = build_dm_event(&b_keys, a_pubkey, "npub check");
    b_client
        .send_event(&dm_event)
        .await
        .expect("send_event failed");

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if handler.call_count().await >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("DM never reached the handler");

    let events = handler.events_snapshot().await;
    let expected_npub = b_keys
        .public_key()
        .to_bech32()
        .expect("to_bech32 failed");
    assert_eq!(events[0].chat_id, expected_npub);
    cancel.cancel();
}

#[tokio::test]
async fn dm_from_unlisted_pubkey_is_dropped_in_closed_trust() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let b_keys = Keys::generate();
    let other_hex = Keys::generate().public_key().to_hex();
    let handler = RecordingHandler::new(false);
    let (adapter, _a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![other_hex],
        ChannelTrust::Closed,
        handler.clone(),
    )
    .await;

    let a_pubkey = PublicKey::from_hex(&adapter.pubkey_hex()).expect("adapter pubkey hex");
    let b_client = connected_client(&b_keys, &relay_url).await;
    let dm_event = build_dm_event(&b_keys, a_pubkey, "should be denied");
    b_client
        .send_event(&dm_event)
        .await
        .expect("send_event failed");

    tokio::time::sleep(SETTLE_WINDOW).await;
    assert_eq!(handler.call_count().await, 0);
    cancel.cancel();
}

#[tokio::test]
async fn dm_from_unlisted_pubkey_is_dropped_in_open_trust() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let b_keys = Keys::generate();
    let other_hex = Keys::generate().public_key().to_hex();
    let handler = RecordingHandler::new(false);
    // Open trust widens CHANNELS only — DMs stay whitelist-gated (D-08).
    let (adapter, _a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![other_hex],
        ChannelTrust::Open,
        handler.clone(),
    )
    .await;

    let a_pubkey = PublicKey::from_hex(&adapter.pubkey_hex()).expect("adapter pubkey hex");
    let b_client = connected_client(&b_keys, &relay_url).await;
    let dm_event = build_dm_event(&b_keys, a_pubkey, "should still be denied");
    b_client
        .send_event(&dm_event)
        .await
        .expect("send_event failed");

    tokio::time::sleep(SETTLE_WINDOW).await;
    assert_eq!(handler.call_count().await, 0);
    cancel.cancel();
}

#[tokio::test]
async fn dm_needs_no_mention() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let b_keys = Keys::generate();
    let b_hex = b_keys.public_key().to_hex();
    let handler = RecordingHandler::new(false);
    let (adapter, _a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![b_hex],
        ChannelTrust::Closed,
        handler.clone(),
    )
    .await;

    let a_pubkey = PublicKey::from_hex(&adapter.pubkey_hex()).expect("adapter pubkey hex");
    let b_client = connected_client(&b_keys, &relay_url).await;
    // build_dm_event's rumor carries no p-tag naming A beyond the mandatory
    // NIP-17 receiver tag the SDK adds automatically -- there is no separate
    // "mention" tag at all, proving the mention gate is not applied to DMs.
    let dm_event = build_dm_event(&b_keys, a_pubkey, "no mention needed");
    b_client
        .send_event(&dm_event)
        .await
        .expect("send_event failed");

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if handler.call_count().await >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("DM never reached the handler");
    cancel.cancel();
}

#[test]
fn dm_subscription_filter_carries_the_p_constraint() {
    let keys = Keys::generate();
    let filter = buzz_dm_subscription_filter(keys.public_key());
    let json = filter.as_json();
    assert!(
        json.contains("\"#p\""),
        "expected filter JSON to carry a #p constraint, got: {json}"
    );
    assert!(
        json.contains(&keys.public_key().to_hex()),
        "expected filter JSON to name the adapter's own pubkey, got: {json}"
    );
}

#[tokio::test]
async fn unwrap_failure_is_dropped_and_logged_not_fatal() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let b_keys = Keys::generate();
    let b_hex = b_keys.public_key().to_hex();
    let handler = RecordingHandler::new(false);
    let (adapter, _a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![b_hex.clone()],
        ChannelTrust::Closed,
        handler.clone(),
    )
    .await;

    let a_pubkey = PublicKey::from_hex(&adapter.pubkey_hex()).expect("adapter pubkey hex");
    let b_client = connected_client(&b_keys, &relay_url).await;

    // An undecryptable gift wrap addressed to A must be dropped silently,
    // without killing the subscription loop.
    let bad_event = build_undecryptable_gift_wrap(a_pubkey);
    b_client
        .send_event(&bad_event)
        .await
        .expect("send_event failed");
    tokio::time::sleep(SETTLE_WINDOW).await;
    assert_eq!(handler.call_count().await, 0);

    // A SUBSEQUENT valid DM must still arrive -- proving the loop survived.
    let good_event = build_dm_event(&b_keys, a_pubkey, "still alive");
    b_client
        .send_event(&good_event)
        .await
        .expect("send_event failed");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if handler.call_count().await >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("subscription loop did not survive the unwrap failure");
    cancel.cancel();
}

#[test]
fn raw_gift_wrap_content_is_not_the_plaintext() {
    let sender = Keys::generate();
    let receiver = Keys::generate();
    let plaintext = "this must never appear on the wire";
    let event = build_dm_event(&sender, receiver.public_key(), plaintext);
    assert_ne!(
        event.content, plaintext,
        "gift-wrap outer content must never equal the plaintext"
    );
    assert_eq!(event.kind, Kind::GiftWrap);
}

// ---------------------------------------------------------------------------
// Task 2 — send gift-wrapped DMs and route outbound by chat target
// ---------------------------------------------------------------------------

#[tokio::test]
async fn send_to_npub_chat_id_produces_a_gift_wrap() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let handler = RecordingHandler::new(false);
    let (adapter, _a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![],
        ChannelTrust::Open,
        handler.clone(),
    )
    .await;

    let b_keys = Keys::generate();
    let (_b_client, mut b_notifications) = spawn_dm_observer(&relay_url, &b_keys).await;
    let b_npub = b_keys.public_key().to_bech32().expect("to_bech32 failed");

    adapter
        .send_message(&b_npub, "secret for B", None)
        .await
        .expect("send_message (DM) failed");

    let received = wait_for_event(&mut b_notifications, Duration::from_secs(5), |e| {
        e.kind == Kind::GiftWrap
    })
    .await
    .expect("gift wrap never arrived at B");

    let unwrapped =
        UnwrappedGift::from_gift_wrap(&b_keys, &received).expect("B failed to unwrap");
    assert_eq!(unwrapped.rumor.content, "secret for B");
    cancel.cancel();
}

#[tokio::test]
async fn send_to_channel_chat_id_produces_a_channel_event() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let handler = RecordingHandler::new(false);
    let (adapter, _a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![],
        ChannelTrust::Open,
        handler.clone(),
    )
    .await;

    let channel = "plain-channel-id";
    let (_observer, mut observer_notifications) = spawn_channel_observer(&relay_url, channel).await;

    adapter
        .send_message(channel, "hello channel", None)
        .await
        .expect("send_message (channel) failed");

    let received = wait_for_event(&mut observer_notifications, Duration::from_secs(5), |e| {
        e.kind == Kind::Custom(9)
    })
    .await
    .expect("channel event never arrived");
    assert_eq!(received.content, "hello channel");
    cancel.cancel();
}

#[tokio::test]
async fn dm_reply_round_trips_end_to_end() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let b_keys = Keys::generate();
    let b_hex = b_keys.public_key().to_hex();
    let handler = RecordingHandler::new(true); // echoes content back
    let (adapter, _a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![b_hex],
        ChannelTrust::Closed,
        handler.clone(),
    )
    .await;

    let a_pubkey = PublicKey::from_hex(&adapter.pubkey_hex()).expect("adapter pubkey hex");
    let b_client = connected_client(&b_keys, &relay_url).await;
    let mut b_notifications = subscribe_notifications(&b_client);
    let dm_filter = buzz_dm_subscription_filter(b_keys.public_key());
    b_client.subscribe(dm_filter).await.expect("subscribe failed");

    let dm_event = build_dm_event(&b_keys, a_pubkey, "echo this back");
    b_client
        .send_event(&dm_event)
        .await
        .expect("send_event failed");

    let reply = wait_for_event(&mut b_notifications, Duration::from_secs(5), |e| {
        e.kind == Kind::GiftWrap && e.id != dm_event.id
    })
    .await
    .expect("reply gift wrap never arrived at B");

    let unwrapped = UnwrappedGift::from_gift_wrap(&b_keys, &reply).expect("B failed to unwrap reply");
    assert_eq!(unwrapped.rumor.content, "echo this back");
    cancel.cancel();
}

#[test]
fn chat_target_parser_table() {
    let npub_keys = Keys::generate();
    let npub = npub_keys.public_key().to_bech32().expect("to_bech32 failed");
    let hex64 = Keys::generate().public_key().to_hex();

    match parse_buzz_chat_target(&npub).expect("npub should parse") {
        BuzzChatTarget::DirectMessage(pk) => assert_eq!(pk, npub_keys.public_key()),
        other => panic!("expected DirectMessage, got {other:?}"),
    }

    match parse_buzz_chat_target(&hex64).expect("hex64 should parse") {
        BuzzChatTarget::DirectMessage(pk) => assert_eq!(pk.to_hex(), hex64),
        other => panic!("expected DirectMessage, got {other:?}"),
    }

    match parse_buzz_chat_target("general-chat").expect("plain id should parse") {
        BuzzChatTarget::Channel(id) => assert_eq!(id, "general-chat"),
        other => panic!("expected Channel, got {other:?}"),
    }

    assert!(parse_buzz_chat_target("").is_err(), "empty chat_id must error");
    assert!(
        parse_buzz_chat_target("npub1thisisnotvalidbech32atall").is_err(),
        "malformed npub1-prefixed string must error, not fall through to Channel"
    );
}

#[tokio::test]
async fn send_to_malformed_npub_errors_without_publishing() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let handler = RecordingHandler::new(false);
    let (adapter, a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![],
        ChannelTrust::Open,
        handler.clone(),
    )
    .await;

    // Observe everything A's own pubkey might publish -- nothing should
    // appear after the malformed send attempt.
    let observer_keys = Keys::generate();
    let observer_client = connected_client(&observer_keys, &relay_url).await;
    let mut observer_notifications = subscribe_notifications(&observer_client);
    observer_client
        .subscribe(Filter::new().author(a_keys.public_key()))
        .await
        .expect("subscribe failed");
    tokio::time::sleep(Duration::from_millis(50)).await;

    let result = adapter
        .send_message("npub1thisisnotvalidbech32atall", "should not publish", None)
        .await;
    assert!(result.is_err(), "malformed npub send must return an error");

    assert_no_event(&mut observer_notifications, SETTLE_WINDOW, |_| true).await;
    cancel.cancel();
}

#[tokio::test]
async fn markdown_v2_send_delegates_to_send_message() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let handler = RecordingHandler::new(false);
    let (adapter, _a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![],
        ChannelTrust::Open,
        handler.clone(),
    )
    .await;

    let channel = "md-channel";
    let (_observer, mut observer_notifications) = spawn_channel_observer(&relay_url, channel).await;

    adapter
        .send_message_markdown_v2(channel, "same content either way", None)
        .await
        .expect("send_message_markdown_v2 failed");

    let received = wait_for_event(&mut observer_notifications, Duration::from_secs(5), |e| {
        e.kind == Kind::Custom(9)
    })
    .await
    .expect("channel event never arrived via markdown_v2 surface");
    assert_eq!(received.content, "same content either way");
    cancel.cancel();
}

// ---------------------------------------------------------------------------
// Task 3 — profile metadata, real deletion, and the no-panic sweep
// ---------------------------------------------------------------------------

/// Spawn an agent with an optional configured display name (via
/// `PlatformGatewayConfig.extra["display_name"]`, the flatten catch-all
/// field), open trust, no whitelist restriction.
async fn spawn_agent_with_display_name(
    relay_url: &str,
    display_name: Option<&str>,
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

    let mut config = buzz_config(vec![], vec![], ChannelTrust::Open);
    if let Some(name) = display_name {
        config
            .extra
            .insert("display_name".to_string(), serde_json::Value::String(name.to_string()));
    }

    let cancel = CancellationToken::new();
    let cancel_for_task = cancel.clone();
    let adapter_for_task = adapter.clone();
    let task = tokio::spawn(async move {
        let _ = run_buzz_adapter(adapter_for_task, config, handler, cancel_for_task).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    (adapter, keys, cancel, task)
}

/// Connect an observer subscribed to every event authored by `author`, so a
/// test can watch for metadata/deletion events published on connect/demand.
async fn spawn_author_observer(relay_url: &str, author: PublicKey) -> (Client, Notifications) {
    let keys = Keys::generate();
    let client = connected_client(&keys, relay_url).await;
    let mut notifications = subscribe_notifications(&client);
    client
        .subscribe(Filter::new().author(author))
        .await
        .expect("subscribe failed");
    tokio::time::sleep(Duration::from_millis(50)).await;
    let _ = &mut notifications;
    (client, notifications)
}

#[tokio::test]
async fn profile_metadata_is_published_on_connect() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let handler = RecordingHandler::new(false);

    // Observer must subscribe BEFORE the agent spawns/connects, since the
    // metadata publish happens as part of the adapter's startup sequence.
    let observer_keys = Keys::generate();
    let observer_client = connected_client(&observer_keys, &relay_url).await;
    let mut observer_notifications = subscribe_notifications(&observer_client);
    observer_client
        .subscribe(Filter::new().kind(Kind::Metadata))
        .await
        .expect("subscribe failed");
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (_adapter, _a_keys, cancel, _task) =
        spawn_agent_with_display_name(&relay_url, Some("My Buzz Agent"), handler.clone()).await;

    let received = wait_for_event(&mut observer_notifications, Duration::from_secs(5), |e| {
        e.kind == Kind::Metadata
    })
    .await
    .expect("metadata event never arrived");

    let metadata = Metadata::try_from(&received).expect("metadata parse failed");
    assert_eq!(metadata.name.as_deref(), Some("My Buzz Agent"));
    cancel.cancel();
}

#[tokio::test]
async fn profile_metadata_falls_back_to_a_derived_name() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let handler = RecordingHandler::new(false);

    let observer_keys = Keys::generate();
    let observer_client = connected_client(&observer_keys, &relay_url).await;
    let mut observer_notifications = subscribe_notifications(&observer_client);
    observer_client
        .subscribe(Filter::new().kind(Kind::Metadata))
        .await
        .expect("subscribe failed");
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (_adapter, _a_keys, cancel, _task) =
        spawn_agent_with_display_name(&relay_url, None, handler.clone()).await;

    let received = wait_for_event(&mut observer_notifications, Duration::from_secs(5), |e| {
        e.kind == Kind::Metadata
    })
    .await
    .expect("metadata event never arrived");

    let metadata = Metadata::try_from(&received).expect("metadata parse failed");
    let name = metadata.name.expect("expected a fallback name to be set");
    assert!(!name.is_empty(), "fallback display name must not be empty");
    cancel.cancel();
}

#[tokio::test]
async fn delete_message_publishes_a_deletion_event() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let handler = RecordingHandler::new(false);
    let (adapter, a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![],
        ChannelTrust::Open,
        handler.clone(),
    )
    .await;

    let (_observer, mut observer_notifications) = spawn_author_observer(&relay_url, a_keys.public_key()).await;

    let published = adapter
        .send_message("some-channel", "delete me later", None)
        .await
        .expect("send_message failed");

    adapter
        .delete_message("some-channel", &published.message_id)
        .await
        .expect("delete_message failed");

    let deletion = wait_for_event(&mut observer_notifications, Duration::from_secs(5), |e| {
        e.kind == Kind::EventDeletion
    })
    .await
    .expect("deletion event never arrived");

    let target_id = EventId::from_hex(&published.message_id).expect("valid event id");
    assert!(
        deletion.tags.event_ids().any(|id| id == target_id),
        "deletion event must reference the deleted event id via an e-tag"
    );
    cancel.cancel();
}

#[tokio::test]
async fn delete_message_on_a_foreign_event_id_errors() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let handler = RecordingHandler::new(false);
    let (adapter, a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![],
        ChannelTrust::Open,
        handler.clone(),
    )
    .await;

    let (_observer, mut observer_notifications) = spawn_author_observer(&relay_url, a_keys.public_key()).await;

    // An event id this adapter never published.
    let foreign_event = EventBuilder::new(Kind::TextNote, "foreign")
        .finalize(&Keys::generate())
        .expect("event signing failed");

    let result = adapter
        .delete_message("some-channel", &foreign_event.id.to_hex())
        .await;
    assert!(result.is_err(), "deleting a foreign event id must error");

    assert_no_event(&mut observer_notifications, SETTLE_WINDOW, |e| {
        e.kind == Kind::EventDeletion
    })
    .await;
    cancel.cancel();
}

#[tokio::test]
async fn edit_message_is_a_noop_and_publishes_nothing() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let handler = RecordingHandler::new(false);
    let (adapter, a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![],
        ChannelTrust::Open,
        handler.clone(),
    )
    .await;

    let (_observer, mut observer_notifications) = spawn_author_observer(&relay_url, a_keys.public_key()).await;

    adapter
        .edit_message("some-channel", "deadbeef", "new content")
        .await
        .expect("edit_message must succeed (no-op)");
    adapter
        .edit_message_markdown_v2("some-channel", "deadbeef", "new content")
        .await
        .expect("edit_message_markdown_v2 must succeed (no-op)");

    assert_no_event(&mut observer_notifications, SETTLE_WINDOW, |_| true).await;
    cancel.cancel();
}

#[tokio::test]
async fn adapter_has_no_panicking_method() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let handler = RecordingHandler::new(false);
    let (adapter, _a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![],
        ChannelTrust::Open,
        handler.clone(),
    )
    .await;
    let dyn_adapter: Arc<dyn PlatformAdapter> = adapter.clone();

    assert_eq!(dyn_adapter.platform(), ironhermes_core::Platform::Buzz);

    let sent = dyn_adapter
        .send_message("sweep-channel", "sweep", None)
        .await
        .expect("send_message should not panic/error here");
    let _ = dyn_adapter
        .send_message_markdown_v2("sweep-channel", "sweep md", None)
        .await;
    let _ = dyn_adapter
        .edit_message("sweep-channel", &sent.message_id, "edited")
        .await;
    let _ = dyn_adapter
        .edit_message_markdown_v2("sweep-channel", &sent.message_id, "edited md")
        .await;
    let _ = dyn_adapter.delete_message("sweep-channel", &sent.message_id).await;
    let _ = dyn_adapter.add_reaction("sweep-channel", &sent.message_id, "👍").await;
    let _ = dyn_adapter.send_chat_action("sweep-channel", "typing").await;
    let _ = dyn_adapter.is_running();
    let _ = dyn_adapter.supports_in_place_edits();

    cancel.cancel();
}

// ---------------------------------------------------------------------------
// CR-02 regression (47.6 code review) — `send_dm` must surface relay
// rejection instead of reporting a rejected gift-wrap publish as delivered.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn send_dm_surfaces_relay_rejection_instead_of_reporting_success() {
    let (_relay, relay_url) =
        spawn_rejecting_relay("blocked: rate limited").await;
    let handler = RecordingHandler::new(false);
    let (adapter, _a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![],
        ChannelTrust::Open,
        handler.clone(),
    )
    .await;

    assert_eq!(
        adapter.own_events_len_for_test(),
        0,
        "own_events must start empty"
    );

    let b_keys = Keys::generate();
    let b_npub = b_keys.public_key().to_bech32().expect("to_bech32 failed");

    let result = adapter.send_message(&b_npub, "secret for B", None).await;

    assert!(
        result.is_err(),
        "send_dm must surface a relay rejection as Err, not report the DM as delivered; got {result:?}"
    );
    let err_text = result.unwrap_err().to_string();
    assert!(
        err_text.contains("blocked: rate limited") || err_text.to_lowercase().contains("reject"),
        "error should name the relay rejection reason; got: {err_text}"
    );

    assert_eq!(
        adapter.own_events_len_for_test(),
        0,
        "a rejected DM publish must NOT be recorded in own_events"
    );

    cancel.cancel();
}

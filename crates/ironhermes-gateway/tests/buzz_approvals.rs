#![cfg(feature = "buzz")]
//! Buzz approval-command guard + end-to-end approval round trip
//! (Phase 47.6 Plan 06, D-08/D-14/P1-2).
//!
//! Every test in this file verifies against an in-process NIP-01 relay
//! harness, mirroring `buzz_dm_roundtrip.rs`'s (plan 05) and
//! `buzz_relay_roundtrip.rs`'s (plan 01) identical harnesses — cargo
//! integration test files each compile as an independent binary, so the
//! harness is duplicated here rather than imported; the *behavior* it
//! exercises is not forked, only the plumbing that runs it.

use ironhermes_core::{
    ApprovalOutcome, ApprovalsStore, AuditConfig, AuditLog, ChannelTrust, Config, MessageEvent,
    Platform, PlatformGatewayConfig, ProviderResolver,
};
use ironhermes_gateway::adapter::{MessageHandler, PlatformAdapter};
use ironhermes_gateway::approval::ApprovalCoordinator;
use ironhermes_gateway::buzz::{
    BuzzAdapter, buzz_dm_subscription_filter, is_approval_command, run_buzz_adapter,
};
use ironhermes_gateway::handler::GatewayMessageHandler;
use ironhermes_gateway::session::{SessionKey, SessionStore};
use ironhermes_tools::ToolRegistry;
use nostr_sdk::prelude::*;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex as TokioMutex, RwLock};
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// local_relay — minimal in-process NIP-01 relay (mirrors plans 01/05's harness)
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
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .expect("LocalRelay: bind failed");
            let addr = listener.local_addr().expect("LocalRelay: local_addr failed");
            let url = format!("ws://{addr}");
            let shutdown = Arc::new(Notify::new());
            let (event_tx, _) = broadcast::channel::<Event>(1024);

            let shutdown_accept = shutdown.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = shutdown_accept.notified() => break,
                        accepted = listener.accept() => {
                            let Ok((stream, _)) = accepted else { break };
                            let event_tx = event_tx.clone();
                            let shutdown_conn = shutdown_accept.clone();
                            tokio::spawn(handle_connection(stream, event_tx, shutdown_conn));
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
                    if !handle_client_message(msg, &mut write, &event_tx, &mut subs).await {
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

/// Records every `MessageEvent` it receives — the stub every test in this
/// file uses in place of the real `GatewayMessageHandler` (task 3 drives the
/// real handler separately; task 2 only needs to prove whether an event
/// reaches ANY handler at all).
struct RecordingHandler {
    events: Arc<TokioMutex<Vec<MessageEvent>>>,
}

impl RecordingHandler {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            events: Arc::new(TokioMutex::new(Vec::new())),
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
        _adapter: Arc<dyn PlatformAdapter>,
        _cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        self.events.lock().await.push(event.clone());
        Ok(())
    }
}

/// Spawn `run_buzz_adapter` for a freshly generated identity ("agent A"),
/// connected to `relay_url`, with the given channel config and `handler`
/// wired in. Mirrors plan 01/05's identical `spawn_agent_full` helper.
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
    // publishes — same rationale as plans 01/05's identical sleep.
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

/// Build (and sign) a genuine NIP-17 gift-wrapped DM from `sender` to
/// `receiver` — uses the SDK's own `PrivateDirectMessageBuilder`
/// (Don't-Hand-Roll: ephemeral-key wrapping, NIP-44 encryption and padding
/// are never reimplemented here).
fn build_dm_event(sender: &Keys, receiver: PublicKey, content: &str) -> Event {
    PrivateDirectMessageBuilder::new(receiver, content)
        .finalize(sender)
        .expect("gift wrap build failed")
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

/// Poll `handler.call_count()` until it reaches `at_least`, or panic after a
/// generous timeout. Used by the positive-dispatch tests below.
async fn wait_for_dispatch(handler: &RecordingHandler, at_least: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if handler.call_count().await >= at_least {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("event never reached the handler")
}

// ---------------------------------------------------------------------------
// Task 2 — Buzz approval-command guard: DM-only, pubkey re-validated
// ---------------------------------------------------------------------------

#[tokio::test]
async fn approval_command_from_whitelisted_dm_is_dispatched() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let b_keys = Keys::generate();
    let b_hex = b_keys.public_key().to_hex();
    let handler = RecordingHandler::new();
    let (adapter, _a_keys, cancel, _task) =
        spawn_agent(&relay_url, vec![b_hex], ChannelTrust::Closed, handler.clone()).await;

    let a_pubkey = PublicKey::from_hex(&adapter.pubkey_hex()).expect("adapter pubkey hex");
    let b_client = connected_client(&b_keys, &relay_url).await;
    let dm_event = build_dm_event(&b_keys, a_pubkey, "/approve abc123");
    b_client
        .send_event(&dm_event)
        .await
        .expect("send_event failed");

    wait_for_dispatch(&handler, 1).await;
    cancel.cancel();
}

#[tokio::test]
async fn approval_command_from_unlisted_dm_is_rejected() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let b_keys = Keys::generate();
    let other_hex = Keys::generate().public_key().to_hex();
    let handler = RecordingHandler::new();
    let (adapter, _a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![other_hex],
        ChannelTrust::Closed,
        handler.clone(),
    )
    .await;

    let a_pubkey = PublicKey::from_hex(&adapter.pubkey_hex()).expect("adapter pubkey hex");
    let b_client = connected_client(&b_keys, &relay_url).await;
    let dm_event = build_dm_event(&b_keys, a_pubkey, "/approve abc123");
    b_client
        .send_event(&dm_event)
        .await
        .expect("send_event failed");

    tokio::time::sleep(SETTLE_WINDOW).await;
    assert_eq!(handler.call_count().await, 0);
    cancel.cancel();
}

#[tokio::test]
async fn approval_command_from_unlisted_dm_is_rejected_in_open_trust() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let b_keys = Keys::generate();
    let other_hex = Keys::generate().public_key().to_hex();
    let handler = RecordingHandler::new();
    // Open trust widens CHANNELS only — DMs (and approval commands
    // especially) stay whitelist-gated (D-08).
    let (adapter, _a_keys, cancel, _task) = spawn_agent(
        &relay_url,
        vec![other_hex],
        ChannelTrust::Open,
        handler.clone(),
    )
    .await;

    let a_pubkey = PublicKey::from_hex(&adapter.pubkey_hex()).expect("adapter pubkey hex");
    let b_client = connected_client(&b_keys, &relay_url).await;
    let dm_event = build_dm_event(&b_keys, a_pubkey, "/approve abc123");
    b_client
        .send_event(&dm_event)
        .await
        .expect("send_event failed");

    tokio::time::sleep(SETTLE_WINDOW).await;
    assert_eq!(handler.call_count().await, 0);
    cancel.cancel();
}

#[tokio::test]
async fn approval_command_with_empty_whitelist_is_rejected() {
    for trust in [ChannelTrust::Closed, ChannelTrust::Open] {
        let (_relay, relay_url) = spawn_mock_relay().await;
        let b_keys = Keys::generate();
        let handler = RecordingHandler::new();
        let (adapter, _a_keys, cancel, _task) =
            spawn_agent(&relay_url, vec![], trust, handler.clone()).await;

        let a_pubkey = PublicKey::from_hex(&adapter.pubkey_hex()).expect("adapter pubkey hex");
        let b_client = connected_client(&b_keys, &relay_url).await;
        let dm_event = build_dm_event(&b_keys, a_pubkey, "/approve abc123");
        b_client
            .send_event(&dm_event)
            .await
            .expect("send_event failed");

        tokio::time::sleep(SETTLE_WINDOW).await;
        assert_eq!(
            handler.call_count().await,
            0,
            "empty whitelist must reject every approval command under {trust:?} trust"
        );
        cancel.cancel();
    }
}

#[tokio::test]
async fn approval_command_in_a_channel_is_rejected() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let b_keys = Keys::generate();
    let b_hex = b_keys.public_key().to_hex();
    let handler = RecordingHandler::new();
    let channel = "ops-channel";
    let (adapter, _a_keys, cancel, _task) = spawn_agent_full(
        &relay_url,
        vec![channel.to_string()],
        vec![b_hex],
        ChannelTrust::Closed,
        handler.clone(),
    )
    .await;

    let a_pubkey = PublicKey::from_hex(&adapter.pubkey_hex()).expect("adapter pubkey hex");
    let b_client = connected_client(&b_keys, &relay_url).await;
    // A whitelisted sender, a valid mention -- everything the ordinary
    // channel gate + mention gate would accept -- but the content is an
    // approval command, which must still be rejected (D-14 DM-only).
    let channel_event =
        build_channel_event(&b_keys, channel, "/approve abc123", Some(a_pubkey));
    b_client
        .send_event(&channel_event)
        .await
        .expect("send_event failed");

    tokio::time::sleep(SETTLE_WINDOW).await;
    assert_eq!(handler.call_count().await, 0);
    cancel.cancel();
}

#[tokio::test]
async fn deny_command_follows_the_same_rules() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let b_keys = Keys::generate();
    let b_hex = b_keys.public_key().to_hex();
    let handler = RecordingHandler::new();
    let (adapter, _a_keys, cancel, _task) =
        spawn_agent(&relay_url, vec![b_hex], ChannelTrust::Closed, handler.clone()).await;

    let a_pubkey = PublicKey::from_hex(&adapter.pubkey_hex()).expect("adapter pubkey hex");
    let b_client = connected_client(&b_keys, &relay_url).await;
    let dm_event = build_dm_event(&b_keys, a_pubkey, "/deny abc123");
    b_client
        .send_event(&dm_event)
        .await
        .expect("send_event failed");

    // Positive: a whitelisted DM /deny reaches the handler exactly like /approve.
    wait_for_dispatch(&handler, 1).await;
    cancel.cancel();

    // Negative: unlisted sender + channel-posted variants also mirror /approve.
    let (_relay2, relay_url2) = spawn_mock_relay().await;
    let other_hex = Keys::generate().public_key().to_hex();
    let handler2 = RecordingHandler::new();
    let channel = "ops-channel";
    let (adapter2, _a_keys2, cancel2, _task2) = spawn_agent_full(
        &relay_url2,
        vec![channel.to_string()],
        vec![other_hex],
        ChannelTrust::Closed,
        handler2.clone(),
    )
    .await;
    let a2_pubkey = PublicKey::from_hex(&adapter2.pubkey_hex()).expect("adapter pubkey hex");

    // Unlisted DM /deny.
    let b2_client = connected_client(&b_keys, &relay_url2).await;
    let unlisted_dm = build_dm_event(&b_keys, a2_pubkey, "/deny abc123");
    b2_client
        .send_event(&unlisted_dm)
        .await
        .expect("send_event failed");
    tokio::time::sleep(SETTLE_WINDOW).await;
    assert_eq!(handler2.call_count().await, 0, "unlisted /deny DM must be rejected");

    // Channel-posted /deny from a fresh whitelisted sender.
    let c_keys = Keys::generate();
    let c_hex = c_keys.public_key().to_hex();
    let (_relay3, relay_url3) = spawn_mock_relay().await;
    let handler3 = RecordingHandler::new();
    let (adapter3, _a_keys3, cancel3, _task3) = spawn_agent_full(
        &relay_url3,
        vec![channel.to_string()],
        vec![c_hex],
        ChannelTrust::Closed,
        handler3.clone(),
    )
    .await;
    let a3_pubkey = PublicKey::from_hex(&adapter3.pubkey_hex()).expect("adapter pubkey hex");
    let c_client = connected_client(&c_keys, &relay_url3).await;
    let channel_deny = build_channel_event(&c_keys, channel, "/deny abc123", Some(a3_pubkey));
    c_client
        .send_event(&channel_deny)
        .await
        .expect("send_event failed");
    tokio::time::sleep(SETTLE_WINDOW).await;
    assert_eq!(
        handler3.call_count().await,
        0,
        "channel-posted /deny must be rejected even from a whitelisted, mentioning sender"
    );

    cancel2.cancel();
    cancel3.cancel();
}

#[test]
fn approval_command_detection_is_exact() {
    let cases: &[(&str, bool)] = &[
        ("/approve", true),
        ("/deny", true),
        ("/approve abc123", true),
        ("/deny abc123", true),
        ("   /approve abc123", true),
        ("   /deny", true),
        ("/approvecommand", false),
        ("/denywhatever", false),
        ("this is /approve abc123", false),
        ("this is /deny abc123", false),
        ("", false),
        ("just some text", false),
    ];
    for (input, expected) in cases {
        assert_eq!(
            is_approval_command(input),
            *expected,
            "is_approval_command({input:?}) expected {expected}"
        );
    }
}

#[tokio::test]
async fn non_approval_message_from_whitelisted_channel_sender_still_flows() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let b_keys = Keys::generate();
    let b_hex = b_keys.public_key().to_hex();
    let handler = RecordingHandler::new();
    let channel = "ops-channel";
    let (adapter, _a_keys, cancel, _task) = spawn_agent_full(
        &relay_url,
        vec![channel.to_string()],
        vec![b_hex],
        ChannelTrust::Closed,
        handler.clone(),
    )
    .await;

    let a_pubkey = PublicKey::from_hex(&adapter.pubkey_hex()).expect("adapter pubkey hex");
    let b_client = connected_client(&b_keys, &relay_url).await;
    let channel_event =
        build_channel_event(&b_keys, channel, "hey, can you help with this?", Some(a_pubkey));
    b_client
        .send_event(&channel_event)
        .await
        .expect("send_event failed");

    wait_for_dispatch(&handler, 1).await;
    cancel.cancel();
}

// ---------------------------------------------------------------------------
// Task 3 — end-to-end approval round trip: real ApprovalCoordinator,
// real GatewayMessageHandler, over the relay harness (D-14, P1-2, T-47.6-06-03).
// ---------------------------------------------------------------------------

/// Build an in-memory `SessionStore` — fresh per test, so `ctx_session_id`
/// always falls back to `SessionKey::to_string_key()` (no prior session row),
/// matching what these tests independently compute as the coordinator's
/// `session_id`.
fn build_test_session_store() -> Arc<RwLock<SessionStore>> {
    let state_store = Arc::new(std::sync::Mutex::new(
        ironhermes_state::StateStore::new(":memory:").expect("in-memory StateStore"),
    ));
    Arc::new(RwLock::new(SessionStore::new(state_store)))
}

/// Build a real `GatewayMessageHandler` — no `AgentRuntime` wired: the
/// `/approve`/`/deny` intercept (handler.rs lines ~888-920) returns before
/// ever touching `agent_runtime`, so these tests need nothing beyond the
/// approval coordinator installed via `set_approval_coordinator`.
fn build_test_handler(
    store: Arc<RwLock<SessionStore>>,
    coordinator: Arc<ApprovalCoordinator>,
) -> GatewayMessageHandler {
    let config = Config::default();
    let resolver = ProviderResolver::build(&config).unwrap();
    let tool_registry = Arc::new(RwLock::new(ToolRegistry::new()));
    let mut handler = GatewayMessageHandler::new(config, resolver, store, tool_registry);
    handler.set_approval_coordinator(coordinator);
    handler
}

/// Build an `AuditLog` at a fresh temp path (never the operator's real
/// `~/.ironhermes/audit.jsonl`), returning the path so the test can read the
/// JSONL back and assert on the `surface` field.
fn make_temp_audit_log() -> (Arc<AuditLog>, PathBuf) {
    let dir = tempfile::tempdir()
        .expect("tempdir for audit log")
        .keep();
    let path = dir.join("audit.jsonl");
    (
        Arc::new(AuditLog::with_path(path.clone(), AuditConfig::default())),
        path,
    )
}

fn make_temp_approvals_store() -> Arc<ApprovalsStore> {
    let dir = tempfile::tempdir()
        .expect("tempdir for approvals store")
        .keep();
    Arc::new(ApprovalsStore::with_path(dir.join("approvals.json")))
}

/// The session_id a real Buzz DM turn from `sender_hex` addressed to this
/// agent (chat_id = the DM's `sender_npub`, per plan 05's routing rule) would
/// compute as `ctx_session_id`, absent any prior session row. Built from the
/// REAL `SessionKey` type (not a hand-formatted string) so this can never
/// drift from `handler.rs`'s own derivation.
fn dm_session_id(chat_id_npub: &str, sender_hex: &str) -> String {
    SessionKey::new(Platform::Buzz, chat_id_npub)
        .with_user(sender_hex)
        .to_string_key()
}

/// The end-to-end D-14 loop: a pending approval parked directly on the
/// coordinator produces a gift-wrapped DM prompt to the operator; the
/// operator's genuine gift-wrapped `/approve` DM is unwrapped by the REAL
/// Buzz inbound loop, re-validated by task 2's guard, and dispatched to the
/// REAL `GatewayMessageHandler`'s `/approve` intercept, which resolves the
/// SAME coordinator and replies with a gift-wrapped confirmation. Also
/// proves the audit half: exactly one entry, surface names buzz.
#[tokio::test]
async fn parked_approval_resolves_via_a_real_operator_approve_dm() {
    let (_relay, relay_url) = spawn_mock_relay().await;

    let agent_keys = Keys::generate();
    let agent_adapter = Arc::new(BuzzAdapter::new(agent_keys.clone(), relay_url.clone()));
    agent_adapter.connect().await.expect("agent connect failed");
    let agent_pubkey = agent_keys.public_key();

    let operator_keys = Keys::generate();
    let operator_hex = operator_keys.public_key().to_hex();
    let operator_npub = operator_keys
        .public_key()
        .to_bech32()
        .expect("to_bech32 failed");

    let (audit_log, audit_path) = make_temp_audit_log();
    let coord = Arc::new(ApprovalCoordinator::new(
        30,
        agent_adapter.clone() as Arc<dyn PlatformAdapter>,
        make_temp_approvals_store(),
        audit_log,
    ));

    let store = build_test_session_store();
    let handler = build_test_handler(store, coord.clone());
    let handler: Arc<dyn MessageHandler> = Arc::new(handler);

    let cancel = CancellationToken::new();
    let config = buzz_config(vec![], vec![operator_hex.clone()], ChannelTrust::Closed);
    let agent_adapter_for_task = agent_adapter.clone();
    let cancel_for_task = cancel.clone();
    let handler_for_task = handler.clone();
    let _adapter_task = tokio::spawn(async move {
        let _ = run_buzz_adapter(agent_adapter_for_task, config, handler_for_task, cancel_for_task)
            .await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Operator subscribes to gift-wrapped DMs addressed to itself BEFORE the
    // approval is parked, so it observes both the prompt and (later) the
    // confirmation reply on the same stream.
    let operator_client = connected_client(&operator_keys, &relay_url).await;
    let mut operator_notifications = subscribe_notifications(&operator_client);
    operator_client
        .subscribe(buzz_dm_subscription_filter(operator_keys.public_key()))
        .await
        .expect("operator subscribe failed");
    tokio::time::sleep(Duration::from_millis(50)).await;

    let session_id = dm_session_id(&operator_npub, &operator_hex);
    let pending_marker = "pending-marker-approve-9f31";
    let coord_req = coord.clone();
    let session_id_for_task = session_id.clone();
    let request_handle = tokio::spawn(async move {
        coord_req
            .request(
                &session_id_for_task,
                &operator_npub,
                "srv__delete",
                pending_marker,
                &serde_json::json!({}),
            )
            .await
    });

    let prompt_event = wait_for_event(&mut operator_notifications, Duration::from_secs(5), |e| {
        e.kind == Kind::GiftWrap
    })
    .await
    .expect("approval prompt never arrived at the operator");
    let unwrapped_prompt = UnwrappedGift::from_gift_wrap(&operator_keys, &prompt_event)
        .expect("operator failed to unwrap the prompt");
    assert!(
        unwrapped_prompt.rumor.content.contains(pending_marker),
        "prompt must contain the pending id, got: {}",
        unwrapped_prompt.rumor.content
    );

    // Operator approves via a genuine gift-wrapped DM to the agent.
    let approve_dm = build_dm_event(&operator_keys, agent_pubkey, "/approve");
    operator_client
        .send_event(&approve_dm)
        .await
        .expect("send_event (approve) failed");

    let outcome = tokio::time::timeout(Duration::from_secs(5), request_handle)
        .await
        .expect("parked request never resolved")
        .expect("request task must not panic");
    assert_eq!(outcome, ApprovalOutcome::Approved);

    let confirmation = wait_for_event(&mut operator_notifications, Duration::from_secs(5), |e| {
        e.kind == Kind::GiftWrap && e.id != prompt_event.id && e.id != approve_dm.id
    })
    .await
    .expect("confirmation reply never arrived at the operator");
    let unwrapped_confirmation = UnwrappedGift::from_gift_wrap(&operator_keys, &confirmation)
        .expect("operator failed to unwrap the confirmation");
    assert!(
        unwrapped_confirmation.rumor.content.contains("Approved"),
        "confirmation must say Approved, got: {}",
        unwrapped_confirmation.rumor.content
    );

    // Audit: exactly one entry, surface names buzz.
    let audit_contents = std::fs::read_to_string(&audit_path).expect("read audit log");
    let lines: Vec<&str> = audit_contents.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "exactly one audit entry must be appended after a successful approval, got: {audit_contents}"
    );
    let entry: serde_json::Value = serde_json::from_str(lines[0]).expect("valid audit JSON");
    assert_eq!(entry["surface"], "buzz");
    assert_eq!(entry["decision"], "approved");

    cancel.cancel();
}

/// Mirror of the approve round trip for `/deny`.
#[tokio::test]
async fn parked_approval_denies_via_a_real_operator_deny_dm() {
    let (_relay, relay_url) = spawn_mock_relay().await;

    let agent_keys = Keys::generate();
    let agent_adapter = Arc::new(BuzzAdapter::new(agent_keys.clone(), relay_url.clone()));
    agent_adapter.connect().await.expect("agent connect failed");
    let agent_pubkey = agent_keys.public_key();

    let operator_keys = Keys::generate();
    let operator_hex = operator_keys.public_key().to_hex();
    let operator_npub = operator_keys
        .public_key()
        .to_bech32()
        .expect("to_bech32 failed");

    let (audit_log, _audit_path) = make_temp_audit_log();
    let coord = Arc::new(ApprovalCoordinator::new(
        30,
        agent_adapter.clone() as Arc<dyn PlatformAdapter>,
        make_temp_approvals_store(),
        audit_log,
    ));

    let store = build_test_session_store();
    let handler = build_test_handler(store, coord.clone());
    let handler: Arc<dyn MessageHandler> = Arc::new(handler);

    let cancel = CancellationToken::new();
    let config = buzz_config(vec![], vec![operator_hex.clone()], ChannelTrust::Closed);
    let agent_adapter_for_task = agent_adapter.clone();
    let cancel_for_task = cancel.clone();
    let handler_for_task = handler.clone();
    let _adapter_task = tokio::spawn(async move {
        let _ = run_buzz_adapter(agent_adapter_for_task, config, handler_for_task, cancel_for_task)
            .await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let operator_client = connected_client(&operator_keys, &relay_url).await;
    let mut operator_notifications = subscribe_notifications(&operator_client);
    operator_client
        .subscribe(buzz_dm_subscription_filter(operator_keys.public_key()))
        .await
        .expect("operator subscribe failed");
    tokio::time::sleep(Duration::from_millis(50)).await;

    let session_id = dm_session_id(&operator_npub, &operator_hex);
    let pending_marker = "pending-marker-deny-4a02";
    let coord_req = coord.clone();
    let session_id_for_task = session_id.clone();
    let request_handle = tokio::spawn(async move {
        coord_req
            .request(
                &session_id_for_task,
                &operator_npub,
                "srv__delete",
                pending_marker,
                &serde_json::json!({}),
            )
            .await
    });

    let prompt_event = wait_for_event(&mut operator_notifications, Duration::from_secs(5), |e| {
        e.kind == Kind::GiftWrap
    })
    .await
    .expect("approval prompt never arrived at the operator");

    let deny_dm = build_dm_event(&operator_keys, agent_pubkey, "/deny");
    operator_client
        .send_event(&deny_dm)
        .await
        .expect("send_event (deny) failed");

    let outcome = tokio::time::timeout(Duration::from_secs(5), request_handle)
        .await
        .expect("parked request never resolved")
        .expect("request task must not panic");
    assert_eq!(outcome, ApprovalOutcome::Denied);

    let confirmation = wait_for_event(&mut operator_notifications, Duration::from_secs(5), |e| {
        e.kind == Kind::GiftWrap && e.id != prompt_event.id && e.id != deny_dm.id
    })
    .await
    .expect("confirmation reply never arrived at the operator");
    let unwrapped_confirmation = UnwrappedGift::from_gift_wrap(&operator_keys, &confirmation)
        .expect("operator failed to unwrap the confirmation");
    assert!(
        unwrapped_confirmation.rumor.content.contains("Denied"),
        "confirmation must say Denied, got: {}",
        unwrapped_confirmation.rumor.content
    );

    cancel.cancel();
}

/// The negative case: an UNLISTED identity's `/approve` for the SAME pending
/// entry must leave the request unresolved — this is the only acceptable
/// outcome for an unauthorized approval attempt (T-47.6-06-01). The unlisted
/// DM never reaches the handler at all (dropped at the DM whitelist gate
/// before task 2's approval guard even runs), so the parked future has no
/// path to resolution.
#[tokio::test]
async fn unlisted_identity_approve_does_not_resolve_the_pending_request() {
    let (_relay, relay_url) = spawn_mock_relay().await;

    let agent_keys = Keys::generate();
    let agent_adapter = Arc::new(BuzzAdapter::new(agent_keys.clone(), relay_url.clone()));
    agent_adapter.connect().await.expect("agent connect failed");
    let agent_pubkey = agent_keys.public_key();

    let operator_keys = Keys::generate();
    let operator_hex = operator_keys.public_key().to_hex();
    let operator_npub = operator_keys
        .public_key()
        .to_bech32()
        .expect("to_bech32 failed");

    // Attacker identity — NOT in the whitelist below.
    let attacker_keys = Keys::generate();

    let (audit_log, _audit_path) = make_temp_audit_log();
    let coord = Arc::new(ApprovalCoordinator::new(
        30,
        agent_adapter.clone() as Arc<dyn PlatformAdapter>,
        make_temp_approvals_store(),
        audit_log,
    ));

    let store = build_test_session_store();
    let handler = build_test_handler(store, coord.clone());
    let handler: Arc<dyn MessageHandler> = Arc::new(handler);

    let cancel = CancellationToken::new();
    // Only the operator is whitelisted -- the attacker is not.
    let config = buzz_config(vec![], vec![operator_hex.clone()], ChannelTrust::Closed);
    let agent_adapter_for_task = agent_adapter.clone();
    let cancel_for_task = cancel.clone();
    let handler_for_task = handler.clone();
    let _adapter_task = tokio::spawn(async move {
        let _ = run_buzz_adapter(agent_adapter_for_task, config, handler_for_task, cancel_for_task)
            .await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let session_id = dm_session_id(&operator_npub, &operator_hex);
    let coord_req = coord.clone();
    let session_id_for_task = session_id.clone();
    let request_handle = tokio::spawn(async move {
        coord_req
            .request(
                &session_id_for_task,
                &operator_npub,
                "srv__delete",
                "pending-marker-unlisted-1",
                &serde_json::json!({}),
            )
            .await
    });
    // Let the request park (insert into the pending map, prompt sent).
    tokio::time::sleep(Duration::from_millis(200)).await;

    // The attacker sends /approve for the SAME session_id it has no way of
    // legitimately knowing, addressed to the agent.
    let attacker_client = connected_client(&attacker_keys, &relay_url).await;
    let attacker_approve = build_dm_event(&attacker_keys, agent_pubkey, "/approve");
    attacker_client
        .send_event(&attacker_approve)
        .await
        .expect("send_event failed");

    tokio::time::sleep(SETTLE_WINDOW).await;
    assert!(
        !request_handle.is_finished(),
        "an unlisted identity's /approve must NOT resolve the parked request"
    );

    request_handle.abort();
    let _ = request_handle.await;
    cancel.cancel();
}

/// The CHANNEL-ORIGIN case (D-14's outbound half): an approval parked for a
/// turn that arrived in a CHANNEL (chat_id would be the channel identifier,
/// sender_id the operator's pubkey hex) must still prompt the OPERATOR
/// privately via DM -- never the channel. Mirrors `approval_target_for`'s
/// already-unit-tested Buzz-channel derivation (handler.rs, plan 09:
/// `event.platform == Buzz && event.chat_type == "channel"` routes to
/// `event.sender_id`, a bare hex pubkey which `parse_buzz_chat_target`
/// classifies as a DM target) -- reproduced directly here rather than called,
/// since that helper is `pub(crate)` to `ironhermes-gateway` and out of this
/// plan's `handler.rs`-untouched file scope.
#[tokio::test]
async fn channel_originated_approval_prompts_dm_never_leaks_to_channel() {
    let (_relay, relay_url) = spawn_mock_relay().await;

    let agent_keys = Keys::generate();
    let agent_adapter = Arc::new(BuzzAdapter::new(agent_keys.clone(), relay_url.clone()));
    agent_adapter.connect().await.expect("agent connect failed");

    let operator_keys = Keys::generate();
    let operator_hex = operator_keys.public_key().to_hex();
    let channel_id = "ops-channel-approval-test";

    let (audit_log, _audit_path) = make_temp_audit_log();
    let coord = Arc::new(ApprovalCoordinator::new(
        30,
        agent_adapter.clone() as Arc<dyn PlatformAdapter>,
        make_temp_approvals_store(),
        audit_log,
    ));

    // Observer 1: gift-wrapped DMs addressed to the operator.
    let operator_client = connected_client(&operator_keys, &relay_url).await;
    let mut operator_notifications = subscribe_notifications(&operator_client);
    operator_client
        .subscribe(buzz_dm_subscription_filter(operator_keys.public_key()))
        .await
        .expect("operator subscribe failed");

    // Observer 2: every kind:9 event on the channel the turn originated in.
    let channel_observer_keys = Keys::generate();
    let channel_observer = connected_client(&channel_observer_keys, &relay_url).await;
    let mut channel_notifications = subscribe_notifications(&channel_observer);
    channel_observer
        .subscribe(
            Filter::new()
                .kind(Kind::Custom(9))
                .custom_tag(SingleLetterTag::LOWERCASE_H, channel_id.to_string()),
        )
        .await
        .expect("channel observer subscribe failed");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // approval_target_for's Buzz-channel branch: chat_id = event.sender_id
    // (the operator's hex pubkey), which parse_buzz_chat_target classifies as
    // a DirectMessage target (bare 64-char hex).
    let session_id = SessionKey::new(Platform::Buzz, channel_id)
        .with_user(&operator_hex)
        .to_string_key();
    let pending_marker = "pending-marker-channel-origin-77c1";
    let coord_req = coord.clone();
    let session_id_for_task = session_id.clone();
    let operator_hex_for_task = operator_hex.clone();
    let request_handle = tokio::spawn(async move {
        coord_req
            .request(
                &session_id_for_task,
                &operator_hex_for_task,
                "srv__delete",
                pending_marker,
                &serde_json::json!({}),
            )
            .await
    });

    let prompt_event = wait_for_event(&mut operator_notifications, Duration::from_secs(5), |e| {
        e.kind == Kind::GiftWrap
    })
    .await
    .expect("approval prompt never arrived at the operator's DM");
    let unwrapped_prompt = UnwrappedGift::from_gift_wrap(&operator_keys, &prompt_event)
        .expect("operator failed to unwrap the prompt");
    assert!(
        unwrapped_prompt.rumor.content.contains(pending_marker),
        "DM prompt must contain the pending id, got: {}",
        unwrapped_prompt.rumor.content
    );

    // The negative half that actually matters: no kind:9 channel event
    // carrying the prompt text or pending id was ever published to the
    // channel the turn originated in.
    assert_no_event(&mut channel_notifications, SETTLE_WINDOW, |e| {
        e.content.contains(pending_marker) || e.content.contains("Approval required")
    })
    .await;

    // Clean up: resolve (denied, arbitrary) so the spawned task exits, then
    // abort defensively in case resolve() raced the timeout window.
    coord.resolve(&session_id, false).await;
    request_handle.abort();
    let _ = request_handle.await;
}

// ---------------------------------------------------------------------------
// CR-01 regression (47.6 code review) — the missing combination: a
// channel-originated approval (outbound half, mirroring
// `channel_originated_approval_prompts_dm_never_leaks_to_channel` above)
// resolved by a REAL inbound operator `/approve` DM dispatched through the
// REAL `GatewayMessageHandler` (inbound half, mirroring
// `parked_approval_resolves_via_a_real_operator_approve_dm` above). Before
// the CR-01 fix, `coord.resolve(&ctx_session_id, ...)` always missed here
// (the DM reply's SessionKey never matches the channel turn's SessionKey)
// and the parked request only ever timed out / auto-denied — this test
// pins the fixed behavior: it must resolve `Approved`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn channel_originated_approval_resolves_via_a_real_operator_approve_dm() {
    let (_relay, relay_url) = spawn_mock_relay().await;

    let agent_keys = Keys::generate();
    let agent_adapter = Arc::new(BuzzAdapter::new(agent_keys.clone(), relay_url.clone()));
    agent_adapter.connect().await.expect("agent connect failed");
    let agent_pubkey = agent_keys.public_key();

    let operator_keys = Keys::generate();
    let operator_hex = operator_keys.public_key().to_hex();
    let channel_id = "ops-channel-cr01-e2e";

    let (audit_log, _audit_path) = make_temp_audit_log();
    let coord = Arc::new(ApprovalCoordinator::new(
        30,
        agent_adapter.clone() as Arc<dyn PlatformAdapter>,
        make_temp_approvals_store(),
        audit_log,
    ));

    // The agent side runs the REAL GatewayMessageHandler with this SAME
    // coordinator installed — its /approve intercept is exactly the CR-01
    // fixed code path (handler.rs's resolve() -> resolve_by_chat_id()
    // fallback) under test here.
    let store = build_test_session_store();
    let handler = build_test_handler(store, coord.clone());
    let handler: Arc<dyn MessageHandler> = Arc::new(handler);

    let cancel = CancellationToken::new();
    let config = buzz_config(vec![channel_id.to_string()], vec![operator_hex.clone()], ChannelTrust::Closed);
    let agent_adapter_for_task = agent_adapter.clone();
    let cancel_for_task = cancel.clone();
    let handler_for_task = handler.clone();
    let _adapter_task = tokio::spawn(async move {
        let _ = run_buzz_adapter(agent_adapter_for_task, config, handler_for_task, cancel_for_task)
            .await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Operator subscribes to gift-wrapped DMs addressed to itself BEFORE the
    // approval is parked, so it observes both the prompt and (later) the
    // confirmation reply on the same stream.
    let operator_client = connected_client(&operator_keys, &relay_url).await;
    let mut operator_notifications = subscribe_notifications(&operator_client);
    operator_client
        .subscribe(buzz_dm_subscription_filter(operator_keys.public_key()))
        .await
        .expect("operator subscribe failed");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Outbound half: park the approval EXACTLY as a real channel-originated
    // turn would (session_id keyed on the CHANNEL's SessionKey, chat_id =
    // approval_target_for's Buzz-channel branch = the operator's hex
    // pubkey) — the same derivation
    // `channel_originated_approval_prompts_dm_never_leaks_to_channel`
    // pins above.
    let session_id = SessionKey::new(Platform::Buzz, channel_id)
        .with_user(&operator_hex)
        .to_string_key();
    let pending_marker = "pending-marker-cr01-e2e-4b21";
    let coord_req = coord.clone();
    let session_id_for_task = session_id.clone();
    let operator_hex_for_task = operator_hex.clone();
    let request_handle = tokio::spawn(async move {
        coord_req
            .request(
                &session_id_for_task,
                &operator_hex_for_task,
                "srv__delete",
                pending_marker,
                &serde_json::json!({}),
            )
            .await
    });

    let prompt_event = wait_for_event(&mut operator_notifications, Duration::from_secs(5), |e| {
        e.kind == Kind::GiftWrap
    })
    .await
    .expect("approval prompt never arrived at the operator's DM");
    let unwrapped_prompt = UnwrappedGift::from_gift_wrap(&operator_keys, &prompt_event)
        .expect("operator failed to unwrap the prompt");
    assert!(
        unwrapped_prompt.rumor.content.contains(pending_marker),
        "DM prompt must contain the pending id, got: {}",
        unwrapped_prompt.rumor.content
    );

    // Inbound half: a REAL gift-wrapped `/approve` DM from the operator,
    // received by the REAL `run_buzz_adapter` loop and dispatched to the
    // REAL `GatewayMessageHandler::handle_slash_command`'s /approve
    // intercept — exercising the actual CR-01 fix, not a direct
    // `coord.resolve(...)` call.
    let approve_dm = build_dm_event(&operator_keys, agent_pubkey, "/approve");
    operator_client
        .send_event(&approve_dm)
        .await
        .expect("send_event (approve) failed");

    let outcome = tokio::time::timeout(Duration::from_secs(5), request_handle)
        .await
        .expect(
            "parked channel-originated approval never resolved — CR-01 regression: the \
             operator's real /approve DM failed to resolve a channel-originated pending \
             approval",
        )
        .expect("request task must not panic");
    assert_eq!(
        outcome,
        ApprovalOutcome::Approved,
        "channel-originated approval must resolve Approved via the operator's real DM reply"
    );

    let confirmation = wait_for_event(&mut operator_notifications, Duration::from_secs(5), |e| {
        e.kind == Kind::GiftWrap && e.id != prompt_event.id && e.id != approve_dm.id
    })
    .await
    .expect("confirmation reply never arrived at the operator");
    let unwrapped_confirmation = UnwrappedGift::from_gift_wrap(&operator_keys, &confirmation)
        .expect("operator failed to unwrap the confirmation");
    assert!(
        unwrapped_confirmation.rumor.content.contains("Approved"),
        "confirmation must say Approved, got: {}",
        unwrapped_confirmation.rumor.content
    );

    cancel.cancel();
}

#![cfg(feature = "buzz")]
//! Phase 47.6 Plan 09 — the first automated test that drives the REAL
//! `GatewayMessageHandler` through a real `AgentRuntime::run_turn` on the
//! Buzz path, plus the platform-identity and media-fallback behaviors this
//! plan adds.
//!
//! # Why a real `AgentRuntime`, and how it stays test-only
//!
//! Every Buzz test before this plan (plan 01, plan 05) used a stub
//! `MessageHandler` that echoes — which is exactly why the placeholder bug
//! (D-13, T-47.6-09-04) survived a green suite. `telegram_media_delivery.rs`
//! (Phase 36.17.2.2 Plan 06) hit the same wall previously and documented why
//! it chose to hand-drive the production composition instead of injecting a
//! fake `AgentRuntime`: `AnyClient` is a closed enum
//! (`ChatCompletions`/`AnthropicMessages`/`CodexResponses`), not a trait, so
//! there is no seam to substitute a fake streaming provider without either
//! growing `AnyClient` a `Fake` variant or building an invasive delta-
//! injection point on `AgentRuntime`.
//!
//! This plan's headline test needs something stronger than that precedent
//! allows: proof that the REAL `GatewayMessageHandler` (session key
//! construction, hook firing, `StreamConsumer` mode selection, media
//! dispatch) produces a real relay event containing the agent's real
//! response — a stub handler cannot prove that. The seam added here is
//! narrow and test-only in spirit: `AgentRuntime::for_tests_with_base_url`
//! (new in this plan, gated `#[cfg(any(test, feature = "test-support"))]`
//! exactly like the pre-existing `AgentRuntime::for_tests()`) lets the
//! runtime's `AnyClient::ChatCompletions` point at a `wiremock::MockServer`
//! instead of the unreachable `localhost:0` `for_tests()` uses. This drives
//! the REAL `AgentLoop`, REAL tool registry, and REAL hook registry against
//! a canned HTTP response — the network is faked, not the production code.

use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use ironhermes_agent::AgentRuntime;
use ironhermes_core::{Config, MessageEvent, MessageResponse, Platform, ProviderResolver};
use ironhermes_gateway::adapter::{MessageHandler, PlatformAdapter};
use ironhermes_gateway::buzz::BuzzAdapter;
use ironhermes_gateway::handler::GatewayMessageHandler;
use ironhermes_gateway::session::{SessionKey, SessionStore};
use ironhermes_hooks::{HookEvent, HookEventKind, HookRegistry, HooksConfig};
use ironhermes_tools::ToolRegistry;
use nostr_sdk::prelude::*;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

// ===========================================================================
// local_relay — minimal in-process NIP-01 relay, copied from
// buzz_relay_roundtrip.rs (plan 01). Each integration-test binary is its own
// crate, so this harness is duplicated per-file per this codebase's existing
// convention (RecordingAdapter is likewise redefined in several test files).
// See buzz_relay_roundtrip.rs's own module doc for why this exists instead
// of `nostr-relay-builder`.
// ===========================================================================

mod local_relay {
    use futures::{SinkExt, StreamExt};
    use nostr_sdk::prelude::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::{Notify, broadcast};
    use tokio_tungstenite::tungstenite::Message as WsMessage;

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
            let addr = listener
                .local_addr()
                .expect("LocalRelay: local_addr failed");
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

async fn spawn_mock_relay() -> (local_relay::LocalRelay, String) {
    let relay = local_relay::LocalRelay::start().await;
    let url = relay.url.clone();
    (relay, url)
}

async fn connected_client(keys: &Keys, relay_url: &str) -> Client {
    let authenticator = SignerAuthenticator::new(keys.clone());
    let client = Client::builder().authenticator(authenticator).build();
    client.add_relay(relay_url).await.expect("add_relay failed");
    client.connect().await;
    client
}

type Notifications = std::pin::Pin<Box<dyn futures::Stream<Item = ClientNotification> + Send>>;

fn subscribe_notifications(client: &Client) -> Notifications {
    client.notifications()
}

async fn wait_for_event<F>(
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

// ===========================================================================
// Wiremock SSE fixture — a fixed chat-completions response, no tool calls.
// ===========================================================================

const FIXED_RESPONSE: &str = "The real agent answer, not a placeholder.";
/// A substring of [`FIXED_RESPONSE`] with no MarkdownV2-reserved characters
/// (notably no trailing `.`), so assertions against Telegram's
/// `escape_outside_code_blocks`-escaped edit body don't have to account for
/// the escape transform.
const FIXED_RESPONSE_UNESCAPED_SUBSTRING: &str = "The real agent answer, not a placeholder";

fn sse_fixed_response(text: &str) -> String {
    let chunks = [
        format!(
            r#"{{"id":"c1","object":"chat.completion.chunk","created":1,"model":"test-model","choices":[{{"index":0,"delta":{{"content":{}}}}}]}}"#,
            serde_json::to_string(text).unwrap()
        ),
        r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"test-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#.to_string(),
    ];
    let mut body = String::new();
    for c in chunks {
        body.push_str("data: ");
        body.push_str(&c);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

async fn mount_fixed_response(server: &MockServer, text: &str) {
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse_fixed_response(text), "text/event-stream"),
        )
        .mount(server)
        .await;
}

// ===========================================================================
// Handler construction harness
// ===========================================================================

fn build_test_session_store() -> Arc<RwLock<SessionStore>> {
    let state_store = Arc::new(std::sync::Mutex::new(
        ironhermes_state::StateStore::new(":memory:").expect("in-memory StateStore"),
    ));
    Arc::new(RwLock::new(SessionStore::new(state_store)))
}

fn build_test_handler(store: Arc<RwLock<SessionStore>>) -> GatewayMessageHandler {
    let config = Config::default();
    let resolver = ProviderResolver::build(&config).unwrap();
    let tool_registry = Arc::new(RwLock::new(ToolRegistry::new()));
    GatewayMessageHandler::new(config, resolver, store, tool_registry)
}

/// Build a `HookRegistry` that records every fired `HookEventKind` into a
/// shared, externally-inspectable `Vec`.
fn build_recording_hook_registry() -> (Arc<HookRegistry>, Arc<StdMutex<Vec<HookEventKind>>>) {
    let captured = Arc::new(StdMutex::new(Vec::new()));
    let captured_cb = captured.clone();
    let mut registry = HookRegistry::new(HooksConfig::default());
    registry.add_listener(Arc::new(move |event: HookEvent| {
        captured_cb.lock().unwrap().push(event.kind);
    }));
    (Arc::new(registry), captured)
}

/// `HookRegistry::fire` dispatches every listener via `tokio::spawn` — fire-
/// and-forget, NOT synchronous (see `ironhermes_hooks::registry::HookRegistry::fire`).
/// A `ResponseSent` hook fired at the very end of `run_agent` may not have
/// run its spawned listener task yet by the time `handler.handle(...).await`
/// returns. Poll with a bounded timeout instead of asserting immediately.
async fn wait_for_hook_count(
    captured: &Arc<StdMutex<Vec<HookEventKind>>>,
    expected_at_least: usize,
    timeout: Duration,
) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if captured.lock().unwrap().len() >= expected_at_least {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            return; // let the caller's own assertion report the shortfall
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn buzz_channel_event(chat_id: &str, sender_id: &str, content: &str) -> MessageEvent {
    MessageEvent {
        platform: Platform::Buzz,
        // Phase 47.6 Plan 08 (T-47.6-08-REPLY): must be a syntactically
        // valid 64-char hex event id, NOT the pre-fix placeholder
        // "buzz-msg-1" — `GatewayMessageHandler` now threads a Buzz CHANNEL
        // turn's `message_id` through to `BuzzAdapter::send_message` as the
        // reply-target `thread_id`, which calls `EventId::from_hex` on it.
        // A non-hex placeholder here made every real-BuzzAdapter test in
        // this file silently fail to publish (logged, not propagated) the
        // moment that plumbing landed.
        message_id: "b1".repeat(32),
        chat_id: chat_id.to_string(),
        sender_id: sender_id.to_string(),
        content: content.to_string(),
        attachments: Vec::new(),
        thread_id: None,
        chat_type: "channel".to_string(),
        chat_name: None,
        sender_name: None,
        replied_to_id: None,
    }
}

fn telegram_dm_event(chat_id: &str, sender_id: &str, content: &str) -> MessageEvent {
    MessageEvent {
        platform: Platform::Telegram,
        message_id: "tg-msg-1".to_string(),
        chat_id: chat_id.to_string(),
        sender_id: sender_id.to_string(),
        content: content.to_string(),
        attachments: Vec::new(),
        thread_id: None,
        chat_type: "dm".to_string(),
        chat_name: None,
        sender_name: None,
        replied_to_id: None,
    }
}

// ===========================================================================
// RecordingAdapter — Telegram-shaped adapter recording call sequence
// (edit-in-place path). Standalone per-file definition, matching the
// existing multi-file duplication convention (concurrent_turns_gateway.rs,
// telegram_media_delivery.rs each define their own).
// ===========================================================================

#[derive(Debug, Clone)]
#[allow(dead_code)] // fields read via {calls:?} Debug formatting in assertion failure messages
enum Call {
    Send { content: String },
    EditPlain { message_id: String, content: String },
    EditMarkdownV2 { message_id: String, content: String },
    Delete { message_id: String },
}

struct RecordingAdapter {
    calls: StdMutex<Vec<Call>>,
    next_id: StdMutex<u64>,
}

impl RecordingAdapter {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: StdMutex::new(Vec::new()),
            next_id: StdMutex::new(1),
        })
    }

    fn alloc_id(&self) -> String {
        let mut n = self.next_id.lock().unwrap();
        let id = *n;
        *n += 1;
        format!("tg-{id}")
    }

    fn calls_snapshot(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl PlatformAdapter for RecordingAdapter {
    fn platform(&self) -> Platform {
        Platform::Telegram
    }

    async fn send_message(
        &self,
        chat_id: &str,
        content: &str,
        _thread_id: Option<&str>,
    ) -> anyhow::Result<MessageResponse> {
        let id = self.alloc_id();
        self.calls.lock().unwrap().push(Call::Send {
            content: content.to_string(),
        });
        Ok(MessageResponse {
            message_id: id,
            chat_id: chat_id.to_string(),
            platform: Platform::Telegram,
        })
    }

    async fn send_message_markdown_v2(
        &self,
        chat_id: &str,
        content: &str,
        thread_id: Option<&str>,
    ) -> anyhow::Result<MessageResponse> {
        self.send_message(chat_id, content, thread_id).await
    }

    async fn edit_message(
        &self,
        _chat_id: &str,
        message_id: &str,
        content: &str,
    ) -> anyhow::Result<()> {
        self.calls.lock().unwrap().push(Call::EditPlain {
            message_id: message_id.to_string(),
            content: content.to_string(),
        });
        Ok(())
    }

    async fn edit_message_markdown_v2(
        &self,
        _chat_id: &str,
        message_id: &str,
        content: &str,
    ) -> anyhow::Result<()> {
        self.calls.lock().unwrap().push(Call::EditMarkdownV2 {
            message_id: message_id.to_string(),
            content: content.to_string(),
        });
        Ok(())
    }

    async fn delete_message(&self, _chat_id: &str, message_id: &str) -> anyhow::Result<()> {
        self.calls.lock().unwrap().push(Call::Delete {
            message_id: message_id.to_string(),
        });
        Ok(())
    }

    fn is_running(&self) -> bool {
        true
    }
}

// ===========================================================================
// Tests — Task 3 headline: real handler, real AgentRuntime, Buzz path
// ===========================================================================

/// The headline test: a real `GatewayMessageHandler`, driven by a real
/// `AgentRuntime` (network faked via wiremock, nothing else), fed a Buzz
/// `MessageEvent`, publishes an event to the relay authored by the agent's
/// own identity whose content contains the fixed response — and never
/// publishes an event whose entire body is the placeholder block character.
#[tokio::test]
async fn real_handler_buzz_turn_publishes_the_response() {
    let (_relay, relay_url) = spawn_mock_relay().await;

    let agent_keys = Keys::generate();
    let buzz_adapter = Arc::new(BuzzAdapter::new(agent_keys.clone(), relay_url.clone()));
    buzz_adapter.connect().await.expect("BuzzAdapter connect");
    let adapter: Arc<dyn PlatformAdapter> = buzz_adapter.clone();

    // Independent observer client subscribed to every Kind::Custom(9) event —
    // this is what a real relay subscriber (another Buzz participant) sees.
    let observer_keys = Keys::generate();
    let observer = connected_client(&observer_keys, &relay_url).await;
    observer
        .subscribe(Filter::new().kind(Kind::Custom(9)))
        .await
        .expect("observer subscribe failed");
    let mut notifications = subscribe_notifications(&observer);

    let server = MockServer::start().await;
    mount_fixed_response(&server, FIXED_RESPONSE).await;
    let runtime = Arc::new(AgentRuntime::for_tests_with_base_url(server.uri()));

    let store = build_test_session_store();
    let mut handler = build_test_handler(store.clone());
    handler.set_agent_runtime(runtime);

    let event = buzz_channel_event("buzz-channel-1", "sender-hex-abc", "hello agent");
    let cancel = CancellationToken::new();
    handler
        .handle(&event, adapter, cancel)
        .await
        .expect("handle failed");

    let received = wait_for_event(&mut notifications, Duration::from_secs(10), |e| {
        e.content.contains(FIXED_RESPONSE)
    })
    .await
    .expect("timed out waiting for the agent's real response to publish");

    assert_eq!(
        received.pubkey,
        agent_keys.public_key(),
        "the published event must be authored by the agent's own identity"
    );
    assert_ne!(
        received.content, "\u{2588}",
        "the published event must never be the bare placeholder block character"
    );
}

/// The turn's session must be retrievable under a Buzz-platform session key
/// — and NOT under the equivalent Telegram key for the same chat_id (T-47.6-09-02).
#[tokio::test]
async fn real_handler_buzz_turn_stores_a_buzz_session() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let agent_keys = Keys::generate();
    let buzz_adapter = Arc::new(BuzzAdapter::new(agent_keys.clone(), relay_url.clone()));
    buzz_adapter.connect().await.expect("BuzzAdapter connect");
    let adapter: Arc<dyn PlatformAdapter> = buzz_adapter;

    let server = MockServer::start().await;
    mount_fixed_response(&server, FIXED_RESPONSE).await;
    let runtime = Arc::new(AgentRuntime::for_tests_with_base_url(server.uri()));

    let store = build_test_session_store();
    let mut handler = build_test_handler(store.clone());
    handler.set_agent_runtime(runtime);

    let chat_id = "buzz-channel-session-test";
    let event = buzz_channel_event(chat_id, "sender-hex-def", "hello");
    let cancel = CancellationToken::new();
    handler
        .handle(&event, adapter, cancel)
        .await
        .expect("handle failed");

    let store = store.read().await;
    let buzz_key = SessionKey::new(Platform::Buzz, chat_id).with_user("sender-hex-def");
    let telegram_key = SessionKey::new(Platform::Telegram, chat_id).with_user("sender-hex-def");

    assert!(
        store.get(&buzz_key).is_some(),
        "the turn's session must be retrievable under a Buzz-platform session key"
    );
    assert!(
        store.get(&telegram_key).is_none(),
        "a Buzz conversation must never land in Telegram's session namespace"
    );
}

/// The equivalent Telegram turn is unchanged: placeholder send first, then a
/// final MarkdownV2 edit finalizing the placeholder with the real response
/// (T-47.6-09-07 regression fence).
#[tokio::test]
async fn real_handler_telegram_turn_is_unchanged() {
    let adapter = RecordingAdapter::new();
    let adapter_dyn: Arc<dyn PlatformAdapter> = adapter.clone();

    let server = MockServer::start().await;
    mount_fixed_response(&server, FIXED_RESPONSE).await;
    let runtime = Arc::new(AgentRuntime::for_tests_with_base_url(server.uri()));

    let store = build_test_session_store();
    let mut handler = build_test_handler(store);
    handler.set_agent_runtime(runtime);

    let event = telegram_dm_event("tg-chat-1", "tg-user-1", "hello");
    let cancel = CancellationToken::new();
    handler
        .handle(&event, adapter_dyn, cancel)
        .await
        .expect("handle failed");

    let calls = adapter.calls_snapshot();
    assert!(
        matches!(calls.first(), Some(Call::Send { content }) if content == "\u{2588}"),
        "Telegram turn must still send the placeholder block character first, got {calls:?}"
    );
    assert!(
        calls.iter().any(|c| matches!(
            c,
            Call::EditMarkdownV2 { content, .. } if content.contains(FIXED_RESPONSE_UNESCAPED_SUBSTRING)
        )),
        "Telegram turn must still finalize via a MarkdownV2 edit containing the response, got {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| matches!(c, Call::Delete { .. })),
        "a successful turn with real response text must not delete the placeholder, got {calls:?}"
    );
}

// ===========================================================================
// Tests — Task 1: platform-driven identity, observed via hook payloads
// ===========================================================================

/// A Buzz turn's `MessageReceived` AND `ResponseSent` hooks both report the
/// Buzz platform; the equivalent Telegram turn's hooks both report Telegram
/// (T-47.6-09-03 / telegram_turn_identity_is_unchanged).
#[tokio::test]
async fn hooks_report_the_event_own_platform_for_buzz_and_telegram() {
    // ---- Buzz turn ----
    {
        let (_relay, relay_url) = spawn_mock_relay().await;
        let agent_keys = Keys::generate();
        let buzz_adapter = Arc::new(BuzzAdapter::new(agent_keys, relay_url));
        buzz_adapter.connect().await.expect("connect");
        let adapter: Arc<dyn PlatformAdapter> = buzz_adapter;

        let server = MockServer::start().await;
        mount_fixed_response(&server, FIXED_RESPONSE).await;
        let runtime = Arc::new(AgentRuntime::for_tests_with_base_url(server.uri()));

        let store = build_test_session_store();
        let mut handler = build_test_handler(store);
        handler.set_agent_runtime(runtime);
        let (hook_registry, captured) = build_recording_hook_registry();
        handler.set_hook_registry(hook_registry);

        let event = buzz_channel_event("buzz-hook-chat", "sender-hook-1", "hi");
        handler
            .handle(&event, adapter, CancellationToken::new())
            .await
            .expect("handle failed");
        // HookRegistry::fire is fire-and-forget (tokio::spawn per listener) —
        // poll for both hooks instead of asserting immediately.
        wait_for_hook_count(&captured, 2, Duration::from_secs(2)).await;

        let events = captured.lock().unwrap().clone();
        let received_platform = events.iter().find_map(|k| match k {
            HookEventKind::MessageReceived { platform, .. } => Some(platform.clone()),
            _ => None,
        });
        let sent_platform = events.iter().find_map(|k| match k {
            HookEventKind::ResponseSent { platform, .. } => Some(platform.clone()),
            _ => None,
        });
        assert_eq!(
            received_platform.as_deref(),
            Some("buzz"),
            "MessageReceived hook must report the buzz platform, got {events:?}"
        );
        assert_eq!(
            sent_platform.as_deref(),
            Some("buzz"),
            "ResponseSent hook must report the buzz platform, got {events:?}"
        );
    }

    // ---- Telegram turn ----
    {
        let adapter = RecordingAdapter::new();
        let adapter_dyn: Arc<dyn PlatformAdapter> = adapter;

        let server = MockServer::start().await;
        mount_fixed_response(&server, FIXED_RESPONSE).await;
        let runtime = Arc::new(AgentRuntime::for_tests_with_base_url(server.uri()));

        let store = build_test_session_store();
        let mut handler = build_test_handler(store);
        handler.set_agent_runtime(runtime);
        let (hook_registry, captured) = build_recording_hook_registry();
        handler.set_hook_registry(hook_registry);

        let event = telegram_dm_event("tg-hook-chat", "tg-hook-user", "hi");
        handler
            .handle(&event, adapter_dyn, CancellationToken::new())
            .await
            .expect("handle failed");
        wait_for_hook_count(&captured, 2, Duration::from_secs(2)).await;

        let events = captured.lock().unwrap().clone();
        let received_platform = events.iter().find_map(|k| match k {
            HookEventKind::MessageReceived { platform, .. } => Some(platform.clone()),
            _ => None,
        });
        let sent_platform = events.iter().find_map(|k| match k {
            HookEventKind::ResponseSent { platform, .. } => Some(platform.clone()),
            _ => None,
        });
        assert_eq!(
            received_platform.as_deref(),
            Some("telegram"),
            "MessageReceived hook must still report telegram for a Telegram turn, got {events:?}"
        );
        assert_eq!(
            sent_platform.as_deref(),
            Some("telegram"),
            "ResponseSent hook must still report telegram for a Telegram turn, got {events:?}"
        );
    }
}

// ===========================================================================
// Tests — Task 2: send-once delivery mode, end-to-end over the real handler
// ===========================================================================

/// End-to-end (mirrors the unit-level stream_consumer.rs assertions, but
/// through the real handler + real relay): a Buzz turn publishes the agent's
/// response and never publishes a placeholder-only event.
#[tokio::test]
async fn buzz_turn_publishes_the_response_not_a_placeholder() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let agent_keys = Keys::generate();
    let buzz_adapter = Arc::new(BuzzAdapter::new(agent_keys.clone(), relay_url.clone()));
    buzz_adapter.connect().await.expect("connect");
    let adapter: Arc<dyn PlatformAdapter> = buzz_adapter;

    let observer_keys = Keys::generate();
    let observer = connected_client(&observer_keys, &relay_url).await;
    observer
        .subscribe(Filter::new().kind(Kind::Custom(9)))
        .await
        .expect("observer subscribe failed");
    let mut notifications = subscribe_notifications(&observer);

    let server = MockServer::start().await;
    mount_fixed_response(&server, FIXED_RESPONSE).await;
    let runtime = Arc::new(AgentRuntime::for_tests_with_base_url(server.uri()));

    let store = build_test_session_store();
    let mut handler = build_test_handler(store);
    handler.set_agent_runtime(runtime);

    let event = buzz_channel_event("buzz-sendonce-chat", "sender-hex-so", "hello");
    handler
        .handle(&event, adapter, CancellationToken::new())
        .await
        .expect("handle failed");

    let received = wait_for_event(&mut notifications, Duration::from_secs(10), |_| true)
        .await
        .expect("no event published at all");
    assert!(
        received.content.contains(FIXED_RESPONSE),
        "the published event must contain the real response, got: {}",
        received.content
    );
    assert_ne!(
        received.content, "\u{2588}",
        "no event whose entire body is the placeholder character may be published"
    );
}

/// Telegram still sends a placeholder before streaming (regression fence for
/// the mode branch never routing Telegram down the send-once path).
#[tokio::test]
async fn telegram_turn_still_publishes_a_placeholder_first() {
    let adapter = RecordingAdapter::new();
    let adapter_dyn: Arc<dyn PlatformAdapter> = adapter.clone();

    let server = MockServer::start().await;
    mount_fixed_response(&server, FIXED_RESPONSE).await;
    let runtime = Arc::new(AgentRuntime::for_tests_with_base_url(server.uri()));

    let store = build_test_session_store();
    let mut handler = build_test_handler(store);
    handler.set_agent_runtime(runtime);

    let event = telegram_dm_event("tg-sendonce-chat", "tg-user-so", "hello");
    handler
        .handle(&event, adapter_dyn, CancellationToken::new())
        .await
        .expect("handle failed");

    let calls = adapter.calls_snapshot();
    assert!(
        matches!(calls.first(), Some(Call::Send { content }) if content == "\u{2588}"),
        "Telegram must still send the placeholder before streaming, got {calls:?}"
    );
}

// ===========================================================================
// Tests — Task 3: media fallback notice, end-to-end over the real handler
// ===========================================================================

/// A turn whose response carries a media tag, on an adapter with no media
/// sender (Buzz has none — D-15/D-18), publishes a text notice naming the
/// artifact's local path, following the response body.
#[tokio::test]
async fn buzz_turn_with_media_tag_publishes_a_fallback_notice() {
    let (_relay, relay_url) = spawn_mock_relay().await;
    let agent_keys = Keys::generate();
    let buzz_adapter = Arc::new(BuzzAdapter::new(agent_keys.clone(), relay_url.clone()));
    buzz_adapter.connect().await.expect("connect");
    let adapter: Arc<dyn PlatformAdapter> = buzz_adapter;

    let observer_keys = Keys::generate();
    let observer = connected_client(&observer_keys, &relay_url).await;
    observer
        .subscribe(Filter::new().kind(Kind::Custom(9)))
        .await
        .expect("observer subscribe failed");
    let mut notifications = subscribe_notifications(&observer);

    let media_path = "/tmp/plan09-buzz-media-fallback-test.png";
    let response_with_media = format!("Here you go: <MEDIA: {media_path}>");

    let server = MockServer::start().await;
    mount_fixed_response(&server, &response_with_media).await;
    let runtime = Arc::new(AgentRuntime::for_tests_with_base_url(server.uri()));

    let store = build_test_session_store();
    let mut handler = build_test_handler(store);
    handler.set_agent_runtime(runtime);

    let event = buzz_channel_event("buzz-media-chat", "sender-hex-media", "make me an image");
    handler
        .handle(&event, adapter, CancellationToken::new())
        .await
        .expect("handle failed");

    let notice = wait_for_event(&mut notifications, Duration::from_secs(10), |e| {
        e.content.contains(media_path)
    })
    .await
    .expect("timed out waiting for the media fallback notice to publish");

    assert!(
        notice.content.contains(media_path),
        "the fallback notice must name the artifact's local path: {}",
        notice.content
    );
}

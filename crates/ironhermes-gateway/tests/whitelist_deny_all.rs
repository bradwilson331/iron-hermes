//! D-08 gap 1 (Phase 49.1 Plan 05): six independent, live, automated
//! deny-all proofs — one per inbound adapter arm — plus a positive control.
//!
//! CONTEXT.md D-08 names the claim under test: one shared
//! `PlatformGatewayConfig.whitelist` field (`ironhermes-core/src/config.rs:3122`)
//! gates Telegram, Discord, Slack and Buzz, with the field's own doc comment
//! asserting "one shared field gates every platform's inbound access". That
//! blanket claim is what this file tests — each adapter gets its OWN test
//! function, its own config/whitelist, and (where feasible) its own
//! handler, so a passing suite here cannot be explained by one adapter's
//! correctness standing in for another's.
//!
//! ## Extraction rationale (read this before extending any arm)
//!
//! Buzz has a live, in-process-relay analog already
//! (`tests/buzz_approvals.rs::approval_command_with_empty_whitelist_is_rejected`)
//! — its whitelist check runs inside `run_buzz_adapter`, which already takes
//! `handler: Arc<dyn MessageHandler>` as a parameter, so a test-only
//! `RecordingHandler` substitutes cleanly for the real
//! `GatewayMessageHandler` with zero production-code changes. The three
//! Buzz arms below (DM, channel-closed, channel-open) and the positive
//! control reuse that exact shape, duplicated in-file per this crate's own
//! convention (`buzz_approvals.rs`'s own header: "cargo integration test
//! files each compile as an independent binary, so the harness is
//! duplicated here rather than imported").
//!
//! Telegram, Discord and Slack have **no live-network mock in this
//! repository** (confirmed in 49.1-PATTERNS.md's "No Analog Found" table)
//! and their whitelist checks are NOT parameterized over a swappable
//! `Arc<dyn MessageHandler>` the way Buzz's is — the concrete
//! `Arc<GatewayMessageHandler>`/`handle_with_multimodal` call sits
//! downstream of the check and pulls in the full agent loop (rate limiter,
//! turn registry, LLM calls), which is far too heavy to stand up for a
//! deny-all proof and, per D-16, must never be exercised this invasively.
//!
//! Per this plan's own `<action>` guidance, the middle path taken for all
//! three is: **extract the boolean deny-all decision itself into a named
//! `pub` function that the production callback calls as its early-return
//! guard**, then test that exact function directly:
//!
//! - `discord_whitelist_allows` (`src/discord.rs`) — `EventHandler::message`
//!   now calls it before ever constructing a `DiscordAdapter` or touching
//!   `self.handler`.
//! - `telegram_whitelist_allows` (`src/telegram.rs`) — `runner.rs`'s
//!   Telegram dispatch loop calls it before the group-mention check and
//!   before any per-chat worker dispatch.
//! - `slack_whitelist_allows` (`src/slack.rs`) — `on_push_event` calls it
//!   before ever cloning `state.handler`/`state.adapter` or spawning the
//!   handler task.
//!
//! This is an EXTRACTION of the real code, not a parallel reimplementation:
//! in all three cases the production callback's early-return guard IS this
//! exact function, so a `false` result is dispositive proof that the
//! handler is never reached for that call — there is no code path from a
//! denied gate to a dispatched handler. The plan's own text calls this out
//! explicitly as the acceptable middle path when the real entry point is
//! "buried inside a closure or a `serenity::async_trait` impl and is not
//! callable" (Discord) or has no live-network harness at all (Telegram,
//! Slack) — see 49.1-PATTERNS.md §1 "No Analog Found".
//!
//! **Visibility note (deviation from the plan's literal `pub(crate)`
//! wording):** the three extracted functions are declared `pub fn`, not
//! `pub(crate) fn`. `cargo` integration tests under `tests/` compile as a
//! separate crate against the library's public API — a `pub(crate)` item is
//! invisible to them. This is the same visibility every other adapter-arm
//! helper this file drives (`is_approval_command`,
//! `buzz_dm_subscription_filter`, `run_buzz_adapter`) already uses.
//!
//! All three extractions needed a production-code change: Discord, Telegram
//! and Slack. Buzz needed none (already parameterized over the trait).
//!
//! ## Running this file
//!
//! The Buzz arms are behind the `buzz` cargo feature (matching
//! `buzz_approvals.rs`). The plain
//! `cargo nextest run -p ironhermes-gateway --test whitelist_deny_all`
//! command from this plan's own `<verify>` block therefore only builds and
//! runs the Telegram/Discord/Slack arms (3 of 7) unless `--features buzz`
//! is also passed — run:
//!
//! ```text
//! cargo nextest run -p ironhermes-gateway --test whitelist_deny_all --features buzz --no-fail-fast
//! ```
//!
//! for all 7. This mirrors how `buzz_approvals.rs` itself must already be
//! invoked; the feature gate is a pre-existing crate convention, not
//! something introduced by this plan.

#![allow(clippy::too_many_arguments)]

use ironhermes_gateway::discord::discord_whitelist_allows;
use ironhermes_gateway::slack::slack_whitelist_allows;
use ironhermes_gateway::telegram::telegram_whitelist_allows;

// ===========================================================================
// Tests 4-6: Telegram / Discord / Slack — the extracted-gate arms.
//
// Each test drives the REAL production function (imported from the crate,
// never reimplemented here) with an empty whitelist and an unlisted
// sender, and asserts it denies. Because the production callback's
// early-return guard IS this function (see the extraction rationale
// above), a `false` result here is dispositive: there is no path from a
// denied gate to a dispatched handler, so "the gate denies" and "the
// handler is never called" are the same fact, observed at its single
// decision point instead of via a downstream spy.
// ===========================================================================

/// D-08 gap 1, Telegram arm. Mirrors `runner.rs`'s dispatch-loop check
/// exactly — this is that function, not a copy of its logic.
#[tokio::test]
async fn telegram_empty_whitelist_delivers_nothing() {
    let unlisted_sender = "987654321";
    assert!(
        !telegram_whitelist_allows(&[], unlisted_sender),
        "an empty Telegram whitelist must deny every sender, including {unlisted_sender}"
    );
    // A non-empty whitelist that simply doesn't name this sender must also
    // deny — the empty-whitelist case is not a special "vacuously true"
    // case being mistaken for a working deny-all.
    assert!(!telegram_whitelist_allows(
        &["111222333".to_string()],
        unlisted_sender
    ));
}

/// D-08 gap 1, Discord arm. Mirrors `EventHandler::message`'s guard exactly.
#[tokio::test]
async fn discord_empty_whitelist_delivers_nothing() {
    let unlisted_sender: u64 = 555_444_333_222_111;
    assert!(
        !discord_whitelist_allows(&[], unlisted_sender),
        "an empty Discord whitelist must deny every sender, including {unlisted_sender}"
    );
    assert!(!discord_whitelist_allows(&[111_222_333], unlisted_sender));
}

/// D-08 gap 1, Slack arm. Mirrors `on_push_event`'s guard exactly.
#[tokio::test]
async fn slack_empty_whitelist_delivers_nothing() {
    let unlisted_sender = "U0UNLISTED1";
    assert!(
        !slack_whitelist_allows(&[], unlisted_sender),
        "an empty Slack whitelist must deny every sender, including {unlisted_sender}"
    );
    assert!(!slack_whitelist_allows(
        &["U0OTHER0001".to_string()],
        unlisted_sender
    ));
}

// ===========================================================================
// Buzz arms (tests 1-3) + the positive control (test 7): full live-relay
// harness, gated behind the `buzz` feature — see the module doc above.
// ===========================================================================

#[cfg(feature = "buzz")]
mod buzz_arms {
    //! Harness duplicated from `buzz_approvals.rs` per this crate's own
    //! stated convention (see that file's header comment) — each
    //! integration-test binary compiles independently, so importing across
    //! `tests/*.rs` files is not available; only the *behavior* under test
    //! is shared, not the plumbing that drives it.

    use ironhermes_core::{ChannelTrust, MessageEvent, PlatformGatewayConfig};
    use ironhermes_gateway::adapter::{MessageHandler, PlatformAdapter};
    use ironhermes_gateway::buzz::{BuzzAdapter, is_approval_command, run_buzz_adapter};
    use nostr_sdk::prelude::*;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Mutex as TokioMutex;
    use tokio_util::sync::CancellationToken;

    // -----------------------------------------------------------------
    // local_relay — minimal in-process NIP-01 relay (mirrors
    // buzz_approvals.rs / buzz_relay_roundtrip.rs's identical harness).
    // -----------------------------------------------------------------
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

    async fn spawn_mock_relay() -> (local_relay::LocalRelay, String) {
        let relay = local_relay::LocalRelay::start().await;
        let url = relay.url.clone();
        (relay, url)
    }

    async fn connected_client(keys: &Keys, relay_url: &str) -> Client {
        let authenticator = SignerAuthenticator::new(keys.clone());
        let client = Client::builder().authenticator(authenticator).build();
        client
            .add_relay(relay_url)
            .await
            .expect("add_relay failed");
        client.connect().await;
        client
    }

    pub const SETTLE_WINDOW: Duration = Duration::from_millis(800);

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

    /// Records every `MessageEvent` it receives — substitutes for the real
    /// `GatewayMessageHandler` in these deny-all/positive-control proofs,
    /// exactly as `run_buzz_adapter`'s own `handler: Arc<dyn MessageHandler>`
    /// parameter allows in production.
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

        tokio::time::sleep(Duration::from_millis(50)).await;

        (adapter, keys, cancel, task)
    }

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

    fn build_dm_event(sender: &Keys, receiver: PublicKey, content: &str) -> Event {
        PrivateDirectMessageBuilder::new(receiver, content)
            .finalize(sender)
            .expect("gift wrap build failed")
    }

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

    // -----------------------------------------------------------------
    // Test 1: Buzz DM arm.
    // -----------------------------------------------------------------

    /// D-08 gap 1, Buzz DM arm: empty whitelist denies a DM from an
    /// unlisted sender in BOTH trust modes — `channel_trust` only ever
    /// widens CHANNEL access, never DMs (T-47.6-05-02).
    #[tokio::test]
    async fn buzz_dm_empty_whitelist_delivers_nothing() {
        for trust in [ChannelTrust::Closed, ChannelTrust::Open] {
            let (_relay, relay_url) = spawn_mock_relay().await;
            let sender_keys = Keys::generate();
            let handler = RecordingHandler::new();
            let (adapter, _a_keys, cancel, _task) =
                spawn_agent(&relay_url, vec![], trust, handler.clone()).await;

            let a_pubkey = PublicKey::from_hex(&adapter.pubkey_hex()).expect("adapter pubkey hex");
            let sender_client = connected_client(&sender_keys, &relay_url).await;
            let dm_event = build_dm_event(&sender_keys, a_pubkey, "hello from an unlisted DM");
            sender_client
                .send_event(&dm_event)
                .await
                .expect("send_event failed");

            tokio::time::sleep(SETTLE_WINDOW).await;
            assert_eq!(
                handler.call_count().await,
                0,
                "empty whitelist must deny every Buzz DM under {trust:?} trust"
            );
            cancel.cancel();
        }
    }

    // -----------------------------------------------------------------
    // Test 2: Buzz channel arm, Closed trust.
    // -----------------------------------------------------------------

    /// D-08 gap 1, Buzz channel arm (`ChannelTrust::Closed`): empty
    /// whitelist denies an ordinary channel message from an unlisted
    /// sender.
    #[tokio::test]
    async fn buzz_channel_empty_whitelist_delivers_nothing_when_closed() {
        let (_relay, relay_url) = spawn_mock_relay().await;
        let sender_keys = Keys::generate();
        let handler = RecordingHandler::new();
        let channel = "ops-channel-deny-all";
        let (adapter, _a_keys, cancel, _task) = spawn_agent_full(
            &relay_url,
            vec![channel.to_string()],
            vec![],
            ChannelTrust::Closed,
            handler.clone(),
        )
        .await;

        let a_pubkey = PublicKey::from_hex(&adapter.pubkey_hex()).expect("adapter pubkey hex");
        let sender_client = connected_client(&sender_keys, &relay_url).await;
        let channel_event = build_channel_event(
            &sender_keys,
            channel,
            "hey, can you help with this?",
            Some(a_pubkey),
        );
        sender_client
            .send_event(&channel_event)
            .await
            .expect("send_event failed");

        tokio::time::sleep(SETTLE_WINDOW).await;
        assert_eq!(
            handler.call_count().await,
            0,
            "empty whitelist must deny an ordinary channel message under Closed trust"
        );
        cancel.cancel();
    }

    // -----------------------------------------------------------------
    // Test 3: Buzz channel arm, Open trust — the narrow-bypass proof.
    // -----------------------------------------------------------------

    /// D-08 `channel_trust: open` scope: an ordinary channel message from
    /// an unlisted sender IS delivered under Open trust (the deliberate
    /// opt-in bypass — `buzz.rs:1264-1284`), but an approval command from
    /// the same unlisted sender is NOT — proving the bypass is narrow
    /// (ordinary messages only) and that the approval re-check still fires
    /// regardless of trust mode. Call count must be exactly 1, never 0 (the
    /// message case) and never 2 (a leaked approval).
    #[tokio::test]
    async fn buzz_channel_open_trust_delivers_but_approval_command_still_rejected() {
        let (_relay, relay_url) = spawn_mock_relay().await;
        let sender_keys = Keys::generate();
        let handler = RecordingHandler::new();
        let channel = "ops-channel-open-trust";
        let (adapter, _a_keys, cancel, _task) = spawn_agent_full(
            &relay_url,
            vec![channel.to_string()],
            vec![], // empty whitelist — Open trust is the only reason anything gets through
            ChannelTrust::Open,
            handler.clone(),
        )
        .await;

        let a_pubkey = PublicKey::from_hex(&adapter.pubkey_hex()).expect("adapter pubkey hex");
        let sender_client = connected_client(&sender_keys, &relay_url).await;

        // Ordinary channel message: delivered under Open trust despite the
        // empty whitelist.
        let ordinary =
            build_channel_event(&sender_keys, channel, "just a normal message", Some(a_pubkey));
        sender_client
            .send_event(&ordinary)
            .await
            .expect("send_event (ordinary) failed");
        wait_for_dispatch(&handler, 1).await;

        // Approval command from the SAME unlisted sender: must still be
        // rejected — Open trust never widens approval authority.
        let approval = build_channel_event(&sender_keys, channel, "/approve abc123", Some(a_pubkey));
        sender_client
            .send_event(&approval)
            .await
            .expect("send_event (approval) failed");
        tokio::time::sleep(SETTLE_WINDOW).await;

        assert_eq!(
            handler.call_count().await,
            1,
            "exactly one delivery expected: the ordinary message under Open trust, \
             with the approval command still rejected regardless of trust mode"
        );
        cancel.cancel();
    }

    // -----------------------------------------------------------------
    // Test 7: the positive control.
    // -----------------------------------------------------------------

    /// Test 7 is load-bearing (plan `<action>`): without a positive
    /// control, all six deny-all tests would still pass if the harness were
    /// wired so nothing is ever delivered under ANY configuration. This
    /// proves the harness genuinely can deliver: a whitelisted DM sender
    /// reaches the handler.
    #[tokio::test]
    async fn non_empty_whitelist_delivers_to_listed_sender() {
        let (_relay, relay_url) = spawn_mock_relay().await;
        let sender_keys = Keys::generate();
        let sender_hex = sender_keys.public_key().to_hex();
        let handler = RecordingHandler::new();
        let (adapter, _a_keys, cancel, _task) = spawn_agent(
            &relay_url,
            vec![sender_hex],
            ChannelTrust::Closed,
            handler.clone(),
        )
        .await;

        let a_pubkey = PublicKey::from_hex(&adapter.pubkey_hex()).expect("adapter pubkey hex");
        let sender_client = connected_client(&sender_keys, &relay_url).await;
        let dm_event = build_dm_event(&sender_keys, a_pubkey, "hello from a whitelisted sender");
        sender_client
            .send_event(&dm_event)
            .await
            .expect("send_event failed");

        wait_for_dispatch(&handler, 1).await;
        assert!(
            handler.call_count().await >= 1,
            "a whitelisted sender must be delivered — the harness must be able to \
             deliver at all, or the six deny-all tests above prove nothing"
        );
        cancel.cancel();
    }

    // Keep `is_approval_command` linked in for the mutation-check header
    // comment's discoverability; not exercised directly by this module
    // (buzz_approvals.rs already covers it) but re-imported here so a
    // future reader following `use` statements lands on the real fn.
    #[allow(dead_code)]
    fn _reference_is_approval_command(s: &str) -> bool {
        is_approval_command(s)
    }
}

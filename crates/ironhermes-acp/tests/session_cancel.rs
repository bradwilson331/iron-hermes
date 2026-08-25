//! Behavior tests for `session/cancel` (Phase 36.8 plan 03, task 3).
//!
//! `unknown_session_returns_error` and `no_in_flight_turn_is_a_noop` drive
//! `handle_session_cancel` directly against a real `AcpSessionManager` — fast,
//! deterministic, no live connection needed. `in_flight_turn_is_cancelled_...` drives the
//! full ACP server over `Channel::duplex()` against a deliberately slow (delayed)
//! wiremock response so the cancel is deterministic (a controllable stub, not a
//! sleep-based race — `AgentLoop`'s `tokio::select!` races the LLM call against the
//! cancellation token, so a still-in-flight delayed response is exactly the condition
//! needed to prove the token actually interrupts a real turn).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{CancelNotification, InitializeRequest, StopReason};
use agent_client_protocol::{Agent, Channel, Client};
use ironhermes_acp::session_cancel::handle_session_cancel;
use ironhermes_acp::session_manager::AcpSessionManager;
use ironhermes_core::{Config, ModelsCache, ProviderConfig, ProviderResolver};
use ironhermes_state::StateStore;
use tokio::sync::Mutex as TokioMutex;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn build_config_and_resolver() -> (Arc<Config>, Arc<ProviderResolver>) {
    let mut config = Config::default();
    // `Config::default()`'s main provider is `openrouter` with its real production base_url,
    // and `ProviderResolver::build` resolves API keys from the process environment while
    // reading the operator's real `$IRONHERMES_HOME/models-cache.json` — so an unpinned
    // helper sends this test's prompt and a real credential to a third party on any machine
    // with `OPENROUTER_API_KEY`/`ANTHROPIC_API_KEY`/`OPENAI_API_KEY` exported. Pin the
    // endpoint to loopback port 1 (refuses instantly, never leaves the machine) with a
    // literal key so no env var is consulted, and use `build_with_cache` so the static model
    // table wins regardless of what is on the operator's disk.
    config.providers.insert(
        "openrouter".to_string(),
        ProviderConfig {
            base_url: Some("http://127.0.0.1:1/v1".to_string()),
            api_key: Some("test-key-not-a-real-credential".to_string()),
            ..Default::default()
        },
    );
    let resolver = ProviderResolver::build_with_cache(&config, ModelsCache::default())
        .expect("ProviderResolver::build_with_cache with isolated Config");
    (Arc::new(config), Arc::new(resolver))
}

fn build_config_and_resolver_pointed_at(server_uri: &str) -> (Arc<Config>, Arc<ProviderResolver>) {
    let mut config = Config::default();
    config.providers.insert(
        "openrouter".to_string(),
        ProviderConfig {
            base_url: Some(server_uri.to_string()),
            api_key: Some("test-key".to_string()),
            ..Default::default()
        },
    );
    let resolver =
        ProviderResolver::build(&config).expect("ProviderResolver::build with mocked provider");
    (Arc::new(config), Arc::new(resolver))
}

fn build_state_store() -> (Arc<Mutex<StateStore>>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir for state.db");
    let store = StateStore::new(tmp.path().join("state.db")).expect("StateStore::new");
    (Arc::new(Mutex::new(store)), tmp)
}

/// Behavior: cancelling an unknown session id returns an error, not a panic.
#[tokio::test]
async fn unknown_session_returns_error() {
    let (config, resolver) = build_config_and_resolver();
    let (state_store, _tmp) = build_state_store();
    let manager = Arc::new(TokioMutex::new(AcpSessionManager::new(
        state_store,
        config,
        resolver,
    )));

    let cancel_tokens = manager.lock().await.cancel_tokens();
    let result = handle_session_cancel(
        CancelNotification::new("acp_does_not_exist"),
        cancel_tokens,
    )
    .await;

    assert!(
        result.is_err(),
        "cancelling an unknown session id must return an Err, not silently succeed"
    );
}

/// Behavior: a cancel for a session with no in-flight turn is a no-op success, not an
/// error.
#[tokio::test]
async fn no_in_flight_turn_is_a_noop() {
    let (config, resolver) = build_config_and_resolver();
    let (state_store, _tmp) = build_state_store();
    let manager = Arc::new(TokioMutex::new(AcpSessionManager::new(
        state_store,
        config,
        resolver,
    )));

    let session_id = {
        let mut mgr = manager.lock().await;
        mgr.create(PathBuf::from("/tmp"), None)
            .await
            .expect("session create should succeed")
    };

    let cancel_tokens = manager.lock().await.cancel_tokens();
    let result =
        handle_session_cancel(CancelNotification::new(session_id.clone()), cancel_tokens).await;

    assert!(
        result.is_ok(),
        "cancelling a session with no in-flight turn must succeed as a no-op, got: {result:?}"
    );
}

/// SSE body for a normal (fast) turn — used for the "session recovers after cancel" half
/// of the in-flight test.
fn sse_normal_turn() -> String {
    let chunks = [
        r#"{"id":"c2","object":"chat.completion.chunk","created":2,"model":"test-model","choices":[{"index":0,"delta":{"content":"back to normal"}}]}"#,
        r#"{"id":"c2","object":"chat.completion.chunk","created":2,"model":"test-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7}}"#,
    ];
    let mut body = String::new();
    for c in chunks {
        body.push_str("data: ");
        body.push_str(c);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

/// Behaviors: cancelling a session with an in-flight turn causes `run_turn` to return
/// and the prompt response to carry the cancelled stop reason; and after a cancel, the
/// NEXT `session/prompt` on the same session runs normally — the token is reset per
/// turn, not latched for the session's lifetime (T-36.8-16).
#[tokio::test]
async fn in_flight_turn_is_cancelled_and_session_recovers_for_next_prompt() {
    let server = MockServer::start().await;
    // First turn: deliberately slow (5s) so the cancel — sent immediately after — wins
    // the `tokio::select!` race deterministically, not via a timing coincidence.
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse_normal_turn(), "text/event-stream")
                .set_delay(Duration::from_secs(5)),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    // Second turn (after cancel + recovery): fast, normal response.
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse_normal_turn(), "text/event-stream"),
        )
        .mount(&server)
        .await;

    let (config, resolver) = build_config_and_resolver_pointed_at(&server.uri());
    let (state_store, _state_tmp) = build_state_store();
    let (agent_channel, client_channel) = Channel::duplex();

    let agent_task = tokio::spawn(async move {
        ironhermes_acp::entry::run_acp_over(config, resolver, state_store, agent_channel).await
    });

    let tmp = tempfile::tempdir().expect("tempdir");
    let tmp_path = tmp.path().to_path_buf();

    let client_result = Client
        .builder()
        .name("acp-session-cancel-test-client")
        .connect_with(client_channel, async move |cx| {
            cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;

            cx.build_session(&tmp_path)
                .block_task()
                .run_until(async move |mut session| {
                    let session_id = session.session_id().clone();

                    // Turn 1: kick off the slow prompt, then cancel it almost immediately.
                    session.send_prompt("this one takes a while")?;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    session
                        .connection()
                        .send_notification_to(Agent, CancelNotification::new(session_id.clone()))?;

                    let first_stop_reason = read_stop_reason(&mut session).await;
                    assert_eq!(
                        first_stop_reason,
                        Some(StopReason::Cancelled),
                        "the cancelled turn's prompt response must carry the Cancelled stop reason"
                    );

                    // Turn 2: same session, no cancel this time — must run normally, proving
                    // the token was reset per-turn rather than latched.
                    session.send_prompt("now run normally")?;
                    let second_stop_reason = read_stop_reason(&mut session).await;
                    assert_eq!(
                        second_stop_reason,
                        Some(StopReason::EndTurn),
                        "a prompt on the same session AFTER a prior cancel must complete \
                         normally, not immediately re-cancel: {second_stop_reason:?}"
                    );

                    Ok(())
                })
                .await
        })
        .await;

    client_result.expect("client exchange should succeed");
    agent_task.abort();
}

/// Reads updates until the stop reason arrives, discarding notification content —
/// this test only cares about the terminal stop reason for each turn.
async fn read_stop_reason<Link>(
    session: &mut agent_client_protocol::ActiveSession<'_, Link>,
) -> Option<StopReason>
where
    Link: agent_client_protocol::role::HasPeer<agent_client_protocol::Agent>,
{
    loop {
        let message = session
            .read_update()
            .await
            .expect("session channel should not close before StopReason");
        match message {
            agent_client_protocol::SessionMessage::StopReason(reason) => return Some(reason),
            agent_client_protocol::SessionMessage::SessionMessage(_) => continue,
            _ => return None,
        }
    }
}

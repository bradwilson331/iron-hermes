//! SC-1 for the ACP surface (Phase 57 plan 17, D-21): the REAL ACP `terminal` intercept —
//! the one every `session/prompt` turn installs — screens through the booted sidecar
//! service and decides bypass from the ACP session's LIVE DEFCON level, never from the
//! `autonomous.yolo` flag it used to capture when the closure was built.
//!
//! The sidecar is the in-process `FakeBackend` (`install_fake_for_tests`); no binary is
//! spawned. The tests mutate two process-globals (`IRONHERMES_HOME` and the installed
//! service), so this crate's `--test-threads=1` rule applies to this file. The harness is
//! the one `execute_code_gate.rs` uses: a real `run_acp_over` agent on an in-memory duplex
//! channel, a `wiremock` provider, and a test client that answers
//! `session/request_permission` requests.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome, SessionUpdate, ToolCallContent,
    ToolCallStatus,
};
use agent_client_protocol::{Channel, Client, Responder};
use ironhermes_acp::handlers::POLICY_DENIAL_PREFIX;
use ironhermes_acp::tool_render::DENIAL_HEADLINE;
use ironhermes_core::config::DefconLevel;
use ironhermes_core::defcon::session_defcon;
use ironhermes_core::{AuditEntry, Config, ProviderResolver};
use ironhermes_security::policy::Severity;
use ironhermes_security::testing::{
    FakeBackend, clear_installed_for_tests, install_fake_for_tests,
};
use ironhermes_state::StateStore;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Restores every process-global this file touches, including on a failed assertion.
struct Globals {
    prev_home: Option<String>,
}

impl Globals {
    fn new(home: &std::path::Path) -> Self {
        let prev_home = std::env::var("IRONHERMES_HOME").ok();
        // SAFETY: this file runs with `--test-threads=1`, so nothing else touches the env.
        unsafe { std::env::set_var("IRONHERMES_HOME", home) };
        Self { prev_home }
    }
}

impl Drop for Globals {
    fn drop(&mut self) {
        match &self.prev_home {
            // SAFETY: as in `new`.
            Some(v) => unsafe { std::env::set_var("IRONHERMES_HOME", v) },
            None => unsafe { std::env::remove_var("IRONHERMES_HOME") },
        }
        clear_installed_for_tests();
    }
}

fn content_text(content: &ToolCallContent) -> Option<String> {
    match content {
        ToolCallContent::Content(c) => match &c.content {
            ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn build_config_and_resolver_pointed_at(server_uri: &str) -> (Arc<Config>, Arc<ProviderResolver>) {
    let mut config = Config::default();
    config.providers.insert(
        "openrouter".to_string(),
        ironhermes_core::ProviderConfig {
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

fn sse_tool_call(tool_name: &str, call_id: &str, arguments: serde_json::Value) -> String {
    let args_str = arguments.to_string();
    let chunk1 = serde_json::json!({
        "id": "c1", "object": "chat.completion.chunk", "created": 1, "model": "test-model",
        "choices": [{"index": 0, "delta": {"tool_calls": [{
            "index": 0, "id": call_id, "type": "function",
            "function": {"name": tool_name, "arguments": args_str}
        }]}}]
    })
    .to_string();
    let chunk2 = r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"test-model","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
    let mut body = String::new();
    for c in [chunk1.as_str(), chunk2] {
        body.push_str("data: ");
        body.push_str(c);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

fn sse_final_text_with_usage() -> String {
    let chunks = [
        r#"{"id":"c2","object":"chat.completion.chunk","created":2,"model":"test-model","choices":[{"index":0,"delta":{"content":"done"}}]}"#,
        r#"{"id":"c2","object":"chat.completion.chunk","created":2,"model":"test-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":20,"completion_tokens":2,"total_tokens":22}}"#,
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

async fn mount_sse(server: &MockServer, sse: String, times: Option<u64>) {
    let mock = Mock::given(method("POST")).respond_with(
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_raw(sse, "text/event-stream"),
    );
    match times {
        Some(n) => mock.up_to_n_times(n).mount(server).await,
        None => mock.mount(server).await,
    }
}

/// One turn: the tool call, then a plain final text for every later provider call.
async fn mount_tool_call_then_final_text(server: &MockServer, tool_call_sse: String) {
    mount_sse(server, tool_call_sse, Some(1)).await;
    mount_sse(server, sse_final_text_with_usage(), None).await;
}

/// Two turns: tool call, final text, tool call, final text, in strict registration order.
async fn mount_two_turns_of_tool_call_then_final_text(
    server: &MockServer,
    first_turn_tool_call_sse: String,
    second_turn_tool_call_sse: String,
) {
    mount_sse(server, first_turn_tool_call_sse, Some(1)).await;
    mount_sse(server, sse_final_text_with_usage(), Some(1)).await;
    mount_sse(server, second_turn_tool_call_sse, Some(1)).await;
    mount_sse(server, sse_final_text_with_usage(), None).await;
}

/// Drains `session` until its `StopReason` arrives, collecting every `session/update`
/// notification and answering every `session/request_permission` request with `AllowOnce`.
async fn drain_allowing_permission_requests<Link>(
    session: &mut agent_client_protocol::ActiveSession<'_, Link>,
) -> (Vec<SessionUpdate>, usize)
where
    Link: agent_client_protocol::role::HasPeer<agent_client_protocol::Agent>,
{
    use agent_client_protocol::schema::v1::PermissionOptionKind;
    use agent_client_protocol::util::MatchDispatch;
    let updates = Arc::new(Mutex::new(Vec::new()));
    let permission_requests_seen = Arc::new(AtomicUsize::new(0));

    loop {
        let message = session
            .read_update()
            .await
            .expect("session channel should not close before StopReason");
        match message {
            agent_client_protocol::SessionMessage::SessionMessage(dispatch) => {
                let updates_for_notif = updates.clone();
                let seen = permission_requests_seen.clone();
                MatchDispatch::new(dispatch)
                    .if_notification(
                        async move |notif: agent_client_protocol::schema::v1::SessionNotification| {
                            updates_for_notif.lock().unwrap().push(notif.update);
                            Ok(())
                        },
                    )
                    .await
                    .if_request(
                        async move |req: RequestPermissionRequest,
                                     responder: Responder<RequestPermissionResponse>| {
                            seen.fetch_add(1, Ordering::SeqCst);
                            let option = req
                                .options
                                .iter()
                                .find(|o| matches!(o.kind, PermissionOptionKind::AllowOnce))
                                .unwrap_or_else(|| {
                                    panic!(
                                        "server did not advertise AllowOnce among: {:?}",
                                        req.options
                                    )
                                });
                            let outcome = RequestPermissionOutcome::Selected(
                                SelectedPermissionOutcome::new(option.option_id.clone()),
                            );
                            responder.respond(RequestPermissionResponse::new(outcome))
                        },
                    )
                    .await
                    .otherwise_ignore()
                    .expect("dispatch matching should not error");
            }
            agent_client_protocol::SessionMessage::StopReason(_) => break,
            _ => break,
        }
    }

    let collected = updates.lock().unwrap().clone();
    let seen = permission_requests_seen.load(Ordering::SeqCst);
    (collected, seen)
}

fn shell_rows(home: &std::path::Path) -> Vec<AuditEntry> {
    let path = home.join("logs").join("audit.jsonl");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str::<AuditEntry>(l).expect("audit row parses"))
        .filter(|r| r.kind == "shell_command")
        .collect()
}

fn completed(updates: &[SessionUpdate]) -> bool {
    updates.iter().any(|u| {
        matches!(
            u,
            SessionUpdate::ToolCallUpdate(update)
                if matches!(update.fields.status, Some(ToolCallStatus::Completed))
        )
    })
}

/// A canary the sidecar blocks never runs through the real ACP `terminal` intercept, at
/// every level an ACP session can be at, including level 5: a Block is never bypassed
/// (D-21). The denial ledger records the call as refused by policy, so the client sees the
/// denial headline (the headline is chosen from the ledger, never from output text) and
/// the model sees `POLICY_DENIAL_PREFIX`.
#[tokio::test]
async fn acp_gated_call_canary_blocked() {
    let home = tempfile::tempdir().expect("tempdir");
    let _globals = Globals::new(home.path());
    install_fake_for_tests(
        FakeBackend::new().block_if_contains("CANARY-BLOCK", Severity::Critical),
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let sentinel = tmp.path().join("canary-sentinel.txt");
    let command = format!("touch {} # CANARY-BLOCK", sentinel.display());

    let server = MockServer::start().await;
    mount_tool_call_then_final_text(
        &server,
        sse_tool_call(
            "terminal",
            "call_term_canary",
            serde_json::json!({ "command": command }),
        ),
    )
    .await;

    let (config, resolver) = build_config_and_resolver_pointed_at(&server.uri());
    let (state_store, _state_tmp) = build_state_store();
    let (agent_channel, client_channel) = Channel::duplex();

    let agent_task = tokio::spawn(async move {
        ironhermes_acp::entry::run_acp_over(config, resolver, state_store, agent_channel).await
    });

    let session_cwd = tmp.path().to_path_buf();
    let client_result = Client
        .builder()
        .name("acp-canary-blocked-test-client")
        .connect_with(client_channel, async move |cx| {
            cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;

            cx.build_session(&session_cwd)
                .block_task()
                .run_until(async move |mut session| {
                    let sid = session.session_id().to_string();
                    // Level 5 is the strictest case for "a Block is never bypassed".
                    session_defcon().set_session_level(&sid, DefconLevel::FIVE);
                    session.send_prompt("please touch that file")?;
                    let (updates, permission_requests_seen) =
                        drain_allowing_permission_requests(&mut session).await;
                    session_defcon().forget(&sid);

                    assert_eq!(
                        permission_requests_seen, 0,
                        "a sidecar Block is not an approval question"
                    );
                    let failed_content = updates
                        .iter()
                        .find_map(|u| match u {
                            SessionUpdate::ToolCallUpdate(update)
                                if matches!(update.fields.status, Some(ToolCallStatus::Failed)) =>
                            {
                                update.fields.content.as_ref()
                            }
                            _ => None,
                        })
                        .expect("a blocked canary must surface as a FAILED tool_call_update");
                    assert!(
                        failed_content
                            .iter()
                            .filter_map(content_text)
                            .any(|t| t.starts_with(DENIAL_HEADLINE)),
                        "the denial ledger must have recorded the block, so the client sees \
                         the denial headline: {failed_content:?}"
                    );
                    Ok(())
                })
                .await
        })
        .await;

    client_result.expect("client exchange should succeed");

    let requests = server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.method.as_str() == "POST")
        .collect::<Vec<_>>();
    agent_task.abort();
    assert!(requests.len() >= 2, "tool-call turn plus its follow-up");
    let follow_up = String::from_utf8_lossy(&requests[1].body);
    assert!(
        follow_up.contains(POLICY_DENIAL_PREFIX),
        "the model must be told the call was refused by policy: {follow_up}"
    );

    assert!(
        !sentinel.exists(),
        "a blocked canary must never run: the sentinel file must not exist"
    );
    let rows = shell_rows(home.path());
    assert_eq!(rows.len(), 1, "one audited row for the call: {rows:?}");
    assert_eq!(rows[0].decision, "blocked", "{rows:?}");
}

/// The same Tier-1 `curl` through one ACP session follows that session's live level: at 5
/// it is an audited bypass (no permission request, `shell_command` row with resolution
/// `bypass`); set to 3 afterwards the very next turn takes the approval path (one
/// permission request). The `Config` never changes: only the session's level does.
#[tokio::test]
async fn acp_follows_session_level() {
    let home = tempfile::tempdir().expect("tempdir");
    let _globals = Globals::new(home.path());
    install_fake_for_tests(FakeBackend::new());

    let server = MockServer::start().await;
    let args = serde_json::json!({ "command": "curl https://x" });
    mount_two_turns_of_tool_call_then_final_text(
        &server,
        sse_tool_call("terminal", "call_term_curl_a", args.clone()),
        sse_tool_call("terminal", "call_term_curl_b", args),
    )
    .await;

    let (config, resolver) = build_config_and_resolver_pointed_at(&server.uri());
    let (state_store, _state_tmp) = build_state_store();
    let (agent_channel, client_channel) = Channel::duplex();

    let agent_task = tokio::spawn(async move {
        ironhermes_acp::entry::run_acp_over(config, resolver, state_store, agent_channel).await
    });

    let tmp = tempfile::tempdir().expect("tempdir");
    let session_cwd = tmp.path().to_path_buf();
    let client_result = Client
        .builder()
        .name("acp-follows-level-test-client")
        .connect_with(client_channel, async move |cx| {
            cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;

            cx.build_session(&session_cwd)
                .block_task()
                .run_until(async move |mut session| {
                    let sid = session.session_id().to_string();

                    // Turn 1 at level 5: an audited bypass, no operator question.
                    session_defcon().set_session_level(&sid, DefconLevel::FIVE);
                    session.send_prompt("please fetch that url")?;
                    let (updates_1, seen_1) =
                        drain_allowing_permission_requests(&mut session).await;
                    assert_eq!(
                        seen_1, 0,
                        "level 5 bypasses a Tier-1 Warn: no permission request"
                    );
                    assert!(
                        completed(&updates_1),
                        "the bypassed call runs: {updates_1:?}"
                    );

                    // Turn 2 at level 3, same session, same config: the approval path.
                    session_defcon().set_session_level(&sid, DefconLevel::THREE);
                    session.send_prompt("please fetch that url again")?;
                    let (_updates_2, seen_2) =
                        drain_allowing_permission_requests(&mut session).await;
                    session_defcon().forget(&sid);
                    assert_eq!(
                        seen_2, 1,
                        "level 3 must take the approval path exactly once"
                    );
                    Ok(())
                })
                .await
        })
        .await;

    client_result.expect("client exchange should succeed");
    agent_task.abort();

    let rows = shell_rows(home.path());
    assert_eq!(rows.len(), 2, "one audited row per call: {rows:?}");
    assert_eq!(rows[0].resolution, "bypass", "{rows:?}");
    assert_ne!(rows[1].resolution, "bypass", "{rows:?}");
}

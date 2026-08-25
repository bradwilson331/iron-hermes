//! Behavior tests for the ACP `execute_code`/`terminal` permission-gate wiring (Phase
//! 36.8 plan 04, task 3, CLI-06, D-15). Drives real turns over the in-memory
//! `Channel::duplex()` harness against a `wiremock`-mocked provider, with the TEST CLIENT
//! answering `session/request_permission` requests the agent sends mid-turn — mirrors
//! `event_bridge.rs`'s task-2 harness, extended to handle incoming REQUESTS (not just
//! notifications) via `MatchDispatch::if_request`.
//!
//! Plan 47.7-02 (D-12): denial visibility is proven end-to-end at the bottom of this file
//! — a real gated turn, denied the way buzz-acp denies (`RejectOnce`), must surface the
//! denial headline on the terminal `tool_call_update` AND the `POLICY_DENIAL_PREFIX`
//! marker in the tool-result text echoed back to the model, while an approved-then-failed
//! call must NOT be mislabelled as a policy denial.

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
use ironhermes_core::{Config, ProviderResolver};
use ironhermes_state::StateStore;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Mirrors `tests/tool_render.rs`'s own `text_of` helper — extracts the plain text from a
/// `ToolCallContent::Content(Content { content: ContentBlock::Text(_), .. })` item, `None`
/// for any other variant (Diff/Terminal) or content kind (image/resource).
fn content_text(content: &ToolCallContent) -> Option<String> {
    match content {
        ToolCallContent::Content(c) => match &c.content {
            ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Mirrors `event_bridge.rs`'s helper: a `Config`/`ProviderResolver` whose main provider
/// resolves to the mocked `server_uri`.
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

/// Isolated, tempdir-backed `StateStore` — never touches the operator's real
/// `$IRONHERMES_HOME/state.db`.
fn build_state_store() -> (Arc<Mutex<StateStore>>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir for state.db");
    let store = StateStore::new(tmp.path().join("state.db")).expect("StateStore::new");
    (Arc::new(Mutex::new(store)), tmp)
}

/// SSE body: one tool call for `tool_name` with `arguments`, then a `finish_reason:
/// tool_calls` chunk carrying usage.
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

/// SSE body for the follow-up (final) turn after the tool result comes back.
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

/// Mount the tool-call SSE for the first provider call, then a plain final-text SSE for
/// every subsequent call (the follow-up turn after the tool result is appended).
async fn mount_tool_call_then_final_text(server: &MockServer, tool_call_sse: String) {
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(tool_call_sse, "text/event-stream"),
        )
        .up_to_n_times(1)
        .mount(server)
        .await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse_final_text_with_usage(), "text/event-stream"),
        )
        .mount(server)
        .await;
}

/// Plan 06 UAT gap fix (step 9): mounts FOUR provider responses in strict registration
/// order — tool_call, final_text, tool_call, final_text — so TWO separate
/// `session/prompt` turns (each a tool-call round trip followed by the follow-up
/// round trip after the tool result) get served correctly. `up_to_n_times(1)` mocks are
/// consumed in registration order, falling through to the next one once exhausted (the
/// same mechanism `mount_tool_call_then_final_text` already relies on for one turn).
async fn mount_two_turns_of_tool_call_then_final_text(
    server: &MockServer,
    first_turn_tool_call_sse: String,
    second_turn_tool_call_sse: String,
) {
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(first_turn_tool_call_sse, "text/event-stream"),
        )
        .up_to_n_times(1)
        .mount(server)
        .await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse_final_text_with_usage(), "text/event-stream"),
        )
        .up_to_n_times(1)
        .mount(server)
        .await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(second_turn_tool_call_sse, "text/event-stream"),
        )
        .up_to_n_times(1)
        .mount(server)
        .await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse_final_text_with_usage(), "text/event-stream"),
        )
        .mount(server)
        .await;
}

/// How the test client answers every `session/request_permission` request it sees.
#[derive(Clone, Copy)]
enum PermissionAnswer {
    AllowOnce,
    AllowAlways,
    RejectOnce,
}

impl PermissionAnswer {
    /// Plan 06 UAT gap fix (found via live Zed UAT, step 9): whether a given
    /// server-advertised `PermissionOption` is the one this answer selects. The test
    /// client picks its response by matching `PermissionOptionKind` against the actual
    /// request's own `options` list — NEVER by hardcoding an option-id string — so this
    /// test exercises the real id round-trip a live client depends on, not just our own
    /// assumption about what string the server uses internally.
    fn matches(self, option: &agent_client_protocol::schema::v1::PermissionOption) -> bool {
        use agent_client_protocol::schema::v1::PermissionOptionKind;
        matches!(
            (self, &option.kind),
            (PermissionAnswer::AllowOnce, PermissionOptionKind::AllowOnce)
                | (PermissionAnswer::AllowAlways, PermissionOptionKind::AllowAlways)
                | (PermissionAnswer::RejectOnce, PermissionOptionKind::RejectOnce)
        )
    }
}

/// Drains `session` until its `StopReason` arrives, collecting every `session/update`
/// notification AND answering every `session/request_permission` REQUEST the agent sends
/// mid-turn with `answer`. Returns the collected updates plus how many permission
/// requests were seen — extends `event_bridge.rs`'s `read_all_updates` to also handle
/// incoming requests (not just notifications) via `MatchDispatch::if_request`.
async fn drain_answering_permission_requests<Link>(
    session: &mut agent_client_protocol::ActiveSession<'_, Link>,
    answer: PermissionAnswer,
) -> (Vec<SessionUpdate>, usize)
where
    Link: agent_client_protocol::role::HasPeer<agent_client_protocol::Agent>,
{
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
                            // Use the SERVER's own advertised option id — never a
                            // hardcoded string — so this test proves the real id
                            // round-trip, not just our assumption about it.
                            let option = req
                                .options
                                .iter()
                                .find(|o| answer.matches(o))
                                .unwrap_or_else(|| {
                                    panic!(
                                        "server did not advertise the expected option \
                                         among: {:?}",
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
            // `SessionMessage` is `#[non_exhaustive]` — treat any future variant as a
            // reason to stop collecting rather than looping forever.
            _ => break,
        }
    }

    let collected = updates.lock().unwrap().clone();
    let seen = permission_requests_seen.load(Ordering::SeqCst);
    (collected, seen)
}

/// An `execute_code` call always routes through the D-15 dangerous-op permission path
/// (task 3's must-have truth #6) — this must be true regardless of payload content, since
/// D-11's gate-only classify_arg is always opaque for `execute_code`. Approving it lets
/// the real sandbox run, creating the sentinel file.
#[tokio::test]
async fn execute_code_call_raises_a_permission_request_and_runs_when_approved() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sentinel = tmp.path().join("approved-sentinel.txt");

    let server = MockServer::start().await;
    let args = serde_json::json!({"code": format!("open('{}', 'w').close()", sentinel.display())});
    mount_tool_call_then_final_text(
        &server,
        sse_tool_call("execute_code", "call_exec_approve", args),
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
        .name("acp-execute-code-approve-test-client")
        .connect_with(client_channel, async move |cx| {
            cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;

            cx.build_session(&session_cwd)
                .block_task()
                .run_until(async move |mut session| {
                    session.send_prompt("please run some python")?;
                    let (_updates, permission_requests_seen) =
                        drain_answering_permission_requests(&mut session, PermissionAnswer::AllowOnce)
                            .await;

                    assert!(
                        permission_requests_seen >= 1,
                        "an execute_code call must raise at least one \
                         session/request_permission request (D-15)"
                    );
                    Ok(())
                })
                .await
        })
        .await;

    client_result.expect("client exchange should succeed");
    agent_task.abort();

    assert!(
        sentinel.exists(),
        "an approved execute_code call should have actually run and created the sentinel file"
    );
}

/// A denied outcome must yield a tool error (surfaced as a FAILED `tool_call_update`) and
/// must produce NO execution side effect — the sentinel file the payload would have
/// created must not exist.
#[tokio::test]
async fn execute_code_denied_outcome_yields_a_tool_error_and_no_side_effect() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sentinel = tmp.path().join("denied-sentinel.txt");

    let server = MockServer::start().await;
    let args = serde_json::json!({"code": format!("open('{}', 'w').close()", sentinel.display())});
    mount_tool_call_then_final_text(&server, sse_tool_call("execute_code", "call_exec_deny", args))
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
        .name("acp-execute-code-deny-test-client")
        .connect_with(client_channel, async move |cx| {
            cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;

            cx.build_session(&session_cwd)
                .block_task()
                .run_until(async move |mut session| {
                    session.send_prompt("please run some python")?;
                    let (updates, permission_requests_seen) = drain_answering_permission_requests(
                        &mut session,
                        PermissionAnswer::RejectOnce,
                    )
                    .await;

                    assert!(permission_requests_seen >= 1);
                    assert!(
                        updates.iter().any(|u| matches!(
                            u,
                            SessionUpdate::ToolCallUpdate(update)
                                if matches!(update.fields.status, Some(ToolCallStatus::Failed))
                        )),
                        "a denied execute_code call must surface as a FAILED tool_call_update \
                         (an explanatory tool error), got: {updates:?}"
                    );
                    Ok(())
                })
                .await
        })
        .await;

    client_result.expect("client exchange should succeed");
    agent_task.abort();

    assert!(
        !sentinel.exists(),
        "a denied execute_code call must never run — the sentinel file must not exist"
    );
}

/// A tier-2 (hard-block) `terminal` command is refused outright — `execute_gated_command`
/// returns `Blocked` before any approval branch is ever reached, so no permission request
/// is sent (a hard block is not a decision the user gets to override).
#[tokio::test]
async fn tier2_blocked_terminal_command_is_refused_without_any_permission_request() {
    let server = MockServer::start().await;
    let args = serde_json::json!({"command": "rm -rf /"});
    mount_tool_call_then_final_text(&server, sse_tool_call("terminal", "call_term_block", args)).await;

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
        .name("acp-terminal-block-test-client")
        .connect_with(client_channel, async move |cx| {
            cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;

            cx.build_session(&session_cwd)
                .block_task()
                .run_until(async move |mut session| {
                    session.send_prompt("please clean everything up")?;
                    let (updates, permission_requests_seen) =
                        drain_answering_permission_requests(&mut session, PermissionAnswer::AllowOnce)
                            .await;

                    assert_eq!(
                        permission_requests_seen, 0,
                        "a tier-2 blocked command must never raise a permission request"
                    );
                    assert!(
                        updates.iter().any(|u| matches!(
                            u,
                            SessionUpdate::ToolCallUpdate(update)
                                if matches!(update.fields.status, Some(ToolCallStatus::Failed))
                        )),
                        "a tier-2 blocked command must surface as a FAILED tool_call_update, \
                         got: {updates:?}"
                    );
                    Ok(())
                })
                .await
        })
        .await;

    client_result.expect("client exchange should succeed");
    agent_task.abort();
}

/// A benign `terminal` command (classifies `Allow`, no forced approval) runs without any
/// permission request being sent.
#[tokio::test]
async fn benign_terminal_command_runs_without_prompting() {
    let server = MockServer::start().await;
    let args = serde_json::json!({"command": "echo acp-benign-test"});
    mount_tool_call_then_final_text(&server, sse_tool_call("terminal", "call_term_benign", args))
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
        .name("acp-terminal-benign-test-client")
        .connect_with(client_channel, async move |cx| {
            cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;

            cx.build_session(&session_cwd)
                .block_task()
                .run_until(async move |mut session| {
                    session.send_prompt("please say hello")?;
                    let (updates, permission_requests_seen) =
                        drain_answering_permission_requests(&mut session, PermissionAnswer::AllowOnce)
                            .await;

                    assert_eq!(
                        permission_requests_seen, 0,
                        "a benign command classified Allow must run without any permission \
                         request"
                    );
                    assert!(
                        updates.iter().any(|u| matches!(
                            u,
                            SessionUpdate::ToolCallUpdate(update)
                                if matches!(update.fields.status, Some(ToolCallStatus::Completed))
                        )),
                        "a benign command must complete successfully, got: {updates:?}"
                    );
                    Ok(())
                })
                .await
        })
        .await;

    client_result.expect("client exchange should succeed");
    agent_task.abort();
}

// ── plan 06 UAT gap fix (step 9, D-14 allow-always persistence) ────────────────────

/// UAT step 9 (found via live Zed UAT, task 3 checkpoint): choosing "allow always" on the
/// permission prompt correctly ran the command, but the NEXT identical Tier-1 request in
/// the SAME session re-prompted instead of being suppressed. Drives TWO separate
/// `session/prompt` turns in the SAME session, each triggering the identical Tier-1
/// `terminal` command (`curl https://x`, a real Tier-1 pattern per
/// `ironhermes_hooks::guardrail`'s own test suite). The first turn answers with the
/// server's own advertised `AllowAlways` option; the second turn must complete with ZERO
/// additional `session/request_permission` requests — the editor-observable contract
/// D-14's session-scoped grant exists to provide.
#[tokio::test]
async fn allow_always_suppresses_second_identical_tier1_request_in_same_session() {
    let server = MockServer::start().await;
    let cmd = "curl https://x";
    let args = serde_json::json!({"command": cmd});
    mount_two_turns_of_tool_call_then_final_text(
        &server,
        sse_tool_call("terminal", "call_term_curl_1", args.clone()),
        sse_tool_call("terminal", "call_term_curl_2", args),
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
        .name("acp-allow-always-same-session-test-client")
        .connect_with(client_channel, async move |cx| {
            cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;

            cx.build_session(&session_cwd)
                .block_task()
                .run_until(async move |mut session| {
                    // Turn 1: prompt, allow-always the Tier-1 curl call.
                    session.send_prompt("please fetch that url")?;
                    let (updates_1, seen_1) = drain_answering_permission_requests(
                        &mut session,
                        PermissionAnswer::AllowAlways,
                    )
                    .await;
                    assert_eq!(
                        seen_1, 1,
                        "the first curl call must raise exactly one permission request"
                    );
                    assert!(
                        updates_1.iter().any(|u| matches!(
                            u,
                            SessionUpdate::ToolCallUpdate(update)
                                if matches!(update.fields.status, Some(ToolCallStatus::Completed))
                        )),
                        "the approved first curl call must actually run, got: {updates_1:?}"
                    );

                    // Turn 2: SAME command, SAME session — must run with NO new prompt.
                    session.send_prompt("please fetch that url again")?;
                    let (updates_2, seen_2) = drain_answering_permission_requests(
                        &mut session,
                        PermissionAnswer::AllowAlways,
                    )
                    .await;
                    assert_eq!(
                        seen_2, 0,
                        "an allow-always grant must suppress a second identical Tier-1 \
                         request in the same session — got {seen_2} additional permission \
                         request(s)"
                    );
                    assert!(
                        updates_2.iter().any(|u| matches!(
                            u,
                            SessionUpdate::ToolCallUpdate(update)
                                if matches!(update.fields.status, Some(ToolCallStatus::Completed))
                        )),
                        "the second (auto-approved) curl call must still actually run, got: \
                         {updates_2:?}"
                    );

                    Ok(())
                })
                .await
        })
        .await;

    client_result.expect("client exchange should succeed");
    agent_task.abort();
}

/// D-14 session-scope boundary check (the inverse of the test above): an `allow-always`
/// grant made in one session must NEVER suppress the identical Tier-1 request in a
/// DIFFERENT session — each `AcpSession` gets its own fresh `ApprovalsStore`
/// (`AcpSessionManager::build_and_insert`), and this proves that holds at the protocol
/// level, not just in `approval_bridge.rs`'s unit tests.
#[tokio::test]
async fn allow_always_does_not_leak_into_a_new_session() {
    let server = MockServer::start().await;
    let cmd = "curl https://x";
    let args = serde_json::json!({"command": cmd});
    mount_two_turns_of_tool_call_then_final_text(
        &server,
        sse_tool_call("terminal", "call_term_curl_session_a", args.clone()),
        sse_tool_call("terminal", "call_term_curl_session_b", args),
    )
    .await;

    let (config, resolver) = build_config_and_resolver_pointed_at(&server.uri());
    let (state_store, _state_tmp) = build_state_store();
    let (agent_channel, client_channel) = Channel::duplex();

    let agent_task = tokio::spawn(async move {
        ironhermes_acp::entry::run_acp_over(config, resolver, state_store, agent_channel).await
    });

    let tmp_a = tempfile::tempdir().expect("tempdir a");
    let tmp_b = tempfile::tempdir().expect("tempdir b");
    let cwd_a = tmp_a.path().to_path_buf();
    let cwd_b = tmp_b.path().to_path_buf();

    let client_result = Client
        .builder()
        .name("acp-allow-always-cross-session-test-client")
        .connect_with(client_channel, async move |cx| {
            cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;

            // Session A: allow-always the curl call.
            cx.build_session(&cwd_a)
                .block_task()
                .run_until(async move |mut session| {
                    session.send_prompt("please fetch that url")?;
                    let (_updates, seen) = drain_answering_permission_requests(
                        &mut session,
                        PermissionAnswer::AllowAlways,
                    )
                    .await;
                    assert_eq!(seen, 1, "session A's curl call must raise one permission request");
                    Ok(())
                })
                .await?;

            // Session B: identical command, DIFFERENT session — must still prompt.
            cx.build_session(&cwd_b)
                .block_task()
                .run_until(async move |mut session| {
                    session.send_prompt("please fetch that url")?;
                    let (_updates, seen) = drain_answering_permission_requests(
                        &mut session,
                        PermissionAnswer::AllowAlways,
                    )
                    .await;
                    assert_eq!(
                        seen, 1,
                        "an allow-always grant in session A must NOT suppress the identical \
                         request in a brand-new session B (D-14 session scope)"
                    );
                    Ok(())
                })
                .await
        })
        .await;

    client_result.expect("client exchange should succeed");
    agent_task.abort();
}

// ── plan 47.7-02 (D-12): denial visibility on BOTH ends ────────────────────

/// D-12 positive case: a policy denial (buzz-acp's own always-deny `RejectOnce` behavior)
/// must be distinguishable from an ordinary tool failure at BOTH ends — the terminal
/// `tool_call_update` a client renders, AND the tool-result text the model reads back (the
/// only path a Buzz channel reader actually sees, since buzz-acp's `handle_session_update`
/// does not render `tool_call_update` content into the channel — see RESEARCH.md).
#[tokio::test]
async fn denied_execute_code_call_surfaces_denial_headline_and_prefix_to_both_ends() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sentinel = tmp.path().join("denial-visibility-sentinel.txt");

    let server = MockServer::start().await;
    let args = serde_json::json!({"code": format!("open('{}', 'w').close()", sentinel.display())});
    mount_tool_call_then_final_text(
        &server,
        sse_tool_call("execute_code", "call_exec_denial_visibility", args),
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
        .name("acp-execute-code-denial-visibility-test-client")
        .connect_with(client_channel, async move |cx| {
            cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;

            cx.build_session(&session_cwd)
                .block_task()
                .run_until(async move |mut session| {
                    session.send_prompt("please run some python")?;
                    let (updates, permission_requests_seen) = drain_answering_permission_requests(
                        &mut session,
                        PermissionAnswer::RejectOnce,
                    )
                    .await;

                    assert!(
                        permission_requests_seen >= 1,
                        "a denied execute_code call must still have raised a permission request"
                    );

                    // (a) the terminal FAILED tool_call_update carries the denial headline —
                    // proving a client that renders tool-call content can show the verdict.
                    let failed_content = updates.iter().find_map(|u| match u {
                        SessionUpdate::ToolCallUpdate(update)
                            if matches!(update.fields.status, Some(ToolCallStatus::Failed)) =>
                        {
                            update.fields.content.as_ref()
                        }
                        _ => None,
                    });
                    let failed_content =
                        failed_content.expect("expected a FAILED tool_call_update carrying content");
                    let has_headline = failed_content
                        .iter()
                        .filter_map(content_text)
                        .any(|text| text.starts_with(DENIAL_HEADLINE));
                    assert!(
                        has_headline,
                        "the terminal tool_call_update's content must lead with the denial \
                         headline: {failed_content:?}"
                    );

                    Ok(())
                })
                .await
        })
        .await;

    client_result.expect("client exchange should succeed");

    // (b) the tool-result text handed back into the conversation carries
    // POLICY_DENIAL_PREFIX — proving the model has the material to explain the refusal in
    // its own reply, the only path a Buzz channel reader actually sees. The tool result is
    // echoed back to the provider as a `tool` role message on the FOLLOW-UP completion
    // request (the second request this turn made), so assert against that request body —
    // observable without adding a new production accessor.
    let requests = server.received_requests().await.unwrap_or_default();
    agent_task.abort();
    assert!(
        requests.len() >= 2,
        "expected at least 2 provider requests (the tool-call turn plus its follow-up), got {}",
        requests.len()
    );
    let follow_up_body = String::from_utf8_lossy(&requests[1].body);
    assert!(
        follow_up_body.contains(POLICY_DENIAL_PREFIX),
        "the follow-up completion request must echo POLICY_DENIAL_PREFIX back to the model \
         so it can explain the refusal in its own reply, got body: {follow_up_body}"
    );

    assert!(
        !sentinel.exists(),
        "a denied execute_code call must never run — the sentinel file must not exist"
    );
}

/// D-12 negative case (the inverse of the test above): a tool that IS approved and then
/// fails on its own (missing the required `code` argument — `ExecuteCodeTool::execute`
/// returns `Err` before the sandbox ever runs) must produce a FAILED `tool_call_update`
/// whose content does NOT contain the denial headline. Without this case, a rendering
/// change that unconditionally prefixed every failure would still pass the positive test
/// above.
#[tokio::test]
async fn approved_then_failed_execute_code_call_does_not_carry_the_denial_headline() {
    let server = MockServer::start().await;
    // Deliberately missing "code" — ExecuteCodeTool::execute returns
    // `Err("Missing required parameter: code")` BEFORE the sandbox ever runs, so this is a
    // genuine tool failure (GatedOutcome::Failed), not a policy decision.
    let args = serde_json::json!({});
    mount_tool_call_then_final_text(
        &server,
        sse_tool_call("execute_code", "call_exec_approved_then_failed", args),
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
        .name("acp-execute-code-approved-then-failed-test-client")
        .connect_with(client_channel, async move |cx| {
            cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;

            cx.build_session(&session_cwd)
                .block_task()
                .run_until(async move |mut session| {
                    session.send_prompt("please run some python")?;
                    let (updates, permission_requests_seen) = drain_answering_permission_requests(
                        &mut session,
                        PermissionAnswer::AllowOnce,
                    )
                    .await;

                    assert!(
                        permission_requests_seen >= 1,
                        "every execute_code call is forced through approval (D-15), approved \
                         or not"
                    );

                    let failed_content = updates.iter().find_map(|u| match u {
                        SessionUpdate::ToolCallUpdate(update)
                            if matches!(update.fields.status, Some(ToolCallStatus::Failed)) =>
                        {
                            update.fields.content.as_ref()
                        }
                        _ => None,
                    });
                    let failed_content = failed_content.expect(
                        "an approved call that fails on its own must still surface a FAILED \
                         tool_call_update",
                    );
                    let has_headline = failed_content
                        .iter()
                        .filter_map(content_text)
                        .any(|text| text.starts_with(DENIAL_HEADLINE));
                    assert!(
                        !has_headline,
                        "an ordinary approved-then-failed tool call must NEVER be mislabelled \
                         as a policy denial: {failed_content:?}"
                    );

                    Ok(())
                })
                .await
        })
        .await;

    client_result.expect("client exchange should succeed");
    agent_task.abort();
}

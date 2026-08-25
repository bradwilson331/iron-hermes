//! Behavior tests for the D-16 / CLI-08 workspace-write boundary contract (Phase 36.8 plan
//! 05, task 3): `tool_render::is_outside_workspace`'s canonicalization + component-based
//! comparison, and the end-to-end approval-gated `write_file`/`patch` path wired in
//! `handlers.rs` (`gate_workspace_write`).

use ironhermes_acp::tool_render::is_outside_workspace;

// ─────────────────────────────────────────────────────────────────────────
// `is_outside_workspace` boundary predicate
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn path_inside_cwd_is_not_outside() {
    let dir = tempfile::tempdir().expect("tempdir");
    let inside = dir.path().join("subdir").join("file.txt");
    std::fs::create_dir_all(inside.parent().unwrap()).expect("mkdir");
    std::fs::write(&inside, "x").expect("write");

    assert!(!is_outside_workspace(dir.path(), &inside));
}

#[test]
fn path_outside_cwd_is_outside() {
    let cwd_dir = tempfile::tempdir().expect("tempdir");
    let other_dir = tempfile::tempdir().expect("tempdir");
    let outside = other_dir.path().join("file.txt");
    std::fs::write(&outside, "x").expect("write");

    assert!(is_outside_workspace(cwd_dir.path(), &outside));
}

#[test]
fn sibling_directory_with_prefix_matching_name_is_not_mistaken_for_a_child() {
    // Component-based comparison must reject this: "<parent>/xyz-workspace-evil" is NOT a
    // child of "<parent>/xyz-workspace", even though the STRING "<parent>/xyz-workspace"
    // is a string-prefix of "<parent>/xyz-workspace-evil".
    let root = tempfile::tempdir().expect("tempdir");
    let base_name = root.path().file_name().unwrap().to_str().unwrap().to_string();
    let parent = root.path().parent().unwrap();

    let workspace = parent.join(format!("{base_name}-workspace"));
    let sibling = parent.join(format!("{base_name}-workspace-evil"));
    std::fs::create_dir_all(&workspace).expect("mkdir workspace");
    std::fs::create_dir_all(&sibling).expect("mkdir sibling");
    let sibling_file = sibling.join("file.txt");
    std::fs::write(&sibling_file, "x").expect("write");

    assert!(
        is_outside_workspace(&workspace, &sibling_file),
        "a sibling directory whose name merely starts with the cwd's name must be OUTSIDE"
    );

    std::fs::remove_dir_all(&workspace).ok();
    std::fs::remove_dir_all(&sibling).ok();
}

#[test]
fn parent_traversal_that_resolves_back_inside_cwd_is_treated_as_inside() {
    let root = tempfile::tempdir().expect("tempdir");
    let cwd = root.path().join("cwd");
    let sibling_inside = cwd.join("sibling_inside");
    std::fs::create_dir_all(&sibling_inside).expect("mkdir");

    // "<cwd>/../cwd/sibling_inside/target.txt" — traverses out and straight back into the
    // SAME cwd via a real, existing intermediate directory.
    let traversal_path = cwd.join("..").join("cwd").join("sibling_inside").join("target.txt");

    assert!(!is_outside_workspace(&cwd, &traversal_path));
}

#[test]
fn parent_traversal_that_resolves_outside_cwd_is_treated_as_outside() {
    let root = tempfile::tempdir().expect("tempdir");
    let cwd = root.path().join("cwd");
    let outside_sibling = root.path().join("outside_sibling");
    std::fs::create_dir_all(&cwd).expect("mkdir cwd");
    std::fs::create_dir_all(&outside_sibling).expect("mkdir outside_sibling");

    let traversal_path = cwd.join("..").join("outside_sibling").join("target.txt");

    assert!(is_outside_workspace(&cwd, &traversal_path));
}

#[test]
#[cfg(unix)]
fn symlink_inside_cwd_pointing_outside_is_treated_as_outside() {
    let cwd_dir = tempfile::tempdir().expect("tempdir");
    let outside_dir = tempfile::tempdir().expect("tempdir");
    let outside_target = outside_dir.path().join("real_file.txt");
    std::fs::write(&outside_target, "outside content").expect("write");

    let symlink_path = cwd_dir.path().join("link_to_outside.txt");
    std::os::unix::fs::symlink(&outside_target, &symlink_path).expect("create symlink");

    assert!(
        is_outside_workspace(cwd_dir.path(), &symlink_path),
        "a symlink INSIDE the workspace pointing OUTSIDE it must classify as outside"
    );
}

#[test]
fn not_yet_existing_path_is_classified_by_its_parent_directory() {
    let cwd_dir = tempfile::tempdir().expect("tempdir");
    let not_yet_existing = cwd_dir.path().join("brand_new_file_never_written.txt");
    assert!(!not_yet_existing.exists());

    // The leaf doesn't exist, but its parent (the workspace root) does — must classify by
    // the parent, i.e. INSIDE.
    assert!(!is_outside_workspace(cwd_dir.path(), &not_yet_existing));

    let outside_dir = tempfile::tempdir().expect("tempdir");
    let not_yet_existing_outside = outside_dir.path().join("brand_new_elsewhere.txt");
    assert!(is_outside_workspace(cwd_dir.path(), &not_yet_existing_outside));
}

#[test]
fn negative_check_no_string_prefix_comparison() {
    // Source-level guard (mirrors the plan's own acceptance-criteria grep): tool_render.rs
    // must never compare via `.to_string_lossy().starts_with(...)` or
    // `starts_with(&cwd_string)` — component-based `Path::starts_with` only.
    let source = include_str!("../src/tool_render.rs");
    let stripped: String = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !stripped.contains("starts_with(&cwd_string") && !stripped.contains("to_string_lossy().starts_with"),
        "tool_render.rs must use component-based Path::starts_with, not a string-prefix comparison"
    );
    assert!(
        stripped.contains("canonicalize"),
        "tool_render.rs's boundary check path must canonicalize both sides"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// End-to-end approval-gated write path (`gate_workspace_write`, handlers.rs)
//
// Mirrors `tests/execute_code_gate.rs`'s proven harness exactly: drives a real turn over
// the in-memory `Channel::duplex()` server against a `wiremock`-mocked provider, with the
// TEST CLIENT answering `session/request_permission` requests the agent sends mid-turn.
// ─────────────────────────────────────────────────────────────────────────

mod e2e {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use agent_client_protocol::schema::ProtocolVersion;
    use agent_client_protocol::schema::v1::{
        InitializeRequest, RequestPermissionOutcome, RequestPermissionRequest,
        RequestPermissionResponse, SelectedPermissionOutcome, SessionUpdate, ToolCallStatus,
    };
    use agent_client_protocol::{Channel, Client, Responder};
    use ironhermes_core::{Config, ProviderResolver};
    use ironhermes_state::StateStore;
    use wiremock::matchers::any;
    use wiremock::{Mock, MockServer, ResponseTemplate};

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

    /// Plan 07 (CR-01 regression coverage): mounts FOUR provider responses in strict
    /// registration order — tool_call, final_text, tool_call, final_text — so TWO separate
    /// `session/prompt` turns (each a tool-call round trip followed by the follow-up round
    /// trip after the tool result) get served correctly inside a SINGLE ACP session.
    /// Copied from `tests/execute_code_gate.rs`'s proven `mount_two_turns_of_tool_call_then_final_text`
    /// (integration test binaries do not share helpers).
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

    #[derive(Clone, Copy)]
    enum PermissionAnswer {
        AllowOnce,
        RejectOnce,
    }

    impl PermissionAnswer {
        fn option_id(self) -> &'static str {
            match self {
                PermissionAnswer::AllowOnce => "allow-once",
                PermissionAnswer::RejectOnce => "reject-once",
            }
        }
    }

    /// Mirrors `tests/execute_code_gate.rs`'s proven harness: drains `session` until its
    /// `StopReason`, collecting updates and answering every `session/request_permission`
    /// request with `answer`.
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
                            async move |_req: RequestPermissionRequest,
                                         responder: Responder<RequestPermissionResponse>| {
                                seen.fetch_add(1, Ordering::SeqCst);
                                let outcome = RequestPermissionOutcome::Selected(
                                    SelectedPermissionOutcome::new(answer.option_id()),
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

    /// A write to a path INSIDE the session cwd raises a permission request; approving it
    /// lets the real write happen.
    #[tokio::test]
    async fn write_inside_cwd_raises_permission_request_and_writes_on_approval() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let target = tmp.path().join("inside_target.txt");
        std::fs::write(&target, "original").expect("seed fixture");

        let server = MockServer::start().await;
        // Real LLM-issued calls carry an ABSOLUTE path (the LLM is told the session cwd) —
        // `WriteFileTool::execute` writes to whatever path string it receives verbatim, no
        // cwd-relative resolution baked in, so this must match production behavior.
        let args = serde_json::json!({
            "path": target.to_str().unwrap(),
            "content": "approved new content",
        });
        mount_tool_call_then_final_text(
            &server,
            sse_tool_call("write_file", "call_write_approve", args),
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
            .name("workspace-boundary-inside-approve-client")
            .connect_with(client_channel, async move |cx| {
                cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;

                cx.build_session(&session_cwd)
                    .block_task()
                    .run_until(async move |mut session| {
                        session.send_prompt("please update inside_target.txt")?;
                        let (updates, permission_requests_seen) =
                            drain_answering_permission_requests(&mut session, PermissionAnswer::AllowOnce)
                                .await;

                        assert!(
                            permission_requests_seen >= 1,
                            "a workspace write inside the cwd must raise a permission request (D-16)"
                        );
                        assert!(
                            updates.iter().any(|u| matches!(
                                u,
                                SessionUpdate::ToolCallUpdate(update)
                                    if matches!(update.fields.status, Some(ToolCallStatus::Completed))
                            )),
                            "an approved write must complete, got: {updates:?}"
                        );
                        // Plan-level <verification>: the ToolCall update's content must
                        // include a diff with the correct absolute path.
                        let diff_found = updates.iter().find_map(|u| match u {
                            SessionUpdate::ToolCall(call) => call.content.iter().find_map(|c| {
                                match c {
                                    agent_client_protocol::schema::v1::ToolCallContent::Diff(d) => {
                                        Some(d.path.clone())
                                    }
                                    _ => None,
                                }
                            }),
                            _ => None,
                        });
                        let diff_path = diff_found.unwrap_or_else(|| {
                            panic!("expected a ToolCall update carrying a Diff content, got: {updates:?}")
                        });
                        assert!(diff_path.is_absolute(), "diff path must be absolute: {}", diff_path.display());
                        Ok(())
                    })
                    .await
            })
            .await;

        client_result.expect("client exchange should succeed");
        agent_task.abort();

        let final_content = std::fs::read_to_string(&target).expect("read post-call content");
        assert_eq!(final_content, "approved new content");
    }

    /// A denied permission for a workspace write leaves the target file byte-identical to
    /// its pre-call contents.
    #[tokio::test]
    async fn denied_write_leaves_target_file_byte_identical() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let target = tmp.path().join("guarded.txt");
        std::fs::write(&target, "original content").expect("seed fixture");
        let pre_call_bytes = std::fs::read(&target).expect("read pre-call bytes");

        let server = MockServer::start().await;
        let args = serde_json::json!({"path": "guarded.txt", "content": "malicious overwrite"});
        mount_tool_call_then_final_text(&server, sse_tool_call("write_file", "call_write_deny", args))
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
            .name("workspace-boundary-denied-write-client")
            .connect_with(client_channel, async move |cx| {
                cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;

                cx.build_session(&session_cwd)
                    .block_task()
                    .run_until(async move |mut session| {
                        session.send_prompt("please overwrite guarded.txt")?;
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
                            "a denied write must surface as a FAILED tool_call_update, got: {updates:?}"
                        );
                        Ok(())
                    })
                    .await
            })
            .await;

        client_result.expect("client exchange should succeed");
        agent_task.abort();

        let post_call_bytes = std::fs::read(&target).expect("read post-call bytes");
        assert_eq!(
            pre_call_bytes, post_call_bytes,
            "a denied write must leave the target file byte-identical to its pre-call contents"
        );
    }

    /// A write to a path OUTSIDE the session cwd is classified as a dangerous operation
    /// and raises the dangerous-op permission request, with the outside-workspace reason
    /// stated in the description.
    #[tokio::test]
    async fn write_outside_cwd_raises_dangerous_op_permission_request_with_outside_reason() {
        let session_dir = tempfile::tempdir().expect("tempdir for session cwd");
        let outside_dir = tempfile::tempdir().expect("tempdir outside the session cwd");
        let outside_target = outside_dir.path().join("escape.txt");

        let server = MockServer::start().await;
        // An absolute path escaping the session cwd entirely.
        let args = serde_json::json!({
            "path": outside_target.to_str().unwrap(),
            "content": "escaped write",
        });
        mount_tool_call_then_final_text(
            &server,
            sse_tool_call("write_file", "call_write_outside", args),
        )
        .await;

        let (config, resolver) = build_config_and_resolver_pointed_at(&server.uri());
        let (state_store, _state_tmp) = build_state_store();
        let (agent_channel, client_channel) = Channel::duplex();

        let agent_task = tokio::spawn(async move {
            ironhermes_acp::entry::run_acp_over(config, resolver, state_store, agent_channel).await
        });

        let session_cwd = session_dir.path().to_path_buf();
        let captured_descriptions: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_for_client = captured_descriptions.clone();

        let client_result = Client
            .builder()
            .name("workspace-boundary-outside-write-client")
            .connect_with(client_channel, async move |cx| {
                cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;

                cx.build_session(&session_cwd)
                    .block_task()
                    .run_until(async move |mut session| {
                        session.send_prompt("please write escape.txt outside the project")?;

                        use agent_client_protocol::util::MatchDispatch;
                        let permission_requests_seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                        loop {
                            let message = session.read_update().await.expect("channel open");
                            match message {
                                agent_client_protocol::SessionMessage::SessionMessage(dispatch) => {
                                    let captured = captured_for_client.clone();
                                    let seen = permission_requests_seen.clone();
                                    MatchDispatch::new(dispatch)
                                        .if_notification(
                                            async move |_notif: agent_client_protocol::schema::v1::SessionNotification| {
                                                Ok(())
                                            },
                                        )
                                        .await
                                        .if_request(
                                            async move |req: RequestPermissionRequest,
                                                         responder: Responder<RequestPermissionResponse>| {
                                                seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                                                for content in &req.tool_call.fields.content.clone().unwrap_or_default() {
                                                    if let agent_client_protocol::schema::v1::ToolCallContent::Content(c) = content
                                                        && let agent_client_protocol::schema::v1::ContentBlock::Text(t) = &c.content
                                                    {
                                                        captured.lock().unwrap().push(t.text.clone());
                                                    }
                                                }
                                                let outcome = RequestPermissionOutcome::Selected(
                                                    SelectedPermissionOutcome::new("reject-once"),
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

                        assert!(
                            permission_requests_seen.load(std::sync::atomic::Ordering::SeqCst) >= 1,
                            "a write outside the session cwd must raise a permission request"
                        );
                        Ok(())
                    })
                    .await
            })
            .await;

        client_result.expect("client exchange should succeed");
        agent_task.abort();

        let descriptions = captured_descriptions.lock().unwrap();
        assert!(
            descriptions.iter().any(|d| d.to_lowercase().contains("outside")
                || d.to_lowercase().contains("dangerous")),
            "the permission request's description must state the outside-workspace reason, \
             got: {descriptions:?}"
        );
        assert!(
            !outside_target.exists(),
            "the rejected write must never have happened"
        );
    }

    // ── Plan 07 (CR-01): approved writes on turn 2+ of the SAME session ────────────────

    /// CR-01 regression (36.8-VERIFICATION.md): an approved `write_file` on the SECOND
    /// turn of a session must still reach disk. Before the fix, `handlers.rs` re-derived
    /// the tool `Arc` from the registry each turn, but `run_turn`'s WR-05 cleanup
    /// permanently evicts intercepted names from the registry at the end of every turn —
    /// so turn 1's write succeeds (the Arc is still present at the START of turn 1) while
    /// turn 2's `get_arc("write_file")` returns `None` and the approved write silently
    /// fails with "tool not registered on this runtime".
    #[tokio::test]
    async fn write_file_on_a_second_turn_of_the_same_session_still_reaches_disk_after_approval() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let first_target = tmp.path().join("first.txt");
        let second_target = tmp.path().join("second.txt");

        let server = MockServer::start().await;
        let turn1_args = serde_json::json!({
            "path": first_target.to_str().unwrap(),
            "content": "first turn content",
        });
        let turn2_args = serde_json::json!({
            "path": second_target.to_str().unwrap(),
            "content": "second turn content",
        });
        mount_two_turns_of_tool_call_then_final_text(
            &server,
            sse_tool_call("write_file", "call_write_turn_1", turn1_args),
            sse_tool_call("write_file", "call_write_turn_2", turn2_args),
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
            .name("workspace-boundary-two-turn-write-client")
            .connect_with(client_channel, async move |cx| {
                cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;

                cx.build_session(&session_cwd)
                    .block_task()
                    .run_until(async move |mut session| {
                        // Turn 1: approved write to first.txt.
                        session.send_prompt("please write first.txt")?;
                        let (updates_1, seen_1) = drain_answering_permission_requests(
                            &mut session,
                            PermissionAnswer::AllowOnce,
                        )
                        .await;
                        assert!(
                            seen_1 >= 1,
                            "turn 1's write_file must raise a permission request (D-16)"
                        );
                        assert!(
                            updates_1.iter().any(|u| matches!(
                                u,
                                SessionUpdate::ToolCallUpdate(update)
                                    if matches!(update.fields.status, Some(ToolCallStatus::Completed))
                            )),
                            "turn 1's approved write must complete, got: {updates_1:?}"
                        );

                        // Turn 2: SAME session, approved write to second.txt — this is the
                        // CR-01 regression surface.
                        session.send_prompt("please write second.txt")?;
                        let (updates_2, seen_2) = drain_answering_permission_requests(
                            &mut session,
                            PermissionAnswer::AllowOnce,
                        )
                        .await;
                        assert!(
                            seen_2 >= 1,
                            "turn 2's write_file must ALSO raise a permission request (D-16) \
                             — approval gating must not be skipped on turn 2"
                        );
                        assert!(
                            updates_2.iter().any(|u| matches!(
                                u,
                                SessionUpdate::ToolCallUpdate(update)
                                    if matches!(update.fields.status, Some(ToolCallStatus::Completed))
                            )),
                            "CR-01 regression: an approved write_file on the SECOND turn of \
                             the same ACP session must complete, got: {updates_2:?}"
                        );

                        Ok(())
                    })
                    .await
            })
            .await;

        client_result.expect("client exchange should succeed");
        agent_task.abort();

        let first_content =
            std::fs::read_to_string(&first_target).expect("read turn 1's on-disk content");
        assert_eq!(first_content, "first turn content");

        let second_content = std::fs::read_to_string(&second_target).unwrap_or_else(|e| {
            panic!(
                "CR-01 regression: second.txt (written on turn 2 of the same session) is \
                 missing or unreadable after an APPROVED write_file — error: {e}"
            )
        });
        assert_eq!(
            second_content, "second turn content",
            "CR-01 regression: second.txt's content does not match the approved turn-2 write"
        );
    }

    /// CR-01 regression, `patch` variant: an approved `patch` on the SECOND turn of a
    /// session must still rewrite the target file on disk.
    #[tokio::test]
    async fn patch_on_a_second_turn_of_the_same_session_still_reaches_disk_after_approval() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let seed_target = tmp.path().join("seed.txt");
        let patched_target = tmp.path().join("patched.txt");
        std::fs::write(&patched_target, "alpha beta gamma").expect("seed patched.txt");

        let server = MockServer::start().await;
        let turn1_args = serde_json::json!({
            "path": seed_target.to_str().unwrap(),
            "content": "seed content",
        });
        let turn2_args = serde_json::json!({
            "path": patched_target.to_str().unwrap(),
            "before": "beta",
            "after": "delta",
        });
        mount_two_turns_of_tool_call_then_final_text(
            &server,
            sse_tool_call("write_file", "call_write_seed", turn1_args),
            sse_tool_call("patch", "call_patch_turn_2", turn2_args),
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
            .name("workspace-boundary-two-turn-patch-client")
            .connect_with(client_channel, async move |cx| {
                cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;

                cx.build_session(&session_cwd)
                    .block_task()
                    .run_until(async move |mut session| {
                        // Turn 1: approved write to seed.txt.
                        session.send_prompt("please write seed.txt")?;
                        let (updates_1, seen_1) = drain_answering_permission_requests(
                            &mut session,
                            PermissionAnswer::AllowOnce,
                        )
                        .await;
                        assert!(
                            seen_1 >= 1,
                            "turn 1's write_file must raise a permission request (D-16)"
                        );
                        assert!(
                            updates_1.iter().any(|u| matches!(
                                u,
                                SessionUpdate::ToolCallUpdate(update)
                                    if matches!(update.fields.status, Some(ToolCallStatus::Completed))
                            )),
                            "turn 1's approved write must complete, got: {updates_1:?}"
                        );

                        // Turn 2: SAME session, approved patch against the pre-seeded file
                        // — this is the CR-01 regression surface for `patch`.
                        session.send_prompt("please patch patched.txt")?;
                        let (updates_2, seen_2) = drain_answering_permission_requests(
                            &mut session,
                            PermissionAnswer::AllowOnce,
                        )
                        .await;
                        assert!(
                            seen_2 >= 1,
                            "turn 2's patch must ALSO raise a permission request (D-16) — \
                             approval gating must not be skipped on turn 2"
                        );
                        assert!(
                            updates_2.iter().any(|u| matches!(
                                u,
                                SessionUpdate::ToolCallUpdate(update)
                                    if matches!(update.fields.status, Some(ToolCallStatus::Completed))
                            )),
                            "CR-01 regression: an approved patch on the SECOND turn of the \
                             same ACP session must complete, got: {updates_2:?}"
                        );

                        Ok(())
                    })
                    .await
            })
            .await;

        client_result.expect("client exchange should succeed");
        agent_task.abort();

        let patched_content = std::fs::read_to_string(&patched_target).unwrap_or_else(|e| {
            panic!(
                "CR-01 regression: patched.txt is missing or unreadable after an APPROVED \
                 turn-2 patch — error: {e}"
            )
        });
        assert_eq!(
            patched_content, "alpha delta gamma",
            "CR-01 regression: patched.txt's content does not show the approved turn-2 patch"
        );
    }

    /// Fail-closed guard for T-36.8-28: a DENIED write on the second turn of a session
    /// still raises the permission request, still surfaces as a failed tool call, and
    /// still leaves the target absent from disk. This must pass BOTH before and after the
    /// CR-01 fix — the multi-turn fix must not weaken the fail-closed gate.
    #[tokio::test]
    async fn denied_write_on_a_second_turn_still_leaves_the_target_absent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let approved_first = tmp.path().join("approved_first.txt");
        let rejected_second = tmp.path().join("rejected_second.txt");

        let server = MockServer::start().await;
        let turn1_args = serde_json::json!({
            "path": approved_first.to_str().unwrap(),
            "content": "approved turn 1 content",
        });
        let turn2_args = serde_json::json!({
            "path": rejected_second.to_str().unwrap(),
            "content": "should never reach disk",
        });
        mount_two_turns_of_tool_call_then_final_text(
            &server,
            sse_tool_call("write_file", "call_write_turn_1_approved", turn1_args),
            sse_tool_call("write_file", "call_write_turn_2_denied", turn2_args),
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
            .name("workspace-boundary-two-turn-denied-client")
            .connect_with(client_channel, async move |cx| {
                cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;

                cx.build_session(&session_cwd)
                    .block_task()
                    .run_until(async move |mut session| {
                        // Turn 1: approved.
                        session.send_prompt("please write approved_first.txt")?;
                        let (_updates_1, seen_1) = drain_answering_permission_requests(
                            &mut session,
                            PermissionAnswer::AllowOnce,
                        )
                        .await;
                        assert!(seen_1 >= 1);

                        // Turn 2: SAME session, denied.
                        session.send_prompt("please write rejected_second.txt")?;
                        let (updates_2, seen_2) = drain_answering_permission_requests(
                            &mut session,
                            PermissionAnswer::RejectOnce,
                        )
                        .await;
                        assert!(
                            seen_2 >= 1,
                            "turn 2's write_file must raise a permission request even when \
                             it will be denied"
                        );
                        assert!(
                            updates_2.iter().any(|u| matches!(
                                u,
                                SessionUpdate::ToolCallUpdate(update)
                                    if matches!(update.fields.status, Some(ToolCallStatus::Failed))
                            )),
                            "a denied turn-2 write must surface as a FAILED tool_call_update, \
                             got: {updates_2:?}"
                        );

                        Ok(())
                    })
                    .await
            })
            .await;

        client_result.expect("client exchange should succeed");
        agent_task.abort();

        assert!(
            !rejected_second.exists(),
            "a denied turn-2 write must never reach disk"
        );
    }
}

/// Source-level guard (Plan 07, CR-01): `handlers.rs` must capture the `write_file`/
/// `patch` tool Arcs ONCE at `AgentRuntime` construction (via the `write_file_tool_arc()`/
/// `patch_tool_arc()` accessors) and never re-derive them from the registry per turn. A
/// per-turn registry lookup is exactly the CR-01 defect: `register_intercepted_or_replace`
/// moves the name out of `tools` on first use, and `run_turn`'s WR-05 end-of-turn sweep
/// then drops it from `intercepts` too, so a re-derived lookup on turn 2+ finds nothing.
#[test]
fn negative_check_no_per_turn_registry_arc_lookup_in_handlers() {
    let source = include_str!("../src/handlers.rs");
    let stripped: String = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !stripped.contains("get_arc"),
        "CR-01 regression guard: handlers.rs must not re-derive the write_file/patch tool \
         Arcs via a per-turn registry get_arc lookup — use AgentRuntime's cached \
         write_file_tool_arc()/patch_tool_arc() accessors instead"
    );
    assert!(
        stripped.contains("write_file_tool_arc()"),
        "CR-01 regression guard: handlers.rs must consume AgentRuntime::write_file_tool_arc()"
    );
    assert!(
        stripped.contains("patch_tool_arc()"),
        "CR-01 regression guard: handlers.rs must consume AgentRuntime::patch_tool_arc()"
    );
}

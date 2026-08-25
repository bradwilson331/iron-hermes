//! Buzz-acp dialect conformance suite (Phase 47.7, plan 01).
//!
//! Drives the real `run_acp_over` server over the byte-oriented NDJSON harness in
//! `tests/common/mod.rs`, reproducing `buzz-acp`'s hand-rolled request shapes verbatim (D-05:
//! read verified this session against `~/code/buzz/crates/buzz-acp/src/acp.rs`, read-only
//! reference) rather than round-tripping through `agent_client_protocol`'s own typed request
//! constructors — the point is to prove the FOREIGN dialect works, not the SDK's own
//! serialization of itself.
//!
//! Task 1 (Tests 1-5) proves the core initialize / set_config_option / session_new /
//! session_prompt / session_cancel exchange round-trips with zero protocol errors and zero
//! non-JSON output. Task 2 adds the permission-options wire-compatibility lock-in.

mod common;

use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome,
};
use async_trait::async_trait;
use common::{
    DUMMY_API_KEY, UNROUTABLE_BASE_URL, build_config_and_resolver, build_state_store,
    spawn_agent_over_ndjson,
};
use ironhermes_acp::approval_bridge::{AcpApprovalGate, PermissionRequestSender, offered_permission_options};
use ironhermes_core::{ApprovalGate, ApprovalOutcome, ApprovalsStore};

/// The shared harness must never resolve to a reachable provider or to a credential taken
/// from the process environment. Every test in this file (and in `buzz_pool_reuse.rs` /
/// `buzz_fixture_replay.rs`) drives a real `session/prompt` through
/// `build_config_and_resolver`, so a regression here would send the test's prompt text and
/// whatever `OPENROUTER_API_KEY`/`ANTHROPIC_API_KEY`/`OPENAI_API_KEY` happens to be exported
/// on the developer's box to a third party from `cargo test`.
///
/// Asserting on the RESOLVED endpoint (not on the `Config` we handed in) is the point: the
/// resolver is where the env-var fallback lives, so this fails if the fallback is ever
/// reachable again — including on a machine that has a real key exported right now.
#[test]
fn harness_resolver_never_resolves_a_reachable_provider_or_ambient_credential() {
    let (_config, resolver) = build_config_and_resolver();
    let endpoint = resolver.resolve_for_main();

    assert_eq!(
        endpoint.base_url, UNROUTABLE_BASE_URL,
        "the harness's main provider must stay pinned to an unroutable loopback endpoint"
    );
    assert_eq!(
        endpoint.api_key.as_deref(),
        Some(DUMMY_API_KEY),
        "the harness's main provider must carry the literal dummy key — any other value means \
         a real credential was picked up from the process environment"
    );
}

/// `buzz-acp`'s own `build_initialize_params()` (`acp.rs:124-133`, read-only reference,
/// D-05) — reproduced verbatim, including the bare-integer `protocolVersion: 2` (buzz-acp is
/// intentionally squatting on ACP v2 ahead of the upstream RFD) and the `_meta`/`auth`
/// extension fields real buzz-acp sends.
fn buzz_initialize_params() -> serde_json::Value {
    serde_json::json!({
        "protocolVersion": 2,
        "clientCapabilities": {
            "auth": { "terminal": true },
            "_meta": {
                "goose": { "customNotifications": true },
                "terminal-auth": true
            }
        },
        "clientInfo": { "name": "buzz-acp", "version": "0.1.0" },
    })
}

/// `buzz-acp`'s own `session/new` params shape (`session_new_full`, `acp.rs:638-663`) with no
/// system prompt / session title `_meta` — the minimal-fields case every session starts with.
fn buzz_session_new_params(cwd: &str) -> serde_json::Value {
    serde_json::json!({
        "cwd": cwd,
        "mcpServers": [],
    })
}

/// `buzz-acp`'s own `build_prompt_params()` (`acp.rs:1938-1946`, read-only reference).
fn buzz_prompt_params(session_id: &str, text: &str) -> serde_json::Value {
    serde_json::json!({
        "sessionId": session_id,
        "prompt": [{ "type": "text", "text": text }],
    })
}

/// `buzz-acp`'s own `session_set_config_option` params shape (`acp.rs:711-723`), as sent by
/// `AgentPool::apply_permission_mode` right after `session/new`
/// (`pool.rs:1157-1200`, RESEARCH Pattern 2) — `configId: "mode"`, `value: "dontAsk"`.
fn buzz_set_config_option_params(session_id: &str) -> serde_json::Value {
    serde_json::json!({
        "sessionId": session_id,
        "configId": "mode",
        "value": "dontAsk",
    })
}

/// Test 1: an `initialize` request whose `params.protocolVersion` is the bare integer `2`
/// (NOT a string, NOT an object) — buzz-acp's literal wire shape — gets a success response
/// carrying agent capabilities, not a JSON-RPC error (D-05).
#[tokio::test]
async fn bare_integer_protocol_version_two_is_accepted() {
    let (config, resolver) = build_config_and_resolver();
    let (state_store, _state_tmp) = build_state_store();
    let (agent_task, mut client) = spawn_agent_over_ndjson(config, resolver, state_store);

    let response = client.request("initialize", buzz_initialize_params()).await;

    assert!(
        response.get("error").is_none(),
        "initialize with bare-integer protocolVersion: 2 should not error, got: {response}"
    );
    let result = response
        .get("result")
        .unwrap_or_else(|| panic!("initialize should return a result, got: {response}"));
    assert!(
        result.get("agentCapabilities").is_some_and(serde_json::Value::is_object),
        "initialize response should carry agent capabilities, got: {response}"
    );

    drop(client);
    agent_task.abort();
}

/// Test 2: `session/set_config_option` with buzz-acp's `{sessionId, configId: "mode", value:
/// "dontAsk"}`-shaped params returns an error object with `code == -32601` (RESEARCH Pattern
/// 2 — IronHermes has zero references to this method, so it falls through to the catch-all),
/// AND a subsequent `session/new` on the SAME connection still succeeds (D-05, COVERAGE.md
/// OPT-OUT row).
#[tokio::test]
async fn set_config_option_returns_method_not_found_and_connection_survives() {
    let (config, resolver) = build_config_and_resolver();
    let (state_store, _state_tmp) = build_state_store();
    let (agent_task, mut client) = spawn_agent_over_ndjson(config, resolver, state_store);

    client.request("initialize", buzz_initialize_params()).await;

    let cwd = tempfile::tempdir().expect("tempdir for session cwd");
    let cwd_path = cwd.path().to_string_lossy().to_string();
    let first_session = client
        .request("session/new", buzz_session_new_params(&cwd_path))
        .await;
    let session_id = first_session["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("session/new should return a sessionId, got: {first_session}"))
        .to_string();

    let config_response = client
        .request("session/set_config_option", buzz_set_config_option_params(&session_id))
        .await;
    let error = config_response
        .get("error")
        .unwrap_or_else(|| panic!("session/set_config_option should error, got: {config_response}"));
    assert_eq!(
        error.get("code").and_then(serde_json::Value::as_i64),
        Some(-32601),
        "unimplemented session/set_config_option must return Method not found, got: {error}"
    );

    // The connection must still be usable after the unknown method: a real session/new right
    // after it must still succeed.
    let second_session = client
        .request("session/new", buzz_session_new_params(&cwd_path))
        .await;
    assert!(
        second_session.get("error").is_none(),
        "session/new after set_config_option's -32601 should still succeed, got: {second_session}"
    );
    assert!(
        second_session["result"]["sessionId"].as_str().is_some_and(|s| !s.is_empty()),
        "session/new should still allocate a session id, got: {second_session}"
    );

    drop(client);
    agent_task.abort();
}

/// Test 3: `initialize` -> `session/new` -> `session/prompt` produces a well-formed response
/// frame (either a result or a JSON-RPC error object — CI has no provider configured, so the
/// turn's *content* is not asserted; the *framing* is).
#[tokio::test]
async fn full_buzz_turn_round_trip() {
    let (config, resolver) = build_config_and_resolver();
    let (state_store, _state_tmp) = build_state_store();
    let (agent_task, mut client) = spawn_agent_over_ndjson(config, resolver, state_store);

    client.request("initialize", buzz_initialize_params()).await;

    let cwd = tempfile::tempdir().expect("tempdir for session cwd");
    let cwd_path = cwd.path().to_string_lossy().to_string();
    let session_response = client
        .request("session/new", buzz_session_new_params(&cwd_path))
        .await;
    let session_id = session_response["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("session/new should return a sessionId, got: {session_response}"))
        .to_string();

    let prompt_response = client
        .request(
            "session/prompt",
            buzz_prompt_params(&session_id, "hello from the buzz dialect test"),
        )
        .await;

    // A well-formed JSON-RPC response frame carries exactly one of `result`/`error` — never
    // neither, never both. CI has no provider configured, so `run_turn` failing and surfacing
    // as a JSON-RPC error response is an acceptable outcome; the assertion is on framing.
    let has_result = prompt_response.get("result").is_some();
    let has_error = prompt_response.get("error").is_some();
    assert!(
        has_result ^ has_error,
        "session/prompt response must carry exactly one of result/error, got: {prompt_response}"
    );

    drop(client);
    agent_task.abort();
}

/// Test 4: across the whole Test 3 exchange, every non-empty line the agent emitted parses as
/// JSON with `"jsonrpc":"2.0"`. Zero lines fail to parse (D-05 stdout discipline — every byte
/// the agent writes to its output stream is a single-line JSON-RPC frame).
#[tokio::test]
async fn every_agent_output_line_is_a_single_json_rpc_frame() {
    let (config, resolver) = build_config_and_resolver();
    let (state_store, _state_tmp) = build_state_store();
    let (agent_task, mut client) = spawn_agent_over_ndjson(config, resolver, state_store);

    client.request("initialize", buzz_initialize_params()).await;

    let cwd = tempfile::tempdir().expect("tempdir for session cwd");
    let cwd_path = cwd.path().to_string_lossy().to_string();
    let session_response = client
        .request("session/new", buzz_session_new_params(&cwd_path))
        .await;
    let session_id = session_response["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("session/new should return a sessionId, got: {session_response}"))
        .to_string();

    client
        .request(
            "session/prompt",
            buzz_prompt_params(&session_id, "hello from the buzz dialect test"),
        )
        .await;
    // Drain anything still trickling in (e.g. a late session/update notification racing the
    // prompt response) with a short bounded wait, so the raw-line assertion below covers the
    // WHOLE exchange, not just whatever `request()` happened to consume while filtering for a
    // specific id.
    let _ = client.drain_frames(Duration::from_millis(200)).await;

    let lines = client.raw_lines();
    assert!(!lines.is_empty(), "expected at least one output line from the agent");
    for line in lines {
        let value: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("line failed to parse as JSON: {line:?}: {e}"));
        assert_eq!(
            value.get("jsonrpc").and_then(serde_json::Value::as_str),
            Some("2.0"),
            "every agent output line must be a single well-formed JSON-RPC 2.0 frame, got: {line}"
        );
    }

    drop(client);
    agent_task.abort();
}

/// Test 5: a `session/cancel` notification (no `id`) sent after `session/new` draws no
/// response frame of its own, and a following request on the same connection still gets its
/// matching id back (D-06 floor — `session/cancel` must not desync the connection).
#[tokio::test]
async fn cancel_notification_does_not_desync() {
    let (config, resolver) = build_config_and_resolver();
    let (state_store, _state_tmp) = build_state_store();
    let (agent_task, mut client) = spawn_agent_over_ndjson(config, resolver, state_store);

    client.request("initialize", buzz_initialize_params()).await;

    let cwd = tempfile::tempdir().expect("tempdir for session cwd");
    let cwd_path = cwd.path().to_string_lossy().to_string();
    let session_response = client
        .request("session/new", buzz_session_new_params(&cwd_path))
        .await;
    let session_id = session_response["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("session/new should return a sessionId, got: {session_response}"))
        .to_string();

    // session/cancel is a notification — no id, no response expected.
    client
        .notify("session/cancel", serde_json::json!({ "sessionId": session_id }))
        .await;

    // No response frame of its own: bounded wait, assert nothing arrives.
    let stray = client.drain_frames(Duration::from_millis(300)).await;
    assert!(
        stray.is_empty(),
        "session/cancel notification must not produce a response frame of its own, got: {stray:?}"
    );

    // The connection must still be usable: a fresh request on the same connection completes
    // (not hanging, not EOF-ing) and gets a well-formed response back — proving `request()`'s
    // id-correlation loop still finds its match, i.e. the connection did not desync.
    let response = client
        .request("session/new", buzz_session_new_params(&cwd_path))
        .await;
    assert!(
        response.get("error").is_none(),
        "session/new after session/cancel should still succeed, got: {response}"
    );

    drop(client);
    agent_task.abort();
}

// ── Task 2: permission round-trip proven against buzz-acp's auto-deny behavior ────────────

/// A fake `PermissionRequestSender` that replies exactly the way buzz-acp's
/// `handle_permission_request` (`~/code/buzz/crates/buzz-acp/src/acp.rs:1883-1925`, read-only
/// reference) does: it searches the offered `options` for one whose `kind` is the literal
/// `reject_once` and selects it — mirroring buzz-acp's unconditional auto-denial, not a live
/// human decision.
struct BuzzStyleAutoDenySender;

#[async_trait]
impl PermissionRequestSender for BuzzStyleAutoDenySender {
    async fn send_permission_request(
        &self,
        request: RequestPermissionRequest,
    ) -> Result<RequestPermissionResponse, agent_client_protocol::Error> {
        let reject_once_id: Option<String> = request
            .options
            .iter()
            .find(|opt| opt.kind == PermissionOptionKind::RejectOnce)
            .map(|opt| opt.option_id.0.to_string());
        let outcome = match reject_once_id {
            Some(id) => RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id)),
            None => RequestPermissionOutcome::Cancelled,
        };
        Ok(RequestPermissionResponse::new(outcome))
    }
}

/// A fake sender scripted to buzz-acp's own no-matching-option fallback
/// (`permission_denial_response`, read-only reference): it never finds a `reject_once`
/// option (because it never looks at what was offered) and always returns the protocol's
/// `Cancelled` outcome — the same outcome buzz-acp returns when an adapter offers no
/// `reject_once` option at all.
struct AlwaysCancelledSender;

#[async_trait]
impl PermissionRequestSender for AlwaysCancelledSender {
    async fn send_permission_request(
        &self,
        _request: RequestPermissionRequest,
    ) -> Result<RequestPermissionResponse, agent_client_protocol::Error> {
        Ok(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled))
    }
}

fn fresh_approvals_store() -> Arc<ApprovalsStore> {
    let tmp = tempfile::tempdir().expect("tempdir for approvals.json");
    Arc::new(ApprovalsStore::with_path(tmp.path().join("approvals.json")))
}

/// The `PermissionOption` array `AcpApprovalGate` actually offers contains an element whose
/// `kind` field is the literal snake_case string `reject_once` on the wire. Asserted on the
/// SERIALIZED JSON string form, not the Rust enum variant — buzz-acp matches the wire
/// literal, so a serde rename would break Buzz while a variant-level assertion stayed green.
#[test]
fn offered_permission_options_contain_a_reject_once_kind() {
    let options = offered_permission_options();
    let json = serde_json::to_value(&options).expect("permission options serialize");
    let json_str = json.to_string();
    assert!(
        json_str.contains("\"kind\":\"reject_once\""),
        "offered permission options must contain a reject_once kind on the wire, got: {json_str}"
    );
}

/// Driving `AcpApprovalGate` through a fake that replies exactly the way buzz-acp's
/// `permission_denial_response` does resolves to `Denied` — both when a `reject_once` option
/// is selected AND when the fake returns the protocol's `cancelled` outcome (buzz-acp's
/// no-matching-option fallback). Never `Approved` on either path (D-09 fail-closed).
#[tokio::test]
async fn buzz_style_auto_denial_resolves_to_denied() {
    let gate = AcpApprovalGate::new(
        Arc::new(BuzzStyleAutoDenySender),
        "sess-buzz-1",
        fresh_approvals_store(),
        true,
        Duration::from_secs(30),
    );
    let outcome = gate
        .request_approval("sess-buzz-1", "terminal", "run a script", &serde_json::json!({}))
        .await;
    assert_eq!(
        outcome,
        ApprovalOutcome::Denied,
        "buzz-acp's reject_once selection must resolve to Denied"
    );

    let gate_cancelled = AcpApprovalGate::new(
        Arc::new(AlwaysCancelledSender),
        "sess-buzz-2",
        fresh_approvals_store(),
        true,
        Duration::from_secs(30),
    );
    let outcome_cancelled = gate_cancelled
        .request_approval("sess-buzz-2", "terminal", "run a script", &serde_json::json!({}))
        .await;
    assert_eq!(
        outcome_cancelled,
        ApprovalOutcome::Denied,
        "buzz-acp's no-matching-option cancelled fallback must ALSO resolve to Denied, never \
         Approved (D-09)"
    );
}

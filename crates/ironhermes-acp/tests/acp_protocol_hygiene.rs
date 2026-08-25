//! Protocol hygiene tests (Phase 36.8, plan 01 task 3): probe tolerance, malformed-input
//! resilience, and a source-level invariant guarding D-07 stdout purity for the rest of
//! the phase.
//!
//! Tests 1-3 talk to the real `run_acp_over` server over a raw `agent_client_protocol::Channel`
//! duplex, bypassing the SDK's typed `Client` role helpers so a genuinely unknown method or a
//! structurally malformed `session/prompt` can be sent — exactly what a hand-rolled ACP client
//! (buzz-acp-style) or a broken editor extension might do.

use std::path::Path;
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{InitializeRequest, RequestId, Response as AcpResponse};
use agent_client_protocol::{Channel, RawJsonRpcMessage, TransportFrame};
use futures::StreamExt;
use ironhermes_core::{Config, ModelsCache, ProviderConfig, ProviderResolver};
use ironhermes_state::StateStore;
use std::sync::{Arc, Mutex as StdMutex};

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

/// Phase 36.8 plan 02 task 1: an isolated, tempdir-backed `StateStore` — see the
/// identical helper in `acp_e2e.rs` for rationale (never touch the real state.db).
fn build_state_store() -> (Arc<StdMutex<StateStore>>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir for state.db");
    let store = StateStore::new(tmp.path().join("state.db")).expect("StateStore::new");
    (Arc::new(StdMutex::new(store)), tmp)
}

/// Send a raw JSON-RPC request frame and wait (bounded) for the matching response frame.
async fn send_request_and_await_response(
    tx: &futures::channel::mpsc::UnboundedSender<TransportFrame>,
    rx: &mut futures::channel::mpsc::UnboundedReceiver<TransportFrame>,
    method: &str,
    params: serde_json::Value,
    id: RequestId,
) -> AcpResponse<serde_json::Value> {
    let message = RawJsonRpcMessage::request(method.to_string(), params, id)
        .expect("valid raw JSON-RPC request");
    tx.unbounded_send(TransportFrame::Single(message))
        .expect("channel should accept the frame");

    let frame = tokio::time::timeout(Duration::from_secs(5), rx.next())
        .await
        .unwrap_or_else(|_| panic!("expected a response to `{method}` within 5s (not a hang)"))
        .unwrap_or_else(|| panic!("channel closed before responding to `{method}`"));

    match frame {
        TransportFrame::Single(RawJsonRpcMessage::Response(response)) => response,
        other => panic!("expected a JSON-RPC response frame for `{method}`, got: {other:?}"),
    }
}

/// (1) Unknown-method probe: `ping` (with an id) must get a `-32601` error response, and the
/// connection must keep serving afterward — a subsequent, valid `initialize` on the SAME
/// connection must still succeed.
#[tokio::test]
async fn unknown_method_probe_gets_method_not_found_and_connection_stays_usable() {
    let (config, resolver) = build_config_and_resolver();
    let (state_store, _state_tmp) = build_state_store();
    let (agent_channel, client_channel) = Channel::duplex();
    let agent_task = tokio::spawn(async move {
        ironhermes_acp::entry::run_acp_over(config, resolver, state_store, agent_channel).await
    });
    let agent_client_protocol::Channel { mut rx, tx } = client_channel;

    let ping_id = RequestId::Number(1);
    let response = send_request_and_await_response(
        &tx,
        &mut rx,
        "ping",
        serde_json::json!({}),
        ping_id.clone(),
    )
    .await;
    match response {
        AcpResponse::Error { id, error } => {
            assert_eq!(id, ping_id, "response id must correlate to the ping request");
            assert_eq!(
                i32::from(error.code),
                -32601,
                "unknown method `ping` must get Method not found"
            );
        }
        AcpResponse::Result { .. } => panic!("`ping` unexpectedly succeeded"),
    }

    // The connection must still be usable: a real `initialize` request right after the
    // unknown-method probe must succeed, proving the server kept serving.
    let init_id = RequestId::Number(2);
    let init_params = serde_json::to_value(InitializeRequest::new(ProtocolVersion::V1))
        .expect("InitializeRequest serializes");
    let response =
        send_request_and_await_response(&tx, &mut rx, "initialize", init_params, init_id.clone())
            .await;
    match response {
        AcpResponse::Result { id, .. } => {
            assert_eq!(id, init_id, "initialize response id must correlate");
        }
        AcpResponse::Error { error, .. } => {
            panic!("initialize should still succeed after the unknown-method probe: {error:?}")
        }
    }

    drop(tx);
    agent_task.abort();
}

/// (2) Malformed params: `session/prompt` with a params object missing required fields, and
/// separately with a wrong-typed field, must both produce a JSON-RPC error response — never a
/// panic, never a dropped/hung connection. Both malformed sends run over the SAME connection
/// to prove neither one corrupts it for the other.
#[tokio::test]
async fn malformed_session_prompt_params_get_error_response_not_panic() {
    let (config, resolver) = build_config_and_resolver();
    let (state_store, _state_tmp) = build_state_store();
    let (agent_channel, client_channel) = Channel::duplex();
    let agent_task = tokio::spawn(async move {
        ironhermes_acp::entry::run_acp_over(config, resolver, state_store, agent_channel).await
    });
    let agent_client_protocol::Channel { mut rx, tx } = client_channel;

    // Missing required fields entirely (no sessionId, no prompt).
    let missing_fields_id = RequestId::Number(1);
    let response = send_request_and_await_response(
        &tx,
        &mut rx,
        "session/prompt",
        serde_json::json!({}),
        missing_fields_id.clone(),
    )
    .await;
    match response {
        AcpResponse::Error { id, .. } => {
            assert_eq!(id, missing_fields_id);
        }
        AcpResponse::Result { .. } => {
            panic!("session/prompt with missing required fields should not succeed")
        }
    }

    // Present but wrong-typed field (sessionId as a number instead of a string).
    let wrong_type_id = RequestId::Number(2);
    let response = send_request_and_await_response(
        &tx,
        &mut rx,
        "session/prompt",
        serde_json::json!({ "sessionId": 12345, "prompt": [] }),
        wrong_type_id.clone(),
    )
    .await;
    match response {
        AcpResponse::Error { id, .. } => {
            assert_eq!(id, wrong_type_id);
        }
        AcpResponse::Result { .. } => {
            panic!("session/prompt with a wrong-typed sessionId should not succeed")
        }
    }

    drop(tx);
    agent_task.abort();
}

/// (3) Unknown notification (no `id`) for an unrecognized method must get NO response frame
/// at all within a bounded wait — responding to a notification would itself desync a
/// hand-rolled client per JSON-RPC 2.0.
#[tokio::test]
async fn unknown_notification_gets_no_response() {
    let (config, resolver) = build_config_and_resolver();
    let (state_store, _state_tmp) = build_state_store();
    let (agent_channel, client_channel) = Channel::duplex();
    let agent_task = tokio::spawn(async move {
        ironhermes_acp::entry::run_acp_over(config, resolver, state_store, agent_channel).await
    });
    let agent_client_protocol::Channel { mut rx, tx } = client_channel;

    let notification = RawJsonRpcMessage::notification("totally/unknown".to_string(), serde_json::json!({}))
        .expect("valid raw JSON-RPC notification");
    tx.unbounded_send(TransportFrame::Single(notification))
        .expect("channel should accept the frame");

    let outcome = tokio::time::timeout(Duration::from_millis(500), rx.next()).await;
    assert!(
        outcome.is_err(),
        "an unknown notification must produce no response frame within the bounded wait, got: {outcome:?}"
    );

    drop(tx);
    agent_task.abort();
}

/// (4) Source-level D-07 invariant: no `.rs` file under `crates/ironhermes-acp/src/` (other
/// than `entry.rs`, which legitimately owns the transport) writes to stdout. Strips `//`
/// comment lines before scanning, so a comment mentioning `println!` cannot trip this test —
/// and so it also fails if a LATER plan reintroduces a real stdout write.
#[test]
fn no_stdout_writes_outside_entry_rs() {
    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let forbidden = ["println!", "print!(", "std::io::stdout"];

    let mut offenders = Vec::new();
    for entry in walk_rs_files(&src_dir) {
        if entry.file_name().and_then(|n| n.to_str()) == Some("entry.rs") {
            continue; // entry.rs legitimately owns the transport's stdout handle.
        }

        let contents = std::fs::read_to_string(&entry)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", entry.display()));
        let code_only: String = contents
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        for needle in forbidden {
            if code_only.contains(needle) {
                offenders.push(format!("{}: contains `{needle}`", entry.display()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "found stdout writes outside entry.rs (D-07 violation):\n{}",
        offenders.join("\n")
    );
}

fn walk_rs_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("failed to read dir {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.expect("valid dir entry");
        let path = entry.path();
        if path.is_dir() {
            files.extend(walk_rs_files(&path));
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            files.push(path);
        }
    }
    files
}

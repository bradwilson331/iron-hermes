//! Phase 57 Plan 22 (G-1, D-07): the model's NEXT request carries the refusal text.
//!
//! Observed live in 57-16: after the operator denied `curl`, the agent fetched the same
//! data with `web_read`; after a sidecar-unavailable block it tried `execute_code` and then
//! `search_files`. The tool result never said who refused, and never said not to work
//! around the refusal.
//!
//! This drives the real path: a real `AgentRuntime::run_turn`, a real `AgentLoop`, a real
//! tool-dispatch path into a `terminal_intercept` that calls the real
//! `ironhermes_hooks::execute_gated_command` and returns `Ok(outcome.to_string())`, as the
//! gateway, web server and CLI do. Only the network is faked, by a `wiremock::MockServer`
//! serving canned SSE chat-completions (the seam `yolo_turn_e2e` uses).
//!
//! The assertion reads the REQUEST BODY the mock server received for the second LLM call:
//! that is the text the model actually sees.
//!
//! `AuditLog::load` reads `IRONHERMES_HOME`, so every test points it at its own `TempDir`
//! and serializes on [`ENV_LOCK`].

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use ironhermes_agent::{AgentRuntime, TurnRequest};
use ironhermes_core::{ApprovalGate, ApprovalOutcome, AuditConfig, AuditLog, ChatMessage};
use ironhermes_hooks::{
    DangerousCommandGuardrail, GatedOutcome, NO_CIRCUMVENTION_NOTICE, approval_refusal_text,
    execute_gated_command,
};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Serializes tests that mutate the process-global `IRONHERMES_HOME`.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Output the `run` closure would produce if (wrongly) invoked. Must never reach the model.
const RUN_SENTINEL: &str = "SENTINEL-COMMAND-OUTPUT-7731";

// ===========================================================================
// Canned chat-completions SSE fixtures
// ===========================================================================

fn sse(chunks: &[String]) -> String {
    let mut body = String::new();
    for c in chunks {
        body.push_str("data: ");
        body.push_str(c);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

/// A completion in which the model issues ONE `terminal` tool call.
fn sse_terminal_call(call_id: &str, command: &str) -> String {
    let arguments = serde_json::json!({ "command": command }).to_string();
    let call_chunk = serde_json::json!({
        "id": "c1",
        "object": "chat.completion.chunk",
        "created": 1,
        "model": "test-model",
        "choices": [{
            "index": 0,
            "delta": { "tool_calls": [{
                "index": 0,
                "id": call_id,
                "type": "function",
                "function": { "name": "terminal", "arguments": arguments },
            }]},
        }],
    })
    .to_string();
    let finish_chunk = r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"test-model","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#
        .to_string();
    sse(&[call_chunk, finish_chunk])
}

/// A plain-text completion that ends the turn.
fn sse_text(text: &str) -> String {
    sse(&[
        format!(
            r#"{{"id":"c2","object":"chat.completion.chunk","created":1,"model":"test-model","choices":[{{"index":0,"delta":{{"content":{}}}}}]}}"#,
            serde_json::to_string(text).unwrap()
        ),
        r#"{"id":"c2","object":"chat.completion.chunk","created":1,"model":"test-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#
            .to_string(),
    ])
}

fn sse_response(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_raw(body, "text/event-stream")
}

/// One `terminal` tool call, then the closing text completion for every later request.
async fn mount_one_tool_call_then_text(server: &MockServer, command: &str) {
    Mock::given(any())
        .respond_with(sse_response(sse_terminal_call("call-0", command)))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(server)
        .await;
    Mock::given(any())
        .respond_with(sse_response(sse_text("understood")))
        .with_priority(2)
        .mount(server)
        .await;
}

// ===========================================================================
// Gate and harness
// ===========================================================================

/// An operator who denies every request and counts how often they were asked.
#[derive(Default)]
struct DenyingGate {
    asked: AtomicU32,
}

#[async_trait]
impl ApprovalGate for DenyingGate {
    async fn request_approval(
        &self,
        _session_id: &str,
        _tool_name: &str,
        _reason: &str,
        _args: &serde_json::Value,
    ) -> ApprovalOutcome {
        self.asked.fetch_add(1, Ordering::SeqCst);
        ApprovalOutcome::Denied
    }

    async fn resolve(&self, _session_id: &str, _approved: bool) -> bool {
        false
    }
}

/// Per-test isolated `IRONHERMES_HOME` plus the env lock guard.
struct Harness {
    _home: tempfile::TempDir,
    audit_path: PathBuf,
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

impl Harness {
    async fn new() -> Self {
        let guard = ENV_LOCK.lock().await;
        let home = tempfile::tempdir().expect("tempdir");
        // SAFETY: serialized by ENV_LOCK; no other thread reads the var concurrently.
        unsafe { std::env::set_var("IRONHERMES_HOME", home.path()) };
        let audit_path = home.path().join("logs").join("audit.jsonl");
        Self {
            _home: home,
            audit_path,
            _guard: guard,
        }
    }

    /// Stream-safe read-back: `audit.jsonl` is NOT strictly line-delimited.
    fn audit_rows(&self) -> Vec<serde_json::Value> {
        let Ok(contents) = std::fs::read_to_string(&self.audit_path) else {
            return Vec::new();
        };
        serde_json::Deserializer::from_str(&contents)
            .into_iter::<serde_json::Value>()
            .map(|r| r.expect("audit.jsonl must parse as a stream of JSON values"))
            .collect()
    }

    fn shell_rows(&self) -> Vec<serde_json::Value> {
        self.audit_rows()
            .into_iter()
            .filter(|r| r["kind"] == "shell_command")
            .collect()
    }
}

/// A `terminal_intercept` that routes every call through the real `execute_gated_command`
/// at policy `false` (DEFCON 3: no bypass) and returns `Ok(outcome.to_string())`.
fn terminal_intercept(
    gate: Arc<DenyingGate>,
    session_id: String,
    ran: Arc<StdMutex<u32>>,
) -> ironhermes_tools::registry::InterceptHandler {
    Arc::new(move |args: serde_json::Value| {
        let gate = gate.clone();
        let session_id = session_id.clone();
        let ran = ran.clone();
        Box::pin(async move {
            let command = args
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let guard = DangerousCommandGuardrail::new();
            let audit_log = AuditLog::load(AuditConfig::default());
            let gate_ref: &dyn ApprovalGate = &*gate;
            let outcome: GatedOutcome = execute_gated_command(
                "terminal",
                &command,
                &guard,
                Some(gate_ref),
                &audit_log,
                &session_id,
                "test",
                "",
                false,
                false,
                false,
                || async move {
                    *ran.lock().unwrap() += 1;
                    Ok(RUN_SENTINEL.to_string())
                },
            )
            .await;
            Ok(outcome.to_string())
        })
    })
}

/// Run one turn whose model issues `command`, and return every request body the mock
/// server received, as text.
async fn run_turn_and_collect_bodies(
    command: &str,
    gate: &Arc<DenyingGate>,
    ran: &Arc<StdMutex<u32>>,
) -> Vec<String> {
    let server = MockServer::start().await;
    mount_one_tool_call_then_text(&server, command).await;

    let runtime = AgentRuntime::for_tests_with_base_url(server.uri());
    let session_id = format!("denial-e2e-{}", uuid::Uuid::new_v4());
    let req = TurnRequest {
        messages: vec![ChatMessage::user("run the command")],
        session_id: session_id.clone(),
        // A stream callback selects the SSE client path the canned fixtures feed.
        stream: Some(Box::new(|_delta: &str| {})),
        turn_id: Some(uuid::Uuid::new_v4()),
        terminal_intercept: Some(terminal_intercept(
            gate.clone(),
            session_id.clone(),
            ran.clone(),
        )),
        ..Default::default()
    };

    tokio::time::timeout(Duration::from_secs(30), runtime.run_turn(req))
        .await
        .expect("the turn must not hang")
        .expect("run_turn failed");

    server
        .received_requests()
        .await
        .expect("request recording is on")
        .iter()
        .map(|r| String::from_utf8_lossy(&r.body).into_owned())
        .collect()
}

// ===========================================================================
// Tests
// ===========================================================================

/// The tracer: the operator denies a gated `terminal` call and the model's next request
/// carries the operator-denied text, including the no-circumvention notice.
#[tokio::test]
async fn operator_denial_reaches_model_with_no_circumvention_notice() {
    let h = Harness::new().await;
    let gate = Arc::new(DenyingGate::default());
    let ran = Arc::new(StdMutex::new(0u32));

    let bodies = run_turn_and_collect_bodies("curl http://example.com/x", &gate, &ran).await;

    assert!(
        bodies.len() >= 2,
        "the model must be called again after the tool result; got {} request(s)",
        bodies.len()
    );
    let expected = approval_refusal_text("terminal", &ApprovalOutcome::Denied).unwrap();
    assert!(
        bodies[1].contains(&expected),
        "the model's next request must carry the operator-denied text verbatim.\n\
         expected: {expected}\nbody: {}",
        bodies[1]
    );
    assert!(expected.ends_with(NO_CIRCUMVENTION_NOTICE));
    assert_eq!(gate.asked.load(Ordering::SeqCst), 1, "operator asked once");
    assert_eq!(*ran.lock().unwrap(), 0, "the denied command must not run");
    for (i, b) in bodies.iter().enumerate() {
        assert!(
            !b.contains(RUN_SENTINEL),
            "run output leaked into request {i}"
        );
    }

    let rows = h.shell_rows();
    assert_eq!(rows.len(), 1, "one shell_command audit row: {rows:?}");
    assert_eq!(rows[0]["decision"], "denied", "{rows:?}");
    assert_eq!(rows[0]["resolution"], "operator", "{rows:?}");
}

/// A policy Block on the gated path reaches the model with the same notice, and the
/// operator is never asked (a Block is not an approval question).
#[tokio::test]
async fn policy_block_reaches_model_with_no_circumvention_notice() {
    let _h = Harness::new().await;
    let gate = Arc::new(DenyingGate::default());
    let ran = Arc::new(StdMutex::new(0u32));

    let bodies = run_turn_and_collect_bodies("rm -rf /", &gate, &ran).await;

    assert!(bodies.len() >= 2, "got {} request(s)", bodies.len());
    assert!(
        bodies[1].contains("Security policy blocked this terminal call ("),
        "the model's next request must say security policy blocked the call: {}",
        bodies[1]
    );
    assert!(
        bodies[1].contains(NO_CIRCUMVENTION_NOTICE),
        "the model's next request must carry the no-circumvention notice: {}",
        bodies[1]
    );
    assert_eq!(gate.asked.load(Ordering::SeqCst), 0, "a Block asks nobody");
    assert_eq!(*ran.lock().unwrap(), 0, "a blocked command must not run");
}

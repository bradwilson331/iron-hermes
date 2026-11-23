//! Phase 54.3 Plan 01: the tracer's end-to-end proof that yolo bypasses are aggregated
//! PER TURN (D-03), not announced per call.
//!
//! This drives the real path: a real `AgentRuntime::run_turn`, a real `AgentLoop`, a
//! real tool-dispatch path into a `terminal_intercept` that calls the real
//! `ironhermes_hooks::execute_gated_command` with `yolo = true`. Only the network is
//! faked — a `wiremock::MockServer` serving a canned SSE chat-completions stream (the
//! same seam `crates/ironhermes-gateway/tests/webhook_denial_delivery.rs` uses, via
//! `AgentRuntime::for_tests_with_base_url`).
//!
//! What it proves, in the terms of the phase's must-haves:
//!
//! - N bypasses inside one `run_turn` produce exactly ONE `notify_yolo_bypass_turn`
//!   call whose `summary.count()` is N, and ZERO per-call `notify_yolo_bypass` calls
//!   (so the `YOLO_LEDGER` task-local really did propagate through the tool-dispatch
//!   path — if the scope were removed from `run_turn`, every bypass would fall back to
//!   the per-call notice and these tests would fail).
//! - The N per-call `resolution == "bypass"` audit rows are untouched (D-04).
//! - Exactly one extra rollup row, `kind == "yolo_bypass_rollup"`, never
//!   `resolution == "bypass"` (D-05), whose `args.count` equals the bypass-row count.
//!
//! # Known, accepted gap: an aborted turn flushes nothing
//!
//! The flush runs on the Ok path, on the Err path (it sits BEFORE `run_result?` in
//! `run_turn`), and on a cooperatively cancelled turn (`agent.run` returns `Ok` with
//! `StopReason::Cancelled`) — all three are tested below. It does NOT run when the whole
//! `run_turn` future is DROPPED: `ws.rs` calls `turn.handle.abort()` (at `:900`, `:2128`,
//! `:2317`, back-to-back with `cancel()`), which drops the future at its next yield, so
//! no flush runs for an aborted turn. The per-call `resolution="bypass"` audit rows
//! survive and remain the forensic truth (D-04), but the rollup row and the UI notice
//! for that turn are lost. RESEARCH assumption A8 rules a drop-guard out of scope for
//! this phase; this paragraph is the record. Do not add a drop guard here.
//!
//! # Process isolation
//!
//! `AuditLog::load` reads `IRONHERMES_HOME` (`ironhermes_core::constants::get_hermes_home`),
//! and both the intercept closure and `run_turn`'s flush resolve their audit path
//! through it, so every test points `IRONHERMES_HOME` at its own `TempDir`. The env var
//! is process-global, so the tests serialize on [`ENV_LOCK`]; under nextest each test
//! is its own process anyway.

use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use ironhermes_agent::{AgentRuntime, TurnRequest};
use ironhermes_core::{
    ApprovalGate, ApprovalOutcome, AuditConfig, AuditLog, ChatMessage, YoloBypassSummary,
};
use ironhermes_hooks::{DangerousCommandGuardrail, execute_gated_command};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Serializes tests that mutate the process-global `IRONHERMES_HOME`.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A command containing a `|` — trips `DangerousCommandGuardrail::has_metachar`, which
/// yields the D-09 "cannot verify safety" `Warn` that yolo overrides.
const METACHAR_COMMAND: &str = "echo a | tr a b";

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
    // `arguments` is itself a JSON document carried inside a JSON string.
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

/// Mount `n` sequential tool-call completions (each `up_to_n_times(1)`, ascending
/// priority), then the closing text completion for every request after them. Without
/// the per-mock bound the model would re-issue the tool call forever.
async fn mount_n_tool_calls_then_text(server: &MockServer, n: u8, text: &str) {
    for i in 0..n {
        Mock::given(any())
            .respond_with(sse_response(sse_terminal_call(
                &format!("call-{i}"),
                METACHAR_COMMAND,
            )))
            .up_to_n_times(1)
            .with_priority(i + 1)
            .mount(server)
            .await;
    }
    Mock::given(any())
        .respond_with(sse_response(sse_text(text)))
        .with_priority(n + 1)
        .mount(server)
        .await;
}

// ===========================================================================
// Recording gate
// ===========================================================================

/// Records BOTH delivery entry points so a test can prove which one fired.
#[derive(Default)]
struct RecordingGate {
    /// Every `notify_yolo_bypass_turn` summary (the aggregated, scoped path).
    summaries: StdMutex<Vec<YoloBypassSummary>>,
    /// Every per-call `notify_yolo_bypass` (the unscoped fallback). Must stay empty on
    /// a scoped turn.
    per_call: StdMutex<Vec<(String, String, String)>>,
}

impl RecordingGate {
    fn summaries(&self) -> Vec<YoloBypassSummary> {
        self.summaries.lock().unwrap().clone()
    }
    fn notifications(&self) -> Vec<(String, String, String)> {
        self.per_call.lock().unwrap().clone()
    }
}

#[async_trait]
impl ApprovalGate for RecordingGate {
    async fn request_approval(
        &self,
        _session_id: &str,
        _tool_name: &str,
        _reason: &str,
        _args: &serde_json::Value,
    ) -> ApprovalOutcome {
        ApprovalOutcome::Denied
    }

    async fn resolve(&self, _session_id: &str, _approved: bool) -> bool {
        false
    }

    async fn notify_yolo_bypass(&self, session_id: &str, tool_name: &str, reason: &str) {
        self.per_call.lock().unwrap().push((
            session_id.to_string(),
            tool_name.to_string(),
            reason.to_string(),
        ));
    }

    async fn notify_yolo_bypass_turn(&self, summary: &YoloBypassSummary) {
        self.summaries.lock().unwrap().push(summary.clone());
    }
}

// ===========================================================================
// Harness
// ===========================================================================

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

    /// Stream-safe read-back: `audit.jsonl` is NOT strictly line-delimited (concurrent
    /// appenders can concatenate entries on one line), so never line-split or `wc -l`.
    fn audit_rows(&self) -> Vec<serde_json::Value> {
        let Ok(contents) = std::fs::read_to_string(&self.audit_path) else {
            return Vec::new();
        };
        serde_json::Deserializer::from_str(&contents)
            .into_iter::<serde_json::Value>()
            .map(|r| r.expect("audit.jsonl must parse as a stream of JSON values"))
            .collect()
    }

    fn rows_where(&self, kind: &str, resolution: &str) -> Vec<serde_json::Value> {
        self.audit_rows()
            .into_iter()
            .filter(|r| r["kind"] == kind && r["resolution"] == resolution)
            .collect()
    }

    fn rollup_rows(&self) -> Vec<serde_json::Value> {
        self.audit_rows()
            .into_iter()
            .filter(|r| r["kind"] == "yolo_bypass_rollup")
            .collect()
    }
}

/// A `terminal_intercept` that routes every call through the real
/// `execute_gated_command` with `yolo = true`, handing it the recording gate for the
/// per-call fallback path.
///
/// `cancel_after_call`: when `Some`, the token is cancelled right after each gated call
/// returns — a deterministic stand-in for "the operator hit /stop after the first
/// bypass" that needs no sleep and no racing spawned task.
fn yolo_terminal_intercept(
    gate: Arc<RecordingGate>,
    session_id: String,
    cancel_after_call: Option<CancellationToken>,
) -> ironhermes_tools::registry::InterceptHandler {
    Arc::new(move |args: serde_json::Value| {
        let gate = gate.clone();
        let session_id = session_id.clone();
        let cancel_after_call = cancel_after_call.clone();
        Box::pin(async move {
            let command = args
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let guard = DangerousCommandGuardrail::new();
            let audit_log = AuditLog::load(AuditConfig::default());
            let gate_ref: &dyn ApprovalGate = &*gate;
            let outcome = execute_gated_command(
                "terminal",
                &command,
                &guard,
                Some(gate_ref),
                &audit_log,
                &session_id,
                "test",
                "",
                true,
                false,
                false,
                || async { Ok("ok".to_string()) },
            )
            .await;
            if let Some(token) = &cancel_after_call {
                token.cancel();
            }
            Ok(outcome.to_string())
        })
    })
}

// ===========================================================================
// Tests
// ===========================================================================

/// The tracer: ONE bypass inside ONE real `run_turn` yields exactly one aggregated
/// delivery, one per-call audit row, and one rollup row.
#[tokio::test]
async fn yolo_turn_e2e_single_bypass_produces_one_summary_and_one_rollup() {
    let h = Harness::new().await;
    let server = MockServer::start().await;
    mount_n_tool_calls_then_text(&server, 1, "done").await;

    let runtime = AgentRuntime::for_tests_with_base_url(server.uri());
    let gate = Arc::new(RecordingGate::default());
    let session_id = format!("yolo-e2e-{}", uuid::Uuid::new_v4());

    let req = TurnRequest {
        messages: vec![ChatMessage::user("run the command")],
        session_id: session_id.clone(),
        // A stream callback selects the SSE (streaming) client path, which is what the
        // canned `text/event-stream` fixtures above feed; without it the client expects
        // a single non-streaming JSON body.
        stream: Some(Box::new(|_delta: &str| {})),
        turn_id: Some(uuid::Uuid::new_v4()),
        yolo_notifier: Some(gate.clone()),
        terminal_intercept: Some(yolo_terminal_intercept(
            gate.clone(),
            session_id.clone(),
            None,
        )),
        ..Default::default()
    };

    tokio::time::timeout(Duration::from_secs(30), runtime.run_turn(req))
        .await
        .expect("the turn must not hang")
        .expect("run_turn failed");

    let summaries = gate.summaries();
    assert_eq!(
        summaries.len(),
        1,
        "exactly ONE notify_yolo_bypass_turn call per bypassing turn (D-03); got {summaries:?}"
    );
    assert_eq!(
        summaries[0].count(),
        1,
        "summary.count() == 1: {summaries:?}"
    );
    assert!(
        gate.notifications().is_empty(),
        "a scoped turn must produce ZERO per-call notify_yolo_bypass calls — a non-empty \
         list means the YOLO_LEDGER scope did not reach the gated path and the bypass fell \
         back to the noisy per-call notice: {:?}",
        gate.notifications()
    );

    assert_eq!(
        h.rows_where("shell_command", "bypass").len(),
        1,
        "the per-call resolution=\"bypass\" audit row is unchanged (D-04); rows: {:?}",
        h.audit_rows()
    );
    let rollups = h.rollup_rows();
    assert_eq!(
        rollups.len(),
        1,
        "exactly one rollup row (D-05): {rollups:?}"
    );
    assert_eq!(
        rollups[0]["resolution"], "rollup",
        "the rollup must never carry resolution=\"bypass\" (it would double-count): {rollups:?}"
    );
    let args: serde_json::Value =
        serde_json::from_str(rollups[0]["args"].as_str().expect("args is a JSON string"))
            .expect("rollup args parse as JSON");
    assert_eq!(args["count"], 1, "rollup args.count: {args}");
}

/// Build a streaming `TurnRequest` for these tests (stream callback selects the SSE
/// client path).
fn turn_request(
    gate: &Arc<RecordingGate>,
    session_id: &str,
    cancel_token: Option<CancellationToken>,
    cancel_after_call: Option<CancellationToken>,
) -> TurnRequest {
    TurnRequest {
        messages: vec![ChatMessage::user("run the commands")],
        session_id: session_id.to_string(),
        cancel_token,
        stream: Some(Box::new(|_delta: &str| {})),
        turn_id: Some(uuid::Uuid::new_v4()),
        yolo_notifier: Some(gate.clone()),
        terminal_intercept: Some(yolo_terminal_intercept(
            gate.clone(),
            session_id.to_string(),
            cancel_after_call,
        )),
        ..Default::default()
    }
}

/// Assert the aggregated-delivery invariants shared by every test below: exactly one
/// `notify_yolo_bypass_turn` of `expected` bypasses, zero per-call notifies, `expected`
/// per-call bypass rows, and exactly one rollup row whose `args.count == expected`.
fn assert_one_summary_and_one_rollup(h: &Harness, gate: &RecordingGate, expected: u32) {
    let summaries = gate.summaries();
    assert_eq!(
        summaries.len(),
        1,
        "exactly ONE notify_yolo_bypass_turn call (D-03); got {summaries:?}"
    );
    assert_eq!(
        summaries[0].count(),
        expected,
        "summary.count(): {summaries:?}"
    );
    assert!(
        gate.notifications().is_empty(),
        "ZERO per-call notify_yolo_bypass calls on a scoped turn — {expected} per-call \
         notifications would mean the YOLO_LEDGER scope was not installed (the Pitfall-1 \
         false green this test exists to catch): {:?}",
        gate.notifications()
    );
    assert_eq!(
        h.rows_where("shell_command", "bypass").len(),
        expected as usize,
        "per-call bypass rows stay 1:1 with bypassed calls (D-04); rows: {:?}",
        h.audit_rows()
    );
    let rollups = h.rollup_rows();
    assert_eq!(
        rollups.len(),
        1,
        "exactly one rollup row (D-05): {rollups:?}"
    );
    assert_eq!(rollups[0]["resolution"], "rollup");
    let args: serde_json::Value =
        serde_json::from_str(rollups[0]["args"].as_str().expect("args is a JSON string"))
            .expect("rollup args parse as JSON");
    assert_eq!(args["count"], expected, "rollup args.count: {args}");
}

/// V4: FIVE bypasses in one real `run_turn` → one summary (`count() == 5`), five
/// per-call rows, one rollup row (`args.count == 5`). Five SEQUENTIAL assistant turns
/// (rather than five parallel `tool_calls` indexes in one chunk) exercise the same
/// sequential `for tool_call in tool_calls` dispatch the production path uses.
///
/// Mutation-tested: deleting the `YOLO_LEDGER.scope(..)` wrapper in `run_turn` makes
/// this fail with 5 per-call notifications and 0 summaries.
#[tokio::test]
async fn yolo_turn_e2e_five_bypasses_produce_one_summary_and_one_rollup() {
    let h = Harness::new().await;
    let server = MockServer::start().await;
    mount_n_tool_calls_then_text(&server, 5, "done").await;

    let runtime = AgentRuntime::for_tests_with_base_url(server.uri());
    let gate = Arc::new(RecordingGate::default());
    let session_id = format!("yolo-e2e-{}", uuid::Uuid::new_v4());

    tokio::time::timeout(
        Duration::from_secs(30),
        runtime.run_turn(turn_request(&gate, &session_id, None, None)),
    )
    .await
    .expect("the turn must not hang")
    .expect("run_turn failed");

    assert_one_summary_and_one_rollup(&h, &gate, 5);
}

/// V5a: the flush fires on the Err path. After the first tool call round-trips (and
/// bypasses), the provider answers the follow-up request with a non-retryable 400, so
/// `run_turn` resolves to `Err`. The rollup row and the single notifier call must still
/// be there — this is precisely what the flush-BEFORE-`run_result?` placement buys, and
/// moving the flush after the `?` is this test's stated falsifier.
#[tokio::test]
async fn yolo_turn_e2e_flush_fires_on_the_err_path() {
    let h = Harness::new().await;
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(sse_response(sse_terminal_call("call-0", METACHAR_COMMAND)))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(400).set_body_raw(
            r#"{"error":{"message":"bad request","code":400}}"#,
            "application/json",
        ))
        .with_priority(2)
        .mount(&server)
        .await;

    let runtime = AgentRuntime::for_tests_with_base_url(server.uri());
    let gate = Arc::new(RecordingGate::default());
    let session_id = format!("yolo-e2e-{}", uuid::Uuid::new_v4());

    let result = tokio::time::timeout(
        Duration::from_secs(30),
        runtime.run_turn(turn_request(&gate, &session_id, None, None)),
    )
    .await
    .expect("the turn must not hang");

    assert!(
        result.is_err(),
        "the provider 400 must surface as Err from run_turn (this test exercises the Err \
         exit); got Ok"
    );
    assert_one_summary_and_one_rollup(&h, &gate, 1);
}

/// V5b: the flush fires on a cooperatively cancelled turn. The token is cancelled right
/// after the first bypass; the loop's next-iteration check returns `Ok` with
/// `StopReason::Cancelled`, and the rollup row plus the single notifier call are present.
#[tokio::test]
async fn yolo_turn_e2e_flush_fires_on_cooperative_cancellation() {
    let h = Harness::new().await;
    let server = MockServer::start().await;
    mount_n_tool_calls_then_text(&server, 1, "never reached").await;

    let runtime = AgentRuntime::for_tests_with_base_url(server.uri());
    let gate = Arc::new(RecordingGate::default());
    let session_id = format!("yolo-e2e-{}", uuid::Uuid::new_v4());
    let token = CancellationToken::new();

    let out = tokio::time::timeout(
        Duration::from_secs(30),
        runtime.run_turn(turn_request(
            &gate,
            &session_id,
            Some(token.clone()),
            Some(token.clone()),
        )),
    )
    .await
    .expect("the turn must not hang")
    .expect("a cooperative cancel resolves Ok, not Err");

    assert_eq!(
        out.stop_reason,
        ironhermes_agent::agent_loop::StopReason::Cancelled,
        "the turn must have ended via cooperative cancellation"
    );
    assert_one_summary_and_one_rollup(&h, &gate, 1);
}

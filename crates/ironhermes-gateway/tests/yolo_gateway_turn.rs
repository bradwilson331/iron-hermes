//! Phase 54.11 Plan 01 (D-19): one gateway turn with N yolo-bypassed calls must send NO bypass chat
//! notice (operator, 2026-11-01: bypass notices go to the log only).
//!
//! The Phase 54.3 per-turn batching (`notify_yolo_bypass_turn`) now logs instead of posting. A bypassed call that runs outside any `YOLO_LEDGER` scope falls back to the
//! per-call `GatewayApprovalGate::notify_yolo_bypass`, which used to post its own
//! `U+26A1 Yolo bypass` message each time: the operator's "many identical messages at once"
//! symptom.
//!
//! * `gateway_turn_bypasses_send_no_chat_notice` drives a REAL
//!   `GatewayMessageHandler` turn (real `AgentRuntime::run_turn`, real `AgentLoop`, real
//!   `execute_gated_command`; only the model endpoint is a wiremock).
//! * `gateway_unscoped_bypasses_send_no_chat_notice` runs three bypassed calls
//!   through `execute_gated_command` with a `GatewayApprovalGate` and NO ledger in scope.
//!   That is the exact fallback every ledger-losing hop (a `tokio::spawn`, a
//!   `spawn_blocking`, a `block_on_sync`) lands in, so it is hop-agnostic.
//!
//! Bypass chat notices are detected as adapter texts that start with the bolt glyph (the per-call
//! notice and the per-turn `short_line()` both do) OR mention bypass/yolo/DEFCON in any case, so a
//! reworded or glyph-less notice cannot slip through. The unscoped test sends nothing else, so it
//! asserts the adapter received no message at all.
//!
//! `IRONHERMES_HOME`, the security service and the DEFCON registry are process-global, so
//! every test takes [`ENV_LOCK`]. Run with `--test-threads=1`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ironhermes_agent::AgentRuntime;
use ironhermes_core::config::DefconLevel;
use ironhermes_core::defcon::session_defcon;
use ironhermes_core::{
    ApprovalGate, ApprovalsStore, AuditConfig, AuditLog, Config, MessageEvent, MessageResponse,
    Platform, ProviderResolver,
};
use ironhermes_gateway::adapter::{MessageHandler, PlatformAdapter};
use ironhermes_gateway::approval::{ApprovalCoordinator, GatewayApprovalGate};
use ironhermes_gateway::handler::GatewayMessageHandler;
use ironhermes_gateway::session::{SessionKey, SessionStore};
use ironhermes_hooks::{DangerousCommandGuardrail, GatePolicy, execute_gated_command};
use ironhermes_security::testing::{
    FakeBackend, clear_installed_for_tests, install_fake_for_tests,
};
use ironhermes_tools::ToolRegistry;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const CHAT: &str = "chat-yolo-turn";
/// A pipe trips the D-09 metacharacter `Warn`, which a bypass-active level overrides.
const METACHAR_COMMAND: &str = "echo a | tr a b";
const BOLT: char = '\u{26a1}';

fn sender() -> String {
    format!("user-{CHAT}")
}

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

/// `n` sequential tool-call completions (each bounded to one use), then closing text.
async fn mount_n_tool_calls_then_text(server: &MockServer, n: u8) {
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
        .respond_with(sse_response(sse_text("done")))
        .with_priority(n + 1)
        .mount(server)
        .await;
}

// ===========================================================================
// Recording adapter
// ===========================================================================

struct RecordingAdapter {
    sent: std::sync::Mutex<Vec<String>>,
}

impl RecordingAdapter {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            sent: std::sync::Mutex::new(Vec::new()),
        })
    }

    fn texts(&self) -> Vec<String> {
        self.sent.lock().unwrap().clone()
    }

    /// Chat messages that announce a yolo bypass: a leading bolt glyph, or any
    /// mention of bypass / yolo / DEFCON (case-insensitive).
    fn bypass_texts(&self) -> Vec<String> {
        self.texts()
            .into_iter()
            .filter(|t| {
                let l = t.to_lowercase();
                t.starts_with(BOLT)
                    || l.contains("bypass")
                    || l.contains("yolo")
                    || l.contains("defcon")
            })
            .collect()
    }
}

#[async_trait]
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
        self.sent.lock().unwrap().push(content.to_string());
        Ok(MessageResponse {
            message_id: "stub-msg-id".to_string(),
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
    async fn edit_message(&self, _: &str, _: &str, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn edit_message_markdown_v2(&self, _: &str, _: &str, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn delete_message(&self, _: &str, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
    fn is_running(&self) -> bool {
        true
    }
}

// ===========================================================================
// Harness
// ===========================================================================

struct Harness {
    home: tempfile::TempDir,
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

impl Harness {
    async fn new() -> Self {
        let guard = ENV_LOCK.lock().await;
        let home = tempfile::tempdir().expect("tempdir");
        // SAFETY: serialized by ENV_LOCK; no other thread reads the var concurrently.
        unsafe { std::env::set_var("IRONHERMES_HOME", home.path()) };
        clear_installed_for_tests();
        install_fake_for_tests(FakeBackend::new());
        session_defcon().set_process_default(DefconLevel::THREE);
        Self {
            home,
            _guard: guard,
        }
    }

    /// Stream-safe read-back (`audit.jsonl` is not strictly line-delimited).
    fn audit_rows(&self) -> Vec<serde_json::Value> {
        let path = self.home.path().join("logs").join("audit.jsonl");
        let Ok(contents) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        serde_json::Deserializer::from_str(&contents)
            .into_iter::<serde_json::Value>()
            .map(|r| r.expect("audit.jsonl must parse as a stream of JSON values"))
            .collect()
    }

    fn bypass_rows(&self) -> Vec<serde_json::Value> {
        self.audit_rows()
            .into_iter()
            .filter(|r| r["kind"] == "shell_command" && r["resolution"] == "bypass")
            .collect()
    }

    fn rollup_rows(&self) -> Vec<serde_json::Value> {
        self.audit_rows()
            .into_iter()
            .filter(|r| r["kind"] == "yolo_bypass_rollup")
            .collect()
    }

    fn coordinator(&self, adapter: &Arc<RecordingAdapter>) -> Arc<ApprovalCoordinator> {
        let adapter_dyn: Arc<dyn PlatformAdapter> = adapter.clone();
        Arc::new(ApprovalCoordinator::new(
            1,
            adapter_dyn,
            Arc::new(ApprovalsStore::with_path(
                self.home.path().join("approvals.json"),
            )),
            Arc::new(AuditLog::load(AuditConfig::default())),
        ))
    }
}

fn bypass_level() -> DefconLevel {
    DefconLevel::try_from(4).expect("level 4")
}

// ===========================================================================
// Tests
// ===========================================================================

/// A real gateway turn with three bypassed `terminal` calls sends no bolt message,
/// keeps one audit row per bypass, and writes one rollup row.
#[tokio::test]
async fn gateway_turn_bypasses_send_no_chat_notice() {
    let h = Harness::new().await;
    let server = MockServer::start().await;
    mount_n_tool_calls_then_text(&server, 3).await;

    let mut config = Config::default();
    config
        .gateway
        .platforms
        .entry("telegram".to_string())
        .or_default()
        .whitelist = vec![sender()];
    let resolver = ProviderResolver::build(&config).unwrap();
    let state_store = Arc::new(std::sync::Mutex::new(
        ironhermes_state::StateStore::new(":memory:").expect("in-memory StateStore"),
    ));
    let session_store = Arc::new(RwLock::new(SessionStore::new(state_store)));
    let mut handler = GatewayMessageHandler::new(
        config.clone(),
        resolver,
        session_store.clone(),
        Arc::new(RwLock::new(ToolRegistry::new())),
    );
    handler.set_agent_runtime(Arc::new(AgentRuntime::for_tests_with_base_url(
        server.uri(),
    )));

    let adapter = RecordingAdapter::new();
    handler.set_approval_coordinator(h.coordinator(&adapter));

    let key = SessionKey::new(Platform::Telegram, CHAT).with_user(sender());
    let session_id = session_store
        .write()
        .await
        .get_or_create(key, &config.model.default, "telegram")
        .session_id
        .clone();
    session_defcon().set_session_level(&session_id, bypass_level());

    let ev = MessageEvent {
        platform: Platform::Telegram,
        message_id: "msg-1".to_string(),
        chat_id: CHAT.to_string(),
        sender_id: sender(),
        content: "run the commands".to_string(),
        attachments: vec![],
        thread_id: None,
        chat_type: "dm".to_string(),
        chat_name: None,
        sender_name: None,
        replied_to_id: None,
    };
    let adapter_dyn: Arc<dyn PlatformAdapter> = adapter.clone();
    tokio::time::timeout(
        Duration::from_secs(30),
        handler.handle(&ev, adapter_dyn, CancellationToken::new()),
    )
    .await
    .expect("the turn must not hang")
    .expect("handle failed");

    let bolts = adapter.bypass_texts();
    assert!(
        bolts.is_empty(),
        "one turn must send NO bypass chat notice; got {}: {bolts:?}",
        bolts.len()
    );
    assert_eq!(
        h.bypass_rows().len(),
        3,
        "one resolution=bypass audit row per bypassed call: {:?}",
        h.audit_rows()
    );
    assert_eq!(
        h.rollup_rows().len(),
        1,
        "exactly one rollup row: {:?}",
        h.audit_rows()
    );
    session_defcon().forget(&session_id);
}

/// Three bypassed calls through the real gate with NO `YOLO_LEDGER` in scope (the fallback
/// every ledger-losing hop lands in) must still send no bolt message.
#[tokio::test]
async fn gateway_unscoped_bypasses_send_no_chat_notice() {
    let h = Harness::new().await;
    let adapter = RecordingAdapter::new();
    let gate = GatewayApprovalGate::new(h.coordinator(&adapter), CHAT.to_string());
    let session_id = format!("yolo-unscoped-{}", uuid::Uuid::new_v4());
    session_defcon().set_session_level(&session_id, bypass_level());

    let guard = DangerousCommandGuardrail::new();
    let audit_log = AuditLog::load(AuditConfig::default());
    let gate_ref: &dyn ApprovalGate = &gate;
    for _ in 0..3 {
        execute_gated_command(
            "terminal",
            METACHAR_COMMAND,
            &guard,
            Some(gate_ref),
            &audit_log,
            &session_id,
            "gateway",
            CHAT,
            GatePolicy::for_session(&session_id),
            false,
            false,
            || async { Ok("ok".to_string()) },
        )
        .await;
    }

    let sent = adapter.texts();
    assert!(
        sent.is_empty(),
        "unscoped bypasses must send NO chat message at all; got {}: {sent:?}",
        sent.len()
    );
    assert_eq!(
        h.bypass_rows().len(),
        3,
        "one resolution=bypass audit row per bypassed call: {:?}",
        h.audit_rows()
    );
    session_defcon().forget(&session_id);
}

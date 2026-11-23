//! Phase 54.8 Plan 09 (D-15) — Telegram `/mcp connect` and OAuth paste-back.
//!
//! The REAL `GatewayMessageHandler` is driven with a fake `OAuthPasteBack` and a
//! recording adapter. `intercept_oauth_paste` is the exact synchronous function the
//! runner's dispatch loop calls, so the tests exercise the loop's decision directly;
//! processing is asynchronous, so assertions poll the recorded calls.
//!
//! Each test binary is its own crate, so the harness is copied per the repo
//! convention (`concurrent_turns_gateway.rs`, `turn_deadline.rs`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use ironhermes_core::{Config, MessageEvent, MessageResponse, Platform, ProviderResolver};
use ironhermes_gateway::adapter::PlatformAdapter;
use ironhermes_gateway::handler::GatewayMessageHandler;
use ironhermes_gateway::multimodal::ProcessedAttachments;
use ironhermes_gateway::session::SessionStore;
use ironhermes_mcp::McpServerConfig;
use ironhermes_mcp::manager::OAuthAuthorizationStart;
use ironhermes_mcp::paste_back::OAuthPasteBack;
use ironhermes_tools::ToolRegistry;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const AUTH_URL: &str = "https://auth.example.test/authorize?state=st-1";
const CALLBACK: &str = "http://127.0.0.1:8765/callback?code=cd-123&state=st-1";

// ---------------------------------------------------------------------------
// Fake OAuthPasteBack
// ---------------------------------------------------------------------------

#[derive(Clone)]
#[allow(dead_code)]
enum CompleteMode {
    Ok,
    Err(String),
    Hang,
}

#[derive(Default)]
struct Calls {
    begin: Vec<String>,
    complete: Vec<String>,
    cancel: Vec<String>,
    reconnect: Vec<String>,
}

struct FakePasteBack {
    calls: std::sync::Mutex<Calls>,
    complete_mode: std::sync::Mutex<CompleteMode>,
}

impl FakePasteBack {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: std::sync::Mutex::new(Calls::default()),
            complete_mode: std::sync::Mutex::new(CompleteMode::Ok),
        })
    }

    #[allow(dead_code)]
    fn set_complete_mode(&self, mode: CompleteMode) {
        *self.complete_mode.lock().unwrap() = mode;
    }

    fn begin_calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().begin.clone()
    }
    fn complete_calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().complete.clone()
    }
    #[allow(dead_code)]
    fn cancel_calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().cancel.clone()
    }
    fn reconnect_calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().reconnect.clone()
    }
}

#[async_trait::async_trait]
impl OAuthPasteBack for FakePasteBack {
    async fn begin(
        &self,
        server_name: &str,
        _config: &McpServerConfig,
    ) -> Result<OAuthAuthorizationStart, String> {
        self.calls
            .lock()
            .unwrap()
            .begin
            .push(server_name.to_string());
        Ok(OAuthAuthorizationStart {
            auth_url: AUTH_URL.to_string(),
            state: "st-1".to_string(),
        })
    }

    async fn complete(&self, callback_url: &str) -> Result<String, String> {
        self.calls
            .lock()
            .unwrap()
            .complete
            .push(callback_url.to_string());
        let mode = self.complete_mode.lock().unwrap().clone();
        match mode {
            CompleteMode::Ok => Ok("demo_cf".to_string()),
            CompleteMode::Err(e) => Err(e),
            CompleteMode::Hang => {
                std::future::pending::<()>().await;
                unreachable!()
            }
        }
    }

    async fn cancel(&self, state: &str) {
        self.calls.lock().unwrap().cancel.push(state.to_string());
    }

    async fn reconnect_server(&self, server_name: &str, _config: &McpServerConfig) -> String {
        self.calls
            .lock()
            .unwrap()
            .reconnect
            .push(server_name.to_string());
        format!("{server_name}: connected (3 MCP tools registered in total)")
    }
}

// ---------------------------------------------------------------------------
// Recording adapter
// ---------------------------------------------------------------------------

struct RecordingAdapter {
    sent: std::sync::Mutex<Vec<(String, String)>>,
    deleted: std::sync::Mutex<Vec<(String, String)>>,
    fail_delete: bool,
}

impl RecordingAdapter {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            sent: std::sync::Mutex::new(Vec::new()),
            deleted: std::sync::Mutex::new(Vec::new()),
            fail_delete: false,
        })
    }

    #[allow(dead_code)]
    fn failing_delete() -> Arc<Self> {
        Arc::new(Self {
            sent: std::sync::Mutex::new(Vec::new()),
            deleted: std::sync::Mutex::new(Vec::new()),
            fail_delete: true,
        })
    }

    fn texts(&self) -> Vec<String> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .map(|(_, t)| t.clone())
            .collect()
    }

    fn deleted_ids(&self) -> Vec<String> {
        self.deleted
            .lock()
            .unwrap()
            .iter()
            .map(|(_, m)| m.clone())
            .collect()
    }
}

#[async_trait::async_trait]
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
        self.sent
            .lock()
            .unwrap()
            .push((chat_id.to_string(), content.to_string()));
        Ok(MessageResponse {
            message_id: "stub-msg-id".to_string(),
            chat_id: chat_id.to_string(),
            platform: Platform::Telegram,
        })
    }

    async fn edit_message(&self, _: &str, _: &str, _: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn edit_message_markdown_v2(&self, _: &str, _: &str, _: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn send_message_markdown_v2(
        &self,
        chat_id: &str,
        content: &str,
        thread_id: Option<&str>,
    ) -> anyhow::Result<MessageResponse> {
        self.send_message(chat_id, content, thread_id).await
    }

    async fn delete_message(&self, chat_id: &str, message_id: &str) -> anyhow::Result<()> {
        if self.fail_delete {
            anyhow::bail!("delete refused");
        }
        self.deleted
            .lock()
            .unwrap()
            .push((chat_id.to_string(), message_id.to_string()));
        Ok(())
    }

    fn is_running(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Builders
// ---------------------------------------------------------------------------

fn build_test_session_store() -> Arc<RwLock<SessionStore>> {
    let state_store = Arc::new(std::sync::Mutex::new(
        ironhermes_state::StateStore::new(":memory:").expect("in-memory StateStore"),
    ));
    Arc::new(RwLock::new(SessionStore::new(state_store)))
}

fn mcp_servers() -> HashMap<String, serde_yaml::Value> {
    serde_yaml::from_str(
        r#"
demo_cf:
  url: https://mcp.cloudflare.com/mcp
  oauth_provider: demo_cf
demo_other:
  url: https://mcp.example.com/mcp
  oauth_provider: demo_other
"#,
    )
    .expect("fixture mcp_servers")
}

fn build_handler(fake: Arc<FakePasteBack>) -> Arc<GatewayMessageHandler> {
    build_handler_with(fake, build_test_session_store(), |_| {})
}

fn build_handler_with(
    fake: Arc<FakePasteBack>,
    store: Arc<RwLock<SessionStore>>,
    configure: impl FnOnce(&mut GatewayMessageHandler),
) -> Arc<GatewayMessageHandler> {
    let config = Config::default();
    let resolver = ProviderResolver::build(&config).unwrap();
    let tool_registry = Arc::new(RwLock::new(ToolRegistry::new()));
    let mut handler = GatewayMessageHandler::new(config, resolver, store, tool_registry);
    handler.set_oauth_paste_back(fake as Arc<dyn OAuthPasteBack>);
    handler.set_mcp_servers_for_tests(mcp_servers());
    configure(&mut handler);
    Arc::new(handler)
}

fn as_dyn(adapter: &Arc<RecordingAdapter>) -> Arc<dyn PlatformAdapter> {
    adapter.clone() as Arc<dyn PlatformAdapter>
}

/// Shared in-memory sink for a tracing fmt subscriber.
#[derive(Clone, Default)]
struct LogBuf(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for LogBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuf {
    type Writer = LogBuf;
    fn make_writer(&'a self) -> LogBuf {
        self.clone()
    }
}

impl LogBuf {
    fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

fn event(
    platform: Platform,
    chat_type: &str,
    chat_id: &str,
    message_id: &str,
    content: &str,
) -> MessageEvent {
    MessageEvent {
        platform,
        message_id: message_id.to_string(),
        chat_id: chat_id.to_string(),
        sender_id: "owner".to_string(),
        content: content.to_string(),
        attachments: vec![],
        thread_id: None,
        chat_type: chat_type.to_string(),
        chat_name: None,
        sender_name: None,
        replied_to_id: None,
    }
}

fn dm(chat_id: &str, message_id: &str, content: &str) -> MessageEvent {
    event(Platform::Telegram, "dm", chat_id, message_id, content)
}

fn no_attachments() -> ProcessedAttachments {
    ProcessedAttachments {
        text_prefix: None,
        image_data_uri: None,
        image_cache_path: None,
    }
}

async fn run_command(
    handler: &Arc<GatewayMessageHandler>,
    adapter: &Arc<RecordingAdapter>,
    ev: &MessageEvent,
) {
    handler
        .handle_with_multimodal(
            ev,
            adapter.clone() as Arc<dyn PlatformAdapter>,
            CancellationToken::new(),
            no_attachments(),
        )
        .await
        .expect("slash command handled");
}

/// Poll `pred` for up to 2 s (processing is asynchronous).
async fn wait_until(mut pred: impl FnMut() -> bool) -> bool {
    for _ in 0..80 {
        if pred() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    pred()
}

// ---------------------------------------------------------------------------
// Task 1 — tracer: connect, then a bare paste completes exactly once
// ---------------------------------------------------------------------------

#[tokio::test]
async fn connect_then_bare_paste_completes_once() {
    let fake = FakePasteBack::new();
    let handler = build_handler(fake.clone());
    let adapter = RecordingAdapter::new();

    run_command(
        &handler,
        &adapter,
        &dm("chat-1", "m-1", "/mcp connect demo_cf"),
    )
    .await;

    assert_eq!(fake.begin_calls(), vec!["demo_cf".to_string()]);
    let texts = adapter.texts();
    assert_eq!(
        texts.len(),
        1,
        "exactly one reply to /mcp connect: {texts:?}"
    );
    assert!(
        texts[0].contains(AUTH_URL),
        "the reply must carry the authorization link: {}",
        texts[0]
    );

    let paste = dm("chat-1", "paste-1", CALLBACK);
    let consumed =
        handler.intercept_oauth_paste(&paste, adapter.clone() as Arc<dyn PlatformAdapter>);
    assert!(consumed, "a callback URL must be consumed synchronously");

    assert!(
        wait_until(|| adapter
            .texts()
            .iter()
            .any(|t| t.contains("Authorized demo_cf")))
        .await,
        "an Authorized reply must follow: {:?}",
        adapter.texts()
    );
    assert_eq!(fake.complete_calls(), vec![CALLBACK.to_string()]);
    assert_eq!(fake.reconnect_calls(), vec!["demo_cf".to_string()]);
    assert!(
        adapter.deleted_ids().contains(&"paste-1".to_string()),
        "the pasted message must be deleted: {:?}",
        adapter.deleted_ids()
    );
}

#[tokio::test]
async fn success_reply_never_echoes_callback() {
    let fake = FakePasteBack::new();
    let handler = build_handler(fake.clone());
    let adapter = RecordingAdapter::new();

    run_command(
        &handler,
        &adapter,
        &dm("chat-1", "m-1", "/mcp connect demo_cf"),
    )
    .await;
    let paste = dm("chat-1", "paste-1", CALLBACK);
    assert!(handler.intercept_oauth_paste(&paste, adapter.clone() as Arc<dyn PlatformAdapter>));
    assert!(wait_until(|| adapter.texts().len() >= 2).await);

    // Skip the first reply: it is the authorization link, which legitimately
    // carries `state=` (never a code).
    for t in adapter.texts().iter().skip(1) {
        assert!(!t.contains("cd-123"), "reply leaked the code: {t}");
        assert!(!t.contains("code="), "reply echoed a callback: {t}");
    }
}

// ---------------------------------------------------------------------------
// Task 2 — refusals, chat binding, expiry, denial, availability, confidentiality
// ---------------------------------------------------------------------------

#[tokio::test]
async fn connect_refused_in_group() {
    let fake = FakePasteBack::new();
    let handler = build_handler(fake.clone());
    let adapter = RecordingAdapter::new();
    let ev = event(
        Platform::Telegram,
        "group",
        "grp-1",
        "m-1",
        "/mcp connect demo_cf",
    );
    run_command(&handler, &adapter, &ev).await;
    let texts = adapter.texts();
    assert_eq!(texts.len(), 1, "{texts:?}");
    assert!(texts[0].contains("private chat"), "{}", texts[0]);
    assert!(fake.begin_calls().is_empty(), "begin must never run");
}

#[tokio::test]
async fn connect_refused_on_non_telegram() {
    let fake = FakePasteBack::new();
    let handler = build_handler(fake.clone());
    let adapter = RecordingAdapter::new();
    let ev = event(
        Platform::Discord,
        "dm",
        "dc-1",
        "m-1",
        "/mcp connect demo_cf",
    );
    run_command(&handler, &adapter, &ev).await;
    let texts = adapter.texts();
    assert_eq!(texts.len(), 1, "{texts:?}");
    assert!(texts[0].contains("private chat"), "{}", texts[0]);
    assert!(fake.begin_calls().is_empty(), "begin must never run");
}

#[tokio::test]
async fn connect_refused_for_non_baseline_issuer() {
    let fake = FakePasteBack::new();
    let handler = build_handler(fake.clone());
    let adapter = RecordingAdapter::new();
    run_command(
        &handler,
        &adapter,
        &dm("chat-1", "m-1", "/mcp connect demo_other"),
    )
    .await;
    let texts = adapter.texts();
    assert_eq!(texts.len(), 1, "{texts:?}");
    assert!(
        texts[0].contains("ironhermes mcp connect demo_other --no-browser"),
        "{}",
        texts[0]
    );
    assert!(fake.begin_calls().is_empty(), "begin must never run");
    assert!(!handler.has_pending_paste_for_tests("chat-1"));
}

#[tokio::test]
async fn connect_unknown_server_reports_not_found() {
    let fake = FakePasteBack::new();
    let handler = build_handler(fake.clone());
    let adapter = RecordingAdapter::new();
    run_command(
        &handler,
        &adapter,
        &dm("chat-1", "m-1", "/mcp connect nope"),
    )
    .await;
    let texts = adapter.texts();
    assert_eq!(texts.len(), 1, "{texts:?}");
    assert!(
        texts[0].contains("not a configured MCP server"),
        "{}",
        texts[0]
    );
    assert!(fake.begin_calls().is_empty(), "begin must never run");
}

#[tokio::test]
async fn paste_without_pending_is_refused_and_deleted() {
    let fake = FakePasteBack::new();
    let handler = build_handler(fake.clone());
    let adapter = RecordingAdapter::new();
    let paste = dm("chat-1", "paste-1", CALLBACK);
    assert!(handler.intercept_oauth_paste(&paste, as_dyn(&adapter)));
    assert!(wait_until(|| !adapter.texts().is_empty()).await);
    assert!(
        adapter.texts()[0].contains("No pending authorization"),
        "{:?}",
        adapter.texts()
    );
    assert!(adapter.deleted_ids().contains(&"paste-1".to_string()));
    assert!(fake.complete_calls().is_empty(), "complete must never run");
}

#[tokio::test]
async fn paste_state_from_other_chat_is_refused() {
    let fake = FakePasteBack::new();
    let handler = build_handler(fake.clone());
    let adapter = RecordingAdapter::new();

    // Chat A started the authorization (state st-1).
    run_command(
        &handler,
        &adapter,
        &dm("chat-A", "a-1", "/mcp connect demo_cf"),
    )
    .await;
    assert!(handler.has_pending_paste_for_tests("chat-A"));
    let sent_before = adapter.texts().len();

    // Chat B pastes A's callback.
    let paste = dm("chat-B", "b-paste", CALLBACK);
    assert!(handler.intercept_oauth_paste(&paste, as_dyn(&adapter)));
    assert!(wait_until(|| adapter.texts().len() > sent_before).await);

    assert!(
        adapter.texts()[sent_before].contains("No pending authorization"),
        "{:?}",
        adapter.texts()
    );
    assert!(fake.complete_calls().is_empty(), "complete must never run");
    assert!(
        handler.has_pending_paste_for_tests("chat-A"),
        "chat A's entry must be untouched"
    );
}

#[tokio::test]
async fn paste_with_mismatched_state_in_own_chat_is_refused() {
    let fake = FakePasteBack::new();
    let handler = build_handler(fake.clone());
    let adapter = RecordingAdapter::new();
    handler.insert_pending_paste_for_tests("chat-1", "st-OTHER", "demo_cf", Duration::ZERO);

    let paste = dm("chat-1", "paste-1", CALLBACK);
    assert!(handler.intercept_oauth_paste(&paste, as_dyn(&adapter)));
    assert!(wait_until(|| !adapter.texts().is_empty()).await);
    assert!(
        adapter.texts()[0].contains("does not belong"),
        "{:?}",
        adapter.texts()
    );
    assert!(fake.complete_calls().is_empty());
    assert_eq!(fake.cancel_calls(), vec!["st-OTHER".to_string()]);
    assert!(!handler.has_pending_paste_for_tests("chat-1"));
}

#[tokio::test]
async fn paste_after_ttl_is_refused() {
    let fake = FakePasteBack::new();
    let handler = build_handler(fake.clone());
    let adapter = RecordingAdapter::new();
    handler.insert_pending_paste_for_tests("chat-1", "st-1", "demo_cf", Duration::from_secs(400));

    let paste = dm("chat-1", "paste-1", CALLBACK);
    assert!(handler.intercept_oauth_paste(&paste, as_dyn(&adapter)));
    assert!(wait_until(|| !adapter.texts().is_empty()).await);
    assert!(
        adapter.texts()[0].contains("expired"),
        "{:?}",
        adapter.texts()
    );
    assert!(fake.complete_calls().is_empty(), "complete must never run");
    assert_eq!(fake.cancel_calls(), vec!["st-1".to_string()]);
    assert!(
        !handler.has_pending_paste_for_tests("chat-1"),
        "entry removed"
    );
}

#[tokio::test]
async fn denial_callback_cancels() {
    let fake = FakePasteBack::new();
    let handler = build_handler(fake.clone());
    let adapter = RecordingAdapter::new();
    run_command(
        &handler,
        &adapter,
        &dm("chat-1", "m-1", "/mcp connect demo_cf"),
    )
    .await;

    let denial = dm(
        "chat-1",
        "paste-1",
        "http://127.0.0.1:8765/callback?error=access_denied&state=st-1",
    );
    assert!(handler.intercept_oauth_paste(&denial, as_dyn(&adapter)));
    assert!(wait_until(|| adapter.texts().len() >= 2).await);
    assert!(
        adapter.texts()[1].contains("denied"),
        "{:?}",
        adapter.texts()
    );
    assert_eq!(fake.cancel_calls(), vec!["st-1".to_string()]);
    assert!(fake.complete_calls().is_empty(), "complete must never run");
    assert!(adapter.deleted_ids().contains(&"paste-1".to_string()));
}

#[tokio::test]
async fn bare_paste_in_group_is_swallowed() {
    let fake = FakePasteBack::new();
    let handler = build_handler(fake.clone());
    let adapter = RecordingAdapter::new();
    let paste = event(Platform::Telegram, "group", "grp-1", "g-paste", CALLBACK);
    assert!(
        handler.intercept_oauth_paste(&paste, as_dyn(&adapter)),
        "a callback in a group is consumed, never dispatched"
    );
    assert!(wait_until(|| !adapter.texts().is_empty()).await);
    assert!(
        adapter.texts()[0].contains("private chat"),
        "{:?}",
        adapter.texts()
    );
    assert!(adapter.deleted_ids().contains(&"g-paste".to_string()));
    assert!(fake.complete_calls().is_empty(), "complete must never run");
}

#[tokio::test]
async fn complete_error_is_reported_without_url() {
    let fake = FakePasteBack::new();
    fake.set_complete_mode(CompleteMode::Err(
        "unknown, already-used, or expired authorization state".to_string(),
    ));
    let handler = build_handler(fake.clone());
    let adapter = RecordingAdapter::new();
    run_command(
        &handler,
        &adapter,
        &dm("chat-1", "m-1", "/mcp connect demo_cf"),
    )
    .await;

    let paste = dm("chat-1", "paste-1", CALLBACK);
    assert!(handler.intercept_oauth_paste(&paste, as_dyn(&adapter)));
    assert!(wait_until(|| adapter.texts().len() >= 2).await);
    let reply = &adapter.texts()[1];
    assert!(
        reply.contains("unknown, already-used, or expired authorization state"),
        "{reply}"
    );
    assert!(!reply.contains("code="), "{reply}");
    assert!(!reply.contains("cd-123"), "{reply}");
    assert!(
        fake.reconnect_calls().is_empty(),
        "no reconnect after a failure"
    );
}

#[tokio::test]
async fn non_callback_text_is_not_intercepted() {
    let fake = FakePasteBack::new();
    let handler = build_handler(fake.clone());
    let adapter = RecordingAdapter::new();
    let ev = dm("chat-1", "m-1", "hello");
    assert!(!handler.intercept_oauth_paste(&ev, as_dyn(&adapter)));
    // Nothing is spawned for a non-callback, so give a stray task a moment anyway.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(adapter.texts().is_empty());
    assert!(adapter.deleted_ids().is_empty());
}

#[tokio::test]
async fn embedded_callback_in_prose_is_consumed() {
    let fake = FakePasteBack::new();
    let store = build_test_session_store();
    let handler = build_handler_with(fake.clone(), store.clone(), |_| {});
    let adapter = RecordingAdapter::new();
    run_command(
        &handler,
        &adapter,
        &dm("chat-1", "m-1", "/mcp connect demo_cf"),
    )
    .await;

    let prose = format!("here: <{CALLBACK}> thanks");
    let paste = dm("chat-1", "paste-1", &prose);
    assert!(handler.intercept_oauth_paste(&paste, as_dyn(&adapter)));
    assert!(
        wait_until(|| adapter
            .texts()
            .iter()
            .any(|t| t.contains("Authorized demo_cf")))
        .await
    );

    assert_eq!(
        fake.complete_calls(),
        vec![CALLBACK.to_string()],
        "complete receives the bare URL, not the sentence"
    );
    assert!(adapter.deleted_ids().contains(&"paste-1".to_string()));
    // Nothing reached the agent: no session was created for this chat and the only
    // replies are the link and the outcome (no placeholder, no streamed answer).
    let key = ironhermes_gateway::session::SessionKey::new(Platform::Telegram, "chat-1")
        .with_user("owner");
    assert!(
        store.read().await.get(&key).is_none(),
        "the pasted message must not create or enter a session"
    );
    assert_eq!(adapter.texts().len(), 2, "{:?}", adapter.texts());
}

#[tokio::test]
async fn hanging_completion_does_not_block_and_times_out() {
    let fake = FakePasteBack::new();
    fake.set_complete_mode(CompleteMode::Hang);
    let handler = build_handler_with(fake.clone(), build_test_session_store(), |h| {
        h.set_paste_process_budget_for_tests(Duration::from_millis(300));
    });
    let adapter = RecordingAdapter::new();
    run_command(
        &handler,
        &adapter,
        &dm("chat-1", "m-1", "/mcp connect demo_cf"),
    )
    .await;

    let paste = dm("chat-1", "paste-1", CALLBACK);
    let started = std::time::Instant::now();
    assert!(handler.intercept_oauth_paste(&paste, as_dyn(&adapter)));
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "the dispatch-loop decision must not wait on the exchange: {:?}",
        started.elapsed()
    );

    // An ordinary command still completes while the exchange hangs.
    run_command(&handler, &adapter, &dm("chat-2", "m-2", "/help")).await;

    assert!(
        wait_until(|| adapter.texts().iter().any(|t| t.contains("timed out"))).await,
        "{:?}",
        adapter.texts()
    );
    assert_eq!(fake.cancel_calls(), vec!["st-1".to_string()]);
}

#[tokio::test]
async fn failing_delete_still_consumes() {
    let fake = FakePasteBack::new();
    let handler = build_handler(fake.clone());
    let adapter = RecordingAdapter::failing_delete();
    run_command(
        &handler,
        &adapter,
        &dm("chat-1", "m-1", "/mcp connect demo_cf"),
    )
    .await;

    let paste = dm("chat-1", "paste-1", CALLBACK);
    assert!(
        handler.intercept_oauth_paste(&paste, as_dyn(&adapter)),
        "a failed delete must not turn a consumed callback into a dispatch"
    );
    assert!(
        wait_until(|| adapter
            .texts()
            .iter()
            .any(|t| t.contains("Authorized demo_cf")))
        .await
    );
    assert_eq!(fake.complete_calls().len(), 1);
}

#[tokio::test]
async fn concurrent_pastes_complete_at_most_once() {
    let fake = FakePasteBack::new();
    let handler = build_handler(fake.clone());
    let adapter = RecordingAdapter::new();
    run_command(
        &handler,
        &adapter,
        &dm("chat-1", "m-1", "/mcp connect demo_cf"),
    )
    .await;

    let first = dm("chat-1", "paste-1", CALLBACK);
    let second = dm("chat-1", "paste-2", CALLBACK);
    assert!(handler.intercept_oauth_paste(&first, as_dyn(&adapter)));
    assert!(handler.intercept_oauth_paste(&second, as_dyn(&adapter)));

    assert!(
        wait_until(|| adapter.texts().len() >= 3).await,
        "{:?}",
        adapter.texts()
    );
    assert_eq!(fake.complete_calls().len(), 1, "complete runs at most once");
    let texts = adapter.texts();
    assert!(
        texts.iter().any(|t| t.contains("Authorized demo_cf")),
        "{texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("No pending authorization")),
        "{texts:?}"
    );
}

#[tokio::test]
async fn paste_flow_never_logs_or_persists_the_code() {
    let logs = LogBuf::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(logs.clone())
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    // Thread-local default: #[tokio::test] is a current-thread runtime, so the
    // spawned processing task logs through this subscriber too.
    let _guard = tracing::subscriber::set_default(subscriber);

    let fake = FakePasteBack::new();
    let store = build_test_session_store();
    let handler = build_handler_with(fake.clone(), store.clone(), |_| {});
    // A failing delete exercises the warn path as well.
    let adapter = RecordingAdapter::failing_delete();

    run_command(
        &handler,
        &adapter,
        &dm("chat-1", "m-1", "/mcp connect demo_cf"),
    )
    .await;
    let paste = dm("chat-1", "paste-1", CALLBACK);
    assert!(handler.intercept_oauth_paste(&paste, as_dyn(&adapter)));
    assert!(
        wait_until(|| adapter
            .texts()
            .iter()
            .any(|t| t.contains("Authorized demo_cf")))
        .await
    );

    let captured = logs.contents();
    // Proof the capture is live (the failing-delete adapter makes the handler warn),
    // so the absence checks below are not vacuous.
    assert!(
        captured.contains("could not delete the pasted OAuth callback message"),
        "log capture saw nothing — the absence checks would be vacuous:\n{captured}"
    );
    assert!(
        !captured.contains("cd-123"),
        "the authorization code reached a log line:\n{captured}"
    );
    assert!(
        !captured.contains("st-1"),
        "the OAuth state reached a log line:\n{captured}"
    );

    let key = ironhermes_gateway::session::SessionKey::new(Platform::Telegram, "chat-1")
        .with_user("owner");
    let guard = store.read().await;
    if let Some(session) = guard.get(&key) {
        for m in &session.messages {
            let rendered = format!("{m:?}");
            assert!(
                !rendered.contains("cd-123"),
                "the code reached session history: {rendered}"
            );
        }
    }
}

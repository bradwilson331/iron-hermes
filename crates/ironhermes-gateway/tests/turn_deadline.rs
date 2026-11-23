//! Phase 54.8 Plan 03 (D-07) — gateway turn deadline integration tests.
//!
//! A hung LLM / tool / MCP call used to hold a chat worker and one of the
//! global run permits forever. These tests drive the REAL
//! `GatewayMessageHandler` against a wiremock provider that answers only
//! after a long delay, and assert the turn is stopped, the user is told, and
//! (Task 3) the run permit comes back through the same functions the runner
//! calls.
//!
//! Each test binary is its own crate, so the harness (RecordingAdapter,
//! event builders) is copied here per the existing convention
//! (`concurrent_turns_gateway.rs`, `buzz_agent_turn.rs`).

use std::sync::Arc;
use std::time::Duration;

use ironhermes_agent::AgentRuntime;
use ironhermes_core::{Config, MessageEvent, MessageResponse, Platform, ProviderResolver};
use ironhermes_gateway::adapter::PlatformAdapter;
use ironhermes_gateway::handler::{GatewayMessageHandler, TURN_TIMEOUT_NOTICE_PREFIX, TurnBudgets};
use ironhermes_gateway::multimodal::ProcessedAttachments;
use ironhermes_gateway::session::SessionStore;
use ironhermes_gateway::turn_slot::{SlotOutcome, run_slash_command, run_turn_slot};
use ironhermes_tools::ToolRegistry;
use tokio::sync::RwLock;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn build_test_session_store() -> Arc<RwLock<SessionStore>> {
    let state_store = Arc::new(std::sync::Mutex::new(
        ironhermes_state::StateStore::new(":memory:").expect("in-memory StateStore"),
    ));
    Arc::new(RwLock::new(SessionStore::new(state_store)))
}

fn build_handler(config: Config, runtime: Arc<AgentRuntime>) -> GatewayMessageHandler {
    let resolver = ProviderResolver::build(&config).unwrap();
    let tool_registry = Arc::new(RwLock::new(ToolRegistry::new()));
    let mut handler =
        GatewayMessageHandler::new(config, resolver, build_test_session_store(), tool_registry);
    handler.set_agent_runtime(runtime);
    handler
}

fn dm_event(chat_id: &str, content: &str) -> MessageEvent {
    MessageEvent {
        platform: Platform::Telegram,
        message_id: "msg-1".to_string(),
        chat_id: chat_id.to_string(),
        sender_id: format!("user-{chat_id}"),
        content: content.to_string(),
        attachments: vec![],
        thread_id: None,
        chat_type: "dm".to_string(),
        chat_name: None,
        sender_name: None,
        replied_to_id: None,
    }
}

fn no_attachments() -> ProcessedAttachments {
    ProcessedAttachments {
        text_prefix: None,
        image_data_uri: None,
        image_cache_path: None,
    }
}

/// A provider that holds every chat-completions request for `delay`.
async fn delayed_provider(delay: Duration) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(delay)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw("data: [DONE]\n\n", "text/event-stream"),
        )
        .mount(&server)
        .await;
    server
}

/// Records every send_message / send_message_markdown_v2 body.
struct RecordingAdapter {
    log: tokio::sync::Mutex<Vec<(String, String)>>,
}

impl RecordingAdapter {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            log: tokio::sync::Mutex::new(Vec::new()),
        })
    }

    async fn texts(&self) -> Vec<String> {
        self.log
            .lock()
            .await
            .iter()
            .map(|(_, t)| t.clone())
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
        self.log
            .lock()
            .await
            .push((chat_id.to_string(), content.to_string()));
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

// ---------------------------------------------------------------------------
// Task 1 — a hung provider turn is stopped and the user is told
// ---------------------------------------------------------------------------

#[tokio::test]
async fn turn_times_out_and_user_is_told() {
    let server = delayed_provider(Duration::from_secs(30)).await;
    let runtime = Arc::new(AgentRuntime::for_tests_with_base_url(server.uri()));
    let mut config = Config::default();
    config.gateway.turn_timeout_secs = 1;
    let handler = build_handler(config, runtime);

    let adapter = RecordingAdapter::new();
    let adapter_dyn: Arc<dyn PlatformAdapter> = adapter.clone();
    let event = dm_event("chat-t1", "hello");

    let outcome = tokio::time::timeout(
        Duration::from_secs(20),
        handler.handle_with_multimodal(
            &event,
            adapter_dyn,
            CancellationToken::new(),
            no_attachments(),
        ),
    )
    .await
    .expect("the 30 s provider delay must not hold the handler past the 1 s deadline");
    assert!(
        outcome.is_ok(),
        "handler returns Ok after a timeout: {outcome:?}"
    );

    let texts = adapter.texts().await;
    assert!(
        texts.iter().any(|t| t.contains("This request timed out")),
        "the user must get the distinct timeout notice, got {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains(TURN_TIMEOUT_NOTICE_PREFIX)),
        "the notice starts with the shared prefix constant, got {texts:?}"
    );
    assert!(
        !texts.iter().any(|t| t.contains("Something went wrong")),
        "the generic error text must not replace the timeout notice, got {texts:?}"
    );
}

// ---------------------------------------------------------------------------
// Task 2 — bounded post-turn cleanup; the /goal path
// ---------------------------------------------------------------------------

/// Small budgets so the deadline / drain / ceiling paths run in seconds.
fn test_budgets(drain_ms: u64, notice_ms: u64, slack_ms: u64) -> TurnBudgets {
    TurnBudgets {
        deadline: Some(Duration::from_secs(1)),
        cancel_grace: Duration::from_millis(200),
        drain: Duration::from_millis(drain_ms),
        notice: Duration::from_millis(notice_ms),
        slack: Duration::from_millis(slack_ms),
    }
}

/// An adapter whose network calls never complete. With `pass_first_send` the
/// very first `send_message` (the placeholder) succeeds so the turn actually
/// starts and the post-turn cleanup is what hangs.
struct HangingAdapter {
    sends: std::sync::atomic::AtomicUsize,
    pass_first_send: bool,
}

impl HangingAdapter {
    fn new(pass_first_send: bool) -> Arc<Self> {
        Arc::new(Self {
            sends: std::sync::atomic::AtomicUsize::new(0),
            pass_first_send,
        })
    }
}

#[async_trait::async_trait]
impl PlatformAdapter for HangingAdapter {
    fn platform(&self) -> Platform {
        Platform::Telegram
    }
    async fn send_message(
        &self,
        chat_id: &str,
        _content: &str,
        _thread_id: Option<&str>,
    ) -> anyhow::Result<MessageResponse> {
        let n = self.sends.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.pass_first_send && n == 0 {
            return Ok(MessageResponse {
                message_id: "placeholder".to_string(),
                chat_id: chat_id.to_string(),
                platform: Platform::Telegram,
            });
        }
        std::future::pending().await
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
        std::future::pending().await
    }
    async fn edit_message_markdown_v2(&self, _: &str, _: &str, _: &str) -> anyhow::Result<()> {
        std::future::pending().await
    }
    async fn delete_message(&self, _: &str, _: &str) -> anyhow::Result<()> {
        std::future::pending().await
    }
    async fn add_reaction(&self, _: &str, _: &str, _: &str) -> anyhow::Result<()> {
        std::future::pending().await
    }
    async fn send_chat_action(&self, _: &str, _: &str) -> anyhow::Result<()> {
        std::future::pending().await
    }
    fn is_running(&self) -> bool {
        true
    }
}

#[tokio::test]
async fn hung_adapter_does_not_hang_the_handler() {
    let server = delayed_provider(Duration::from_secs(30)).await;
    let runtime = Arc::new(AgentRuntime::for_tests_with_base_url(server.uri()));
    let mut handler = build_handler(Config::default(), runtime);
    handler.set_turn_budgets_for_tests(test_budgets(500, 500, 300));

    let adapter: Arc<dyn PlatformAdapter> = HangingAdapter::new(true);
    let event = dm_event("chat-hang", "hello");
    let start = std::time::Instant::now();
    let outcome = tokio::time::timeout(
        Duration::from_secs(10),
        handler.handle_with_multimodal(&event, adapter, CancellationToken::new(), no_attachments()),
    )
    .await
    .expect("a hung adapter must not hold the handler past its bounded cleanup");
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "handler took {:?}",
        start.elapsed()
    );
    assert!(outcome.is_ok(), "{outcome:?}");
}

#[tokio::test]
async fn hung_adapter_placeholder_send_is_bounded() {
    let server = delayed_provider(Duration::from_secs(30)).await;
    let runtime = Arc::new(AgentRuntime::for_tests_with_base_url(server.uri()));
    let mut handler = build_handler(Config::default(), runtime);
    handler.set_turn_budgets_for_tests(test_budgets(500, 500, 300));

    // Even the very first send hangs: the handler must give up, not hang.
    let adapter: Arc<dyn PlatformAdapter> = HangingAdapter::new(false);
    let event = dm_event("chat-hang0", "hello");
    let start = std::time::Instant::now();
    let _ = tokio::time::timeout(
        Duration::from_secs(10),
        handler.handle_with_multimodal(&event, adapter, CancellationToken::new(), no_attachments()),
    )
    .await
    .expect("a hung placeholder send must be bounded");
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn goal_turn_times_out_and_user_is_told() {
    let server = delayed_provider(Duration::from_secs(30)).await;
    let runtime = Arc::new(AgentRuntime::for_tests_with_base_url(server.uri()));
    let mut config = Config::default();
    config.gateway.turn_timeout_secs = 1;
    // /goal builds its judge up front; point it at the same mock with an
    // obviously fake key. The judge is never reached: the turn times out first.
    config.model.api_key = Some("test-key-not-real".to_string());
    config.model.base_url = Some(server.uri());
    let handler = build_handler(config, runtime);

    let adapter = RecordingAdapter::new();
    let adapter_dyn: Arc<dyn PlatformAdapter> = adapter.clone();
    let event = dm_event("chat-goal", "/goal write a haiku about timeouts");
    handler
        .handle_with_multimodal(
            &event,
            adapter_dyn,
            CancellationToken::new(),
            no_attachments(),
        )
        .await
        .expect("/goal start returns immediately");

    // The goal loop runs in a spawned task: wait for the notice.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let texts = adapter.texts().await;
        if texts.iter().any(|t| t.contains("This request timed out")) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no timeout notice from the /goal runner, got {texts:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // The loop stopped: no further provider request after the notice.
    let before = server.received_requests().await.unwrap_or_default().len();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let after = server.received_requests().await.unwrap_or_default().len();
    assert_eq!(
        before, after,
        "the goal loop must stop after a timed-out turn"
    );
}

// Phase 58.3.1 D-05 (WR-03): a /goal turn must render the once-per-session compact notice.
#[tokio::test]
async fn goal_turn_shows_compact_notice_once() {
    let server = MockServer::start().await;
    let json_body = r#"{"id":"c1","object":"chat.completion","created":1,"model":"test-model","choices":[{"index":0,"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":1,"total_tokens":6}}"#;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_raw(json_body, "application/json"))
        .mount(&server)
        .await;

    let mut config = Config::default();
    config.model.default = "test-model".to_string();
    config.model.api_key = Some("test-key-not-real".to_string());
    config.model.base_url = Some(server.uri());
    config.agent.compact_profile = ironhermes_core::config::CompactProfileMode::On;
    let mut models = std::collections::HashMap::new();
    models.insert(
        "test-model".to_string(),
        ironhermes_core::config_extras::ProviderModelConfig {
            context_length: Some(32_000),
            ..Default::default()
        },
    );
    config.providers.insert(
        "openrouter".to_string(),
        ironhermes_core::ProviderConfig {
            base_url: Some(server.uri()),
            api_key: Some("test-key-not-real".to_string()),
            models,
            ..Default::default()
        },
    );
    let runtime = Arc::new(AgentRuntime::for_tests_with_base_url(server.uri()));
    runtime
        .reload_config_and_resolver(
            Arc::new(config.clone()),
            Arc::new(ProviderResolver::build(&config).unwrap()),
        )
        .await
        .expect("reload with compact profile");
    let handler = build_handler(config, runtime);

    let adapter = RecordingAdapter::new();
    let adapter_dyn: Arc<dyn PlatformAdapter> = adapter.clone();
    let event = dm_event(
        "chat-goal-notice",
        "/goal write a haiku about small windows",
    );
    handler
        .handle_with_multimodal(
            &event,
            adapter_dyn,
            CancellationToken::new(),
            no_attachments(),
        )
        .await
        .expect("/goal start returns immediately");

    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let texts = loop {
        let texts = adapter.texts().await;
        if texts
            .iter()
            .any(|t| t.starts_with("/goal complete") || t.starts_with("/goal stopped"))
        {
            break texts;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "/goal never finished, got {texts:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    let notices: Vec<&String> = texts
        .iter()
        .filter(|t| t.contains("compact profile"))
        .collect();
    assert_eq!(
        notices.len(),
        1,
        "expected the compact notice exactly once, got {texts:?}"
    );
    assert!(
        notices[0].contains("--- Context Warnings ---"),
        "notice must use the Context Warnings block: {texts:?}"
    );
}

// ---------------------------------------------------------------------------
// Task 3 — the runner-owned permit path (turn_slot)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn permits_are_restored_after_deadlines_and_next_message_runs() {
    let server = delayed_provider(Duration::from_secs(30)).await;
    let runtime = Arc::new(AgentRuntime::for_tests_with_base_url(server.uri()));
    let mut handler = build_handler(Config::default(), runtime);
    handler.set_turn_budgets_for_tests(test_budgets(500, 500, 300));
    let handler = Arc::new(handler);

    let semaphore = Arc::new(Semaphore::new(2));
    let adapter = RecordingAdapter::new();
    let adapter_dyn: Arc<dyn PlatformAdapter> = adapter.clone();
    let cancel = CancellationToken::new();
    let (ev_a, ev_b) = (dm_event("chat-a", "hello"), dm_event("chat-b", "hello"));

    let slot_a = run_turn_slot(
        &semaphore,
        &handler,
        &ev_a,
        adapter_dyn.clone(),
        &cancel,
        || async { no_attachments() },
    );
    let slot_b = run_turn_slot(
        &semaphore,
        &handler,
        &ev_b,
        adapter_dyn.clone(),
        &cancel,
        || async { no_attachments() },
    );
    let probe = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        semaphore.available_permits()
    };
    let (held, (out_a, out_b)) = tokio::time::timeout(Duration::from_secs(10), async {
        let (a, b, p) = tokio::join!(slot_a, slot_b, probe);
        (p, (a, b))
    })
    .await
    .expect("both hung turns must release their permits within the ceiling");
    assert_eq!(held, 0, "both permits are held while the turns hang");
    assert!(matches!(out_a, SlotOutcome::Completed(Ok(()))), "{out_a:?}");
    assert!(matches!(out_b, SlotOutcome::Completed(Ok(()))), "{out_b:?}");
    assert_eq!(semaphore.available_permits(), 2, "permits restored");

    let texts = adapter.texts().await;
    let notices = texts
        .iter()
        .filter(|t| t.contains("This request timed out"))
        .count();
    assert_eq!(notices, 2, "one timeout notice per chat, got {texts:?}");

    // The next message is served: a third chat's /help completes with a reply.
    let before = adapter.texts().await.len();
    let ev_c = dm_event("chat-c", "/help");
    let out_c = run_turn_slot(
        &semaphore,
        &handler,
        &ev_c,
        adapter_dyn,
        &cancel,
        || async { no_attachments() },
    )
    .await;
    assert!(matches!(out_c, SlotOutcome::Completed(Ok(()))), "{out_c:?}");
    assert!(
        adapter.texts().await.len() > before,
        "the follow-up message must get a reply"
    );
    assert_eq!(semaphore.available_permits(), 2);
}

#[tokio::test]
async fn control_command_runs_while_permits_are_exhausted() {
    let server = delayed_provider(Duration::from_secs(30)).await;
    let runtime = Arc::new(AgentRuntime::for_tests_with_base_url(server.uri()));
    let mut handler = build_handler(Config::default(), runtime);
    // A long deadline: the hung turn keeps its permit for the whole test.
    handler.set_turn_budgets_for_tests(TurnBudgets {
        deadline: Some(Duration::from_secs(5)),
        cancel_grace: Duration::from_millis(200),
        drain: Duration::from_millis(500),
        notice: Duration::from_millis(500),
        slack: Duration::from_millis(300),
    });
    let handler = Arc::new(handler);
    let semaphore = Arc::new(Semaphore::new(1));
    let adapter = RecordingAdapter::new();
    let adapter_dyn: Arc<dyn PlatformAdapter> = adapter.clone();
    let cancel = CancellationToken::new();

    let hung = {
        let (sem, h, a, c) = (
            semaphore.clone(),
            handler.clone(),
            adapter_dyn.clone(),
            cancel.clone(),
        );
        tokio::spawn(async move {
            let ev = dm_event("chat-hung", "hello");
            run_turn_slot(&sem, &h, &ev, a, &c, || async { no_attachments() }).await
        })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        semaphore.available_permits(),
        0,
        "the hung turn holds the only permit"
    );

    let before = adapter.texts().await.len();
    let stop = dm_event("chat-other", "/stop");
    let out = tokio::time::timeout(
        Duration::from_secs(2),
        run_slash_command(&semaphore, &handler, &stop, adapter_dyn, &cancel),
    )
    .await
    .expect("/stop must not wait for a run permit");
    assert!(matches!(out, SlotOutcome::Completed(Ok(()))), "{out:?}");
    assert!(
        adapter.texts().await.len() > before,
        "/stop must reply while every run slot is busy"
    );
    assert_eq!(
        semaphore.available_permits(),
        0,
        "the control lane took no permit"
    );

    hung.abort();
}

#[tokio::test]
async fn ceiling_releases_permit_when_prepare_hangs() {
    let server = delayed_provider(Duration::from_secs(30)).await;
    let runtime = Arc::new(AgentRuntime::for_tests_with_base_url(server.uri()));
    let mut handler = build_handler(Config::default(), runtime);
    // ceiling = 1 s + 200 ms + 200 ms + 200 ms + 300 ms = 1.9 s
    handler.set_turn_budgets_for_tests(test_budgets(200, 200, 300));
    let handler = Arc::new(handler);
    let semaphore = Arc::new(Semaphore::new(1));
    let hanging = HangingAdapter::new(false);
    let adapter_dyn: Arc<dyn PlatformAdapter> = hanging;
    let cancel = CancellationToken::new();
    let ev = dm_event("chat-prep", "hello");

    // The worker's eyes-reaction step, which no per-step budget covers.
    let adapter_prep = adapter_dyn.clone();
    let prepare = move || async move {
        let _ = adapter_prep
            .add_reaction("chat-prep", "msg-1", "eyes")
            .await;
        no_attachments()
    };
    let start = std::time::Instant::now();
    let out = tokio::time::timeout(
        Duration::from_secs(4),
        run_turn_slot(&semaphore, &handler, &ev, adapter_dyn, &cancel, prepare),
    )
    .await
    .expect("the ceiling must bound a hung prepare step");
    assert!(matches!(out, SlotOutcome::CeilingHit), "{out:?}");
    assert!(start.elapsed() < Duration::from_secs(4));
    assert_eq!(
        semaphore.available_permits(),
        1,
        "permit released by the ceiling"
    );
}

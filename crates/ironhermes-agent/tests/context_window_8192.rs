//! Phase 58.3.1 Plan 01: the agent loop honours an 8,192-token window end to end.
//!
//! The strict fake provider below counts a request with a DELIBERATELY DIFFERENT
//! counter (`ceil(chars / divisor)`) from the agent's cl100k estimator, so a green
//! test means the request really fits, not that the estimator agreed with itself.
//! Every request-count assertion uses `chat_posts` (chat-completion POSTs only).

use std::sync::{Arc, Mutex};

use ironhermes_agent::AgentLoop;
use ironhermes_agent::any_client::AnyClient;
use ironhermes_agent::budget::BudgetHandle;
use ironhermes_agent::client::LlmClient;
use ironhermes_agent::context_compressor::estimate_messages_tokens;
use ironhermes_agent::fit;
use ironhermes_core::config::{ApiMode, Config, ProviderConfig};
use ironhermes_core::config_extras::ProviderModelConfig;
use ironhermes_core::models_cache::ModelsCache;
use ironhermes_core::observed_windows;
use ironhermes_core::provider::{ContextLengthSource, ProviderResolver, ResolvedEndpoint};
use ironhermes_core::{
    ChatMessage, FunctionCall, TiktokenEncoding, ToolCall, validate_tool_call_pairing,
};
use ironhermes_tools::ToolRegistry;
use tokio::sync::RwLock;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const WINDOW: usize = 8192;
const CANNED_REPLY: &str = "Here is the summary you asked for.";
const FINAL_QUESTION: &str = "FINAL-QUESTION-MARKER-7731 please summarize what we discussed.";

/// Production path (ironhermes-cli / -gateway) initialises cl100k at startup. Without
/// this the estimator is the `len/4+1` fallback and the divisor-4.0 fake is nearly
/// the same counter. Idempotent; called first in EVERY test.
fn init_estimator() {
    ironhermes_core::init_global_estimator(TiktokenEncoding::Cl100kBase);
}

/// The fake's own count: chars of every message content, tool-call arguments and
/// the compact `tools` JSON, over `divisor`, plus 4 per message and `max_tokens`.
fn fake_count_body(body: &serde_json::Value, divisor: f64) -> usize {
    let mut chars = 0usize;
    let mut msgs = 0usize;
    if let Some(arr) = body.get("messages").and_then(|m| m.as_array()) {
        for m in arr {
            msgs += 1;
            if let Some(c) = m.get("content").and_then(|c| c.as_str()) {
                chars += c.len();
            }
            if let Some(tcs) = m.get("tool_calls").and_then(|t| t.as_array()) {
                for tc in tcs {
                    if let Some(a) = tc.pointer("/function/arguments").and_then(|a| a.as_str()) {
                        chars += a.len();
                    }
                }
            }
        }
    }
    if let Some(tools) = body.get("tools") {
        chars += tools.to_string().len();
    }
    let max_tokens = body.get("max_tokens").and_then(|m| m.as_u64()).unwrap_or(0) as usize;
    (chars as f64 / divisor).ceil() as usize + 4 * msgs + max_tokens
}

#[derive(Clone, Copy)]
struct FakeConfig {
    window: usize,
    divisor: f64,
    /// Reject every request regardless of size.
    always_reject: bool,
    /// Rejection body carries no digits (D-08 halving case).
    no_digits_body: bool,
}

impl FakeConfig {
    fn strict(divisor: f64) -> Self {
        Self {
            window: WINDOW,
            divisor,
            always_reject: false,
            no_digits_body: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Served {
    count: usize,
    status: u16,
}

struct StrictWindowFake {
    cfg: FakeConfig,
    log: Arc<Mutex<Vec<Served>>>,
}

impl Respond for StrictWindowFake {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value =
            serde_json::from_slice(&request.body).expect("request body is JSON");
        let count = fake_count_body(&body, self.cfg.divisor);
        let reject = self.cfg.always_reject || count > self.cfg.window;
        let status = if reject { 400 } else { 200 };
        self.log.lock().unwrap().push(Served { count, status });
        if reject {
            let message = if self.cfg.no_digits_body {
                "Context length exceeded. Try shortening the conversation.".to_string()
            } else {
                format!(
                    "Message too long: {count} tokens exceeds the {}-token context window. Try increasing the Context Length in Model settings, or shorten the conversation.",
                    self.cfg.window
                )
            };
            return ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": {
                    "message": message,
                    "type": "invalid_request_error",
                    "param": "messages",
                    "code": "context_length_exceeded"
                }
            }));
        }
        if body.get("stream").and_then(|s| s.as_bool()) == Some(true) {
            let mut sse = String::new();
            for chunk in [
                format!(
                    r#"{{"id":"c1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{{"index":0,"delta":{{"content":"{CANNED_REPLY}"}}}}]}}"#
                ),
                r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#.to_string(),
            ] {
                sse.push_str("data: ");
                sse.push_str(&chunk);
                sse.push_str("\n\n");
            }
            sse.push_str("data: [DONE]\n\n");
            return ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream");
        }
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "resp-1",
            "object": "chat.completion",
            "created": 0,
            "model": "m",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": CANNED_REPLY },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }
        }))
    }
}

/// Start a server whose ONLY route is POST .../chat/completions, backed by the
/// strict fake. Other requests (probe GETs in later plans) get wiremock's 404 and
/// never reach the fake.
async fn start_fake(cfg: FakeConfig) -> (MockServer, Arc<Mutex<Vec<Served>>>) {
    let server = MockServer::start().await;
    let log = Arc::new(Mutex::new(Vec::new()));
    Mock::given(method("POST"))
        .and(path_regex("/chat/completions$"))
        .respond_with(StrictWindowFake {
            cfg,
            log: log.clone(),
        })
        .mount(&server)
        .await;
    (server, log)
}

/// Chat-completion POSTs only; the single source for request-count assertions.
async fn chat_posts(server: &MockServer) -> Vec<Request> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/chat/completions"))
        .collect()
}

fn client_for(server: &MockServer) -> AnyClient {
    AnyClient::ChatCompletions(LlmClient::new(server.uri(), "test-key", "test-model"))
}

fn empty_registry() -> Arc<RwLock<ToolRegistry>> {
    Arc::new(RwLock::new(ToolRegistry::new()))
}

const SENTENCES: &[&str] = &[
    "The committee met on Tuesday morning to discuss the new schedule for the harbor repairs.",
    "Several members argued that the work should begin before the winter storms arrive.",
    "A long report from the engineering team described the condition of the old stone wall.",
    "Nobody could agree on who should pay for the materials, so the matter was postponed again.",
    "The treasurer reminded everyone that last year's budget had already been spent on lighting.",
    "After lunch the group walked down to the water and looked at the cracked concrete steps.",
    "One of the older fishermen said that the tides had changed since he was a young man.",
    "The mayor promised to write a letter to the regional office asking for more support.",
    "In the afternoon they reviewed the photographs that the surveyor had taken the week before.",
    "It was decided that a smaller team would visit each of the three piers and make notes.",
    "The notes would then be gathered into a single document and shared with the whole town.",
    "A few residents were worried that the noise from the repairs would frighten the visiting birds.",
    "The naturalist explained that the birds usually leave the harbor by the middle of November.",
    "Everyone seemed pleased with that answer and the discussion moved on to the question of parking.",
    "Parking had been a problem for years, especially during the summer festival and the market days.",
    "The clerk read out a list of complaints that had been collected over the past several months.",
    "Most of them concerned the narrow road, the missing signs and the broken lamp near the bakery.",
    "The group agreed to look into each complaint carefully before making any promises to the public.",
    "As the evening came on, people began to gather their coats and their papers and say goodnight.",
    "The final vote was taken just before seven o'clock and the meeting was closed with a short prayer.",
];

/// Natural English prose of about `chars` characters; `salt` shifts the sentence
/// order so successive messages are not byte-identical.
fn prose(chars: usize, salt: usize) -> String {
    let mut out = String::new();
    let mut i = salt;
    while out.len() < chars {
        out.push_str(SENTENCES[i % SENTENCES.len()]);
        out.push(' ');
        i += 7;
    }
    out
}

/// system + 12 user/assistant pairs of prose (two of them route through an
/// assistant tool_call + tool-result pair) + the final user question.
fn long_seed(msg_chars: usize) -> Vec<ChatMessage> {
    let mut seed = vec![ChatMessage::system(
        "You are a careful assistant who answers questions about meeting notes.",
    )];
    for k in 0..12 {
        seed.push(ChatMessage::user(prose(msg_chars, k)));
        if k == 4 || k == 8 {
            let id = format!("call_{k}");
            seed.push(ChatMessage::assistant_tool_calls(vec![ToolCall {
                id: id.clone(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "echo".to_string(),
                    arguments: format!(r#"{{"q":"lookup {k}"}}"#),
                },
            }]));
            seed.push(ChatMessage::tool_result(id, prose(msg_chars, k + 50)));
        }
        seed.push(ChatMessage::assistant(prose(msg_chars, k + 100)));
    }
    seed.push(ChatMessage::user(FINAL_QUESTION));
    seed
}

/// Seed length is tuned so `estimate_messages_tokens` lands near 27.6K, the observed
/// failure (`Message too long: 27652 tokens exceeds the 8192-token context window`).
const SEED_MSG_CHARS: usize = 5_465;

fn assert_every_request_fits_and_pairs(posts: &[Request], log: &Arc<Mutex<Vec<Served>>>) {
    for served in log.lock().unwrap().iter() {
        assert!(
            served.count <= WINDOW,
            "fake counted {} > {WINDOW}",
            served.count
        );
        assert_eq!(served.status, 200, "no request may be answered with a 400");
    }
    for post in posts {
        let body: serde_json::Value = serde_json::from_slice(&post.body).unwrap();
        let msgs: Vec<ChatMessage> =
            serde_json::from_value(body["messages"].clone()).expect("messages deserialize");
        validate_tool_call_pairing(&msgs).expect("tool pairs intact on the wire");
    }
}

#[tokio::test]
async fn cw8192_d20_long_conversation_fits_and_replies() {
    init_estimator();
    let seed = long_seed(SEED_MSG_CHARS);
    let est = estimate_messages_tokens(&seed);
    assert!(
        (27_000..=28_500).contains(&est),
        "seed estimate {est} outside 27,000..=28,500"
    );
    // Proof independence: the fake counts the seed at least 3% above the estimator.
    let fake_seed = fake_count_body(&serde_json::json!({ "messages": seed }), 4.0);
    assert!(
        fake_seed * 100 >= est * 103,
        "fake count {fake_seed} is not >= 3% above estimate {est}: estimator not cl100k, or seed is not prose"
    );

    let (server, log) = start_fake(FakeConfig::strict(4.0)).await;
    let mut agent =
        AgentLoop::new(client_for(&server), empty_registry(), 4).with_context_length(WINDOW);
    let result = agent.run(seed).await.expect("run must reply");

    assert_eq!(result.final_response.as_deref(), Some(CANNED_REPLY));
    let posts = chat_posts(&server).await;
    assert_eq!(posts.len(), 1, "guard compacts before sending: one request");
    assert_every_request_fits_and_pairs(&posts, &log);
    let last_body = String::from_utf8_lossy(&posts.last().unwrap().body).to_string();
    assert!(last_body.contains("FINAL-QUESTION-MARKER-7731"));
}

#[tokio::test]
async fn cw8192_d20_floor_too_large_refuses_without_sending() {
    init_estimator();
    let system = ChatMessage::system(prose(48_000, 0));
    let floor_prompt = estimate_messages_tokens(std::slice::from_ref(&system));
    assert!(
        floor_prompt > WINDOW,
        "system message must exceed the window"
    );
    let floor = fit::floor_tokens(floor_prompt, WINDOW);

    let (server, _log) = start_fake(FakeConfig::strict(4.0)).await;
    let mut agent =
        AgentLoop::new(client_for(&server), empty_registry(), 4).with_context_length(WINDOW);
    let err = agent
        .run(vec![system, ChatMessage::user("hi")])
        .await
        .expect_err("an oversized floor must be refused");
    let text = err.to_string();
    assert!(text.contains("8192"), "{text}");
    assert!(
        text.contains(&floor.to_string()),
        "{text} lacks floor {floor}"
    );
    // No window endpoint, so the window did not come from the server: the override
    // is offered, capped at what the server really has loaded (G-58.3.1-8).
    assert!(text.contains("never higher"), "{text}");
    assert!(chat_posts(&server).await.is_empty(), "nothing may be sent");
}

#[tokio::test]
async fn cw8192_engineless_loop_is_guarded() {
    init_estimator();
    let seed = long_seed(SEED_MSG_CHARS);
    let (server, log) = start_fake(FakeConfig::strict(4.0)).await;
    // Cron-runner shape: no context engine, no compressor; streaming mode.
    let mut agent = AgentLoop::new(client_for(&server), empty_registry(), 4)
        .with_budget(BudgetHandle::new(4))
        .with_streaming(Box::new(|_| {}))
        .with_context_length(WINDOW);
    let result = agent.run(seed).await.expect("run must reply");

    assert_eq!(result.final_response.as_deref(), Some(CANNED_REPLY));
    let posts = chat_posts(&server).await;
    assert!(!posts.is_empty());
    assert_every_request_fits_and_pairs(&posts, &log);
}

/// `n` alternating user/assistant prose messages (ends on a user turn when `n` is odd).
fn alternating_seed(n: usize, msg_chars: usize) -> Vec<ChatMessage> {
    let mut seed = vec![ChatMessage::system("You are a careful assistant.")];
    for k in 0..n {
        if k % 2 == 0 {
            seed.push(ChatMessage::user(prose(msg_chars, k)));
        } else {
            seed.push(ChatMessage::assistant(prose(msg_chars, k)));
        }
    }
    seed
}

/// Exactly one 400 then one accepted request; the accepted one fits by the fake's count.
fn assert_one_rejection_then_fit(log: &Arc<Mutex<Vec<Served>>>) {
    let served = log.lock().unwrap().clone();
    let statuses: Vec<u16> = served.iter().map(|s| s.status).collect();
    assert_eq!(statuses, vec![400, 200], "served: {served:?}");
    assert!(
        served[1].count <= WINDOW,
        "retry counted {}",
        served[1].count
    );
}

#[tokio::test]
async fn cw8192_d06_unknown_window_learns_8192_and_replies() {
    init_estimator();
    // Default window (128,000): the guard lets the whole seed through, the fake rejects
    // it and states its 8192 window; the loop learns it, compresses and retries once.
    let (server, log) = start_fake(FakeConfig::strict(4.0)).await;
    let mut agent = AgentLoop::new(client_for(&server), empty_registry(), 4);
    let result = agent
        .run(long_seed(SEED_MSG_CHARS))
        .await
        .expect("run must reply");

    assert_eq!(result.final_response.as_deref(), Some(CANNED_REPLY));
    let posts = chat_posts(&server).await;
    assert_eq!(posts.len(), 2);
    assert_one_rejection_then_fit(&log);
    let retry: serde_json::Value = serde_json::from_slice(&posts[1].body).unwrap();
    let msgs: Vec<ChatMessage> = serde_json::from_value(retry["messages"].clone()).unwrap();
    validate_tool_call_pairing(&msgs).expect("retry keeps tool pairs intact");
}

#[tokio::test]
async fn cw8192_d21_estimator_undercount_learns_and_retries_once() {
    init_estimator();
    // The fake's count runs ~3x above the estimator (divisor 1.6 on this prose), so the
    // guard's first request (which fits by estimate) is rejected: the backstop fires once.
    // Small messages (~300 tokens) so every compaction pass has something to drop.
    let (server, log) = start_fake(FakeConfig::strict(1.6)).await;
    let mut agent = AgentLoop::new(client_for(&server), empty_registry(), 4)
        .with_streaming(Box::new(|_| {}))
        .with_context_length(WINDOW);
    let result = agent
        .run(alternating_seed(91, 1_540))
        .await
        .expect("run must reply");

    assert_eq!(result.final_response.as_deref(), Some(CANNED_REPLY));
    assert_eq!(chat_posts(&server).await.len(), 2);
    assert_one_rejection_then_fit(&log);
}

#[tokio::test]
async fn cw8192_d08_no_number_halves_window_for_retry() {
    init_estimator();
    // Window 16,384 (the guard lets ~9.5K estimator tokens through); the fake is strict
    // at 8192 and its 400 body has no digits, so the retry is sized for 16,384 / 2.
    let cfg = FakeConfig {
        no_digits_body: true,
        ..FakeConfig::strict(4.0)
    };
    let (server, log) = start_fake(cfg).await;
    let mut agent =
        AgentLoop::new(client_for(&server), empty_registry(), 4).with_context_length(16_384);
    let result = agent
        .run(alternating_seed(9, SEED_MSG_CHARS))
        .await
        .expect("run must reply");

    assert_eq!(result.final_response.as_deref(), Some(CANNED_REPLY));
    assert_eq!(chat_posts(&server).await.len(), 2);
    assert_one_rejection_then_fit(&log);
}

#[tokio::test]
async fn cw8192_d08_second_rejection_clear_error_no_fallback() {
    init_estimator();
    let cfg = FakeConfig {
        always_reject: true,
        ..FakeConfig::strict(4.0)
    };
    let (primary, _log) = start_fake(cfg).await;
    let (fallback, _flog) = start_fake(FakeConfig::strict(4.0)).await;
    let mut agent = AgentLoop::new(client_for(&primary), empty_registry(), 4)
        .with_fallback_named(client_for(&fallback), "fallback")
        .with_context_length(WINDOW);
    let err = agent
        .run(long_seed(SEED_MSG_CHARS))
        .await
        .expect_err("a second rejection must stop the turn");

    let text = err.to_string();
    assert!(text.contains("8192"), "{text}");
    // The rejection text carried the 8192 number, so the server reported this window.
    assert!(
        text.contains("raise the context length in the model server"),
        "{text}"
    );
    assert!(!text.contains("providers.<provider>"), "{text}");
    assert!(text.contains("fresh session"), "{text}");
    assert!(
        !text.contains("invalid_request_error"),
        "provider body must not leak: {text}"
    );
    assert_eq!(chat_posts(&primary).await.len(), 2);
    assert!(chat_posts(&fallback).await.is_empty(), "no failover");
}

// ---------------------------------------------------------------------------
// Plan 04: endpoint-backed window (D-07 / D-22 / D-06 / D-12 / D-09)
// ---------------------------------------------------------------------------

/// Serialises the tests that point `IRONHERMES_HOME` at a temp dir and record
/// observed windows (both are process-global).
static HOME_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Points `IRONHERMES_HOME` at a temp dir and clears the observed-window overlay;
/// restores the previous value on drop.
struct TempHome {
    dir: tempfile::TempDir,
    prev: Option<std::ffi::OsString>,
}

impl TempHome {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let prev = std::env::var_os("IRONHERMES_HOME");
        // SAFETY: callers hold HOME_LOCK, so no other test in this binary reads or
        // writes the variable concurrently.
        unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };
        observed_windows::reset_for_tests();
        Self { dir, prev }
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        observed_windows::reset_for_tests();
        // SAFETY: see `new`.
        unsafe {
            match self.prev.take() {
                Some(v) => std::env::set_var("IRONHERMES_HOME", v),
                None => std::env::remove_var("IRONHERMES_HOME"),
            }
        }
    }
}

fn endpoint_for(server: &MockServer) -> ResolvedEndpoint {
    ResolvedEndpoint {
        base_url: server.uri(),
        api_key: Some("test-key".to_string()),
        api_mode: ApiMode::ChatCompletions,
        default_model: "test-model".to_string(),
        fallback_providers: Vec::new(),
        model_metadata: None,
        config_context_length: None,
        provider_model_context_length: None,
        models: Vec::new(),
    }
}

fn client_for_endpoint(ep: &ResolvedEndpoint) -> AnyClient {
    AnyClient::from_endpoint(ep).expect("client")
}

#[tokio::test]
async fn cw8192_d07_learned_window_persists_and_resolves_as_learned() {
    init_estimator();
    let _guard = HOME_LOCK.lock().await;
    let home = TempHome::new();
    let (server, log) = start_fake(FakeConfig::strict(4.0)).await;
    let mut ep = endpoint_for(&server);
    ep.config_context_length = Some(256_000); // the operator's pin
    assert_eq!(ep.context_length(), 256_000);

    let mut agent = AgentLoop::new(client_for_endpoint(&ep), empty_registry(), 4)
        .with_window_endpoint(ep.clone());
    let result = agent
        .run(long_seed(SEED_MSG_CHARS))
        .await
        .expect("run must reply");
    assert_eq!(result.final_response.as_deref(), Some(CANNED_REPLY));
    assert_eq!(chat_posts(&server).await.len(), 2);
    assert_one_rejection_then_fit(&log);

    // Persisted in models-cache.json (never config.yaml).
    let cache_path = home.dir.path().join("models-cache.json");
    let raw = std::fs::read_to_string(&cache_path).expect("models-cache.json written");
    let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let windows = json["context_windows"]
        .as_object()
        .expect("context_windows");
    assert!(
        windows
            .values()
            .any(|w| w["tokens"] == 8192 && w["source"] == "learned"),
        "{raw}"
    );
    assert!(!home.dir.path().join("config.yaml").exists());

    // The endpoint's tier evaluation now reports the learned window under the pin.
    assert_eq!(
        ep.context_length_with_source(),
        (8192, ContextLengthSource::Learned)
    );

    // Simulated restart: overlay cleared, resolver rebuilt from the disk cache.
    observed_windows::reset_for_tests();
    let mut config = Config::default();
    config.model.provider = "p".to_string();
    config.providers.insert(
        "p".to_string(),
        ProviderConfig {
            base_url: Some(server.uri()),
            default_model: Some("test-model".to_string()),
            ..Default::default()
        },
    );
    let resolver =
        ProviderResolver::build_with_cache(&config, ModelsCache::load()).expect("resolver");
    assert_eq!(
        resolver.resolve_for_main().context_length_with_source(),
        (8192, ContextLengthSource::Learned)
    );
}

#[tokio::test]
async fn cw8192_d22_learned_caps_global_pin() {
    init_estimator();
    let _guard = HOME_LOCK.lock().await;
    let _home = TempHome::new();
    let (server, log) = start_fake(FakeConfig::strict(4.0)).await;
    let mut ep = endpoint_for(&server);
    ep.config_context_length = Some(256_000);

    let mut first = AgentLoop::new(client_for_endpoint(&ep), empty_registry(), 4)
        .with_window_endpoint(ep.clone());
    first
        .run(long_seed(SEED_MSG_CHARS))
        .await
        .expect("first run");
    assert_eq!(log.lock().unwrap().len(), 2, "first run: one 400, one 200");

    // A new loop re-resolves through the tiers: the pin is capped to the learned 8,192.
    let mut second = AgentLoop::new(client_for_endpoint(&ep), empty_registry(), 4)
        .with_window_endpoint(ep.clone());
    let result = second
        .run(long_seed(SEED_MSG_CHARS))
        .await
        .expect("second run");
    assert_eq!(result.final_response.as_deref(), Some(CANNED_REPLY));
    let served = log.lock().unwrap().clone();
    assert_eq!(served.len(), 3, "second run adds exactly one request");
    assert_eq!(served[2].status, 200, "second run sends zero 400s");
    assert!(served[2].count <= WINDOW);
    assert_eq!(chat_posts(&server).await.len(), 3);
}

#[tokio::test]
async fn cw8192_d22_per_model_override_still_resolves_but_retry_uses_server_window() {
    init_estimator();
    let _guard = HOME_LOCK.lock().await;
    let _home = TempHome::new();
    let (server, log) = start_fake(FakeConfig::strict(4.0)).await;
    let mut ep = endpoint_for(&server);
    // Operator override, larger than the server's real window.
    ep.provider_model_context_length = Some(32_768);

    let mut agent = AgentLoop::new(client_for_endpoint(&ep), empty_registry(), 4)
        .with_window_endpoint(ep.clone());
    let result = agent
        .run(long_seed(SEED_MSG_CHARS))
        .await
        .expect("D-06: the user still gets a reply");
    assert_eq!(result.final_response.as_deref(), Some(CANNED_REPLY));
    assert_eq!(chat_posts(&server).await.len(), 2);
    assert_one_rejection_then_fit(&log);

    // The override still wins resolution; only the one retry was sized for the server.
    assert_eq!(
        ep.context_length_with_source(),
        (32_768, ContextLengthSource::PerModelConfig)
    );
}

// ---------------------------------------------------------------------------
// Plan 05: loaded-window probe before the first request (D-10 / D-12)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cw8192_d10_probe_before_first_request_sizes_to_loaded_window() {
    init_estimator();
    let _guard = HOME_LOCK.lock().await;
    let _home = TempHome::new();
    let (server, log) = start_fake(FakeConfig::strict(4.0)).await;
    Mock::given(method("GET"))
        .and(wiremock::matchers::path("/props"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                serde_json::json!({ "default_generation_settings": { "n_ctx": 8192 } }),
            ),
        )
        .mount(&server)
        .await;
    let mut ep = endpoint_for(&server);
    ep.config_context_length = Some(256_000); // the operator's pin

    let mut agent = AgentLoop::new(client_for_endpoint(&ep), empty_registry(), 4)
        .with_window_endpoint(ep.clone());
    let result = agent
        .run(long_seed(SEED_MSG_CHARS))
        .await
        .expect("run must reply");
    assert_eq!(result.final_response.as_deref(), Some(CANNED_REPLY));

    let all = server.received_requests().await.unwrap_or_default();
    assert!(
        all.iter()
            .any(|r| r.method.as_str() == "GET" && r.url.path() == "/props"),
        "probe must GET /props"
    );
    let first_post = all
        .iter()
        .position(|r| r.method.as_str() == "POST")
        .expect("a chat POST");
    assert!(
        all[..first_post].iter().all(|r| r.method.as_str() == "GET")
            && all[first_post..]
                .iter()
                .all(|r| r.method.as_str() == "POST"),
        "every probe GET precedes the first chat POST"
    );
    let posts = chat_posts(&server).await;
    assert_every_request_fits_and_pairs(&posts, &log); // zero 400s, all fit
    assert_eq!(
        ep.context_length_with_source(),
        (8192, ContextLengthSource::Probed)
    );
}

/// G-58.3.1-8: a window the probe read from the server cannot be fixed by an
/// ironhermes override, so the refusal points at the model server instead.
#[tokio::test]
async fn cw8192_g8_probed_floor_refusal_points_at_model_server() {
    init_estimator();
    let _guard = HOME_LOCK.lock().await;
    let _home = TempHome::new();
    let (server, _log) = start_fake(FakeConfig::strict(4.0)).await;
    Mock::given(method("GET"))
        .and(wiremock::matchers::path("/props"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                serde_json::json!({ "default_generation_settings": { "n_ctx": 8192 } }),
            ),
        )
        .mount(&server)
        .await;
    let mut ep = endpoint_for(&server);
    ep.config_context_length = Some(256_000); // the operator's pin

    let mut agent = AgentLoop::new(client_for_endpoint(&ep), empty_registry(), 4)
        .with_window_endpoint(ep.clone());
    let err = agent
        .run(vec![
            ChatMessage::system(prose(48_000, 0)),
            ChatMessage::user("hi"),
        ])
        .await
        .expect_err("an oversized floor must be refused");
    let text = err.to_string();
    assert!(text.contains("8192"), "{text}");
    assert!(
        text.contains("raise the context length in the model server"),
        "{text}"
    );
    assert!(!text.contains("providers.<provider>"), "{text}");
    assert!(chat_posts(&server).await.is_empty(), "nothing may be sent");
}

#[tokio::test]
async fn cw8192_d10_silent_server_falls_back_to_error_path() {
    init_estimator();
    let _guard = HOME_LOCK.lock().await;
    let _home = TempHome::new();
    // No probe route: every status GET is a 404, so the window stays unknown.
    let (server, log) = start_fake(FakeConfig::strict(4.0)).await;
    let mut ep = endpoint_for(&server);
    ep.config_context_length = Some(256_000);

    let mut agent = AgentLoop::new(client_for_endpoint(&ep), empty_registry(), 4)
        .with_window_endpoint(ep.clone());
    let result = agent
        .run(long_seed(SEED_MSG_CHARS))
        .await
        .expect("D-06: the user still gets a reply");
    assert_eq!(result.final_response.as_deref(), Some(CANNED_REPLY));
    assert_eq!(chat_posts(&server).await.len(), 2);
    assert_one_rejection_then_fit(&log);
}

/// A server that answers every chat completion with HTTP 402.
async fn start_payment_required() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex("/chat/completions$"))
        .respond_with(ResponseTemplate::new(402).set_body_json(serde_json::json!({
            "error": { "message": "insufficient credits", "code": "payment_required" }
        })))
        .mount(&server)
        .await;
    server
}

fn assert_fallback_served_only_fitting_requests(log: &Arc<Mutex<Vec<Served>>>, posts: usize) {
    let served = log.lock().unwrap().clone();
    assert!(posts >= 1, "the fallback must have been called");
    assert!(!served.is_empty());
    for s in &served {
        assert_eq!(s.status, 200, "fallback served a 400: {served:?}");
        assert!(s.count <= WINDOW, "fallback request counted {}", s.count);
    }
}

#[tokio::test]
async fn cw8192_d12_fallback_activation_reclamps_window() {
    init_estimator();
    let primary = start_payment_required().await;
    let (fallback, flog) = start_fake(FakeConfig::strict(4.0)).await;
    let mut primary_ep = endpoint_for(&primary);
    primary_ep.provider_model_context_length = Some(128_000);
    let fallback_ep = endpoint_for(&fallback);
    assert_eq!(
        fallback_ep.context_length_with_source(),
        (8192, ContextLengthSource::LocalDefault)
    );

    let mut agent = AgentLoop::new(client_for_endpoint(&primary_ep), empty_registry(), 4)
        .with_window_endpoint(primary_ep)
        .with_fallback_named(client_for_endpoint(&fallback_ep), "fallback")
        .with_fallback_window_endpoint(fallback_ep);
    let result = agent
        .run(long_seed(SEED_MSG_CHARS))
        .await
        .expect("fallback must reply");

    assert_eq!(result.final_response.as_deref(), Some(CANNED_REPLY));
    let posts = chat_posts(&fallback).await;
    assert_fallback_served_only_fitting_requests(&flog, posts.len());
    assert_every_request_fits_and_pairs(&posts, &flog);
}

#[tokio::test]
async fn cw8192_d12_wired_fallback_carries_its_endpoint() {
    init_estimator();
    let primary = start_payment_required().await;
    let (fallback, flog) = start_fake(FakeConfig::strict(4.0)).await;
    let mut config = Config::default();
    config.model.provider = "primary".to_string();
    config.providers.insert(
        "primary".to_string(),
        ProviderConfig {
            base_url: Some(primary.uri()),
            default_model: Some("big-model".to_string()),
            fallback_providers: vec!["fb".to_string()],
            // Large window on the primary so the first request is not pre-shrunk.
            models: [(
                "big-model".to_string(),
                ProviderModelConfig {
                    context_length: Some(128_000),
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        },
    );
    config.providers.insert(
        "fb".to_string(),
        ProviderConfig {
            base_url: Some(fallback.uri()),
            default_model: Some("small-model".to_string()),
            ..Default::default()
        },
    );
    let resolver =
        ProviderResolver::build_with_cache(&config, ModelsCache::default()).expect("resolver");
    let main = resolver.resolve_for_main().clone();
    assert_eq!(main.context_length(), 128_000);

    let agent =
        AgentLoop::new(client_for_endpoint(&main), empty_registry(), 4).with_window_endpoint(main);
    let mut agent = ironhermes_agent::any_client::wire_fallback_if_configured(agent, &resolver);
    let result = agent
        .run(long_seed(SEED_MSG_CHARS))
        .await
        .expect("fallback must reply");

    assert_eq!(result.final_response.as_deref(), Some(CANNED_REPLY));
    let posts = chat_posts(&fallback).await;
    assert_fallback_served_only_fitting_requests(&flog, posts.len());
    assert_every_request_fits_and_pairs(&posts, &flog);
}

#[test]
fn cw8192_d09_every_agentloop_site_wires_window_endpoint() {
    for (name, src) in [
        ("agent_runtime.rs", include_str!("../src/agent_runtime.rs")),
        (
            "subagent_runner.rs",
            include_str!("../src/subagent_runner.rs"),
        ),
        (
            "cron runner.rs",
            include_str!("../../ironhermes-cron-runner/src/runner.rs"),
        ),
    ] {
        let new_at = src
            .find("AgentLoop::new(")
            .unwrap_or_else(|| panic!("{name}: no AgentLoop::new("));
        let wired_at = src
            .find(".with_window_endpoint(")
            .unwrap_or_else(|| panic!("{name}: no .with_window_endpoint("));
        assert!(
            wired_at > new_at,
            "{name}: window endpoint wired before construction"
        );
    }
    assert!(
        include_str!("../src/any_client.rs").contains(".with_fallback_window_endpoint("),
        "any_client.rs must attach the fallback's window endpoint"
    );
}

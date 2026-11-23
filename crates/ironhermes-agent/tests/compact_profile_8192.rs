//! Phase 58.3.1 Plan 07: the compact prompt/tool profile at an 8,192-token window.
//!
//! Harness rules copied from `context_window_8192.rs`: the cl100k estimator is
//! initialised first in EVERY test (otherwise this binary's estimator is the
//! `len/4+1` fallback, nearly the same counter as the divisor-4.0 fake), the strict
//! fake is mounted on POST /chat/completions only, and request counts come from
//! `chat_posts`. Stub-tool sizes follow RESEARCH's measured per-tool tokens.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use ironhermes_agent::AgentLoop;
use ironhermes_agent::any_client::AnyClient;
use ironhermes_agent::client::LlmClient;
use ironhermes_agent::context_compressor::estimate_messages_tokens;
use ironhermes_agent::fit;
use ironhermes_agent::prompt_builder::{PromptBuilder, compact_prompt_for};
use ironhermes_core::config::CompactProfileMode;
use ironhermes_core::{
    ChatMessage, FunctionCall, MessageContent, Role, SkillRegistry, TiktokenEncoding, ToolCall,
    ToolSchema, validate_tool_call_pairing,
};
use ironhermes_tools::{Tool, ToolRegistry};
use tokio::sync::RwLock;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const WINDOW: usize = 8192;
const CANNED_REPLY: &str = "Here is the summary you asked for.";
const FINAL_QUESTION: &str = "FINAL-QUESTION-MARKER-7731 please summarize what we discussed.";
const SOUL_SENTINEL: &str = "SOULSENTINEL-4417";
const AGENTS_MID: &str = "AGENTSMID-6203";
const WEB_WS_SENTINEL: &str = "WEBWS-9021";
const OVERLAY_SENTINEL: &str = "OVERLAY-3390";

fn init_estimator() {
    ironhermes_core::init_global_estimator(TiktokenEncoding::Cl100kBase);
}

/// The fake's own count (see `context_window_8192.rs`): chars over `divisor`, plus 4
/// per message and `max_tokens`. A different counter from the cl100k estimator.
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

#[derive(Debug, Clone, Copy)]
struct Served {
    count: usize,
    status: u16,
}

struct StrictWindowFake {
    window: usize,
    divisor: f64,
    log: Arc<Mutex<Vec<Served>>>,
}

impl Respond for StrictWindowFake {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value =
            serde_json::from_slice(&request.body).expect("request body is JSON");
        let count = fake_count_body(&body, self.divisor);
        if count > self.window {
            self.log.lock().unwrap().push(Served { count, status: 400 });
            return ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": {
                    "message": format!(
                        "Message too long: {count} tokens exceeds the {}-token context window.",
                        self.window
                    ),
                    "type": "invalid_request_error",
                    "param": "messages",
                    "code": "context_length_exceeded"
                }
            }));
        }
        self.log.lock().unwrap().push(Served { count, status: 200 });
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

/// POST-only strict fake: probe GETs never reach it (plan 05).
async fn start_fake(window: usize, divisor: f64) -> (MockServer, Arc<Mutex<Vec<Served>>>) {
    let server = MockServer::start().await;
    let log = Arc::new(Mutex::new(Vec::new()));
    Mock::given(method("POST"))
        .and(path_regex("/chat/completions$"))
        .respond_with(StrictWindowFake {
            window,
            divisor,
            log: log.clone(),
        })
        .mount(&server)
        .await;
    (server, log)
}

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

fn body_of(post: &Request) -> serde_json::Value {
    serde_json::from_slice(&post.body).unwrap()
}

fn tool_names(body: &serde_json::Value) -> Vec<String> {
    let mut v: Vec<String> = body["tools"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|t| t.pointer("/function/name").and_then(|n| n.as_str()))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn system_text(body: &serde_json::Value) -> String {
    body["messages"][0]["content"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------- text fixtures

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

/// Natural English prose of about `chars` characters.
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

/// Plain but token-dense text (short words and punctuation, about 3 chars per
/// token), exactly `chars` long, so a 20,000-char SOUL.md costs a worst-case number
/// of tokens. No injection-looking phrasing: the context scanner must pass it.
fn dense(chars: usize) -> String {
    let unit = "Go on, do it, be so, as I do. ";
    let mut s = unit.repeat(chars / unit.len() + 1);
    s.truncate(chars);
    s
}

fn long_seed(msg_chars: usize) -> Vec<ChatMessage> {
    let mut seed = Vec::new();
    for k in 0..12 {
        seed.push(ChatMessage::user(prose(msg_chars, k)));
        if k == 4 || k == 8 {
            let id = format!("call_{k}");
            seed.push(ChatMessage::assistant_tool_calls(vec![ToolCall {
                id: id.clone(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "terminal".to_string(),
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

/// Tuned (as in `context_window_8192.rs`) so the seed estimates near 27.6K tokens.
const SEED_MSG_CHARS: usize = 5_465;

// ------------------------------------------------------------------ stub tools

struct StubTool {
    name: String,
    toolset: &'static str,
    desc: String,
}

#[async_trait]
impl Tool for StubTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn toolset(&self) -> &str {
        self.toolset
    }
    fn description(&self) -> &str {
        &self.desc
    }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            self.name.clone(),
            self.desc.clone(),
            serde_json::json!({ "type": "object", "properties": {} }),
        )
    }
    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
        Ok("ok".to_string())
    }
}

/// RESEARCH's measured per-tool tokens for the 7 D-03 tools (sum 1,280).
const COMPACT_TOOL_TOKENS: &[(&str, usize)] = &[
    ("terminal", 448),
    ("read_file", 59),
    ("write_file", 79),
    ("patch", 93),
    ("memory", 191),
    ("web_search", 353),
    ("web_read", 57),
];

const EXTRA_TOOLS: &[&str] = &[
    "browser_navigate",
    "browser_click",
    "kanban_create",
    "ros2_publish",
    "image_gen",
    "delegate_task",
    "skills",
    "cronjob",
    "send_message",
    "mcp_demo_search",
];

/// ~5 chars of prose per cl100k token, less ~30 tokens of JSON framing.
fn stub(name: &str, tokens: usize, toolset: &'static str) -> Box<dyn Tool> {
    let chars = tokens.saturating_sub(30) * 5;
    Box::new(StubTool {
        name: name.to_string(),
        toolset,
        desc: prose(chars, name.len()),
    })
}

/// The 7 compact stubs plus the first `extras` extra stubs (~450 tokens each).
fn registry(extras: usize) -> Arc<RwLock<ToolRegistry>> {
    let mut reg = ToolRegistry::new();
    for (name, tokens) in COMPACT_TOOL_TOKENS {
        reg.register(stub(name, *tokens, "test"));
    }
    for name in EXTRA_TOOLS.iter().take(extras) {
        reg.register(stub(name, 450, "test"));
    }
    Arc::new(RwLock::new(reg))
}

// ---------------------------------------------------------------- prompt build

static HOME_LOCK: Mutex<()> = Mutex::new(());

struct HomeGuard {
    _lock: MutexGuard<'static, ()>,
    prev: Option<std::ffi::OsString>,
}

impl HomeGuard {
    fn set(home: &Path) -> Self {
        let lock = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("IRONHERMES_HOME");
        // SAFETY: serialised by HOME_LOCK; held only across synchronous prompt building.
        unsafe { std::env::set_var("IRONHERMES_HOME", home) };
        Self { _lock: lock, prev }
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        // SAFETY: still holding HOME_LOCK (field drops after this body runs).
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var("IRONHERMES_HOME", v),
                None => std::env::remove_var("IRONHERMES_HOME"),
            }
        }
    }
}

/// A system message built by the real `PromptBuilder` from tempdir files.
/// `agents` is the home AGENTS.md; `project` is a `.hermes.md` in the cwd.
fn build_system(soul: &str, agents: Option<&str>, project: Option<&str>) -> ChatMessage {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("SOUL.md"), soul).unwrap();
    if let Some(a) = agents {
        std::fs::write(home.path().join("AGENTS.md"), a).unwrap();
    }
    if let Some(p) = project {
        std::fs::write(cwd.path().join(".hermes.md"), p).unwrap();
    }
    let _guard = HomeGuard::set(home.path());
    PromptBuilder::new("test-model", "cli")
        .with_context_length(WINDOW)
        .load_context(cwd.path())
        .build_system_message()
}

/// Like `build_system`, but with a long home AGENTS.md (AGENTS_MID in the middle) and one
/// skill, so the full prompt carries `## Available Skills` and an over-cap context file.
fn build_system_with_skills(platform: &str) -> ChatMessage {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let skills = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("SOUL.md"), soul_400()).unwrap();
    std::fs::write(
        home.path().join("AGENTS.md"),
        format!("{} {AGENTS_MID} {}", prose(3_000, 1), prose(3_000, 2)),
    )
    .unwrap();
    let skill_dir = skills.path().join("focus-mode");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: focus-mode\ndescription: Deep work skill\n---\nFocus on the task.",
    )
    .unwrap();
    let _guard = HomeGuard::set(home.path());
    let mut builder = PromptBuilder::new("test-model", platform)
        .with_context_length(WINDOW)
        .load_context(cwd.path());
    builder.set_skill_registry(Arc::new(SkillRegistry::load_with_paths(&[skills
        .path()
        .to_path_buf()])));
    builder.build_system_message()
}

/// The post-build mutation both surfaces perform on the built system message (web
/// workspace guidance, TUI personality overlay): append text in place.
fn append_like_surface(msg: &mut ChatMessage, suffix: &str) {
    if let Some(MessageContent::Text(text)) = msg.content.as_mut() {
        text.push_str(suffix);
    }
}

fn text_of(m: &ChatMessage) -> String {
    m.content_text().unwrap_or_default().to_string()
}

fn soul_400() -> String {
    format!("{SOUL_SENTINEL} {}", prose(380, 3))
}

fn agent_for(server: &MockServer, reg: Arc<RwLock<ToolRegistry>>, window: usize) -> AgentLoop {
    AgentLoop::new(client_for(server), reg, 4).with_context_length(window)
}

// ----------------------------------------------------------------------- tests

#[tokio::test]
async fn compact_d02_full_floor_over_half_window_switches_profile() {
    init_estimator();
    // Stub sizes match RESEARCH: the compact set ~1,280 tokens, the rest ~4,500.
    let reg = registry(10);
    let compact_tokens = fit::tool_schema_tokens(
        &reg.read()
            .await
            .get_definitions(Some(&fit::compact_tool_allowlist())),
    );
    assert!(
        (1_100..=1_500).contains(&compact_tokens),
        "compact stubs measure {compact_tokens}"
    );

    let system = build_system(&soul_400(), None, None);
    let full_text = text_of(&system);
    let compact_text = compact_prompt_for(&full_text).expect("built by PromptBuilder");

    let (server, _log) = start_fake(WINDOW, 4.0).await;
    let mut agent = agent_for(&server, reg, WINDOW);
    let result = agent
        .run(vec![system, ChatMessage::user("hi")])
        .await
        .expect("compact request fits and replies");

    let posts = chat_posts(&server).await;
    assert_eq!(posts.len(), 1);
    let body = body_of(&posts[0]);
    let mut want: Vec<String> = COMPACT_TOOL_TOKENS
        .iter()
        .map(|(n, _)| n.to_string())
        .collect();
    want.sort();
    assert_eq!(tool_names(&body), want, "only the D-03 stubs travel");
    let sent = system_text(&body);
    assert_eq!(sent, *compact_text, "outgoing system is the compact twin");
    assert!(sent.contains(SOUL_SENTINEL), "SOUL.md stays in full");
    assert!(sent.len() < full_text.len(), "compact is the shorter text");
    // The stored history keeps the full system message.
    assert_eq!(result.messages[0].role, Role::System);
    assert_eq!(text_of(&result.messages[0]), full_text);
}

#[tokio::test]
async fn compact_d03_kanban_worker_terminators_kept() {
    init_estimator();
    let reg = registry(0);
    {
        let mut w = reg.write().await;
        // Both are in the kanban toolset (exempt from the toolset filter, self-gating).
        w.register(stub("kanban_complete", 120, "kanban"));
        w.register(stub("kanban_create", 120, "kanban"));
    }
    let (server, _log) = start_fake(WINDOW, 4.0).await;
    let mut agent =
        agent_for(&server, reg, WINDOW).with_compact_profile_mode(CompactProfileMode::On);
    agent
        .run(vec![
            ChatMessage::system("You are a worker."),
            ChatMessage::user("go"),
        ])
        .await
        .expect("replies");

    let names = tool_names(&body_of(&chat_posts(&server).await[0]));
    assert!(names.contains(&"kanban_complete".to_string()), "{names:?}");
    assert!(!names.contains(&"kanban_create".to_string()), "{names:?}");
    assert_eq!(names.len(), COMPACT_TOOL_TOKENS.len() + 1, "{names:?}");
}

#[tokio::test]
async fn compact_d01_oversized_soul_refuses_naming_window_and_floor() {
    init_estimator();
    // D-19 overshoot case D: a 20,000-char SOUL.md plus two capped context files.
    let system = build_system(
        &dense(20_000),
        Some(&prose(10_000, 1)),
        Some(&prose(10_000, 2)),
    );
    let compact_msg =
        ChatMessage::system(compact_prompt_for(&text_of(&system)).unwrap().to_string());
    let reg = registry(10);
    let tool_tokens = fit::tool_schema_tokens(
        &reg.read()
            .await
            .get_definitions(Some(&fit::compact_tool_allowlist())),
    );
    let floor_prompt = estimate_messages_tokens(std::slice::from_ref(&compact_msg)) + tool_tokens;
    let floor = fit::floor_tokens(floor_prompt, WINDOW);
    assert!(
        !fit::fits(floor_prompt, WINDOW, 1.0),
        "precondition: compact floor {floor} must overshoot the window"
    );

    let (server, _log) = start_fake(WINDOW, 4.0).await;
    let mut agent = agent_for(&server, reg, WINDOW);
    let err = agent
        .run(vec![system, ChatMessage::user("hi")])
        .await
        .expect_err("an unfittable compact floor is refused");
    let text = err.to_string();
    assert!(text.contains("8192"), "{text}");
    assert!(
        text.contains(&floor.to_string()),
        "{text} lacks floor {floor}"
    );
    assert!(chat_posts(&server).await.is_empty(), "nothing may be sent");
}

#[tokio::test]
async fn compact_d19_operator_size_soul_fits() {
    init_estimator();
    // Operator-size SOUL.md (400 chars) and a small AGENTS.md; a ~27.6K-token conversation.
    let system = build_system(&soul_400(), Some(&prose(300, 5)), None);
    let mut seed = vec![system];
    seed.extend(long_seed(SEED_MSG_CHARS));
    let est = estimate_messages_tokens(&seed);
    assert!(est > 27_000, "seed estimate {est} should be ~27.6K");
    // Proof independence: the fake counts the seed at least 3% above the estimator.
    let fake_seed = fake_count_body(&serde_json::json!({ "messages": seed }), 4.0);
    assert!(
        fake_seed * 100 >= est * 103,
        "fake count {fake_seed} is not >= 3% above estimate {est}"
    );

    let (server, log) = start_fake(WINDOW, 4.0).await;
    let mut agent = agent_for(&server, registry(10), WINDOW);
    let result = agent
        .run(seed)
        .await
        .expect("compact profile fits and replies");

    assert_eq!(result.final_response.as_deref(), Some(CANNED_REPLY));
    let posts = chat_posts(&server).await;
    assert_eq!(posts.len(), 1, "guard compacts before sending: one request");
    for served in log.lock().unwrap().iter() {
        assert!(served.count <= WINDOW, "fake counted {}", served.count);
        assert_eq!(served.status, 200);
    }
    let body = body_of(&posts[0]);
    assert_eq!(tool_names(&body).len(), COMPACT_TOOL_TOKENS.len());
    let msgs: Vec<ChatMessage> = serde_json::from_value(body["messages"].clone()).unwrap();
    validate_tool_call_pairing(&msgs).expect("tool pairs intact on the wire");
    assert!(String::from_utf8_lossy(&posts[0].body).contains("FINAL-QUESTION-MARKER-7731"));
}

#[tokio::test]
async fn compact_force_off_and_on() {
    init_estimator();
    let system = build_system(&soul_400(), None, None);
    let full_text = text_of(&system);
    let msgs = || vec![system.clone(), ChatMessage::user("hi")];

    // Off at 8,192 with a ~5,000-token full floor: the full list and prompt are sent.
    let reg = registry(7);
    let all = reg.read().await.get_definitions(None);
    let floor = estimate_messages_tokens(&msgs()[..1]) + fit::tool_schema_tokens(&all);
    assert!(
        (4_200..=5_600).contains(&floor),
        "full floor {floor} should be ~5,000 (over the 4,096 auto trigger)"
    );
    let (server, _log) = start_fake(WINDOW, 4.0).await;
    let mut agent =
        agent_for(&server, reg, WINDOW).with_compact_profile_mode(CompactProfileMode::Off);
    agent.run(msgs()).await.expect("replies");
    let body = body_of(&chat_posts(&server).await[0]);
    assert_eq!(tool_names(&body).len(), 7 + 7);
    assert_eq!(system_text(&body), full_text);

    // On at 128,000: compact tools although the window is huge.
    let (server, _log) = start_fake(128_000, 4.0).await;
    let mut agent =
        agent_for(&server, registry(7), 128_000).with_compact_profile_mode(CompactProfileMode::On);
    agent.run(msgs()).await.expect("replies");
    let body = body_of(&chat_posts(&server).await[0]);
    assert_eq!(tool_names(&body).len(), COMPACT_TOOL_TOKENS.len());
    assert_ne!(system_text(&body), full_text);

    // Auto at 128,000 keeps the full tool list and the full system prompt.
    let (server, _log) = start_fake(128_000, 4.0).await;
    let mut agent = agent_for(&server, registry(7), 128_000);
    agent.run(msgs()).await.expect("replies");
    let body = body_of(&chat_posts(&server).await[0]);
    assert_eq!(tool_names(&body).len(), 7 + 7);
    assert_eq!(system_text(&body), full_text);
}

#[tokio::test]
async fn compact_d05_notice_once_per_session() {
    init_estimator();
    let system = build_system(&soul_400(), None, None);
    let tracker = Arc::new(ironhermes_agent::pressure_warning::PressureTracker::new());
    let (server, _log) = start_fake(WINDOW, 4.0).await;
    let mut warnings = Vec::new();
    for _ in 0..2 {
        let mut agent = agent_for(&server, registry(10), WINDOW)
            .with_pressure_tracker(tracker.clone())
            .with_session_id("compact-d05-session");
        let result = agent
            .run(vec![system.clone(), ChatMessage::user("hi")])
            .await
            .expect("compact request fits and replies");
        warnings.push(result.context_warnings);
    }
    assert_eq!(warnings[0].len(), 1, "{:?}", warnings[0]);
    assert!(
        warnings[0][0].contains("compact profile"),
        "{:?}",
        warnings[0]
    );
    assert!(warnings[0][0].contains("8192"), "{:?}", warnings[0]);
    assert!(
        !warnings[0][0].contains(SOUL_SENTINEL),
        "the notice carries numbers and fixed text only"
    );
    assert!(
        warnings[1].is_empty(),
        "second run repeats: {:?}",
        warnings[1]
    );
}

#[tokio::test]
async fn compact_d23_small_window_sends_max_tokens_reserve() {
    init_estimator();
    let msgs = || {
        vec![
            ChatMessage::system("You are a helper."),
            ChatMessage::user("hi"),
        ]
    };
    let (server, _log) = start_fake(WINDOW, 4.0).await;
    let mut agent = agent_for(&server, registry(0), WINDOW);
    agent.run(msgs()).await.expect("replies");
    let body = body_of(&chat_posts(&server).await[0]);
    assert_eq!(body["max_tokens"], 1024);

    // Small window without the compact profile: still the reserve (16,384 / 8 = 2,048).
    let (server, _log) = start_fake(16_384, 4.0).await;
    let mut agent =
        agent_for(&server, registry(0), 16_384).with_compact_profile_mode(CompactProfileMode::Off);
    agent.run(msgs()).await.expect("replies");
    let body = body_of(&chat_posts(&server).await[0]);
    assert_eq!(body["max_tokens"], 2048);
}

#[tokio::test]
async fn compact_d23_hosted_window_sends_no_max_tokens() {
    init_estimator();
    let (server, _log) = start_fake(128_000, 4.0).await;
    let mut agent = agent_for(&server, registry(7), 128_000);
    agent
        .run(vec![
            ChatMessage::system("You are a helper."),
            ChatMessage::user("hi"),
        ])
        .await
        .expect("replies");
    let body = body_of(&chat_posts(&server).await[0]);
    assert!(body.get("max_tokens").is_none(), "{body}");
    assert_eq!(tool_names(&body).len(), 7 + 7, "full tool list");
}

#[tokio::test]
async fn compact_d04_web_appended_guidance_rides_compact_prompt() {
    init_estimator();
    let mut system = build_system_with_skills("web");
    let suffix = format!(
        "\n\nYour session workspace for this chat is: /tmp/{WEB_WS_SENTINEL}\nWrite files there with your file tools."
    );
    append_like_surface(&mut system, &suffix);
    let full_text = text_of(&system);
    // Preconditions, so the test cannot pass vacuously.
    assert!(full_text.contains("## Available Skills"), "{full_text}");
    assert!(full_text.contains(AGENTS_MID));
    assert!(full_text.ends_with(&suffix));

    let (server, _log) = start_fake(WINDOW, 4.0).await;
    let mut agent = agent_for(&server, registry(10), WINDOW);
    let result = agent
        .run(vec![system, ChatMessage::user("hi")])
        .await
        .expect("compact request fits and replies");

    let posts = chat_posts(&server).await;
    assert_eq!(posts.len(), 1);
    let body = body_of(&posts[0]);
    let mut want: Vec<String> = COMPACT_TOOL_TOKENS
        .iter()
        .map(|(n, _)| n.to_string())
        .collect();
    want.sort();
    assert_eq!(tool_names(&body), want, "only the D-03 stubs travel");
    let sent = system_text(&body);
    assert!(sent.contains(SOUL_SENTINEL), "SOUL.md stays in full");
    assert!(sent.contains("[...truncated AGENTS.md"), "{sent}");
    assert!(!sent.contains("## Available Skills"), "{sent}");
    assert!(!sent.contains(AGENTS_MID), "context file is capped");
    assert!(sent.ends_with(&suffix), "workspace guidance rides along");
    assert!(sent.len() < full_text.len(), "compact is the shorter text");
    // The stored history keeps the full system message plus the appended text.
    assert_eq!(text_of(&result.messages[0]), full_text);
    assert!(
        result.context_warnings[0].contains("context files are shortened"),
        "{:?}",
        result.context_warnings
    );
}

#[tokio::test]
async fn compact_d04_tui_overlay_rides_compact_prompt() {
    init_estimator();
    let mut system = build_system_with_skills("cli");
    let suffix = format!("\n\n{OVERLAY_SENTINEL} Speak like a calm ship's captain.");
    append_like_surface(&mut system, &suffix);
    let full_text = text_of(&system);
    // Preconditions, so the test cannot pass vacuously.
    assert!(full_text.contains("## Available Skills"), "{full_text}");
    assert!(full_text.contains(AGENTS_MID));
    assert!(full_text.ends_with(&suffix));

    let (server, _log) = start_fake(WINDOW, 4.0).await;
    let mut agent = agent_for(&server, registry(10), WINDOW);
    let result = agent
        .run(vec![system, ChatMessage::user("hi")])
        .await
        .expect("compact request fits and replies");

    let posts = chat_posts(&server).await;
    assert_eq!(posts.len(), 1);
    let body = body_of(&posts[0]);
    let mut want: Vec<String> = COMPACT_TOOL_TOKENS
        .iter()
        .map(|(n, _)| n.to_string())
        .collect();
    want.sort();
    assert_eq!(tool_names(&body), want, "only the D-03 stubs travel");
    let sent = system_text(&body);
    assert!(sent.contains(SOUL_SENTINEL), "SOUL.md stays in full");
    assert!(sent.contains("[...truncated AGENTS.md"), "{sent}");
    assert!(!sent.contains("## Available Skills"), "{sent}");
    assert!(!sent.contains(AGENTS_MID), "context file is capped");
    assert!(sent.ends_with(&suffix), "personality overlay rides along");
    assert!(sent.len() < full_text.len(), "compact is the shorter text");
    assert_eq!(text_of(&result.messages[0]), full_text);
}

#[tokio::test]
async fn compact_d05_notice_without_twin_claims_tools_only() {
    init_estimator();
    let plain = "PLAIN-SYSTEM-5521 You are a helper.";
    let (server, _log) = start_fake(WINDOW, 4.0).await;
    let mut agent = agent_for(&server, registry(10), WINDOW);
    let result = agent
        .run(vec![ChatMessage::system(plain), ChatMessage::user("hi")])
        .await
        .expect("compact tool set fits and replies");

    let body = body_of(&chat_posts(&server).await[0]);
    assert_eq!(system_text(&body), plain, "no twin: sent in full");
    let mut want: Vec<String> = COMPACT_TOOL_TOKENS
        .iter()
        .map(|(n, _)| n.to_string())
        .collect();
    want.sort();
    assert_eq!(tool_names(&body), want, "the compact tool set stays");
    assert_eq!(
        result.context_warnings.len(),
        1,
        "{:?}",
        result.context_warnings
    );
    let notice = &result.context_warnings[0];
    assert!(notice.contains("compact profile"), "{notice}");
    assert!(notice.contains("8192"), "{notice}");
    assert!(!notice.contains("context files are shortened"), "{notice}");
    assert!(!notice.contains("skills index"), "{notice}");
}

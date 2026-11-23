//! Phase 58.3.1 Plan 06 (D-09): auxiliary model calls recover from a
//! context-length rejection at an 8,192-token window.
//!
//! The fake counts a request as `ceil(chars / 4) + 4 * messages + max_tokens`, a
//! different counter from the agent's cl100k estimator. Only chat-completion POSTs
//! are counted. Every test uses its own mock server, so base URLs (and therefore
//! observed-window keys) are unique per test.

use std::sync::{Arc, Mutex, OnceLock};

use ironhermes_agent::any_client::AnyClient;
use ironhermes_agent::client::LlmClient;
use ironhermes_core::observed_windows::{self, ObservedWindowSource};
use ironhermes_core::{ChatMessage, TiktokenEncoding};
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const WINDOW: usize = 8192;

/// Production initialises cl100k at startup; without it the estimator is the
/// `len/4` fallback and the fake's counter would coincide with it. Also points
/// `IRONHERMES_HOME` at a process-lifetime tempdir so `observed_windows::record`
/// never writes the developer's real models-cache.json.
fn init() {
    ironhermes_core::init_global_estimator(TiktokenEncoding::Cl100kBase);
    static HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
    HOME.get_or_init(|| {
        let dir = tempfile::tempdir().expect("tempdir");
        // SAFETY: set once, before any test in this binary reads the variable.
        unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };
        dir
    });
}

fn prose(chars: usize) -> String {
    "The quick brown fox jumps over the lazy dog. "
        .repeat(chars / 45 + 1)
        .chars()
        .take(chars)
        .collect()
}

fn fake_count(body: &serde_json::Value) -> usize {
    let msgs = body["messages"].as_array().cloned().unwrap_or_default();
    let chars: usize = msgs
        .iter()
        .filter_map(|m| m["content"].as_str())
        .map(str::len)
        .sum();
    let max_tokens = body["max_tokens"].as_u64().unwrap_or(0) as usize;
    chars.div_ceil(4) + 4 * msgs.len() + max_tokens
}

#[derive(Clone, Copy)]
enum Mode {
    /// Reject bodies counted over the window; the body names the window.
    Strict,
    /// Reject everything; the body names the window.
    AlwaysReject,
    /// Reject over-window bodies with an error text that has no digits.
    NoDigits,
    /// Answer 500 to everything.
    ServerError,
}

struct Fake {
    mode: Mode,
    counts: Arc<Mutex<Vec<usize>>>,
}

impl Respond for Fake {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&request.body).expect("json body");
        let count = fake_count(&body);
        self.counts.lock().unwrap().push(count);
        let reject = match self.mode {
            Mode::Strict | Mode::NoDigits => count > WINDOW,
            Mode::AlwaysReject => true,
            Mode::ServerError => {
                return ResponseTemplate::new(500).set_body_string("upstream exploded");
            }
        };
        if reject {
            let message = if matches!(self.mode, Mode::NoDigits) {
                "Context length exceeded. Try shortening the conversation.".to_string()
            } else {
                format!(
                    "Message too long: {count} tokens exceeds the {WINDOW}-token context window. Try increasing the Context Length in Model settings, or shorten the conversation."
                )
            };
            return ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": {
                    "message": message,
                    "type": "invalid_request_error",
                    "code": "context_length_exceeded"
                }
            }));
        }
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "r1", "object": "chat.completion", "created": 0, "model": "m",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        }))
    }
}

async fn start(mode: Mode) -> (MockServer, Arc<Mutex<Vec<usize>>>) {
    let server = MockServer::start().await;
    let counts = Arc::new(Mutex::new(Vec::new()));
    Mock::given(method("POST"))
        .and(path_regex("/chat/completions$"))
        .respond_with(Fake {
            mode,
            counts: counts.clone(),
        })
        .mount(&server)
        .await;
    (server, counts)
}

async fn bodies(server: &MockServer) -> Vec<serde_json::Value> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/chat/completions"))
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect()
}

fn any_client(server: &MockServer, model: &str) -> AnyClient {
    AnyClient::ChatCompletions(LlmClient::new(server.uri(), "k", model))
}

/// 12,000 tokens by the fake's counter, well over the 8,192 window.
fn big_prompt() -> Vec<ChatMessage> {
    vec![
        ChatMessage::system("You summarize."),
        ChatMessage::user(prose(48_000)),
    ]
}

#[tokio::test]
async fn aux_fit_learns_window_and_retries_once() {
    init();
    let (server, counts) = start(Mode::Strict).await;
    let model = "aux-learn";
    let client = any_client(&server, model);
    let res = client
        .chat_completion_fitting(&big_prompt(), None, None, None, None, None)
        .await;
    assert!(res.is_ok(), "retry must succeed: {:?}", res.err());
    let counts = counts.lock().unwrap().clone();
    assert_eq!(counts.len(), 2, "exactly one retry: {counts:?}");
    assert!(counts[0] > WINDOW && counts[1] <= WINDOW, "{counts:?}");
    let learned = observed_windows::lookup(&server.uri(), model).expect("window learned");
    assert_eq!(learned.tokens, WINDOW);
    assert_eq!(learned.source, ObservedWindowSource::Learned);
}

#[tokio::test]
async fn aux_fit_caps_max_tokens_on_retry() {
    init();
    let (server, _) = start(Mode::Strict).await;
    let client = any_client(&server, "aux-cap");
    let msgs = vec![ChatMessage::user(prose(2_000))];
    client
        .chat_completion_fitting(&msgs, None, None, Some(20_000), None, None)
        .await
        .expect("retry succeeds");
    let b = bodies(&server).await;
    assert_eq!(b.len(), 2);
    assert_eq!(b[0]["max_tokens"].as_u64(), Some(20_000));
    assert!(
        b[1]["max_tokens"].as_u64().unwrap() <= 2_048,
        "{}",
        b[1]["max_tokens"]
    );
}

#[tokio::test]
async fn aux_fit_second_rejection_returns_clear_error() {
    init();
    let (server, counts) = start(Mode::AlwaysReject).await;
    let client = any_client(&server, "aux-second");
    let err = client
        .chat_completion_fitting(&big_prompt(), None, None, None, None, None)
        .await
        .expect_err("both attempts rejected");
    assert_eq!(counts.lock().unwrap().len(), 2, "exactly two requests");
    let text = format!("{err}");
    assert!(text.contains("8192"), "{text}");
    assert!(
        text.contains("raise the context length in the model server"),
        "{text}"
    );
    assert!(!text.contains("providers.<provider>"), "{text}");
    let detail = text
        .split("Provider detail: ")
        .nth(1)
        .expect("detail present");
    // 400 characters plus the existing 13-character truncation marker.
    assert!(
        detail.chars().count() <= 400 + " …[truncated]".chars().count(),
        "{}",
        detail.len()
    );
}

#[tokio::test]
async fn aux_fit_no_number_halves_assumed_window() {
    init();
    let (server, counts) = start(Mode::NoDigits).await;
    let client = any_client(&server, "aux-halve");
    client
        .chat_completion_fitting(&big_prompt(), None, None, None, None, None)
        .await
        .expect("halved retry fits");
    let counts = counts.lock().unwrap().clone();
    assert_eq!(counts.len(), 2, "{counts:?}");
    assert!(
        counts[1] <= counts[0] / 2,
        "retry sized for half: {counts:?}"
    );
    assert!(
        observed_windows::lookup(&server.uri(), "aux-halve").is_none(),
        "no number in the error text, so nothing is learned"
    );
}

#[tokio::test]
async fn aux_fit_non_context_errors_pass_through() {
    init();
    let (server, counts) = start(Mode::ServerError).await;
    let client = any_client(&server, "aux-500");
    let err = client
        .chat_completion_fitting(&big_prompt(), None, None, None, None, None)
        .await
        .expect_err("500 surfaces");
    assert_eq!(
        counts.lock().unwrap().len(),
        1,
        "no retry for a non-context error"
    );
    let text = format!("{err:#}");
    assert!(text.contains("500"), "{text}");
    assert!(!text.contains("auxiliary model"), "unchanged error: {text}");
}

#[tokio::test]
async fn aux_fit_presizes_with_known_window() {
    init();
    let (server, counts) = start(Mode::Strict).await;
    let model = "aux-presize";
    assert!(observed_windows::record(
        &server.uri(),
        model,
        WINDOW,
        ObservedWindowSource::Learned
    ));
    let client = any_client(&server, model);
    client
        .chat_completion_fitting(&big_prompt(), None, None, None, None, None)
        .await
        .expect("first request already fits");
    let counts = counts.lock().unwrap().clone();
    assert_eq!(counts.len(), 1, "zero 400s: {counts:?}");
    assert!(counts[0] <= WINDOW, "{counts:?}");
}

#[tokio::test]
async fn aux_fit_llmclient_path_recovers() {
    init();
    let (server, counts) = start(Mode::Strict).await;
    let model = "aux-llm";
    let client = LlmClient::new(server.uri(), "k", model);
    client
        .chat_completion_fitting(&big_prompt(), None, None, None, None, None)
        .await
        .expect("retry succeeds");
    assert_eq!(counts.lock().unwrap().len(), 2);
    assert_eq!(
        observed_windows::lookup(&server.uri(), model).map(|w| w.tokens),
        Some(WINDOW)
    );
}

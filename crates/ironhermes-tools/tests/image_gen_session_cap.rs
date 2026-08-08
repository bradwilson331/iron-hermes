//! Phase 01 Plan 03 — per-session generation cap (SAFE-05 / D-03), interim
//! acknowledgment (D-04), and usage-tracing wiring tests.
//!
//! These tests prove the abuse/runaway guard for `image_gen`:
//!   - `test_under_cap_allows`: a session under the cap reserves a slot (the
//!     cap check passes) — generation may proceed.
//!   - `test_at_cap_hard_blocks`: once a session reaches `session_cap` successful
//!     generations, the next reservation returns a clear, non-retried block
//!     message AND no fal client call is made (the cap is checked before submit).
//!   - `test_per_session_isolation`: session A at the cap is blocked while a
//!     distinct session B (different `SessionKey`) still reserves successfully —
//!     proving the counter is keyed by `SessionKey`, not global.
//!   - `test_interim_ack_emitted_before_submit`: for an allowed generation the
//!     interim "generating…" ack is surfaced through the tool's ack sink BEFORE
//!     `FalClient::submit` is reached (D-04) — giving D-04 automated coverage
//!     independent of the 01-04 human-verify checkpoint.
//!
//! The cap tests exercise the cap-check logic directly against the shared
//! counter without a live fal call; only the ack-ordering test stands up a
//! `wiremock` server, and it records the submit hit to assert ordering.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use ironhermes_core::types::Platform;
use ironhermes_core::{Config, SessionKey};
use ironhermes_tools::fal::FalClient;
use ironhermes_tools::image_gen::{AckSink, ImageGenTool, SessionCounter};
use ironhermes_tools::registry::Tool;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MODEL: &str = "fal-ai/flux/schnell";

/// A fresh shared counter (empty map), the same shape the per-session
/// registration injects into every `ImageGenTool` on a registry.
fn new_counter() -> SessionCounter {
    Arc::new(Mutex::new(std::collections::HashMap::new()))
}

/// Config whose `image_gen.session_cap` is overridden so tests are fast and
/// explicit about the limit they exercise.
fn config_with_cap(cap: u32) -> Arc<Config> {
    let mut cfg = Config::default();
    cfg.image_gen.session_cap = cap;
    Arc::new(cfg)
}

fn session(chat_id: &str) -> SessionKey {
    SessionKey::new(Platform::Telegram, chat_id)
}

/// Build a tool bound to one session, sharing a counter, with no ack sink and a
/// default (production) fal client. The cap check never touches the client.
fn tool_for(session_key: SessionKey, config: Arc<Config>, counter: SessionCounter) -> ImageGenTool {
    ImageGenTool::new_with_session(session_key, config, FalClient::new(), counter, None)
}

#[test]
fn test_under_cap_allows() {
    let counter = new_counter();
    let key = session("chat-A");
    let tool = tool_for(key, config_with_cap(20), counter);

    // No generations yet — the reservation must succeed (generation may proceed).
    assert!(
        tool.try_reserve_slot().is_ok(),
        "a session under the cap must be allowed to proceed past the cap check"
    );
}

#[test]
fn test_at_cap_hard_blocks() {
    let counter = new_counter();
    let key = session("chat-A");
    let cap = 3u32;
    let tool = tool_for(key.clone(), config_with_cap(cap), counter.clone());

    // Simulate `cap` successful generations.
    for _ in 0..cap {
        tool.try_reserve_slot()
            .expect("under-cap reservations succeed");
        tool.record_success();
    }

    // The next reservation must hard-block with a clear, non-retried message —
    // and it must NOT have invoked the fal client (the check is pure / pre-submit).
    let blocked = tool.try_reserve_slot();
    assert!(
        blocked.is_err(),
        "at the cap, the next generation must be blocked"
    );
    let msg = blocked.unwrap_err();
    // The message names the per-session limit (SAFE-05 / D-03) and is clearly
    // non-retried (so the agent does not loop on it).
    let lower = msg.to_lowercase();
    assert!(
        lower.contains("limit") || lower.contains("cap"),
        "block message must name the session limit, got: {msg}"
    );
    assert!(
        !lower.contains("retry") && !lower.contains("try again"),
        "block message must be non-retried, got: {msg}"
    );

    // Counter is unchanged by a blocked reservation (the cap holds steady).
    assert_eq!(
        *counter.lock().unwrap().get(&session("chat-A")).unwrap(),
        cap,
        "a blocked reservation must not increment the counter"
    );
}

#[test]
fn test_per_session_isolation() {
    let counter = new_counter();
    let cap = 2u32;
    let cfg = config_with_cap(cap);

    let key_a = session("chat-A");
    let key_b = session("chat-B");
    let tool_a = tool_for(key_a, cfg.clone(), counter.clone());
    let tool_b = tool_for(key_b, cfg, counter.clone());

    // Drive session A to its cap.
    for _ in 0..cap {
        tool_a.try_reserve_slot().expect("A under cap");
        tool_a.record_success();
    }

    // A is now blocked …
    assert!(
        tool_a.try_reserve_slot().is_err(),
        "session A at the cap must be blocked"
    );
    // … but B (a different SessionKey) is still allowed — proving the counter is
    // keyed by SessionKey, not global.
    assert!(
        tool_b.try_reserve_slot().is_ok(),
        "session B must be unaffected by session A reaching the cap"
    );
}

/// Records each event (ack vs submit) in order so the test can assert the
/// interim ack precedes the fal submit.
#[derive(Clone, Default)]
struct OrderRecorder {
    events: Arc<Mutex<Vec<String>>>,
}

impl OrderRecorder {
    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }
}

impl AckSink for OrderRecorder {
    fn ack(&self, _message: &str) {
        self.events.lock().unwrap().push("ack".to_string());
    }
}

#[tokio::test]
async fn test_interim_ack_emitted_before_submit() {
    let recorder = OrderRecorder::default();
    let events = recorder.events.clone();

    // Stand up a fal mock whose submit endpoint records "submit" the moment it is
    // hit, then returns a request id. We deliberately do NOT mount status/result,
    // so execute() will error after submit — but by then the ordering invariant
    // we care about (ack before submit) has already been observed.
    let server = MockServer::start().await;
    let submit_hits = AtomicUsize::new(0);
    {
        let events = events.clone();
        Mock::given(method("POST"))
            .and(path(format!("/{MODEL}")))
            .respond_with(move |_: &wiremock::Request| {
                submit_hits.fetch_add(1, Ordering::SeqCst);
                events.lock().unwrap().push("submit".to_string());
                ResponseTemplate::new(200).set_body_json(json!({
                    "request_id": "req-ack",
                    "status_url": "",
                    "response_url": "",
                    "cancel_url": ""
                }))
            })
            .mount(&server)
            .await;
    }

    let counter = new_counter();
    let key = session("chat-ack");
    // Phase 47 Plan 06: execute() now resolves the effective model from
    // `image_gen.t2i.model` (D-01/D-03) instead of the legacy flat
    // `default_model` key — pin t2i to the fal MODEL this test mocks so the
    // submit POST lands on `/{MODEL}` as asserted below.
    let mut config = (*config_with_cap(20)).clone();
    config.image_gen.t2i.provider = Some("fal".to_string());
    config.image_gen.t2i.model = MODEL.to_string();
    let tool = ImageGenTool::new_with_session(
        key,
        Arc::new(config),
        FalClient::with_base_url(server.uri()),
        counter,
        Some(Arc::new(recorder.clone())),
    );

    // Ensure FAL_KEY is present so execute() reaches the submit path.
    // SAFETY: single-threaded test; this binary reads FAL_KEY nowhere else.
    let prior = std::env::var("FAL_KEY").ok();
    unsafe {
        std::env::set_var("FAL_KEY", "test-key-not-real");
    }

    // We do not care whether the overall call succeeds (status/result unmounted);
    // only the ordering of ack vs submit.
    let _ = tool
        .execute(json!({ "prompt": "a serene mountain lake" }))
        .await;

    unsafe {
        match prior {
            Some(v) => std::env::set_var("FAL_KEY", v),
            None => std::env::remove_var("FAL_KEY"),
        }
    }

    let observed = recorder.events();
    assert!(
        observed.contains(&"ack".to_string()),
        "an interim ack must be surfaced for an allowed generation, got: {observed:?}"
    );
    assert!(
        observed.contains(&"submit".to_string()),
        "the fal submit must be reached for an allowed (under-cap) generation, got: {observed:?}"
    );
    let ack_idx = observed.iter().position(|e| e == "ack").unwrap();
    let submit_idx = observed.iter().position(|e| e == "submit").unwrap();
    assert!(
        ack_idx < submit_idx,
        "the interim 'generating…' ack must be surfaced BEFORE FalClient::submit (D-04), got: {observed:?}"
    );
}

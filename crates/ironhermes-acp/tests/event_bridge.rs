//! Behavior tests for `AcpEventBridge` (Phase 36.8 plan 03, CLI-05).
//!
//! Task 1's tests use a collecting fake `NotificationSink` instead of a live connection —
//! fast, deterministic, no stdio/transport involved. Task 2's tests drive the bridge
//! through the real `handle_session_prompt` path over an in-memory `Channel::duplex()`
//! (the harness `acp_e2e.rs` established), against a `wiremock`-mocked provider endpoint
//! so `run_turn` completes successfully without a real API key.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, SessionUpdate, ToolCallStatus,
};
use agent_client_protocol::{Channel, Client};
use ironhermes_acp::event_bridge::{AcpEventBridge, NotificationSink};
use ironhermes_acp::handlers::DenialLedger;
use ironhermes_core::{Config, ProviderResolver};
use ironhermes_state::StateStore;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Mirrors `event_bridge.rs`'s private `KEEPALIVE_INTERVAL_SECS` default (60s) — the
/// heartbeat tests below never set `IRONHERMES_ACP_KEEPALIVE_SECS`, so they always run
/// against this same default value. Kept here (rather than exported from `src`) because
/// tests only need the NUMBER to compute clock advances, not the constant itself.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(60);

/// Process-wide mutex for tests that mutate `IRONHERMES_ACP_KEEPALIVE_SECS`. Required for
/// all tests using `std::env::set_var`/`remove_var` (Rust 2024 marks these `unsafe`, and
/// env state is shared across every test in this binary) — mirrors the `env_lock()`
/// convention already used in `ironhermes-core::provider`'s test module, but as a
/// `tokio::sync::Mutex` (rather than `std::sync::Mutex`) since every heartbeat test below
/// holds the guard across `.await` points (clock advances) for its full duration — even the
/// ones that never touch the env var — so bridge construction never races test 7's mutation
/// of the same var.
fn env_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Collects every notification sent to it, in the order received.
#[derive(Clone, Default)]
struct CollectingSink {
    notifications: Arc<Mutex<Vec<agent_client_protocol::schema::v1::SessionNotification>>>,
}

impl NotificationSink for CollectingSink {
    fn send(&self, notification: agent_client_protocol::schema::v1::SessionNotification) {
        self.notifications.lock().unwrap().push(notification);
    }
}

impl CollectingSink {
    fn updates(&self) -> Vec<SessionUpdate> {
        self.notifications
            .lock()
            .unwrap()
            .iter()
            .map(|n| n.update.clone())
            .collect()
    }
}

/// Builds a bridge over a fresh `CollectingSink`, runs `drive` against the bridge's
/// callbacks, then drops the bridge and every callback so the channel closes, awaits the
/// drain task to guarantee every enqueued update has been forwarded, and returns the
/// sink's collected updates in order.
async fn drive_and_collect(
    drive: impl FnOnce(
        &ironhermes_agent::agent_loop::StreamCallback,
        &ironhermes_agent::agent_loop::ToolProgressCallback,
        &ironhermes_agent::agent_loop::ToolResultCallback,
    ),
) -> Vec<SessionUpdate> {
    let sink = CollectingSink::default();
    let (bridge, drain_handle) = AcpEventBridge::new(
        Arc::new(sink.clone()),
        "acp_test_session",
        "/tmp",
        DenialLedger::new(),
    );

    let stream = bridge.stream_callback();
    let tool_progress = bridge.tool_progress_callback();
    let tool_result = bridge.tool_result_callback();

    drive(&stream, &tool_progress, &tool_result);

    drop(stream);
    drop(tool_progress);
    drop(tool_result);
    drop(bridge);

    drain_handle.await.expect("drain task should not panic");

    sink.updates()
}

fn text_of(update: &SessionUpdate) -> Option<String> {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => match &chunk.content {
            ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        },
        _ => None,
    }
}

#[tokio::test]
async fn three_successive_stream_calls_preserve_order() {
    let updates = drive_and_collect(|stream, _tool_progress, _tool_result| {
        stream("a");
        stream("b");
        stream("c");
    })
    .await;

    let texts: Vec<Option<String>> = updates.iter().map(text_of).collect();
    assert_eq!(
        texts,
        vec![
            Some("a".to_string()),
            Some("b".to_string()),
            Some("c".to_string())
        ]
    );
}

#[tokio::test]
async fn first_tool_progress_produces_tool_call_second_produces_update() {
    let updates = drive_and_collect(|_stream, tool_progress, _tool_result| {
        tool_progress("terminal", "started");
        tool_progress("terminal", "running");
    })
    .await;

    assert_eq!(updates.len(), 2);
    assert!(matches!(updates[0], SessionUpdate::ToolCall(_)));
    assert!(matches!(updates[1], SessionUpdate::ToolCallUpdate(_)));

    let SessionUpdate::ToolCall(first_call) = &updates[0] else {
        unreachable!()
    };
    let SessionUpdate::ToolCallUpdate(second_update) = &updates[1] else {
        unreachable!()
    };
    assert_eq!(first_call.tool_call_id, second_update.tool_call_id);
}

#[tokio::test]
async fn tool_result_ok_produces_completed_update_on_same_id() {
    let updates = drive_and_collect(|_stream, tool_progress, tool_result| {
        tool_progress("terminal", "started");
        tool_result("terminal", true, "ok output");
    })
    .await;

    assert_eq!(updates.len(), 2);
    let SessionUpdate::ToolCall(call) = &updates[0] else {
        panic!("expected ToolCall first, got {:?}", updates[0]);
    };
    let SessionUpdate::ToolCallUpdate(update) = &updates[1] else {
        panic!("expected ToolCallUpdate second, got {:?}", updates[1]);
    };
    assert_eq!(call.tool_call_id, update.tool_call_id);
    assert_eq!(update.fields.status, Some(ToolCallStatus::Completed));
}

#[tokio::test]
async fn tool_result_failure_produces_failed_update() {
    let updates = drive_and_collect(|_stream, tool_progress, tool_result| {
        tool_progress("terminal", "started");
        tool_result("terminal", false, "error output");
    })
    .await;

    let SessionUpdate::ToolCallUpdate(update) = &updates[1] else {
        panic!("expected ToolCallUpdate second, got {:?}", updates[1]);
    };
    assert_eq!(update.fields.status, Some(ToolCallStatus::Failed));
}

#[tokio::test]
async fn two_simultaneously_in_flight_tools_get_distinct_ids_and_each_result_updates_only_its_own()
 {
    let updates = drive_and_collect(|_stream, tool_progress, tool_result| {
        tool_progress("terminal", "started");
        tool_progress("browser", "started");
        tool_result("terminal", true, "terminal output");
        tool_result("browser", false, "browser error");
    })
    .await;

    assert_eq!(updates.len(), 4);
    let SessionUpdate::ToolCall(terminal_call) = &updates[0] else {
        panic!("expected ToolCall");
    };
    let SessionUpdate::ToolCall(browser_call) = &updates[1] else {
        panic!("expected ToolCall");
    };
    assert_ne!(terminal_call.tool_call_id, browser_call.tool_call_id);

    let SessionUpdate::ToolCallUpdate(terminal_update) = &updates[2] else {
        panic!("expected ToolCallUpdate");
    };
    let SessionUpdate::ToolCallUpdate(browser_update) = &updates[3] else {
        panic!("expected ToolCallUpdate");
    };
    assert_eq!(terminal_update.tool_call_id, terminal_call.tool_call_id);
    assert_eq!(browser_update.tool_call_id, browser_call.tool_call_id);
    assert_eq!(terminal_update.fields.status, Some(ToolCallStatus::Completed));
    assert_eq!(browser_update.fields.status, Some(ToolCallStatus::Failed));
}

#[tokio::test]
async fn tool_result_with_no_matching_in_flight_call_emits_tool_call_then_terminal_update() {
    let updates = drive_and_collect(|_stream, _tool_progress, tool_result| {
        tool_result("terminal", true, "ok output");
    })
    .await;

    assert_eq!(updates.len(), 2);
    assert!(matches!(updates[0], SessionUpdate::ToolCall(_)));
    assert!(matches!(updates[1], SessionUpdate::ToolCallUpdate(_)));
    let SessionUpdate::ToolCall(call) = &updates[0] else {
        unreachable!()
    };
    let SessionUpdate::ToolCallUpdate(update) = &updates[1] else {
        unreachable!()
    };
    assert_eq!(call.tool_call_id, update.tool_call_id);
    assert_eq!(update.fields.status, Some(ToolCallStatus::Completed));
}

#[tokio::test]
async fn clearing_after_terminal_update_allocates_a_fresh_id_on_next_sighting() {
    let updates = drive_and_collect(|_stream, tool_progress, tool_result| {
        tool_progress("terminal", "started");
        tool_result("terminal", true, "ok output");
        tool_progress("terminal", "started-again");
    })
    .await;

    assert_eq!(updates.len(), 3);
    let SessionUpdate::ToolCall(first_call) = &updates[0] else {
        panic!("expected ToolCall");
    };
    assert!(matches!(updates[2], SessionUpdate::ToolCall(_)));
    let SessionUpdate::ToolCall(second_call) = &updates[2] else {
        unreachable!()
    };
    assert_ne!(first_call.tool_call_id, second_call.tool_call_id);
}

#[tokio::test]
async fn no_reasoning_or_other_opt_out_variant_is_ever_produced() {
    let updates = drive_and_collect(|stream, tool_progress, tool_result| {
        stream("hi");
        tool_progress("terminal", "started");
        tool_result("terminal", true, "ok output");
    })
    .await;

    for update in &updates {
        assert!(
            matches!(
                update,
                SessionUpdate::AgentMessageChunk(_)
                    | SessionUpdate::ToolCall(_)
                    | SessionUpdate::ToolCallUpdate(_)
            ),
            "unexpected update variant: {update:?}"
        );
    }
}

#[tokio::test]
async fn send_update_is_forwarded_through_the_same_channel() {
    let sink = CollectingSink::default();
    let (bridge, drain_handle) = AcpEventBridge::new(
        Arc::new(sink.clone()),
        "acp_test_session",
        "/tmp",
        DenialLedger::new(),
    );

    let stream = bridge.stream_callback();
    stream("hello");
    drop(stream);

    bridge.send_update(SessionUpdate::UsageUpdate(
        agent_client_protocol::schema::v1::UsageUpdate::new(10, 200_000),
    ));
    drop(bridge);

    drain_handle.await.expect("drain task should not panic");

    let updates = sink.updates();
    assert_eq!(updates.len(), 2);
    assert!(matches!(updates[0], SessionUpdate::AgentMessageChunk(_)));
    assert!(matches!(updates[1], SessionUpdate::UsageUpdate(_)));
}

// ─────────────────────────────────────────────────────────────────────────
// Plan 03 (D-07): deterministic virtual-clock coverage for the self-terminating
// keepalive heartbeat. All seven tests below run on a paused tokio clock
// (`#[tokio::test(start_paused = true)]`) and use `tokio::time::advance(...)` instead of
// real sleeps, so this whole block completes in well under a second of wall clock. They
// reuse `CollectingSink` (defined above) rather than a second collecting fake.
// ─────────────────────────────────────────────────────────────────────────

/// Builds a fresh bridge (default keepalive interval — no env var override) over a fresh
/// `CollectingSink`, without driving or dropping anything yet. The seven tests below need
/// to interleave callback calls with clock advances, so they can't use the synchronous
/// `drive_and_collect` shape above (which drives everything before dropping in one go).
async fn new_heartbeat_bridge() -> (CollectingSink, AcpEventBridge, tokio::task::JoinHandle<()>) {
    let sink = CollectingSink::default();
    let (bridge, drain_handle) =
        AcpEventBridge::new(
        Arc::new(sink.clone()),
        "acp_test_session",
        "/tmp",
        DenialLedger::new(),
    );
    // Let the newly spawned ticker task run its FIRST poll now, while the clock is still
    // at this test's t=0 baseline — its `tokio::time::sleep(interval)` call must register
    // its deadline against t=0, not against whatever time a later `advance()` call would
    // otherwise land the task's first poll on.
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
    (sink, bridge, drain_handle)
}

/// Advances the paused clock by `duration`, then yields the current task repeatedly so the
/// executor gets a chance to poll the ticker task (woken by the now-elapsed timer) all the
/// way to its next await point — `tokio::time::advance` moves the clock and wakes elapsed
/// timers, but does not itself guarantee the woken task has been driven forward before it
/// returns control to the caller.
async fn advance_and_let_background_tasks_run(duration: Duration) {
    tokio::time::advance(duration).await;
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
}

/// Test 1: a tool call in flight with no other traffic — advancing the clock past one
/// interval produces exactly one additional `ToolCallUpdate` on the SAME id, status
/// `InProgress`.
#[tokio::test(start_paused = true)]
async fn heartbeat_fires_once_after_one_silent_interval() {
    let _guard = env_lock().lock().await;
    let (sink, bridge, drain_handle) = new_heartbeat_bridge().await;
    let tool_progress = bridge.tool_progress_callback();

    tool_progress("terminal", "started");
    drop(tool_progress);

    advance_and_let_background_tasks_run(KEEPALIVE_INTERVAL + Duration::from_secs(1)).await;

    drop(bridge);
    drain_handle.await.expect("drain task should not panic");

    let updates = sink.updates();
    assert_eq!(
        updates.len(),
        2,
        "expected ToolCall + exactly one heartbeat ToolCallUpdate: {updates:?}"
    );
    let SessionUpdate::ToolCall(call) = &updates[0] else {
        panic!("expected ToolCall first, got {:?}", updates[0]);
    };
    let SessionUpdate::ToolCallUpdate(heartbeat) = &updates[1] else {
        panic!("expected ToolCallUpdate (heartbeat) second, got {:?}", updates[1]);
    };
    assert_eq!(heartbeat.tool_call_id, call.tool_call_id);
    assert_eq!(heartbeat.fields.status, Some(ToolCallStatus::InProgress));
}

/// Test 2: continued silence past three intervals produces three heartbeats, all on the
/// same id.
#[tokio::test(start_paused = true)]
async fn heartbeat_fires_repeatedly_across_multiple_silent_intervals() {
    let _guard = env_lock().lock().await;
    let (sink, bridge, drain_handle) = new_heartbeat_bridge().await;
    let tool_progress = bridge.tool_progress_callback();

    tool_progress("terminal", "started");
    drop(tool_progress);

    // `tokio::time::advance` jumps the clock in one shot and wakes only the timers already
    // pending at the moment of the jump — the ticker's loop re-arms its NEXT `sleep(...)`
    // using the already-advanced "now", so a single large advance only ever produces one
    // wake. Advance one interval at a time (three times) to let the ticker's loop actually
    // re-arm and fire three separate times.
    for _ in 0..3 {
        advance_and_let_background_tasks_run(KEEPALIVE_INTERVAL + Duration::from_secs(1)).await;
    }

    drop(bridge);
    drain_handle.await.expect("drain task should not panic");

    let updates = sink.updates();
    assert_eq!(
        updates.len(),
        4,
        "expected ToolCall + 3 heartbeats, all on the same id: {updates:?}"
    );
    let SessionUpdate::ToolCall(call) = &updates[0] else {
        panic!("expected ToolCall first, got {:?}", updates[0]);
    };
    for update in &updates[1..] {
        let SessionUpdate::ToolCallUpdate(heartbeat) = update else {
            panic!("expected ToolCallUpdate heartbeat, got {update:?}");
        };
        assert_eq!(heartbeat.tool_call_id, call.tool_call_id);
        assert_eq!(heartbeat.fields.status, Some(ToolCallStatus::InProgress));
    }
}

/// Test 3: traffic resets the timer — a `tool_progress` fired part-way through an interval
/// means no heartbeat is emitted until a full interval has elapsed since THAT emission, not
/// since the tool call originally started, and no LONGER than that either. The upper bound
/// is the half that matters live: buzz-acp drops a connection on its idle timeout, so an
/// operator lowering `IRONHERMES_ACP_KEEPALIVE_SECS` to sit under that timeout needs the
/// worst case to be the interval they set rather than twice it.
#[tokio::test(start_paused = true)]
async fn traffic_mid_interval_resets_the_heartbeat_timer() {
    let _guard = env_lock().lock().await;
    let (sink, bridge, drain_handle) = new_heartbeat_bridge().await;
    let tool_progress = bridge.tool_progress_callback();

    tool_progress("terminal", "started"); // t=0: ToolCall, last_emit=0
    advance_and_let_background_tasks_run(KEEPALIVE_INTERVAL / 2).await; // t=30
    tool_progress("terminal", "still going"); // t=30: ToolCallUpdate, last_emit=30
    drop(tool_progress);

    // Advance just past the ORIGINAL interval boundary measured from bridge creation
    // (t=60) — no heartbeat should have fired yet, because the reset at t=30 means a full
    // interval has not elapsed since the last emission.
    advance_and_let_background_tasks_run(KEEPALIVE_INTERVAL / 2 + Duration::from_secs(1)).await; // t=61
    assert_eq!(
        sink.updates().len(),
        2,
        "no heartbeat should fire before a full interval has elapsed since the t=30 reset: {:?}",
        sink.updates()
    );

    // Advance just past last_emit(30) + interval(60) = 90. This is the assertion that
    // pins the documented guarantee: worst-case wire silence is ONE interval after the
    // last traffic, not two. A ticker that re-arms a whole fresh interval on every
    // mid-interval reset would not fire until t=121 and would leave this at 2 — which is
    // exactly the behavior this test used to encode as expected.
    advance_and_let_background_tasks_run(KEEPALIVE_INTERVAL / 2).await; // t=91
    drop(bridge);
    drain_handle.await.expect("drain task should not panic");

    let updates = sink.updates();
    assert_eq!(
        updates.len(),
        3,
        "expected ToolCall + ToolCallUpdate(progress) + exactly one heartbeat: {updates:?}"
    );
    assert!(matches!(updates[2], SessionUpdate::ToolCallUpdate(_)));
}

/// Test 4: with NO tool call ever in flight, advancing the clock produces zero heartbeats
/// — the bridge never invents a tool call that is not running.
#[tokio::test(start_paused = true)]
async fn no_heartbeat_when_no_tool_call_in_flight() {
    let _guard = env_lock().lock().await;
    let (sink, bridge, drain_handle) = new_heartbeat_bridge().await;

    // Deliberately never drives any callback.
    advance_and_let_background_tasks_run(KEEPALIVE_INTERVAL * 3 + Duration::from_secs(1)).await;

    drop(bridge);
    drain_handle.await.expect("drain task should not panic");

    assert!(
        sink.updates().is_empty(),
        "expected zero heartbeats with nothing ever in flight: {:?}",
        sink.updates()
    );
}

/// Test 5: after `tool_result` clears the in-flight entry, advancing the clock produces
/// zero further heartbeats.
#[tokio::test(start_paused = true)]
async fn no_further_heartbeat_after_tool_result_clears_in_flight() {
    let _guard = env_lock().lock().await;
    let (sink, bridge, drain_handle) = new_heartbeat_bridge().await;
    let tool_progress = bridge.tool_progress_callback();
    let tool_result = bridge.tool_result_callback();

    tool_progress("terminal", "started");
    tool_result("terminal", true, "done");
    drop(tool_progress);
    drop(tool_result);

    advance_and_let_background_tasks_run(KEEPALIVE_INTERVAL * 3 + Duration::from_secs(1)).await;

    drop(bridge);
    drain_handle.await.expect("drain task should not panic");

    let updates = sink.updates();
    assert_eq!(
        updates.len(),
        2,
        "expected only the ToolCall + terminal ToolCallUpdate, zero heartbeats: {updates:?}"
    );
    assert!(matches!(updates[1], SessionUpdate::ToolCallUpdate(_)));
}

/// Test 6 (T-47.7-08): dropping the bridge and awaiting the drain handle completes — the
/// heartbeat ticker cannot keep the channel open. Wrapped in `tokio::time::timeout`, raced
/// (via `tokio::join!`) against an explicit clock advance, so a strong-sender regression
/// manifests as a bounded timeout failure rather than hanging the whole test suite.
#[tokio::test(start_paused = true)]
async fn drain_completes_after_bridge_drop_even_with_ticker_pending() {
    let _guard = env_lock().lock().await;
    let (sink, bridge, drain_handle) = new_heartbeat_bridge().await;
    let tool_progress = bridge.tool_progress_callback();

    tool_progress("terminal", "started");
    drop(tool_progress);
    drop(bridge);

    // Give a (hypothetically buggy, strong-sender-holding) ticker several intervals'
    // worth of virtual time to keep the channel alive, concurrently with a bounded
    // timeout on the drain handle itself.
    let (timeout_result, ()) = tokio::join!(
        tokio::time::timeout(Duration::from_secs(2), drain_handle),
        tokio::time::advance(KEEPALIVE_INTERVAL * 6),
    );

    let join_result = timeout_result.expect(
        "drain_handle must resolve promptly after bridge drop — a timeout here means the \
         keepalive ticker is holding a strong sender open (regression)",
    );
    join_result.expect("drain task should not panic");

    // The turn's one real ToolCall must still have reached the sink before the drop.
    assert!(!sink.updates().is_empty());
}

/// Test 7 (D-07): `IRONHERMES_ACP_KEEPALIVE_SECS=0` disables the heartbeat entirely — zero
/// emissions regardless of how far the clock advances. Sets and restores the env var
/// within a single test (serialized via `env_lock`) so a parallel test never observes a
/// mutated value — running this file twice in a row gives identical results.
#[tokio::test(start_paused = true)]
async fn keepalive_secs_zero_disables_heartbeat_entirely() {
    let _guard = env_lock().lock().await;

    // SAFETY: test-only env var mutation, held behind env_lock for this test's full
    // duration (mirrors ironhermes-core::provider's env_lock convention for Rust 2024's
    // unsafe std::env::set_var/remove_var).
    unsafe {
        std::env::set_var("IRONHERMES_ACP_KEEPALIVE_SECS", "0");
    }

    let (sink, bridge, drain_handle) = new_heartbeat_bridge().await;
    let tool_progress = bridge.tool_progress_callback();
    tool_progress("terminal", "started");
    drop(tool_progress);

    advance_and_let_background_tasks_run(KEEPALIVE_INTERVAL * 10).await;

    drop(bridge);
    drain_handle.await.expect("drain task should not panic");

    // SAFETY: test-only cleanup, still held behind env_lock
    unsafe {
        std::env::remove_var("IRONHERMES_ACP_KEEPALIVE_SECS");
    }

    let updates = sink.updates();
    assert_eq!(
        updates.len(),
        1,
        "expected only the ToolCall from tool_progress, zero heartbeats: {updates:?}"
    );
    assert!(matches!(updates[0], SessionUpdate::ToolCall(_)));
}

// ─────────────────────────────────────────────────────────────────────────
// Task 2: full `session/prompt` round trip through the real ACP server
// (`run_acp_over` over `Channel::duplex()` — the harness `acp_e2e.rs`
// established), against a `wiremock`-mocked provider so `run_turn` completes
// successfully without a real API key or network access.
// ─────────────────────────────────────────────────────────────────────────

/// A `Config`/`ProviderResolver` whose main provider ("openrouter", the default)
/// resolves to `server_uri` with a literal test api key — every `TurnRequest.stream`
/// wiring forces streaming mode (`AgentRuntime::run_turn` calls `with_streaming` whenever
/// `TurnRequest.stream` is `Some`, which `handle_session_prompt` always sets), so mocked
/// responses must be SSE bodies, not plain JSON.
fn build_config_and_resolver_pointed_at(server_uri: &str) -> (Arc<Config>, Arc<ProviderResolver>) {
    let mut config = Config::default();
    config.providers.insert(
        "openrouter".to_string(),
        ironhermes_core::ProviderConfig {
            base_url: Some(server_uri.to_string()),
            api_key: Some("test-key".to_string()),
            ..Default::default()
        },
    );
    let resolver =
        ProviderResolver::build(&config).expect("ProviderResolver::build with mocked provider");
    (Arc::new(config), Arc::new(resolver))
}

/// Isolated, tempdir-backed `StateStore` (mirrors `acp_e2e.rs`) — never touches the
/// operator's real `$IRONHERMES_HOME/state.db`.
fn build_state_store() -> (Arc<Mutex<StateStore>>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir for state.db");
    let store = StateStore::new(tmp.path().join("state.db")).expect("StateStore::new");
    (Arc::new(Mutex::new(store)), tmp)
}

/// SSE body: two content deltas, then a `finish_reason: stop` chunk carrying `usage` —
/// the simple (no tool call) case task 2's acceptance criteria targets.
fn sse_text_only_with_usage() -> String {
    let chunks = [
        r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"test-model","choices":[{"index":0,"delta":{"content":"hello "}}]}"#,
        r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"test-model","choices":[{"index":0,"delta":{"content":"world"}}]}"#,
        r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"test-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":12,"completion_tokens":3,"total_tokens":15}}"#,
    ];
    let mut body = String::new();
    for c in chunks {
        body.push_str("data: ");
        body.push_str(c);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

/// SSE body for a turn that requests the always-registered `terminal` tool.
fn sse_tool_call_terminal() -> String {
    let chunks = [
        r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"test-model","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_acp_test","type":"function","function":{"name":"terminal","arguments":"{\"command\":\"echo acp-tool-test\"}"}}]}}]}"#,
        r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"test-model","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#,
    ];
    let mut body = String::new();
    for c in chunks {
        body.push_str("data: ");
        body.push_str(c);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

/// SSE body for the second (final) turn after the tool result comes back.
fn sse_final_text_with_usage() -> String {
    let chunks = [
        r#"{"id":"c2","object":"chat.completion.chunk","created":2,"model":"test-model","choices":[{"index":0,"delta":{"content":"done"}}]}"#,
        r#"{"id":"c2","object":"chat.completion.chunk","created":2,"model":"test-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":20,"completion_tokens":2,"total_tokens":22}}"#,
    ];
    let mut body = String::new();
    for c in chunks {
        body.push_str("data: ");
        body.push_str(c);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

/// Reads every `session/update` notification until the prompt's stop reason arrives,
/// returning them in order. Mirrors the SDK's own `ActiveSession::read_to_string` but
/// collects the full `SessionUpdate` (not just concatenated text) — needed to assert on
/// `tool_call`/`tool_call_update`/`usage_update` variants, not just streamed text.
async fn read_all_updates<Link>(
    session: &mut agent_client_protocol::ActiveSession<'_, Link>,
) -> Vec<SessionUpdate>
where
    Link: agent_client_protocol::role::HasPeer<agent_client_protocol::Agent>,
{
    use agent_client_protocol::util::MatchDispatch;
    let mut updates = Vec::new();
    loop {
        let message = session
            .read_update()
            .await
            .expect("session channel should not close before StopReason");
        match message {
            agent_client_protocol::SessionMessage::SessionMessage(dispatch) => {
                MatchDispatch::new(dispatch)
                    .if_notification(
                        async |notif: agent_client_protocol::schema::v1::SessionNotification| {
                            updates.push(notif.update);
                            Ok(())
                        },
                    )
                    .await
                    .otherwise_ignore()
                    .expect("dispatch matching should not error");
            }
            agent_client_protocol::SessionMessage::StopReason(_) => break,
            // `SessionMessage` is `#[non_exhaustive]` — treat any future variant as a
            // reason to stop collecting rather than looping forever.
            _ => break,
        }
    }
    updates
}

/// Task 2 acceptance criterion: a full `session/prompt` round trip (no tool call) emits a
/// `usage_update` notification, carrying the turn's real token totals from the mocked
/// provider response.
#[tokio::test]
async fn full_prompt_round_trip_emits_usage_update() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse_text_only_with_usage(), "text/event-stream"),
        )
        .mount(&server)
        .await;

    let (config, resolver) = build_config_and_resolver_pointed_at(&server.uri());
    let (state_store, _state_tmp) = build_state_store();
    let (agent_channel, client_channel) = Channel::duplex();

    let agent_task = tokio::spawn(async move {
        ironhermes_acp::entry::run_acp_over(config, resolver, state_store, agent_channel).await
    });

    let tmp = tempfile::tempdir().expect("tempdir");
    let tmp_path = tmp.path().to_path_buf();

    let client_result = Client
        .builder()
        .name("acp-event-bridge-usage-test-client")
        .connect_with(client_channel, async move |cx| {
            cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;

            cx.build_session(&tmp_path)
                .block_task()
                .run_until(async move |mut session| {
                    session.send_prompt("hello from the usage-update test")?;
                    let updates = read_all_updates(&mut session).await;

                    assert!(
                        updates
                            .iter()
                            .any(|u| matches!(u, SessionUpdate::AgentMessageChunk(_))),
                        "expected at least one agent_message_chunk, got: {updates:?}"
                    );
                    let usage_update = updates.iter().find_map(|u| match u {
                        SessionUpdate::UsageUpdate(usage) => Some(usage),
                        _ => None,
                    });
                    let usage_update = usage_update.unwrap_or_else(|| {
                        panic!("expected a UsageUpdate notification, got: {updates:?}")
                    });
                    assert_eq!(
                        usage_update.used, 15,
                        "usage update should carry the turn's real total_tokens"
                    );

                    Ok(())
                })
                .await
        })
        .await;

    client_result.expect("client exchange should succeed");
    agent_task.abort();
}

/// Plan-level `<verification>`: a tool-using prompt yields, in order, at least one
/// `agent_message_chunk`-or-`tool_call` activity, a `tool_call` immediately followed
/// (eventually) by its terminal `tool_call_update`, and a trailing `usage_update` — and a
/// trajectory file exists under the session's cwd afterward (D-18).
#[tokio::test]
async fn tool_using_prompt_yields_ordered_updates_and_writes_trajectory() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse_tool_call_terminal(), "text/event-stream"),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse_final_text_with_usage(), "text/event-stream"),
        )
        .mount(&server)
        .await;

    let (config, resolver) = build_config_and_resolver_pointed_at(&server.uri());
    let (state_store, _state_tmp) = build_state_store();
    let (agent_channel, client_channel) = Channel::duplex();

    let agent_task = tokio::spawn(async move {
        ironhermes_acp::entry::run_acp_over(config, resolver, state_store, agent_channel).await
    });

    let tmp = tempfile::tempdir().expect("tempdir");
    let tmp_path = tmp.path().to_path_buf();
    let tmp_path_for_check = tmp_path.clone();

    let client_result = Client
        .builder()
        .name("acp-event-bridge-tool-call-test-client")
        .connect_with(client_channel, async move |cx| {
            cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;

            let session_id_str = cx
                .build_session(&tmp_path)
                .block_task()
                .run_until(async move |mut session| {
                    let session_id_str = session.session_id().to_string();
                    session.send_prompt("please run a shell command")?;
                    let updates = read_all_updates(&mut session).await;

                    assert!(
                        updates
                            .iter()
                            .any(|u| matches!(u, SessionUpdate::AgentMessageChunk(_))),
                        "plan-level <verification>: a tool-using turn must still yield at \
                         least one agent_message_chunk (the final turn's streamed text), \
                         got: {updates:?}"
                    );

                    let tool_call_pos = updates
                        .iter()
                        .position(|u| matches!(u, SessionUpdate::ToolCall(_)));
                    let tool_call_update_pos = updates.iter().position(|u| {
                        matches!(
                            u,
                            SessionUpdate::ToolCallUpdate(update)
                                if matches!(
                                    update.fields.status,
                                    Some(ToolCallStatus::Completed | ToolCallStatus::Failed)
                                )
                        )
                    });
                    let usage_pos = updates
                        .iter()
                        .position(|u| matches!(u, SessionUpdate::UsageUpdate(_)));

                    let tool_call_pos = tool_call_pos
                        .unwrap_or_else(|| panic!("expected a ToolCall update, got: {updates:?}"));
                    let tool_call_update_pos = tool_call_update_pos.unwrap_or_else(|| {
                        panic!("expected a terminal ToolCallUpdate, got: {updates:?}")
                    });
                    let usage_pos = usage_pos
                        .unwrap_or_else(|| panic!("expected a UsageUpdate, got: {updates:?}"));

                    assert!(
                        tool_call_pos < tool_call_update_pos,
                        "ToolCall must precede its terminal ToolCallUpdate: {updates:?}"
                    );
                    assert!(
                        tool_call_update_pos < usage_pos,
                        "the terminal ToolCallUpdate must precede the trailing UsageUpdate: {updates:?}"
                    );

                    Ok(session_id_str)
                })
                .await?;

            Ok(session_id_str)
        })
        .await;

    let session_id_str = client_result.expect("client exchange should succeed");
    agent_task.abort();

    // D-18: a trajectory file must exist under the session's cwd after a tool-using turn.
    let traj_path = tmp_path_for_check
        .join(".ironhermes")
        .join("sessions")
        .join(&session_id_str)
        .join("trajectories.jsonl");
    assert!(
        traj_path.exists(),
        "expected a trajectory file at {}",
        traj_path.display()
    );
}

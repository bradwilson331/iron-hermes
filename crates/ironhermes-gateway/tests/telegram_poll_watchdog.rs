//! Phase 54.8 Plan 07: the Telegram poll supervisor (D-04 / D-05 / D-06).
//!
//! Every test uses millisecond `PollTuning` values against a silent TCP
//! listener or a wiremock Telegram, never real-time minutes. The tests run on
//! the default current-thread tokio runtime on purpose: the supervisor and its
//! generations are `tokio::spawn`ed onto that same thread, so a
//! `tracing::subscriber::set_default` scoped to the test sees their log lines.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ironhermes_gateway::poll_supervisor::{PollStats, PollTuning, run_poll_supervisor};
use ironhermes_gateway::telegram::{TelegramAdapter, TelegramHttpTuning};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const TOKEN: &str = "test-token-abc";

/// A TCP listener that accepts every connection and never answers, so a
/// `getUpdates` against it hangs exactly like a black-holed route.
async fn silent_listener() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });
    format!("http://{addr}")
}

fn short_http_tuning() -> TelegramHttpTuning {
    TelegramHttpTuning {
        poll_deadline: Duration::from_millis(150),
        ..TelegramHttpTuning::default()
    }
}

fn fast_tuning() -> PollTuning {
    PollTuning {
        stale_after: Duration::from_millis(500),
        tick: Duration::from_millis(50),
        min_respawn_gap: Duration::from_millis(100),
        join_grace: Duration::from_millis(100),
        trace_every: Duration::from_secs(600),
        handoff_timeout: Duration::from_millis(200),
        backoff_base: Duration::from_millis(10),
        backoff_cap: Duration::from_millis(50),
    }
}

#[tokio::test]
async fn supervisor_respawns_after_stall() {
    let base = silent_listener().await;
    let adapter = Arc::new(TelegramAdapter::for_tests(TOKEN, base, short_http_tuning()));
    let (tx, _rx) = mpsc::channel(8);
    let parent = CancellationToken::new();
    let stats = Arc::new(PollStats::default());
    let task = tokio::spawn(run_poll_supervisor(
        adapter.clone(),
        tx,
        parent.clone(),
        fast_tuning(),
        stats.clone(),
    ));

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        // 0 only before the first generation task has been polled.
        assert!(
            stats.max_active_generations() <= 1,
            "two poll generations ran at once"
        );
        if stats.generations() >= 3 && adapter.http_generation() >= 2 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no respawn within 3 s: generations={} http_generation={}",
            stats.generations(),
            adapter.http_generation()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    parent.cancel();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("supervisor must return within 1 s of a parent cancel")
        .expect("supervisor task must not panic");
    assert_eq!(stats.max_active_generations(), 1);
}

#[tokio::test]
async fn supervisor_stops_on_parent_cancel_without_respawn() {
    let base = silent_listener().await;
    let adapter = Arc::new(TelegramAdapter::for_tests(TOKEN, base, short_http_tuning()));
    let (tx, _rx) = mpsc::channel(8);
    let parent = CancellationToken::new();
    let stats = Arc::new(PollStats::default());
    let task = tokio::spawn(run_poll_supervisor(
        adapter.clone(),
        tx,
        parent.clone(),
        fast_tuning(),
        stats.clone(),
    ));
    // Cancel well before the first stale window (500 ms).
    tokio::time::sleep(Duration::from_millis(60)).await;
    parent.cancel();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("supervisor must return after the parent cancel")
        .expect("supervisor task must not panic");
    assert_eq!(stats.generations(), 1, "a parent cancel must not respawn");
    assert_eq!(stats.respawns(), 0);
    assert_eq!(adapter.http_generation(), 0, "no rebuild without a stall");
}

// ---------------------------------------------------------------------------
// Task 2: hand-off, back-pressure, closed channel, 409.
// ---------------------------------------------------------------------------

use std::io::Write;
use std::sync::Mutex;

use ironhermes_gateway::poll_supervisor::{GenerationExit, PollShared, run_poll_generation};
use ironhermes_gateway::telegram::TgUpdate;
use wiremock::matchers::{method, path};
use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

/// A `tracing` writer that appends into a shared buffer.
#[derive(Clone, Default)]
struct LogBuf(Arc<Mutex<Vec<u8>>>);

impl Write for LogBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl LogBuf {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

/// Capture INFO+ tracing output for the lifetime of the returned guard. The
/// tests run on a current-thread runtime, so spawned tasks log into it too.
fn capture_logs() -> (LogBuf, tracing::subscriber::DefaultGuard) {
    let buf = LogBuf::default();
    let writer = buf.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .finish();
    (buf, tracing::subscriber::set_default(subscriber))
}

/// `offset` field of a getUpdates request body (None = absent).
fn req_offset(req: &Request) -> Option<i64> {
    serde_json::from_slice::<serde_json::Value>(&req.body)
        .ok()?
        .get("offset")?
        .as_i64()
}

/// Wiremock matcher over the request's `offset`.
struct OffsetIs(fn(Option<i64>) -> bool);

impl Match for OffsetIs {
    fn matches(&self, request: &Request) -> bool {
        (self.0)(req_offset(request))
    }
}

fn updates_body(ids: &[i64]) -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "result": ids.iter().map(|id| serde_json::json!({"update_id": id})).collect::<Vec<_>>(),
    })
}

/// A getUpdates reply that takes 20 ms, so the mock never turns the poller
/// into a hot loop.
fn reply(body: serde_json::Value) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .set_body_json(body)
        .set_delay(Duration::from_millis(20))
}

async fn mount_get_updates(
    server: &MockServer,
    when: fn(Option<i64>) -> bool,
    body: serde_json::Value,
) {
    Mock::given(method("POST"))
        .and(path(format!("/bot{TOKEN}/getUpdates")))
        .and(OffsetIs(when))
        .respond_with(reply(body))
        .mount(server)
        .await;
}

fn dummy_update(id: i64) -> TgUpdate {
    TgUpdate {
        update_id: id,
        message: None,
        callback_query: None,
    }
}

/// Wait until some recorded request carries `offset`.
async fn wait_for_offset(server: &MockServer, offset: i64, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    loop {
        let seen = server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .any(|r| req_offset(r) == Some(offset));
        if seen {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn recv_within(rx: &mut mpsc::Receiver<TgUpdate>, within: Duration) -> Option<TgUpdate> {
    tokio::time::timeout(within, rx.recv()).await.ok().flatten()
}

fn mock_adapter(server: &MockServer) -> Arc<TelegramAdapter> {
    Arc::new(TelegramAdapter::for_tests(
        TOKEN,
        server.uri(),
        TelegramHttpTuning::default(),
    ))
}

#[tokio::test]
async fn offset_advances_only_after_handoff() {
    let server = MockServer::start().await;
    mount_get_updates(&server, |o| o.is_none(), updates_body(&[41, 42])).await;
    mount_get_updates(&server, |o| o == Some(43), updates_body(&[])).await;
    let (tx, mut rx) = mpsc::channel(8);
    let parent = CancellationToken::new();
    let stats = Arc::new(PollStats::default());
    let task = tokio::spawn(run_poll_supervisor(
        mock_adapter(&server),
        tx,
        parent.clone(),
        fast_tuning(),
        stats.clone(),
    ));

    let first = recv_within(&mut rx, Duration::from_secs(2))
        .await
        .expect("update 41");
    let second = recv_within(&mut rx, Duration::from_secs(2))
        .await
        .expect("update 42");
    assert_eq!((first.update_id, second.update_id), (41, 42));
    assert!(
        wait_for_offset(&server, 43, Duration::from_secs(2)).await,
        "a later request must carry offset 43"
    );
    // Exactly once: nothing further arrives.
    assert!(
        recv_within(&mut rx, Duration::from_millis(200))
            .await
            .is_none(),
        "an update was delivered twice"
    );
    parent.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(1), task).await;
}

#[tokio::test]
async fn update_redelivered_after_cancel_during_handoff() {
    let server = MockServer::start().await;
    mount_get_updates(&server, |o| o.is_none_or(|v| v < 8), updates_body(&[7])).await;
    mount_get_updates(&server, |o| o.is_some_and(|v| v >= 8), updates_body(&[])).await;
    let (tx, mut rx) = mpsc::channel(1);
    tx.send(dummy_update(1000)).await.expect("prefill");
    let parent = CancellationToken::new();
    let stats = Arc::new(PollStats::default());
    let shared = PollShared::new(
        mock_adapter(&server),
        tx,
        parent.clone(),
        fast_tuning(),
        stats.clone(),
    );

    // Generation 1 fetches update 7 and blocks handing it off.
    let cancel1 = parent.child_token();
    let gen1 = tokio::spawn(run_poll_generation(shared.clone(), cancel1.clone()));
    let deadline = Instant::now() + Duration::from_secs(2);
    while stats.phase() != 1 {
        assert!(Instant::now() < deadline, "phase never became HandingOff");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // Simulate a respawn: cancel mid-hand-off, then start the next generation.
    cancel1.cancel();
    let exit1 = tokio::time::timeout(Duration::from_secs(1), gen1)
        .await
        .expect("generation 1 must finish after the cancel")
        .expect("no panic");
    assert_eq!(exit1, GenerationExit::Cancelled);
    let cancel2 = parent.child_token();
    let gen2 = tokio::spawn(run_poll_generation(shared, cancel2.clone()));

    // Drain: the prefilled item first, then the re-fetched update 7.
    let filler = recv_within(&mut rx, Duration::from_secs(2))
        .await
        .expect("prefilled item");
    assert_eq!(filler.update_id, 1000);
    let seven = recv_within(&mut rx, Duration::from_secs(2))
        .await
        .expect("update 7 must be re-delivered, not lost");
    assert_eq!(seven.update_id, 7);
    assert!(
        wait_for_offset(&server, 8, Duration::from_secs(2)).await,
        "the request after delivery must carry offset 8"
    );
    cancel2.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(1), gen2).await;
}

#[tokio::test]
async fn backpressure_does_not_respawn_and_recovers_when_drained() {
    let (logs, _guard) = capture_logs();
    let server = MockServer::start().await;
    mount_get_updates(&server, |o| o.is_none_or(|v| v < 10), updates_body(&[9])).await;
    mount_get_updates(&server, |o| o.is_some_and(|v| v >= 10), updates_body(&[])).await;
    let (tx, mut rx) = mpsc::channel(1);
    tx.send(dummy_update(1000)).await.expect("prefill");
    let parent = CancellationToken::new();
    let stats = Arc::new(PollStats::default());
    let tuning = PollTuning {
        stale_after: Duration::from_millis(300),
        ..fast_tuning()
    };
    let task = tokio::spawn(run_poll_supervisor(
        mock_adapter(&server),
        tx,
        parent.clone(),
        tuning,
        stats.clone(),
    ));

    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(stats.generations(), 1, "back-pressure must not respawn");
    assert_eq!(stats.respawns(), 0);
    assert!(
        logs.text().contains("dispatch back-pressure"),
        "the watchdog must report back-pressure, log was:\n{}",
        logs.text()
    );

    // Drain: polling resumes end to end without a restart.
    let filler = recv_within(&mut rx, Duration::from_secs(2))
        .await
        .expect("prefilled item");
    assert_eq!(filler.update_id, 1000);
    let nine = recv_within(&mut rx, Duration::from_secs(2))
        .await
        .expect("update 9");
    assert_eq!(nine.update_id, 9);
    assert!(
        wait_for_offset(&server, 10, Duration::from_secs(1)).await,
        "polling must resume with offset 10 after the drain"
    );
    assert_eq!(stats.generations(), 1);
    parent.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(1), task).await;
}

#[tokio::test]
async fn closed_channel_stops_supervisor() {
    let (logs, _guard) = capture_logs();
    let server = MockServer::start().await;
    mount_get_updates(&server, |_| true, updates_body(&[3])).await;
    let (tx, rx) = mpsc::channel(8);
    drop(rx);
    let parent = CancellationToken::new();
    let stats = Arc::new(PollStats::default());
    let task = tokio::spawn(run_poll_supervisor(
        mock_adapter(&server),
        tx,
        parent.clone(),
        fast_tuning(),
        stats.clone(),
    ));
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("a closed dispatch channel must stop the supervisor")
        .expect("no panic");
    assert_eq!(stats.generations(), 1, "no respawn after a closed channel");
    assert!(
        logs.text().contains("dispatch channel closed"),
        "log was:\n{}",
        logs.text()
    );
    assert!(
        !parent.is_cancelled(),
        "a closed channel must not cancel the parent"
    );
}

#[tokio::test]
async fn conflict_409_cancels_parent_and_stops_supervisor() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/bot{TOKEN}/getUpdates")))
        .respond_with(ResponseTemplate::new(409).set_body_json(serde_json::json!({
            "ok": false,
            "error_code": 409,
            "description": "Conflict: terminated by other getUpdates request"
        })))
        .mount(&server)
        .await;
    let (tx, _rx) = mpsc::channel(8);
    let parent = CancellationToken::new();
    let stats = Arc::new(PollStats::default());
    let task = tokio::spawn(run_poll_supervisor(
        mock_adapter(&server),
        tx,
        parent.clone(),
        fast_tuning(),
        stats.clone(),
    ));
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .expect("a fatal 409 must stop the supervisor")
        .expect("no panic");
    assert!(
        parent.is_cancelled(),
        "a fatal 409 must cancel the parent token"
    );
    assert_eq!(stats.generations(), 1, "no respawn after a fatal conflict");
}

// ---------------------------------------------------------------------------
// Task 3: the D-06 poll trace.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn poll_trace_emitted_at_cadence() {
    let (logs, _guard) = capture_logs();
    let server = MockServer::start().await;
    mount_get_updates(&server, |_| true, updates_body(&[])).await;
    let (tx, _rx) = mpsc::channel(8);
    let parent = CancellationToken::new();
    let stats = Arc::new(PollStats::default());
    let tuning = PollTuning {
        trace_every: Duration::from_millis(150),
        ..fast_tuning()
    };
    let task = tokio::spawn(run_poll_supervisor(
        mock_adapter(&server),
        tx,
        parent.clone(),
        tuning,
        stats.clone(),
    ));
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let text = logs.text();
        if text.contains("Telegram poll ok") {
            assert!(
                text.contains("updates="),
                "trace must carry updates=: {text}"
            );
            let line = text
                .lines()
                .find(|l| l.contains("Telegram poll ok"))
                .expect("trace line");
            assert!(line.contains("INFO"), "trace must be INFO level: {line}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no `Telegram poll ok` line within 2 s, log was:\n{text}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    parent.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(1), task).await;
}

#[tokio::test]
async fn poll_trace_silent_when_no_successful_poll() {
    let (logs, _guard) = capture_logs();
    let base = silent_listener().await;
    let adapter = Arc::new(TelegramAdapter::for_tests(TOKEN, base, short_http_tuning()));
    let (tx, _rx) = mpsc::channel(8);
    let parent = CancellationToken::new();
    let stats = Arc::new(PollStats::default());
    let tuning = PollTuning {
        trace_every: Duration::from_millis(150),
        ..fast_tuning()
    };
    let task = tokio::spawn(run_poll_supervisor(
        adapter,
        tx,
        parent.clone(),
        tuning,
        stats.clone(),
    ));
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        !logs.text().contains("Telegram poll ok"),
        "a heartbeat without a successful poll is a lie, log was:\n{}",
        logs.text()
    );
    parent.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(1), task).await;
}

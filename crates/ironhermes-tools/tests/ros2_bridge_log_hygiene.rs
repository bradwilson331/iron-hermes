//! Log-hygiene regression tests for `Ros2BridgeClient`'s reader task (Phase
//! 49.8 Plan 20, CR-03): a malformed inbound rosbridge frame must never reach
//! the log verbatim, and malformed-frame logging must be rate-limited per
//! connection.
//!
//! `ros2_malformed_frame_is_logged_without_payload` and
//! `ros2_malformed_frame_log_is_rate_limited` are this plan's Task 2
//! RED/GREEN target tests. Both install a documented
//! `tracing_subscriber::fmt()` global subscriber (once per process, via
//! [`init_capture`]) whose `MakeWriter` appends into a shared buffer — no
//! doc-hidden tracing-test API is used. Lines are attributed by
//! `connection_id=<id>` (connection ids are process-unique), so the two
//! tests in this binary cannot confuse each other's lines even though they
//! share one global subscriber and one capture buffer.

#[path = "support/mock_rosbridge.rs"]
mod mock_rosbridge;

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use ironhermes_tools::ros2::bridge_client::{MALFORMED_FRAME_LOG_LIMIT, Ros2BridgeClient};
use mock_rosbridge::{MockConfig, MockRosbridge};
use serde_json::json;
use tracing_subscriber::fmt::MakeWriter;

/// A `MakeWriter` that appends every write into a shared, process-global
/// buffer.
#[derive(Clone)]
struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("capture buffer mutex poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for CaptureWriter {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

static CAPTURE: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();

/// Installs the global `tracing_subscriber::fmt()` subscriber once per
/// process — every event from any worker thread (including the client's
/// spawned reader task) is appended to the shared buffer. Both tests in this
/// binary call this; only the first call actually installs the global
/// default (`tracing::subscriber::set_global_default` can only succeed
/// once), which is why the buffer — not the subscriber — is the `OnceLock`'s
/// payload.
fn init_capture() -> Arc<Mutex<Vec<u8>>> {
    CAPTURE
        .get_or_init(|| {
            let buf = Arc::new(Mutex::new(Vec::new()));
            let writer = CaptureWriter(buf.clone());
            let subscriber = tracing_subscriber::fmt()
                .with_ansi(false)
                .with_max_level(tracing::Level::TRACE)
                .with_writer(writer)
                .finish();
            let _ = tracing::subscriber::set_global_default(subscriber);
            buf
        })
        .clone()
}

fn captured_logs(buf: &Arc<Mutex<Vec<u8>>>) -> String {
    String::from_utf8_lossy(&buf.lock().expect("capture buffer mutex poisoned")).into_owned()
}

// ---------------------------------------------------------------------------
// ros2_malformed_frame_is_logged_without_payload (CR-03, Task 2 RED/GREEN target)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_malformed_frame_is_logged_without_payload() {
    let capture = init_capture();

    let malformed =
        "not json at all\nIRONHERMES-CR03-SECRET-7f3a\n2026-01-01T00:00:00Z INFO forged-record";
    let mock = MockRosbridge::start_with(MockConfig {
        raw_topic_frames: [(
            "/leak".to_string(),
            vec![
                malformed.to_string(),
                json!({"op": "publish", "topic": "/leak", "msg": {"data": 1}}).to_string(),
            ],
        )]
        .into_iter()
        .collect(),
        ..Default::default()
    })
    .await;

    let client = Ros2BridgeClient::connect(&mock.url).await.expect("connect");
    let mut sub = client
        .subscribe("/leak", None, 0, 1)
        .await
        .expect("subscribe");

    let msg = tokio::time::timeout(Duration::from_secs(2), sub.rx.recv())
        .await
        .expect(
            "must receive the valid publish frame before the deadline — proves the malformed \
             frame ahead of it was already processed",
        )
        .expect("channel must not be closed");
    assert_eq!(msg, json!({"data": 1}));

    let logs = captured_logs(&capture);
    let connection_id = client.connection_id();
    let connection_prefix = format!("connection_id={connection_id}");
    assert!(
        logs.contains("inbound frame failed to parse") && logs.contains(&connection_prefix),
        "expected a parse-failure line naming connection_id={connection_id}; got logs: {logs}"
    );
    assert!(
        logs.contains("frame_bytes="),
        "expected a frame_bytes field in the parse-failure line; got: {logs}"
    );

    assert!(
        !logs.contains("IRONHERMES-CR03-SECRET-7f3a"),
        "the malformed frame's secret must never reach the log; got: {logs}"
    );
    assert!(
        !logs.contains("forged-record"),
        "the malformed frame's embedded forged record must never reach the log; got: {logs}"
    );
}

// ---------------------------------------------------------------------------
// ros2_malformed_frame_log_is_rate_limited (CR-03, Task 2 RED/GREEN target)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_malformed_frame_log_is_rate_limited() {
    let capture = init_capture();

    let mut frames: Vec<String> = (0..20)
        .map(|i| format!("not json FLOOD-SECRET-{i}"))
        .collect();
    frames.push(json!({"op": "publish", "topic": "/flood", "msg": {"data": 1}}).to_string());

    let mock = MockRosbridge::start_with(MockConfig {
        raw_topic_frames: [("/flood".to_string(), frames)].into_iter().collect(),
        ..Default::default()
    })
    .await;

    let client = Ros2BridgeClient::connect(&mock.url).await.expect("connect");
    let connection_id = client.connection_id();
    let mut sub = client
        .subscribe("/flood", None, 0, 1)
        .await
        .expect("subscribe");

    let msg = tokio::time::timeout(Duration::from_secs(2), sub.rx.recv())
        .await
        .expect("must receive the valid publish frame before the deadline")
        .expect("channel must not be closed");
    assert_eq!(msg, json!({"data": 1}));

    let connection_prefix = format!("connection_id={connection_id}");
    let parse_failure_lines = captured_logs(&capture)
        .lines()
        .filter(|line| {
            line.contains("inbound frame failed to parse") && line.contains(&connection_prefix)
        })
        .count();
    assert_eq!(
        parse_failure_lines as u64, MALFORMED_FRAME_LOG_LIMIT,
        "expected exactly MALFORMED_FRAME_LOG_LIMIT parse-failure lines for connection_id={connection_id}; got {parse_failure_lines}"
    );

    drop(mock);
    let start = std::time::Instant::now();
    let closed = loop {
        if client.is_closed() {
            break true;
        }
        if start.elapsed() > Duration::from_secs(3) {
            break false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(
        closed,
        "client must report closed after the mock drops the connection"
    );

    let logs = captured_logs(&capture);
    assert!(
        logs.contains(&connection_prefix)
            && logs.contains("suppressed")
            && logs.contains("malformed_frames=20"),
        "expected a suppression summary naming connection_id={connection_id}, suppressed, and \
         malformed_frames=20; got logs: {logs}"
    );
    assert!(
        !logs.contains("FLOOD-SECRET-"),
        "no flood secret must ever reach the log; got: {logs}"
    );
}

#![allow(dead_code)]
//! Shared NDJSON test harness (Phase 47.7, plan 01 task 1).
//!
//! Drives the real `ironhermes_acp::entry::run_acp_over` server over a **byte** transport
//! (`agent_client_protocol::ByteStreams` over `tokio::io::duplex`) rather than the typed
//! `agent_client_protocol::Channel::duplex()` harness `tests/acp_e2e.rs` uses — this exercises
//! real NDJSON line framing exactly as a spawned subprocess (buzz-acp's `AcpClient`) would, not
//! the SDK's own in-process frame-passing shortcut. `build_state_store` is the same isolated,
//! tempdir-backed store `tests/acp_e2e.rs` uses; `build_config_and_resolver` additionally pins
//! the provider endpoint and credential (see its own doc) so no test on this harness can reach
//! a real provider or read the operator's real config.

use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use agent_client_protocol::ByteStreams;
use ironhermes_core::{Config, ModelsCache, ProviderConfig, ProviderResolver};
use ironhermes_state::StateStore;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// Unroutable base_url for the main provider in every isolated test config. Port 1 on
/// loopback refuses instantly, so a turn that reaches the HTTP layer fails fast and locally
/// instead of leaving the machine. `http://` is only accepted by
/// `ProviderResolver`'s URL safety check for `localhost`/`127.0.0.1`, which is exactly the
/// case here.
pub const UNROUTABLE_BASE_URL: &str = "http://127.0.0.1:1/v1";

/// Literal placeholder credential. Present so no key-resolution site ever falls through to
/// the process environment.
pub const DUMMY_API_KEY: &str = "test-key-not-a-real-credential";

/// Build a `Config`/`ProviderResolver` pair that cannot reach a real provider and cannot
/// pick up a real credential.
///
/// `Config::default()`'s main provider is `openrouter` with its real production `base_url`,
/// and `ProviderResolver::build` resolves API keys from the process environment
/// (`OPENROUTER_API_KEY`/`ANTHROPIC_API_KEY`/`OPENAI_API_KEY`) while reading the operator's
/// real `$IRONHERMES_HOME/models-cache.json`. Left alone, a test that reaches the HTTP layer
/// would therefore send its prompt and a real key to a third party. Two things prevent that
/// here:
///
/// 1. The main provider is pinned to [`UNROUTABLE_BASE_URL`] with an explicit
///    [`DUMMY_API_KEY`], so no ambient env var is consulted for it and no request can leave
///    the machine.
/// 2. `build_with_cache` (never `build`) pins the static model table, so resolution does not
///    depend on whatever is on the operator's disk.
///
/// Tests that need a turn to actually SUCCEED point at their own `wiremock` server instead
/// (see each file's `build_config_and_resolver_pointed_at`).
pub fn build_config_and_resolver() -> (Arc<Config>, Arc<ProviderResolver>) {
    let mut config = Config::default();
    config.providers.insert(
        "openrouter".to_string(),
        ProviderConfig {
            base_url: Some(UNROUTABLE_BASE_URL.to_string()),
            api_key: Some(DUMMY_API_KEY.to_string()),
            ..Default::default()
        },
    );
    let resolver = ProviderResolver::build_with_cache(&config, ModelsCache::default())
        .expect("ProviderResolver::build_with_cache with isolated Config");
    (Arc::new(config), Arc::new(resolver))
}

/// An isolated, tempdir-backed `StateStore` so these tests never touch the operator's real
/// `$IRONHERMES_HOME/state.db`. The returned `TempDir` must be kept alive for the duration of
/// the test (dropping it deletes the directory). Copied verbatim from `tests/acp_e2e.rs`.
pub fn build_state_store() -> (Arc<StdMutex<StateStore>>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir for state.db");
    let store = StateStore::new(tmp.path().join("state.db")).expect("StateStore::new");
    (Arc::new(StdMutex::new(store)), tmp)
}

/// In-memory duplex buffer size for the byte transport. Generous enough that a single
/// `session/update` burst never backpressures mid-write during these tests.
const DUPLEX_BUF_SIZE: usize = 256 * 1024;

/// Bound on a single line read — a dialect mismatch must surface as a test failure with the
/// frames collected so far, never as a hung CI job.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Spawn the real ACP server over a byte-oriented NDJSON transport and return a handle to the
/// server task plus an `NdjsonClient` for the test to drive it — mirroring exactly what a
/// spawned `ironhermes acp` subprocess talking over real stdin/stdout would exercise.
///
/// `outgoing` is the stream the AGENT writes to (its stdout) and `incoming` is the stream the
/// agent reads from (its stdin) — getting these backwards produces a silent hang, not a
/// compile error, so this is the one place in the harness that must be gotten right.
pub fn spawn_agent_over_ndjson(
    config: Arc<Config>,
    resolver: Arc<ProviderResolver>,
    state_store: Arc<StdMutex<StateStore>>,
) -> (tokio::task::JoinHandle<anyhow::Result<()>>, NdjsonClient) {
    let (agent_side, client_side) = tokio::io::duplex(DUPLEX_BUF_SIZE);

    let (agent_read, agent_write) = tokio::io::split(agent_side);
    // The SDK's `ByteStreams::new` wants `futures`-io traits, not tokio's — `compat()` /
    // `compat_write()` bridge tokio's `AsyncRead`/`AsyncWrite` into the `futures` equivalents.
    let outgoing = agent_write.compat_write();
    let incoming = agent_read.compat();
    let byte_streams = ByteStreams::new(outgoing, incoming);

    let agent_task = tokio::spawn(async move {
        ironhermes_acp::entry::run_acp_over(config, resolver, state_store, byte_streams).await
    });

    let (client_read, client_write) = tokio::io::split(client_side);
    let client = NdjsonClient {
        reader: BufReader::new(client_read),
        writer: client_write,
        next_id: 1,
        received_raw: Vec::new(),
    };

    (agent_task, client)
}

/// A raw NDJSON client driving the ACP server over the byte transport, mirroring buzz-acp's own
/// hand-rolled framing (`write_ndjson`/`send_request`/`send_notification` in
/// `~/code/buzz/crates/buzz-acp/src/acp.rs`, read-only reference, D-05). Holds a monotonic
/// request-id counter (mirroring buzz-acp's own `next_id` counter) and records every raw line
/// it has read, in order, so a test can assert on bytes (Test 4), not on already-parsed values.
pub struct NdjsonClient {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
    next_id: u64,
    received_raw: Vec<String>,
}

impl NdjsonClient {
    /// Write `line` plus a single `\n`, then flush — mirrors buzz-acp's `write_ndjson` exactly.
    pub async fn send_line(&mut self, line: &str) {
        self.writer
            .write_all(line.as_bytes())
            .await
            .expect("write NDJSON line");
        self.writer.write_all(b"\n").await.expect("write trailing newline");
        self.writer.flush().await.expect("flush NDJSON line");
    }

    /// Allocate the next request id, send `method`/`params` as a JSON-RPC request, then read
    /// frames until one carries that id — returning the whole frame so a test can assert on
    /// `result` or `error.code` directly.
    pub async fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let id = self.next_id;
        self.next_id += 1;
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.send_line(&msg.to_string()).await;

        loop {
            let Some(frame) = self.next_frame().await else {
                panic!(
                    "connection closed before a response to `{method}` (id={id}) arrived; \
                     frames collected so far: {:?}",
                    self.received_raw
                );
            };
            if frame.get("id") == Some(&serde_json::json!(id)) {
                return frame;
            }
            // A notification (or a response to an earlier, already-abandoned request) arrived
            // first — keep reading until the id we're waiting on shows up.
        }
    }

    /// Send a JSON-RPC notification (no `id`, no response read).
    pub async fn notify(&mut self, method: &str, params: serde_json::Value) {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.send_line(&msg.to_string()).await;
    }

    /// Read one line and parse it as JSON, bounded by `READ_TIMEOUT`. Returns `None` on clean
    /// EOF. Blank lines are skipped transparently (never surfaced, never recorded).
    pub async fn next_frame(&mut self) -> Option<serde_json::Value> {
        let line = self.read_line_raw().await?;
        Some(serde_json::from_str(&line).unwrap_or_else(|e| {
            panic!("line failed to parse as JSON: {line:?}: {e}")
        }))
    }

    /// Collect whatever frames arrive within `duration` (best-effort; used to drain
    /// notifications after an exchange that may produce more than one frame). A read error or
    /// clean EOF ends collection early rather than panicking — this is a best-effort drain, not
    /// an assertion.
    pub async fn drain_frames(&mut self, duration: Duration) -> Vec<serde_json::Value> {
        let mut frames = Vec::new();
        let deadline = tokio::time::Instant::now() + duration;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let mut line = String::new();
            match tokio::time::timeout(remaining, self.reader.read_line(&mut line)).await {
                Ok(Ok(0)) | Err(_) => break, // clean EOF or the bounded wait elapsed
                Ok(Ok(_)) => {
                    let trimmed = line.trim_end_matches('\n').to_string();
                    if trimmed.is_empty() {
                        continue;
                    }
                    self.received_raw.push(trimmed.clone());
                    if let Ok(value) = serde_json::from_str(&trimmed) {
                        frames.push(value);
                    }
                }
                Ok(Err(_)) => break,
            }
        }
        frames
    }

    /// Every raw line this client has read so far, in order (no trailing `\n`, blank lines
    /// never included) — lets a test assert byte-level framing invariants (Test 4) instead of
    /// only already-parsed values.
    pub fn raw_lines(&self) -> &[String] {
        &self.received_raw
    }

    async fn read_line_raw(&mut self) -> Option<String> {
        loop {
            let mut line = String::new();
            let read = tokio::time::timeout(READ_TIMEOUT, self.reader.read_line(&mut line))
                .await
                .unwrap_or_else(|_| {
                    panic!(
                        "timed out after {READ_TIMEOUT:?} waiting for a response line \
                         (possible protocol desync); frames collected so far: {:?}",
                        self.received_raw
                    )
                })
                .expect("read_line should not error on an in-memory duplex stream");
            if read == 0 {
                return None; // clean EOF
            }
            let trimmed = line.trim_end_matches('\n').to_string();
            if trimmed.is_empty() {
                continue; // NDJSON never emits blank lines; skip defensively rather than assert
            }
            self.received_raw.push(trimmed.clone());
            return Some(trimmed);
        }
    }
}

//! D-08 drift guard: replay the real buzz-acp wire frames captured during the live UAT
//! against the actual ACP server, asserting response SHAPE, never model-generated text.
//!
//! # What was recorded, and against what
//!
//! The fixtures in `tests/fixtures/*.ndjson` are scrubbed captures of a real Buzz
//! session, recorded during the phase 47.7 live UAT on **2026-08-11**:
//!
//! - **Buzz side:** local `buzz` checkout at commit **`7f7db63db`** (committed
//!   2026-08-06), run via `cargo run -p buzz-acp`. The `buzz` CLI exposes no
//!   `--version` flag, so the checkout commit is the version identifier. This matches
//!   the tested-against line in `docs/ACP.md` and the environment table in
//!   `.planning/phases/47.7-buzz-using-ih-acp/47.7-UAT-EVIDENCE.md` — keep all three
//!   in sync when refreshing.
//! - **Agent side:** `ironhermes` 0.3.5 (the captured `initialize` responses carry
//!   that `agentInfo`).
//!
//! Coverage is deliberately narrow: the UAT was halted by the operator after check 1,
//! so the captures cover the `initialize` handshake, one streamed-reply turn (which
//! also contains an in-session follow-up turn), and a terminal tool-call turn.
//! Cancellation, long-running-tool keepalive, policy denial and the Zed re-check
//! produced **no** captures — no fixture fabricates them (see 47.7-UAT-EVIDENCE.md).
//!
//! # What this test asserts — and does not
//!
//! For every client-originated request in a fixture, the replayed server must answer
//! with the matching id, exactly one of `result`/`error`, no protocol-layer rejection
//! (`-32700`/`-32600`/`-32601`), the same result-vs-error disposition the recorded
//! exchange had, and — when both are errors — the same error code. Model-generated
//! text, streamed `session/update` content, usage numbers and timing are **never**
//! asserted: both sides of those vary run to run, and a guard that asserts on them
//! becomes a flake and gets deleted. CI has no provider configured, so a
//! `session/prompt` recorded as a live success is allowed to replay as a
//! non-protocol-layer error (the one documented allowance — see
//! [`assert_response_shape`]).
//!
//! Session ids are rewritten during replay: the server mints `acp_{uuid}` per
//! process, so recorded ids can never match a fresh server's. The substitution is
//! explicit (only `params.sessionId`, only through the map built from this replay's
//! own `session/new` responses) so a genuine id-handling regression still fails —
//! see [`rewrite_session_id`].
//!
//! # Failure triage — read this before touching an assertion
//!
//! A failure here means one of two things: **IronHermes regressed**, or **Buzz's
//! dialect changed**. The first diagnostic step is to diff the current `buzz-acp`
//! source against the recorded commit `7f7db63db` — NOT to relax the assertion. These
//! frames are what buzz-acp actually sent; when this suite and the hand-written
//! conformance suite (`buzz_dialect.rs`) disagree, the fixtures were right.
//!
//! # Refreshing the fixtures after a Buzz upgrade
//!
//! Re-capture, never hand-edit. The procedure is plan 06's: run a live UAT with
//! buzz-acp spawning the agent through the capture wrapper
//! `scripts/acp-frame-capture.sh` (raw captures land untracked in
//! `~/.ironhermes/acp-captures/`), then scrub and land the new `.ndjson` files here.
//! A fixture edited by hand to make a red test green defeats the guard's entire
//! purpose (T-47.7-26).
//!
//! **Scrubbing contract (T-47.7-20/T-47.7-25):** these files derive from a real
//! session and were scrubbed structure-preserving (stable placeholder pubkeys/npubs/
//! UUIDs, `/home/operator` paths, placeholder system prompt). Anyone adding a fixture
//! must run the same secret scan plan 06 defined before committing: zero `nsec1`
//! strings, zero real 64-hex values, zero `/Users/` paths, every line parses as JSON,
//! zero sensitive key names. Then update the recorded commit/date here, in
//! `docs/ACP.md`, and in the UAT evidence for the new run.
//!
//! # Extending coverage
//!
//! Fixtures are discovered by directory read — dropping a new scrubbed capture into
//! `tests/fixtures/` extends this suite with no code edit. The harness fails loudly
//! on an empty directory or a request-free fixture (T-47.7-28), on any frame it
//! cannot classify, and on capture traffic it has never seen (an unknown notification
//! method, or an agent-originated request, which needs a client-side responder first).

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use common::{build_config_and_resolver, build_state_store, spawn_agent_over_ndjson};

/// The fixtures directory, resolved from the crate root so `cargo test` works from any cwd.
/// No fixture filename is ever hardcoded — discovery is a directory read (Test 1).
fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Discover every `.ndjson` fixture in `dir`, sorted for deterministic replay order.
///
/// FAILS LOUDLY on an empty (or missing) directory: a replay guard that quietly passes
/// over zero inputs is worse than no guard, because it reads as coverage (T-47.7-28).
fn discover_fixtures(dir: &Path) -> Vec<PathBuf> {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| {
        panic!(
            "fixtures directory {} is missing or unreadable ({e}); the D-08 drift guard \
             has nothing to replay — re-capture via scripts/acp-frame-capture.sh",
            dir.display()
        )
    });
    let mut fixtures: Vec<PathBuf> = entries
        .filter_map(|entry| {
            let path = entry.expect("read_dir entry").path();
            (path.extension().and_then(|e| e.to_str()) == Some("ndjson")).then_some(path)
        })
        .collect();
    fixtures.sort();
    assert!(
        !fixtures.is_empty(),
        "fixtures directory {} contains no .ndjson files; the D-08 drift guard has \
         nothing to replay — a green run over zero fixtures would be fake coverage",
        dir.display()
    );
    fixtures
}

/// One recorded NDJSON line, classified by direction and JSON-RPC kind.
///
/// Direction is derived from the frame's own fields plus a small, explicit method
/// direction table — plan 06's captures interleave both directions in one file in
/// logical exchange order, with no direction annotation:
///
/// - `method` + `id`  -> a client-originated REQUEST (buzz-acp -> agent). The agent
///   never sends requests to the client on the captured path (no permission round-trip
///   exists in any capture — see 47.7-UAT-EVIDENCE.md), so an agent-originated request
///   method appearing with an `id` is rejected loudly rather than misreplayed.
/// - `method`, no `id` -> a notification; direction comes from [`notification_direction`].
/// - `result`/`error` + `id`, no `method` -> a recorded AGENT response, correlated to the
///   client request carrying the same id.
enum RecordedFrame {
    /// buzz-acp -> agent request; `raw` is the full recorded frame, replayed verbatim
    /// (modulo the documented session-id substitution).
    ClientRequest {
        id: serde_json::Value,
        method: String,
        raw: serde_json::Value,
    },
    /// buzz-acp -> agent notification (none exist in the current captures; the arm is
    /// exercised the day a capture with `session/cancel` lands).
    ClientNotification { raw: serde_json::Value },
    /// agent -> client response to the request with the same `id` — the recorded shape
    /// the replayed response is compared against (Test 3).
    ServerResponse {
        id: serde_json::Value,
        raw: serde_json::Value,
    },
    /// agent -> client notification (`session/update` streams). Recorded output only:
    /// the replay produces its own updates (different model, no provider in CI), so
    /// recorded update content is never asserted on.
    ServerNotification,
}

/// Direction table for notification methods (no `id`). Explicit and fail-loud: an
/// unknown notification method means a future capture introduced traffic this harness
/// has never seen, and misclassifying it would silently corrupt the replay.
fn notification_direction(method: &str, path: &Path) -> RecordedDirection {
    match method {
        // Agent -> client: the streamed update channel (the only agent-originated
        // notification on the captured path).
        "session/update" => RecordedDirection::ServerToClient,
        // Client -> agent: buzz-acp's cancel path (not present in current captures).
        "session/cancel" => RecordedDirection::ClientToServer,
        other => panic!(
            "fixture {}: notification method `{other}` has no entry in the direction \
             table — extend notification_direction() before replaying this capture",
            path.display()
        ),
    }
}

enum RecordedDirection {
    ClientToServer,
    ServerToClient,
}

/// Methods the AGENT originates as requests (agent -> client). None appear in the
/// current captures; if one shows up, the harness must grow a client-side responder
/// before the fixture can be replayed, so classification fails loudly instead.
const AGENT_ORIGINATED_REQUEST_PREFIXES: &[&str] =
    &["session/request_permission", "fs/", "terminal/"];

/// Parse one fixture file into classified frames, failing loudly on any line that does
/// not parse or classify. Also FAILS if the file contains no client requests: replaying
/// a request-free file proves nothing and would read as coverage (T-47.7-28).
fn parse_fixture(path: &Path) -> Vec<RecordedFrame> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read fixture {}: {e}", path.display()));

    let mut frames = Vec::new();
    for (lineno, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(line).unwrap_or_else(|e| {
            panic!(
                "fixture {} line {}: not valid JSON ({e})",
                path.display(),
                lineno + 1
            )
        });

        let method = value.get("method").and_then(serde_json::Value::as_str);
        let id = value.get("id").cloned();
        let frame = match (method, id) {
            (Some(m), Some(id)) => {
                assert!(
                    !AGENT_ORIGINATED_REQUEST_PREFIXES
                        .iter()
                        .any(|p| m.starts_with(p)),
                    "fixture {} line {}: `{m}` is an agent-originated request — the replay \
                     harness has no client-side responder for it yet; extend the harness \
                     before replaying this capture",
                    path.display(),
                    lineno + 1
                );
                RecordedFrame::ClientRequest {
                    id,
                    method: m.to_string(),
                    raw: value,
                }
            }
            (Some(m), None) => match notification_direction(m, path) {
                RecordedDirection::ClientToServer => {
                    RecordedFrame::ClientNotification { raw: value }
                }
                RecordedDirection::ServerToClient => RecordedFrame::ServerNotification,
            },
            (None, Some(id)) => {
                assert!(
                    value.get("result").is_some() || value.get("error").is_some(),
                    "fixture {} line {}: frame has an id but neither method, result nor \
                     error — unclassifiable",
                    path.display(),
                    lineno + 1
                );
                RecordedFrame::ServerResponse { id, raw: value }
            }
            (None, None) => panic!(
                "fixture {} line {}: frame has neither method nor id — unclassifiable",
                path.display(),
                lineno + 1
            ),
        };
        frames.push(frame);
    }

    let request_count = frames
        .iter()
        .filter(|f| matches!(f, RecordedFrame::ClientRequest { .. }))
        .count();
    assert!(
        request_count > 0,
        "fixture {} contains no client-originated requests — replaying it proves \
         nothing; a capture must contain at least one request to be a drift guard",
        path.display()
    );

    frames
}

/// Stable map key for a JSON-RPC id value (ids in the captures are integers, but
/// JSON-RPC also allows strings — serialize either canonically).
fn id_key(id: &serde_json::Value) -> String {
    id.to_string()
}

/// Substitute recorded session ids with the live ones this replay's server minted.
///
/// This is the ONE deliberate difference between the recorded bytes and the replayed
/// bytes: session ids are minted per process (`acp_{uuid}`), so the recorded ids can
/// never match a fresh server's. The substitution is explicit and keyed — only the
/// `params.sessionId` field is rewritten, and only through the recorded->live map built
/// from this replay's own `session/new` responses — so a genuine id-handling regression
/// (server returning ids that don't round-trip) still fails instead of being fuzzed over.
fn rewrite_session_id(
    request: &mut serde_json::Value,
    session_id_map: &BTreeMap<String, String>,
    path: &Path,
    method: &str,
) {
    let Some(recorded) = request
        .get("params")
        .and_then(|p| p.get("sessionId"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
    else {
        return;
    };
    let live = session_id_map.get(&recorded).unwrap_or_else(|| {
        panic!(
            "fixture {}: `{method}` references recorded session id {recorded:?} but no \
             prior session/new in this fixture produced it — the capture is out of \
             logical order or truncated",
            path.display()
        )
    });
    request["params"]["sessionId"] = serde_json::Value::String(live.clone());
}

/// Shape-compare one replayed response against the exchange the fixture recorded.
///
/// Asserted (Test 2 + Test 3): the response correlates by id; it carries exactly one of
/// `result`/`error`; it is not a protocol-layer rejection (`-32700` parse / `-32600`
/// invalid request / `-32601` method not found) of a frame a real Buzz session sent;
/// its result-vs-error disposition matches the recorded one; and when both are errors,
/// the codes match.
///
/// Documented allowance: the recorded run had a live provider; CI does not. A
/// `session/prompt` recorded as a success is therefore allowed to replay as a
/// non-protocol-layer error (the turn failing at the provider boundary) — but every
/// other method, and the protocol layer itself, gets no such slack.
///
/// Messages name dispositions and codes, never whole frames — fixture content must not
/// be dumped wholesale into CI logs (T-47.7-25).
fn assert_response_shape(
    path: &Path,
    method: &str,
    id: &serde_json::Value,
    replayed: &serde_json::Value,
    recorded: Option<&serde_json::Value>,
) {
    let fixture = path.display();
    let replayed_result = replayed.get("result").is_some();
    let replayed_error_code = replayed
        .get("error")
        .and_then(|e| e.get("code"))
        .and_then(serde_json::Value::as_i64);

    assert!(
        replayed_result ^ replayed.get("error").is_some(),
        "fixture {fixture}: `{method}` (id {id}) response must carry exactly one of \
         result/error"
    );

    if let Some(code) = replayed_error_code {
        assert!(
            !matches!(code, -32700 | -32600 | -32601),
            "fixture {fixture}: `{method}` (id {id}) was rejected at the protocol layer \
             (code {code}) — the server no longer accepts a frame a real Buzz session \
             sent; diff buzz-acp against the recorded commit before touching this \
             assertion"
        );
    }

    let Some(recorded) = recorded else {
        // The capture recorded no response for this id (does not occur in the current
        // fixtures); Test 2 above is then the whole assertion.
        return;
    };
    let recorded_result = recorded.get("result").is_some();
    let recorded_error_code = recorded
        .get("error")
        .and_then(|e| e.get("code"))
        .and_then(serde_json::Value::as_i64);

    match (recorded_result, replayed_result) {
        (true, true) | (false, false) => {
            if let (Some(rec), Some(rep)) = (recorded_error_code, replayed_error_code) {
                assert_eq!(
                    rec, rep,
                    "fixture {fixture}: `{method}` (id {id}) replays as a different \
                     error code than the recorded exchange"
                );
            }
        }
        (true, false) => {
            // The documented CI allowance, scoped to the one method whose success
            // depended on a live provider.
            assert_eq!(
                method,
                "session/prompt",
                "fixture {fixture}: `{method}` (id {id}) was recorded as a success but \
                 replays as error code {replayed_error_code:?} — dialect drift"
            );
        }
        (false, true) => panic!(
            "fixture {fixture}: `{method}` (id {id}) was recorded as an error (code \
             {recorded_error_code:?}) but replays as a success — the server stopped \
             rejecting something it used to reject, which is drift in the other \
             direction"
        ),
    }
}

/// Replay one fixture against a fresh server instance, asserting per
/// [`assert_response_shape`], then run the connection-liveness epilogue.
async fn replay_fixture(path: &Path) {
    let frames = parse_fixture(path);

    // Correlate each recorded response to its request id up front (Test 3 material).
    let mut recorded_responses: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for frame in &frames {
        if let RecordedFrame::ServerResponse { id, raw } = frame {
            recorded_responses.insert(id_key(id), raw.clone());
        }
    }

    let (config, resolver) = build_config_and_resolver();
    let (state_store, _state_tmp) = build_state_store();
    let (agent_task, mut client) = spawn_agent_over_ndjson(config, resolver, state_store);

    // recorded sessionId -> live sessionId, built from this replay's own session/new
    // responses (see `rewrite_session_id`).
    let mut session_id_map: BTreeMap<String, String> = BTreeMap::new();

    for frame in &frames {
        match frame {
            RecordedFrame::ClientRequest { id, method, raw } => {
                let mut request = raw.clone();
                rewrite_session_id(&mut request, &session_id_map, path, method);
                client.send_line(&request.to_string()).await;

                // Read (timeout-bounded — the harness's READ_TIMEOUT turns a desync
                // into a failure, never a hung CI job) until the response with this
                // exact recorded id arrives. Interleaved live notifications (the
                // replay's own session/update stream) are model output and are
                // deliberately not asserted on.
                let replayed = loop {
                    let Some(frame) = client.next_frame().await else {
                        panic!(
                            "fixture {}: connection closed before a response to \
                             `{method}` (id {id}) arrived",
                            path.display()
                        );
                    };
                    if frame.get("id") == Some(id) && frame.get("method").is_none() {
                        break frame;
                    }
                };

                assert_response_shape(
                    path,
                    method,
                    id,
                    &replayed,
                    recorded_responses.get(&id_key(id)),
                );

                // Learn the recorded->live session-id mapping from session/new.
                if method == "session/new" {
                    let live = replayed
                        .get("result")
                        .and_then(|r| r.get("sessionId"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_else(|| {
                            panic!(
                                "fixture {}: replayed session/new (id {id}) succeeded \
                                 without a sessionId in its result",
                                path.display()
                            )
                        })
                        .to_string();
                    let recorded = recorded_responses
                        .get(&id_key(id))
                        .and_then(|r| r.get("result"))
                        .and_then(|r| r.get("sessionId"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_else(|| {
                            panic!(
                                "fixture {}: capture has no recorded sessionId for \
                                 session/new (id {id}) — cannot build the substitution \
                                 map for later frames",
                                path.display()
                            )
                        })
                        .to_string();
                    session_id_map.insert(recorded, live);
                }
            }
            RecordedFrame::ClientNotification { raw } => {
                // Notifications draw no response (Test 4); desync-freedom is proven by
                // the next request still correlating by id, and by the liveness
                // epilogue below.
                let mut notification = raw.clone();
                let method = notification
                    .get("method")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("<notification>")
                    .to_string();
                rewrite_session_id(&mut notification, &session_id_map, path, &method);
                client.send_line(&notification.to_string()).await;
            }
            // Recorded agent output — the replay produces its own; nothing to send.
            RecordedFrame::ServerResponse { .. } | RecordedFrame::ServerNotification => {}
        }
    }

    // Connection-liveness epilogue (Test 4 + Test 5). After the whole recorded exchange
    // — including any notifications — a harness-authored probe request (NOT part of the
    // recorded exchange; id chosen far outside the captures' id range) must still get
    // its own id back, and must get D-05's documented benign `-32601`. This proves the
    // replay left the connection serving (no desync) and that the catch-all fallback is
    // intact; the current captures contain no unrecognized-method traffic, so without
    // this epilogue the guard would be blind to catch-all drift.
    const LIVENESS_ID: i64 = 999_999;
    client
        .send_line(
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": LIVENESS_ID,
                "method": "ping",
                "params": {}
            })
            .to_string(),
        )
        .await;
    let probe_reply = loop {
        let Some(frame) = client.next_frame().await else {
            panic!(
                "fixture {}: connection closed before the liveness probe (id \
                 {LIVENESS_ID}) was answered — the replay desynced the connection",
                path.display()
            );
        };
        if frame.get("id") == Some(&serde_json::json!(LIVENESS_ID))
            && frame.get("method").is_none()
        {
            break frame;
        }
    };
    let probe_code = probe_reply
        .get("error")
        .and_then(|e| e.get("code"))
        .and_then(serde_json::Value::as_i64);
    assert_eq!(
        probe_code,
        Some(-32601),
        "fixture {}: post-replay liveness probe must draw D-05's benign Method-not-found \
         reply, got disposition {:?}",
        path.display(),
        if probe_reply.get("result").is_some() {
            "result".to_string()
        } else {
            format!("error code {probe_code:?}")
        }
    );

    drop(client);
    agent_task.abort();
}

/// Tests 1–5: discover every captured fixture and replay each against a fresh server.
/// Discovery is a directory read, so a new scrubbed capture dropped into
/// `tests/fixtures/` extends this test with no code edit.
#[tokio::test]
async fn all_captured_buzz_fixtures_replay_with_matching_shapes() {
    let fixtures = discover_fixtures(&fixtures_dir());
    let count = fixtures.len();
    for path in &fixtures {
        replay_fixture(path).await;
    }
    // Belt-and-braces restatement of the discovery invariant, in this test's own output.
    assert!(count >= 1, "expected at least one fixture, replayed {count}");
    println!("replayed {count} captured buzz-acp fixtures");
}

/// T-47.7-28: an empty fixtures directory must fail loudly, naming the directory —
/// never pass as vacuous coverage.
#[test]
fn discovery_fails_loudly_on_empty_directory() {
    let empty = tempfile::tempdir().expect("tempdir");
    let result = std::panic::catch_unwind(|| discover_fixtures(empty.path()));
    let panic_message = match result {
        Ok(_) => panic!("discover_fixtures over an empty directory must fail"),
        Err(payload) => payload
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default(),
    };
    assert!(
        panic_message.contains("no .ndjson files"),
        "empty-directory failure must say the directory is empty, got: {panic_message}"
    );
    assert!(
        panic_message.contains(&empty.path().display().to_string()),
        "empty-directory failure must name the directory, got: {panic_message}"
    );
}

/// T-47.7-28's second face: a fixture with zero client requests must fail loudly,
/// naming the file — replaying only server-originated lines proves nothing.
#[test]
fn parse_fails_loudly_on_fixture_with_no_client_requests() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("server-lines-only.ndjson");
    std::fs::write(
        &path,
        concat!(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"acp_x","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hi"}}}}"#,
            "\n"
        ),
    )
    .expect("write fixture");
    let result = std::panic::catch_unwind(|| parse_fixture(&path));
    let panic_message = match result {
        Ok(_) => panic!("parse_fixture over a request-free file must fail"),
        Err(payload) => payload
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default(),
    };
    assert!(
        panic_message.contains("no client-originated requests"),
        "request-free-fixture failure must say why the file is unusable, got: {panic_message}"
    );
    assert!(
        panic_message.contains(&path.display().to_string()),
        "request-free-fixture failure must name the file, got: {panic_message}"
    );
}

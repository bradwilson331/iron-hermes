use std::fs;
use std::path::PathBuf;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    fs::read_to_string(crate_root().join(path)).expect("failed to read source file")
}

#[test]
fn server_ws_runs_turn_in_spawned_task_and_streams_concurrently() {
    let ws = read("src/server/ws.rs");
    assert!(
        ws.contains("#[cfg(feature = \"server\")]\nuse tokio::sync::mpsc;")
            && ws.contains("#[cfg(feature = \"server\")]\nuse tokio::task::JoinHandle;")
            && ws.contains("#[cfg(feature = \"server\")]\nuse tracing::warn;"),
        "server-only websocket runtime imports must remain cfg-gated"
    );
    assert!(
        ws.contains("#[cfg(feature = \"server\")]\n    let app_state =")
            && ws.contains("#[cfg(feature = \"server\")]\n                {")
            && ws.contains("#[cfg(not(feature = \"server\"))]"),
        "ws_chat must keep explicit feature-boundary branches"
    );
    assert!(
        ws.contains("tokio::spawn"),
        "ws_chat must spawn the turn execution task"
    );
    assert!(
        ws.contains("maybe_event = async") && ws.contains("turn.rx.recv().await"),
        "ws_chat must forward events while the turn is in flight"
    );
}

#[test]
fn malformed_request_path_is_recoverable_and_send_failures_abort_turn() {
    let ws = read("src/server/ws.rs");
    assert!(
        ws.contains("#[get(\"/api/ws/chat\")]") ,
        "ws route annotation must remain /api/ws/chat"
    );
    assert!(
        ws.contains("Invalid request:"),
        "ws_chat must emit protocol errors for malformed JSON"
    );
    // WR-03: anchor `continue;` to the malformed-request branch (within 500 chars
    // after the first `Invalid request:` marker) instead of a global file match.
    // The window is 500 bytes to accommodate the send_raw call between the error
    // format and the continue; statement.
    let inv_pos = ws
        .find("Invalid request:")
        .expect("ws_chat must emit `Invalid request:` for malformed JSON");
    let window_end = (inv_pos + 500).min(ws.len());
    assert!(
        ws[inv_pos..window_end].contains("continue;"),
        "ws_chat malformed request branch must `continue;` within 500 chars after the `Invalid request:` error send"
    );

    // WR-03: anchor abort assertion to the verbatim call site + adjacent log
    // message rather than any `abort()` substring elsewhere in the file.
    assert!(
        ws.contains("turn.handle.abort();"),
        "ws_chat must call `turn.handle.abort();` on socket send failure"
    );
    assert!(
        ws.contains("aborting in-flight turn"),
        "ws_chat must log `aborting in-flight turn` near the abort call site"
    );
}

#[test]
fn server_ws_disconnect_teardown_distinguishes_clean_recv_from_broken_send() {
    let ws = read("src/server/ws.rs");

    assert!(
        ws.contains("websocket recv closed; exiting connection")
            || ws.contains("websocket recv closed cleanly; exiting connection"),
        "clean websocket recv closure should log a clean-exit warning"
    );

    assert!(
        ws.contains("websocket recv failed; closing connection")
            || ws.contains("websocket recv failed; aborting connection"),
        "recv error path should remain explicitly classified"
    );

    assert!(
        ws.contains("websocket send failed; aborting in-flight turn"),
        "send failure path must stay classified as transport-broken and abort in-flight turn"
    );

    assert!(
        ws.contains("session_id = %")
            && ws.contains("reason = %")
            && ws.contains("in_flight ="),
        "disconnect telemetry must include session_id, reason, and in_flight fields"
    );
}

#[test]
fn client_ws_receiver_retries_after_disconnect_and_resets_transient_state() {
    let ui = read("src/components/warp_hermes.rs");
    assert!(
        ui.contains("crate::protocol::ChatRequest")
            && ui.contains("crate::protocol::ChatStreamEvent"),
        "client websocket protocol types must come from crate::protocol"
    );
    assert!(
        !ui.contains("crate::server::ws::ChatRequest")
            && !ui.contains("crate::server::ws::ChatStreamEvent"),
        "client websocket code must not depend on server::ws protocol paths"
    );
    assert!(
        ui.contains("with_automatic_reconnect()"),
        "client websocket initialization must keep automatic reconnect enabled"
    );
    assert!(
        ui.contains("loop {") && ui.contains("let state = ws.connect().await"),
        "client receiver must use an outer reconnect cycle"
    );
    assert!(
        ui.contains("Err(err) => {")
            && ui.contains("scanner_active.set(false);")
            && ui.contains("streaming_block_id.set(None);")
            && ui.contains("continue;"),
        "disconnect/error path must reset transient streaming UI state"
    );
    assert!(
        ui.contains("let _ = ws.send_raw("),
        "submit/rerun websocket sends must remain non-panicking"
    );
}

#[test]
fn client_ws_disconnect_notices_are_generic_and_deduplicated_per_disconnect_window() {
    let ui = read("src/components/warp_hermes.rs");

    assert!(
        ui.contains("fn push_disconnect_notice")
            || ui.contains("push_disconnect_notice("),
        "disconnect notice emission should be funneled through a single helper"
    );

    assert!(
        ui.contains("Connection interrupted. Please retry your message once reconnected."),
        "disconnect transcript copy must remain generic and user-facing"
    );

    assert!(
        ui.contains("dioxus_fullstack::Message::Close { .. }")
            && ui.contains("WebSocket closed; reconnecting")
            && ui.contains("if !disconnect_notified {")
            && ui.contains("break;"),
        "close frames should trigger one reconnect boundary, not repeated receive-error churn"
    );
}

#[test]
fn server_ws_emits_close_frame_on_every_teardown_branch() {
    // HUMAN-UAT Gap 3 regression lock: the server must send a WebSocket
    // close frame before dropping the socket on every teardown branch so
    // proxies/clients never observe `Connection reset without closing
    // handshake`. The close-frame send is best-effort (errors ignored) so
    // it does not block teardown on broken-send paths (D-06 intent).
    let ws = read("src/server/ws.rs");

    assert!(
        ws.contains("fn send_close_frame("),
        "server teardown must funnel close-frame emission through a single helper"
    );

    assert!(
        ws.contains("CloseCode") && ws.contains("Message"),
        "close-frame helper must reference CloseCode and Message types from dioxus_fullstack"
    );

    assert!(
        ws.contains("Message::Close {"),
        "send_close_frame must emit a WebSocket Close variant"
    );

    assert!(
        ws.contains("CloseCode::Normal") && ws.contains("CloseCode::Away"),
        "teardown branches must classify close codes (Normal for clean, Away for failure)"
    );

    // Every break; that exits the ws_chat loop must be preceded by a
    // send_close_frame(...) call. Count invocations and breaks to keep
    // the invariant regression-locked without getting tripped by
    // unrelated formatting changes.
    let close_frame_calls = ws.matches("send_close_frame(").count();
    // Definition + 4 call sites (clean recv, broken recv, broken send,
    // keepalive ping failure).
    assert!(
        close_frame_calls >= 5,
        "expected send_close_frame to be invoked at every teardown branch \
         (clean recv close, broken recv, broken send, keepalive failure); \
         found {close_frame_calls} occurrence(s)"
    );

    assert!(
        ws.contains("\"recv closed cleanly\"")
            && ws.contains("\"recv failed\"")
            && ws.contains("\"send failed\"")
            && ws.contains("\"keepalive failed\""),
        "each teardown branch must carry a distinct close-frame reason string for telemetry parity"
    );
}

#[test]
fn server_ws_emits_application_level_keepalive_ping() {
    // HUMAN-UAT Gap 3 follow-up regression lock: the server must emit
    // application-level WebSocket Ping frames on a periodic interval
    // while otherwise idle, so `dx serve` / hyper (and any other
    // intermediate proxy) does not idle-close the connection after ~9s
    // and surface the drop to the browser as
    // `WebsocketError::ConnectionClosed` with no server-side teardown
    // trace. The keepalive interval must be well under common proxy
    // idle thresholds (10s), and Ping failure must be classified as a
    // send-path failure per D-05 (abort in-flight turn, close frame,
    // break).
    let ws = read("src/server/ws.rs");

    assert!(
        ws.contains("WS_KEEPALIVE_INTERVAL"),
        "keepalive interval must be a named constant for observability"
    );

    assert!(
        ws.contains("Duration::from_secs(5)"),
        "keepalive interval must default to 5 seconds (< 10s proxy idle threshold)"
    );

    assert!(
        ws.contains("tokio::time::interval(WS_KEEPALIVE_INTERVAL)"),
        "keepalive must drive the ping cadence via tokio::time::interval"
    );

    assert!(
        ws.contains("MissedTickBehavior::Skip"),
        "keepalive must skip missed ticks rather than bursting Pings after wake-up"
    );

    assert!(
        ws.contains("keepalive.tick().await")
            && ws.contains("_ = keepalive.tick() =>"),
        "keepalive must participate in the tokio::select! loop and consume the first immediate tick"
    );

    assert!(
        ws.contains("Message::Ping(Bytes::new())"),
        "keepalive must emit a WebSocket Ping frame (browsers auto-pong at protocol level)"
    );

    assert!(
        ws.contains("websocket keepalive ping failed; closing connection"),
        "failed keepalive Ping must classify as a transport-broken send failure"
    );
}

#[test]
fn busy_gate_opportunistically_clears_finished_turn() {
    let ws = read("src/server/ws.rs");
    let finished_pos = ws
        .find("turn.handle.is_finished()")
        .expect("ws_chat must check turn.handle.is_finished() to opportunistically clear finished turns (WR-02)");
    // Use the full `if in_flight_turn.is_some() {` form to anchor to the busy-gate
    // branch specifically (not the telemetry `let in_flight = in_flight_turn.is_some();`
    // lines that appear earlier in the file).
    let busy_pos = ws
        .find("if in_flight_turn.is_some() {")
        .expect("ws_chat must keep the `if in_flight_turn.is_some() {` busy-gate check");
    assert!(
        finished_pos < busy_pos,
        "WR-02: turn.handle.is_finished() opportunistic clear must appear BEFORE the in_flight_turn.is_some() busy-gate (file offsets: finished={finished_pos}, busy={busy_pos})"
    );
    assert!(
        ws.contains("in_flight_turn = None;"),
        "WR-02: opportunistic clear must reset in_flight_turn to None when handle is finished"
    );
}

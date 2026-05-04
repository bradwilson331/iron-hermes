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
    assert!(
        ws.contains("continue;"),
        "ws_chat malformed request branch must continue receiving frames"
    );
    assert!(
        ws.contains("abort()"),
        "ws_chat must abort in-flight turn task on socket send failure"
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
    // Definition + 3 call sites (clean recv, broken recv, broken send)
    assert!(
        close_frame_calls >= 4,
        "expected send_close_frame to be invoked at every teardown branch \
         (clean recv close, broken recv, broken send); found {close_frame_calls} occurrence(s)"
    );

    assert!(
        ws.contains("\"recv closed cleanly\"")
            && ws.contains("\"recv failed\"")
            && ws.contains("\"send failed\""),
        "each teardown branch must carry a distinct close-frame reason string for telemetry parity"
    );
}

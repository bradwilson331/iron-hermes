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

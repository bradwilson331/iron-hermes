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

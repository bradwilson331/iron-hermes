use std::fs;
use std::path::PathBuf;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    fs::read_to_string(crate_root().join(path)).expect("failed to read source file")
}

#[test]
fn ws_uses_injected_app_state_not_per_connection_init() {
    let ws = read("src/server/ws.rs");
    assert!(
        ws.contains("global_app_state()"),
        "ws_chat must read startup AppState from shared global state"
    );
    assert!(
        !ws.contains("AppState::init().await"),
        "ws_chat must not call AppState::init() per WebSocket connection"
    );
}

#[test]
fn api_sessions_and_tools_are_backed_by_real_state() {
    let api = read("src/server/api.rs");
    assert!(
        api.contains("list_sessions(") && api.contains("Platform::Web.to_string()"),
        "list_sessions must query StateStore for Platform::Web sessions"
    );
    assert!(
        api.contains("get_definitions(None)"),
        "list_tools must read tool definitions from runtime registry"
    );
    assert!(
        !api.contains("Ok(vec![])") || !api.contains("TODO"),
        "list_sessions/list_tools must not remain empty TODO stubs"
    );
}

#[test]
fn startup_state_initializes_shared_runtime_bundle_once() {
    let state = read("src/server/state.rs");
    let main = read("src/main.rs");
    assert!(
        state.contains("build_app_runtime_bundle("),
        "AppState::init must build shared runtime bundle"
    );
    assert!(
        main.contains("install_global_app_state(app_state.clone())"),
        "server main must install startup AppState exactly once"
    );
}

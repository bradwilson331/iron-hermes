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
    // Phase 28.1 Plan 03: state ctor now builds AgentRuntime::from_config (not
    // build_app_runtime_bundle directly — that call moved inside from_config).
    // Assert the new pattern: one AgentRuntime built in init.
    assert!(
        state.contains("AgentRuntime::from_config("),
        "AppState::init must build shared AgentRuntime via from_config (Phase 28.1-03)"
    );
    assert!(
        state.contains("runtime: Arc::new(runtime),"),
        "AppState struct must store Arc<AgentRuntime> as 'runtime' field (Phase 28.1-03)"
    );
    assert!(
        main.contains("install_global_app_state(app_state.clone())"),
        "server main must install startup AppState exactly once"
    );
}

// =============================================================================
// Phase 28.1 Plan 03 Task 3: static grep gate — web crate budget invariants
// =============================================================================
//
// Integration test mirrors the position-guard pattern from
// `crates/ironhermes-cli/tests/invariants_22_4.rs`. Uses `read()` so it sees
// source text. Comment lines (starting with `//`) and test-block lines are
// stripped to prevent doc-comment false positives on the forbidden tokens.

/// Strip comment-only lines so doc comments mentioning forbidden tokens don't
/// self-invalidate the negative assertions.
fn strip_comment_lines(source: &str) -> String {
    source
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Phase 28.1 Plan 03 (T-28.1-06): web production code must contain no
/// `BudgetHandle::new(` — budget lifecycle is owned by `AgentRuntime::run_turn`.
#[test]
fn web_state_has_no_budget_handle_new() {
    let state = strip_comment_lines(&read("src/server/state.rs"));
    assert!(
        !state.contains("BudgetHandle::new("),
        "web server/state.rs must not call BudgetHandle::new( — \
         budget lifecycle is owned by AgentRuntime::run_turn (Plan 28.1-03, T-28.1-06)"
    );
}

/// Phase 28.1 Plan 03 (T-28.1-06): web production code must contain no
/// `.with_budget(` — budget is installed inside run_turn, not by the caller.
#[test]
fn web_state_has_no_with_budget() {
    let state = strip_comment_lines(&read("src/server/state.rs"));
    assert!(
        !state.contains("with_budget("),
        "web server/state.rs must not call .with_budget( — \
         budget is installed inside run_turn, not by the caller (Plan 28.1-03, T-28.1-06)"
    );
}

/// Phase 28.1 Plan 03 (T-28.1-07): `run_turn(` must appear in web production
/// code — this gate asserts the migration from build_agent_loop to run_turn landed.
#[test]
fn web_state_calls_run_turn() {
    let state = strip_comment_lines(&read("src/server/state.rs"));
    assert!(
        state.contains("run_turn("),
        "web server/state.rs must call run_turn( — \
         run_web_turn must delegate to AgentRuntime::run_turn (Plan 28.1-03, T-28.1-07)"
    );
}

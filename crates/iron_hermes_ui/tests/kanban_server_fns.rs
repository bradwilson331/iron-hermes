//! Phase 36.3.7.11 Plan 01 Wave 1 — kanban_api.rs source-string assertions.
//!
//! Locks:
//! - D-05: NO `/api/plugins/kanban` REST mount — reads only via `#[server]`.
//! - D-19: every `#[server]` fn takes `board: Option<String>`.
//! - Five fn names exist: fetch_board / fetch_task / fetch_task_events /
//!   fetch_task_runs / fetch_comments.
//!
//! Pattern: source-read assertion (mirrors tests/websocket_lifecycle_parity.rs).

use std::fs;
use std::path::PathBuf;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    fs::read_to_string(crate_root().join(path)).expect("failed to read source file")
}

#[test]
fn kanban_api_has_five_server_fns() {
    let src = read("src/server/kanban_api.rs");
    let server_count = src.matches("#[server]").count();
    assert!(
        server_count >= 5,
        "kanban_api.rs must have at least 5 #[server] fns (D-05); found {}",
        server_count
    );
}

#[test]
fn kanban_api_exposes_canonical_fn_names() {
    let src = read("src/server/kanban_api.rs");
    for name in [
        "fetch_board",
        "fetch_task",
        "fetch_task_events",
        "fetch_task_runs",
        "fetch_comments",
    ] {
        assert!(
            src.contains(&format!("fn {}", name)),
            "kanban_api.rs must export `fn {}` (D-05 read surface)",
            name
        );
    }
}

#[test]
fn kanban_api_every_fn_takes_board_option_string() {
    let src = read("src/server/kanban_api.rs");
    let board_param_count = src.matches("board: Option<String>").count();
    assert!(
        board_param_count >= 5,
        "D-19: every #[server] fn must take `board: Option<String>` — found {} occurrences",
        board_param_count
    );
}

#[test]
fn kanban_api_no_rest_plugins_route() {
    // D-05 enforcement: NO `/api/plugins/kanban` REST mount.
    let src = read("src/server/kanban_api.rs");
    assert!(
        !src.contains("/api/plugins/kanban"),
        "D-05: kanban_api.rs must NOT mount /api/plugins/kanban; \
         the read surface is #[server] fns only"
    );
}

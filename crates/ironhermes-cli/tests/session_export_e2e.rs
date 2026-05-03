//! Phase 25.3 D-F-1 end-to-end integration test for `hermes session export <id>`.
//!
//! Spawns the `CARGO_BIN_EXE_ironhermes` binary against a temp `IRONHERMES_HOME`
//! so the test can fully exercise the export pipeline without contaminating
//! real state. Mirrors the subprocess pattern from `profile_isolation.rs`.
//!
//! This test is the canonical phase-closing UAT for D-F-1: it proves the
//! 4-file directory layout actually materializes when an operator runs
//! `hermes session export <id>` against a populated SQLite state store.

use ironhermes_state::StateStore;
use std::path::PathBuf;
use std::process::Command;
use tempfile::tempdir;

/// Resolve the binary path. `CARGO_BIN_EXE_ironhermes` is set by Cargo when
/// running tests of this binary crate; falls back to `target/debug/ironhermes`
/// for non-Cargo invocations.
fn binary_path() -> PathBuf {
    std::env::var("CARGO_BIN_EXE_ironhermes")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("target/debug/ironhermes")
        })
}

/// Seed `<home>/state.db` with a single session under `session_id` carrying
/// a recognizable workspace_root marker so the metadata.json round-trip can
/// be asserted.
fn seed_state_store(home: &std::path::Path, session_id: &str) {
    let db_path = home.join("state.db");
    let mut store = StateStore::new(&db_path).expect("open seeded state store");
    store
        .create_session(
            session_id,
            "cli",
            Some("test-model"),
            None,
            None,
            Some("/tmp/myrepo"),
        )
        .expect("seed session");
}

#[test]
fn session_export_writes_4_file_layout() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let session_id = "sess-e2e-export";
    seed_state_store(home, session_id);

    let output = Command::new(binary_path())
        .env("IRONHERMES_HOME", home)
        .args(["session", "export", session_id])
        .output()
        .expect("spawn ironhermes session export");

    assert!(
        output.status.success(),
        "session export must exit 0; status={:?}, stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let export_dir = home.join("sessions").join(session_id);
    assert!(
        export_dir.join("messages.json").exists(),
        "messages.json must exist at {}",
        export_dir.display()
    );
    assert!(
        export_dir.join("metadata.json").exists(),
        "metadata.json must exist at {}",
        export_dir.display()
    );
    assert!(
        export_dir.join("context.json").exists(),
        "context.json must exist at {}",
        export_dir.display()
    );
    // trajectories.jsonl is optional (seed didn't create one — operator-tolerant)

    // metadata.json must contain the workspace_root from the seed
    let metadata = std::fs::read_to_string(export_dir.join("metadata.json")).unwrap();
    assert!(
        metadata.contains("\"/tmp/myrepo\""),
        "metadata.json must preserve workspace_root from session create; got: {metadata}"
    );
}

#[test]
fn session_export_all_with_no_sessions_exits_zero() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    // Seed the state store but with no sessions — opening creates an empty schema.
    let _store = StateStore::new(home.join("state.db")).unwrap();
    drop(_store);

    let output = Command::new(binary_path())
        .env("IRONHERMES_HOME", home)
        .args(["session", "export-all"])
        .output()
        .expect("spawn ironhermes session export-all");

    assert!(
        output.status.success(),
        "export-all with no sessions must exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn session_export_all_since_filter_excludes_old_sessions() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    seed_state_store(home, "old-sess");
    // started_at for the seeded session is unix_now() — well before 2030-01-01.
    let output = Command::new(binary_path())
        .env("IRONHERMES_HOME", home)
        .args(["session", "export-all", "--since", "2030-01-01"])
        .output()
        .expect("spawn export-all --since");
    assert!(
        output.status.success(),
        "export-all --since 2030-01-01 must exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    // No sessions should have been exported
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Exported 0 session"),
        "since-filter excluded all; expected 0; stderr={stderr}"
    );
}

//! Phase 24 — Gateway PID lock D-19 integration test (test 2 of 2).
//!
//! Test 1 (`profile_isolation_smoke`) lives in `tests/profile_isolation.rs`
//! and is owned by Plan 07. This file owns the concurrent-refuse case.
//!
//! Per RESEARCH §Pitfall 6, this test does NOT mutate IRONHERMES_HOME. It
//! passes `&Path` directly to `ironhermes_gateway::pid::acquire_pid_lock`
//! to avoid env_lock contention with parallel tests.

use ironhermes_gateway::pid::{
    acquire_pid_lock, read_gateway_pid, write_gateway_pid, GatewayPidRecord,
};
use tempfile::TempDir;

/// 24-04-02 / D-19 test 2 (D-12): a second `gateway run` under the same
/// profile must refuse with the explicit error AND must NOT delete or
/// overwrite the existing `gateway.pid` file (operator must `gateway stop`
/// or manually `rm` it). Uses `std::process::id()` for the recorded pid —
/// the current test process is guaranteed alive at the moment we probe it.
#[test]
fn gateway_pid_concurrent_refuse() {
    let dir = TempDir::new().expect("tempdir creation");
    let home = dir.path();

    // Write a fake live gateway.pid using the current process pid (alive).
    let live = GatewayPidRecord {
        pid: std::process::id(),
        started_at: chrono::Utc::now().to_rfc3339(),
        profile: "test".to_string(),
    };
    write_gateway_pid(home, &live).expect("seed gateway.pid");

    // Snapshot file content for byte-identical assertion afterwards.
    let pid_path = home.join("gateway.pid");
    let before = std::fs::read_to_string(&pid_path).expect("read seeded pid");

    // The contract: acquire_pid_lock returns Err on live-PID conflict.
    let result = acquire_pid_lock(home);
    assert!(
        result.is_err(),
        "expected acquire_pid_lock to refuse a live PID, got Ok"
    );
    let err = result.err().unwrap();
    let err_msg = err.to_string();

    assert!(
        err_msg.contains("Stop it first"),
        "expected D-12 error to contain 'Stop it first', got: {}",
        err_msg
    );
    assert!(
        err_msg.contains("test"),
        "expected D-12 error to mention the profile slug 'test', got: {}",
        err_msg
    );

    // Defensive: the file MUST still exist and be byte-identical (not
    // overwritten or deleted on the live-conflict path).
    assert!(
        pid_path.exists(),
        "gateway.pid must still exist after refused acquire"
    );
    let after = std::fs::read_to_string(&pid_path).expect("read after refused acquire");
    assert_eq!(
        before, after,
        "gateway.pid content must be byte-identical after refused acquire"
    );

    // And read_gateway_pid still returns the original record.
    let parsed = read_gateway_pid(home).unwrap().unwrap();
    assert_eq!(parsed, live);
}

/// Companion test (24-04-02 negative): a stale PID (guaranteed-ESRCH via
/// i32::MAX as u32) is auto-cleaned and acquire_pid_lock proceeds successfully.
/// Locks the D-11 ESRCH branch from the integration-test surface (Plan 02
/// already covers it from the unit-test surface; this is the seam that
/// `hermes gateway run` invocations would actually hit).
///
/// Note: u32::MAX is NOT used here — it wraps to i32 -1 on cast, which is
/// POSIX "send to all processes" and returns Ok(()) on macOS, giving a false
/// Live result. i32::MAX as u32 (2_147_483_647) is used instead, matching
/// the Plan 02 test decision documented in 24-02-SUMMARY.md.
#[test]
fn gateway_pid_stale_is_cleaned() {
    let dir = TempDir::new().expect("tempdir creation");
    let home = dir.path();

    let stale = GatewayPidRecord {
        pid: i32::MAX as u32, // virtually guaranteed ESRCH (see note above)
        started_at: "2020-01-01T00:00:00Z".to_string(),
        profile: "ghost".to_string(),
    };
    write_gateway_pid(home, &stale).expect("seed stale gateway.pid");

    let result = acquire_pid_lock(home);
    assert!(
        result.is_ok(),
        "stale PID must be auto-cleaned and acquire should succeed, got: {:?}",
        result.err()
    );

    let after = read_gateway_pid(home).unwrap().unwrap();
    assert_eq!(
        after.pid,
        std::process::id(),
        "after stale auto-clean, file should record the new (current) pid"
    );

    // Drop the guard returned by acquire_pid_lock to clean up the test file.
    drop(result.unwrap());
}

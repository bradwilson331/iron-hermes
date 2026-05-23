//! Phase 24 — `hermes doctor` gateway.pid liveness integration test (24-05-03).

use std::process::Command;
use tempfile::TempDir;

fn seed_gateway_pid(home: &std::path::Path, pid: u32) {
    std::fs::create_dir_all(home).unwrap();
    // Hand-write a Plan 02 GatewayPidRecord 3-line YAML.
    let body = format!(
        "pid: {}\nstarted_at: 2026-04-28T00:00:00Z\nprofile: default\n",
        pid
    );
    std::fs::write(home.join("gateway.pid"), body).unwrap();
}

/// 24-05-03: `hermes doctor` includes a gateway.pid liveness check on the
/// active profile (D-16). With a live PID (current test process id), the
/// check reports OK.
#[test]
fn profile_doctor() {
    let bin = match std::env::var("CARGO_BIN_EXE_ironhermes") {
        Ok(p) => p,
        Err(_) => {
            eprintln!("Skipping profile_doctor: CARGO_BIN_EXE_ironhermes not set");
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    // Use the current test process id — guaranteed alive when doctor probes it.
    seed_gateway_pid(tmp.path(), std::process::id());
    let out = Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        .args(["doctor"])
        .output()
        .expect("ironhermes doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Gateway PID"),
        "expected 'Gateway PID' line in doctor output, got:\n{}",
        stdout
    );
}

/// Wave 0 stub — Phase 35.1 Plan 03 Task 01 removes #[ignore] and fills assertions.
/// D-07/D-08: first-run auto-detection triggers setup when no API key and no local endpoint.
#[test]
#[ignore = "Wave 0 stub — pending Phase 35.1 Plan 03-01"]
fn d07_d08_first_run_triggers_setup_when_no_api_key_and_no_local_endpoint() {
    // Wave 0 stub — Wave 2 Plan 03 removes #[ignore] and replaces body.
}

/// Companion: doctor with no gateway.pid present reports the absent-file
/// branch (D-16 healthy state — no gateway running).
#[test]
fn doctor_no_pid_file_is_healthy() {
    let bin = match std::env::var("CARGO_BIN_EXE_ironhermes") {
        Ok(p) => p,
        Err(_) => {
            eprintln!("Skipping doctor_no_pid_file_is_healthy: CARGO_BIN_EXE_ironhermes not set");
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let out = Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        .args(["doctor"])
        .output()
        .expect("ironhermes doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Gateway PID") && (stdout.contains("not running") || stdout.contains("OK")),
        "expected absent-pid healthy branch in doctor output, got:\n{}",
        stdout
    );
}

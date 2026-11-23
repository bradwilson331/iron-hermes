//! 54.8-02 (D-14): `ironhermes mcp connect --no-browser` CLI surface.
//!
//! The success path (a real authorization server) cannot run here:
//! `validate_prm_issuer` refuses any non-HTTPS issuer, so it is exercised against a
//! fake `OAuthPasteBack` in `mcp_config.rs` unit tests and live in acceptance A-3.

use std::process::Command;
use tempfile::TempDir;

fn seed_minimum_config(home: &std::path::Path) {
    std::fs::create_dir_all(home).unwrap();
    let cfg = "provider:\n  default: openrouter\nmemory:\n  enabled: true\nskills:\n  generation_enabled: true\n";
    std::fs::write(home.join("config.yaml"), cfg).unwrap();
}

#[test]
fn no_browser_conflicts_with_browser() {
    let tmp = TempDir::new().unwrap();
    seed_minimum_config(tmp.path());
    let out = Command::new(env!("CARGO_BIN_EXE_ironhermes"))
        .env("IRONHERMES_HOME", tmp.path())
        .args(["mcp", "connect", "demo", "--no-browser", "--browser"])
        .output()
        .expect("ironhermes mcp connect");
    assert!(!out.status.success(), "conflicting flags must be refused");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot be used with"),
        "stderr should explain the conflict, got: {stderr}"
    );
}

#[test]
fn no_browser_unknown_server_reports_not_found() {
    let tmp = TempDir::new().unwrap();
    seed_minimum_config(tmp.path());
    let out = Command::new(env!("CARGO_BIN_EXE_ironhermes"))
        .env("IRONHERMES_HOME", tmp.path())
        .args(["mcp", "connect", "missing-server", "--no-browser"])
        .output()
        .expect("ironhermes mcp connect");
    assert_eq!(out.status.code(), Some(1), "unknown server must exit 1");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not found in config"),
        "stderr should say the server is not in config, got: {stderr}"
    );
}

//! Phase 24 — `hermes config show` Profile: prefix integration test (24-05-02).

use std::process::Command;
use tempfile::TempDir;

fn seed_minimum_config(home: &std::path::Path) {
    std::fs::create_dir_all(home).unwrap();
    let cfg = "provider:\n  default: openrouter\nmemory:\n  enabled: true\nskills:\n  generation_enabled: true\n";
    std::fs::write(home.join("config.yaml"), cfg).unwrap();
}

/// 24-05-02: `hermes config show` prepends `Profile: <name>` line ABOVE the
/// Learning Loop banner. For bare hermes, the slug is the literal "default".
#[test]
fn profile_line() {
    let bin = match std::env::var("CARGO_BIN_EXE_ironhermes") {
        Ok(p) => p,
        Err(_) => {
            eprintln!("Skipping profile_line: CARGO_BIN_EXE_ironhermes not set");
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    seed_minimum_config(tmp.path());
    let out = Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        .args(["config", "show"])
        .output()
        .expect("ironhermes config show");
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Find the first non-empty line.
    let first = stdout
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    assert!(
        first.starts_with("Profile:"),
        "expected first non-empty line to start with 'Profile:', got: {:?}\n--- full stdout ---\n{}",
        first,
        stdout
    );
    // For bare hermes, the slug must be the literal "default" sentinel.
    assert!(
        first.contains("default"),
        "expected bare hermes to show 'Profile: default', got: {:?}",
        first
    );
}

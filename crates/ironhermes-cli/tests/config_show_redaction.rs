//! Integration tests for `hermes config show` — Learning Loop banner + secret redaction.
//! Phase 23 Plan 02 — Task 5.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn env_lock() -> &'static std::sync::Mutex<()> {
    use std::sync::OnceLock;
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

fn write_test_config(home: &std::path::Path) {
    std::fs::write(
        home.join("config.yaml"),
        "\
model:
  default: openrouter/qwen-2.5-coder-32b
  api_key: sk-test-12345-secret
  provider: openrouter
memory:
  memory_enabled: true
  user_profile_enabled: true
  provider: file
gateway:
  platforms:
    telegram:
      enabled: true
      token: 1234:secret-telegram-token-xyz
learning:
  skill_generation_enabled: true
",
    )
    .unwrap();
}

#[test]
fn config_show_masks_api_key() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    write_test_config(tmp.path());
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", tmp.path())
        .args(["config", "show"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("sk-tes***")
                .or(predicate::str::contains("sk-test***")),
        )
        .stdout(predicate::str::contains("12345-secret").not())
        .stdout(predicate::str::contains("secret-telegram-token-xyz").not());
}

#[test]
fn config_show_learning_loop_enabled_banner() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    write_test_config(tmp.path());
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", tmp.path())
        .args(["config", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "🧠 Learning Loop: enabled (memory + skill generation)",
        ));
}

#[test]
fn config_show_learning_loop_disabled_banner() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("config.yaml"),
        "\
memory:
  memory_enabled: false
learning:
  skill_generation_enabled: false
",
    )
    .unwrap();
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", tmp.path())
        .args(["config", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains("⚠ Learning Loop: disabled"))
        .stdout(predicate::str::contains(
            "Run `hermes setup memory` to enable",
        ));
}

#[test]
fn config_show_no_config_friendly_message() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", tmp.path())
        .args(["config", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No config.yaml found"));
}

#[test]
fn config_show_non_secrets_unmasked() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    write_test_config(tmp.path());
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", tmp.path())
        .args(["config", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains("openrouter/qwen-2.5-coder-32b"))
        .stdout(predicate::str::contains("provider: openrouter"));
}

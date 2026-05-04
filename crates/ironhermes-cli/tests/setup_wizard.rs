//! Integration tests for `hermes setup` wizard and preflight middleware.
//! Phase 23 Plan 02 — Tasks 2, 3, 7.

use assert_cmd::Command;
use ironhermes_core::config::Config;
use ironhermes_core::config_setter;
use predicates::prelude::*;
use tempfile::TempDir;

fn env_lock() -> &'static std::sync::Mutex<()> {
    use std::sync::OnceLock;
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

// ============================================================================
// Task 2: minimum-viable wizard flow + Learning Loop framing
// ============================================================================

#[test]
fn minimum_viable_answers_seed_full_config() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    unsafe {
        std::env::set_var("IRONHERMES_HOME", tmp.path());
    }

    let mut config = Config::default();
    let block = ironhermes_cli::setup::apply_minimum_viable_answers(
        &mut config,
        "openrouter",
        "sk-test",
        "openai/gpt-4o-mini",
        "y",
    );
    // Persist: typed Config first, then learning.* splice (mirrors run_minimum_viable_flow).
    config.save_to(&tmp.path().join("config.yaml")).unwrap();
    for (k, v) in &block {
        let key_str = k.as_str().unwrap();
        let dotted = format!("learning.{}", key_str);
        let value_str = match v {
            serde_yaml::Value::Bool(b) => b.to_string(),
            serde_yaml::Value::Number(n) => n.to_string(),
            serde_yaml::Value::String(s) => s.clone(),
            other => serde_yaml::to_string(other).unwrap().trim().to_string(),
        };
        config_setter::config_set(tmp.path(), &dotted, &value_str).unwrap();
    }

    assert!(tmp.path().join("config.yaml").exists());
    let loaded = Config::load().expect("config.yaml must reload after wizard");
    assert!(
        loaded.memory.memory_enabled,
        "Learning Loop must default ON"
    );
    let val = config_setter::config_get(tmp.path(), "learning.skill_generation_enabled").unwrap();
    assert_eq!(
        val.as_deref(),
        Some("true"),
        "learning.skill_generation_enabled must be present (D-15)"
    );
}

#[test]
fn learning_loop_no_writes_explicit_false() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    unsafe {
        std::env::set_var("IRONHERMES_HOME", tmp.path());
    }

    let mut config = Config::default();
    let block = ironhermes_cli::setup::apply_minimum_viable_answers(
        &mut config,
        "openrouter",
        "sk-test",
        "openai/gpt-4o-mini",
        "n",
    );
    config.save_to(&tmp.path().join("config.yaml")).unwrap();
    for (k, v) in &block {
        let key_str = k.as_str().unwrap();
        let dotted = format!("learning.{}", key_str);
        let value_str = match v {
            serde_yaml::Value::Bool(b) => b.to_string(),
            serde_yaml::Value::Number(n) => n.to_string(),
            serde_yaml::Value::String(s) => s.clone(),
            other => serde_yaml::to_string(other).unwrap().trim().to_string(),
        };
        config_setter::config_set(tmp.path(), &dotted, &value_str).unwrap();
    }
    let val = config_setter::config_get(tmp.path(), "learning.skill_generation_enabled").unwrap();
    assert_eq!(
        val.as_deref(),
        Some("false"),
        "explicit false, never absent (D-14)"
    );
}

#[test]
fn setup_source_uses_learning_loop_framing_const() {
    // D-16 lock: source must reference the const, not inline the string.
    let src = std::fs::read_to_string("src/setup.rs").expect("setup.rs readable");
    assert!(
        src.contains("LEARNING_LOOP_FRAMING"),
        "setup.rs MUST reference LEARNING_LOOP_FRAMING (D-16 lock)"
    );
}

#[test]
fn wizard_does_not_persist_history() {
    // Anti-Pattern #3: wizard editor must NOT load/save rustyline history.
    let src = std::fs::read_to_string("src/setup.rs").expect("setup.rs readable");
    assert!(
        !src.contains("load_history"),
        "wizard must not call load_history"
    );
    assert!(
        !src.contains("save_history"),
        "wizard must not call save_history"
    );
    assert!(
        !src.contains("set_max_history_size"),
        "wizard must not call set_max_history_size"
    );
}

// ============================================================================
// Task 3: Section flows (model, memory, gateway, tools)
// ============================================================================

#[test]
fn setup_gateway_section_exits_ok() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", tmp.path())
        .args(["setup", "gateway"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Phase 25").or(predicate::str::contains("phase 25")));
}

#[test]
fn setup_tools_section_exits_ok() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", tmp.path())
        .args(["setup", "tools"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Phase 25").or(predicate::str::contains("phase 25")));
}

#[test]
fn setup_agent_section_succeeds_with_phase26_implementation() {
    // Phase 26 Plan 05: hermes setup agent is now implemented (D-19 auxiliary routing).
    // Previously this section was deferred and bailed with "Phase 26".
    // After Plan 05 it exits 0 (graceful skip on EOF in non-interactive context).
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", tmp.path())
        .args(["setup", "agent"])
        .assert()
        .success()
        .stdout(predicate::str::contains("auxiliary").or(predicate::str::contains("routing")));
}

#[test]
fn setup_skills_section_errors_with_deferred_message() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", tmp.path())
        .args(["setup", "skills"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Phase 28").or(predicate::str::contains("phase 28")));
}

#[test]
fn setup_unknown_section_errors_with_help_text() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", tmp.path())
        .args(["setup", "nonsense"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown setup section: nonsense"));
}

// ============================================================================
// Task 4: config set/get with cache-break warnings
// ============================================================================

#[test]
fn config_set_cache_breaking_warns_then_persists() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", tmp.path())
        .args(["config", "set", "model.default", "openai/gpt-4o-mini"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Persisted: model.default = openai/gpt-4o-mini",
        ))
        .stderr(predicate::str::contains("invalidates the prompt cache"));
}

#[test]
fn config_set_non_cache_breaking_no_warning() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", tmp.path())
        .args(["config", "set", "memory.user_profile_enabled", "false"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Persisted: memory.user_profile_enabled = false",
        ))
        .stderr(predicate::str::contains("invalidates the prompt cache").not());
}

#[test]
fn config_get_roundtrip() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", tmp.path())
        .args(["config", "set", "model.default", "test-model"])
        .assert()
        .success();
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", tmp.path())
        .args(["config", "get", "model.default"])
        .assert()
        .success()
        .stdout(predicate::str::contains("test-model"));
}

#[test]
fn config_get_missing_key_silent() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", tmp.path())
        .args(["config", "get", "no.such.key"])
        .assert()
        .success()
        .stdout("");
}

// ============================================================================
// Task 7: Preflight middleware skip-list tests
// ============================================================================

#[test]
fn config_subcommand_skips_preflight() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    // No config.yaml in tmp — preflight WOULD launch wizard if it ran (blocking on stdin).
    // Config subcommand must skip preflight entirely. We use `config path` (Task 1, always succeeds)
    // to verify the skip-list works without depending on Task 4's stub state.
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", tmp.path())
        .args(["config", "path"])
        .assert()
        .success()
        .stdout(predicate::str::contains("config.yaml")); // would hang if preflight engaged
}

#[test]
fn setup_subcommand_skips_preflight() {
    // setup subcommand must NOT recurse into preflight.
    // tools section is a stub message — exits 0.
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", tmp.path())
        .args(["setup", "tools"])
        .assert()
        .success();
}

#[test]
fn version_skips_preflight() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", tmp.path())
        .args(["--version"])
        .assert()
        .success();
}

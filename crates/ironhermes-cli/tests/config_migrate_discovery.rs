//! Integration tests for `hermes config migrate` skill frontmatter discovery.
//! Phase 23 Plan 02 — Task 6.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn env_lock() -> &'static std::sync::Mutex<()> {
    use std::sync::OnceLock;
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[test]
fn migrate_with_no_skills_dir() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", tmp.path())
        .args(["config", "migrate"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No configuration gaps detected"));
}

#[test]
fn migrate_surfaces_skill_config_gap() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    let skills = tmp.path().join("skills/test-skill");
    std::fs::create_dir_all(&skills).unwrap();
    // SKILL.md with requires_config via metadata.hermes.config
    std::fs::write(
        skills.join("SKILL.md"),
        "---\nname: test-skill\ndescription: minimum-length-description-here-for-validation-pass\nversion: 0.1.0\nmetadata:\n  hermes:\n    config:\n      - key: test_skill.api_token\n---\nbody\n",
    )
    .unwrap();
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", tmp.path())
        .args(["config", "migrate"])
        .write_stdin("skip all\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("test_skill.api_token"));
}

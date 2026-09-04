//! The skill catalog must be replaceable in a running process.
//!
//! Regression: `AppRuntimeBundle::skill_registry` was a boot-time
//! `Arc<SkillRegistry>` with no interior mutability, so a skill installed
//! through the web UI landed on disk and in `skills-lock.json` but stayed
//! invisible to `list_skills` — and unusable by the agent's own `skills` tool —
//! until the server was restarted. The operator saw the import succeed and
//! nothing change.

use ironhermes_agent::AgentRuntime;
use ironhermes_core::config::SkillsConfig;

fn write_skill(root: &std::path::Path, name: &str) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).expect("create skill dir");
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: written after boot\n---\n\nbody\n"),
    )
    .expect("write SKILL.md");
}

/// Scan ONLY the tempdir: `extra_paths` is appended to the defaults, so the
/// disabled-by-default master switch is not enough on its own — but pointing
/// the test at a directory nothing else writes keeps the assertions about
/// `added` scoped to what this test created.
fn config_scanning(root: &std::path::Path) -> SkillsConfig {
    SkillsConfig {
        extra_paths: vec![root.to_path_buf()],
        ..SkillsConfig::default()
    }
}

#[tokio::test]
async fn reload_publishes_a_skill_written_after_boot() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let runtime = AgentRuntime::for_tests_with_base_url("http://127.0.0.1:1");

    // The test runtime boots with an empty catalog (`load_with_paths(&[])`).
    assert!(
        runtime.skill_registry().find("late-arrival").is_none(),
        "precondition: the skill must not exist before it is written"
    );

    write_skill(tmp.path(), "late-arrival");

    // Writing to disk alone changes nothing about what the process serves.
    assert!(
        runtime.skill_registry().find("late-arrival").is_none(),
        "a disk write must not retroactively appear without a reload"
    );

    let outcome = runtime
        .reload_skill_registry(&config_scanning(tmp.path()))
        .await;

    assert!(
        outcome.added.contains(&"late-arrival".to_string()),
        "reload must report the new skill as added, got {:?}",
        outcome.added
    );
    assert!(
        runtime.skill_registry().find("late-arrival").is_some(),
        "skill_registry() must serve the SWAPPED-IN catalog, not the boot snapshot"
    );
}

#[tokio::test]
async fn reload_reports_a_removed_skill_and_drops_it_from_the_catalog() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let runtime = AgentRuntime::for_tests_with_base_url("http://127.0.0.1:1");

    write_skill(tmp.path(), "transient");
    let config = config_scanning(tmp.path());
    runtime.reload_skill_registry(&config).await;
    assert!(runtime.skill_registry().find("transient").is_some());

    std::fs::remove_dir_all(tmp.path().join("transient")).expect("remove skill dir");
    let outcome = runtime.reload_skill_registry(&config).await;

    assert!(
        outcome.removed.contains(&"transient".to_string()),
        "reload must report the deleted skill as removed, got {:?}",
        outcome.removed
    );
    assert!(
        runtime.skill_registry().find("transient").is_none(),
        "a deleted skill must not survive in the swapped-in catalog"
    );
}

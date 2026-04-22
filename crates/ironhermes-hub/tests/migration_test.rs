//! End-to-end tests for `migrate_from_hub_manifest` (D-15).
//!
//! Exercises the idempotent one-way migration from the 19.1
//! `$HERMES_HOME/skills/.hub/lock.json` (`HubManifest`) schema to the 21.8
//! `$HERMES_HOME/skills-lock.json` (`SkillLock`) schema.
//!
//! Locks in:
//! - First run with pre-seeded old manifest → Migrated(N) + new lock present
//!   + old file deleted
//! - Second run is a no-op (`NothingToMigrate`) AND the lock file is
//!   byte-identical (idempotence)
//! - Pre-existing new lock entries short-circuit migration (no merge of old)
//! - Missing old manifest → NothingToMigrate

use ironhermes_hub::{migrate_from_hub_manifest, MigrationOutcome, SkillLock};
use std::path::Path;
use std::sync::Mutex;

// HERMES_HOME mutates process-global env; serialize across tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn with_test_hermes_home<F: FnOnce(&Path)>(f: F) {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let prev = std::env::var("HERMES_HOME").ok();
    unsafe {
        std::env::set_var("HERMES_HOME", tmp.path());
    }
    f(tmp.path());
    unsafe {
        match prev {
            Some(v) => std::env::set_var("HERMES_HOME", v),
            None => std::env::remove_var("HERMES_HOME"),
        }
    }
}

/// Seed a 19.1 `HubManifest` at `$HERMES_HOME/skills/.hub/lock.json` with the
/// given `(name, identifier)` entries.
fn seed_old_manifest(home: &Path, entries: Vec<(&str, &str)>) {
    let hub_dir = home.join("skills").join(".hub");
    std::fs::create_dir_all(&hub_dir).unwrap();

    let mut installed = serde_json::Map::new();
    for (name, id) in entries {
        installed.insert(
            name.to_string(),
            serde_json::json!({
                "name": name,
                "source": "skills-sh",
                "identifier": id,
                "content_hash": "old-hash-123",
                "scan_verdict": "clean",
                "install_path": format!("/x/{name}"),
                "files": [format!("skills/{name}/SKILL.md")],
                "installed_at": "2026-04-22T00:00:00Z",
                "updated_at": serde_json::Value::Null,
                "metadata": serde_json::Value::Null,
            }),
        );
    }
    let manifest = serde_json::json!({"installed": installed});
    std::fs::write(
        hub_dir.join("lock.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
}

#[test]
fn migrates_then_noop() {
    with_test_hermes_home(|home| {
        seed_old_manifest(home, vec![("zeta", "z"), ("alpha", "a")]);

        // First run: migrate 2 entries.
        let outcome = migrate_from_hub_manifest().expect("first migration ok");
        assert_eq!(
            outcome,
            MigrationOutcome::Migrated(2),
            "first migration must report 2 entries migrated"
        );

        let lock = SkillLock::load_or_default().unwrap();
        assert_eq!(lock.skills.len(), 2, "new lock must have 2 entries");
        // D-12: alphabetical order via SkillLock::save_atomic.
        assert_eq!(lock.skills[0].name, "alpha");
        assert_eq!(lock.skills[1].name, "zeta");
        // Old manifest file must be deleted after successful migration.
        assert!(
            !home.join("skills").join(".hub").join("lock.json").exists(),
            ".hub/lock.json must be deleted after successful migration"
        );

        // Capture snapshot of the migrated lock for idempotence comparison.
        let snapshot = std::fs::read_to_string(home.join("skills-lock.json")).unwrap();

        // Second run: nothing to migrate (guard 1: new lock has entries).
        let outcome2 = migrate_from_hub_manifest().unwrap();
        assert_eq!(outcome2, MigrationOutcome::NothingToMigrate);

        let after = std::fs::read_to_string(home.join("skills-lock.json")).unwrap();
        assert_eq!(
            snapshot, after,
            "idempotent: second run MUST produce byte-identical skills-lock.json"
        );
    });
}

#[test]
fn noop_when_new_lock_has_entries_and_old_exists() {
    with_test_hermes_home(|home| {
        // Pre-seed BOTH files: new lock (1 entry) AND old .hub/lock.json (2 entries).
        std::fs::write(
            home.join("skills-lock.json"),
            r#"{"version":1,"skills":[
                {"name":"preexisting","source":"skills-sh","identifier":"pre",
                 "repoPath":"r","snapshotHash":"s","computedHash":"c",
                 "installedAt":"2026-04-22T00:00:00Z"}
            ]}"#,
        )
        .unwrap();
        seed_old_manifest(home, vec![("migrate-me", "m"), ("other", "o")]);

        let outcome = migrate_from_hub_manifest().unwrap();
        assert_eq!(
            outcome,
            MigrationOutcome::NothingToMigrate,
            "guard 1 (new lock has entries) must short-circuit before reading old manifest"
        );

        // New lock unchanged — still 1 entry, named "preexisting".
        let lock = SkillLock::load_or_default().unwrap();
        assert_eq!(lock.skills.len(), 1);
        assert_eq!(lock.skills[0].name, "preexisting");

        // Old .hub/lock.json NOT deleted (migration was skipped).
        assert!(
            home.join("skills").join(".hub").join("lock.json").exists(),
            ".hub/lock.json must NOT be deleted when migration is skipped"
        );
    });
}

#[test]
fn noop_when_no_old_manifest() {
    with_test_hermes_home(|_home| {
        // No pre-seed — brand new install, no 19.1 state on disk.
        let outcome = migrate_from_hub_manifest().unwrap();
        assert_eq!(
            outcome,
            MigrationOutcome::NothingToMigrate,
            "guard 2 (no old manifest) must short-circuit"
        );
    });
}

//! `skills-lock.json` v1 — merge-clean lock file for installed skills.
//!
//! Replaces the Phase 19.1 `.hub/*.json` manifest. One file at $HERMES_HOME/skills-lock.json,
//! skills[] sorted alphabetically by name, timestamp-free hashed region (installedAt is
//! metadata, NOT in the hash). On-disk JSON is camelCase to match reference TS exactly
//! (`/Users/twilson/code/skills/src/local-lock.ts`). See D-10..D-14 in CONTEXT.md.

use crate::{HubError, HubErrorKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;

// SP-1: per-module typed() helper.
fn typed(kind: HubErrorKind, msg: impl Into<String>) -> HubError {
    HubError::Typed {
        kind,
        message: msg.into(),
        suggestion: None,
        retry_after_s: None,
    }
}

// ============================================================================
// Types (D-11 schema — camelCase on wire)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SkillLockEntry {
    pub name: String,
    pub source: String, // "skills-sh"
    pub identifier: String,
    pub repo_path: String,                           // "repoPath" on wire
    pub snapshot_hash: String, // "snapshotHash" — skillsComputedHash from server (D-14)
    pub computed_hash: String, // "computedHash" — SHA-256 over installed folder (D-13)
    pub installed_at: chrono::DateTime<chrono::Utc>, // "installedAt" — NOT in hash (D-12)
    #[serde(flatten, default)]
    pub extras: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillLock {
    pub version: u32, // == 1
    #[serde(default)]
    pub skills: Vec<SkillLockEntry>,
    #[serde(flatten, default)]
    pub extras: HashMap<String, serde_json::Value>,
}

impl Default for SkillLock {
    fn default() -> Self {
        Self {
            version: 1,
            skills: Vec::new(),
            extras: HashMap::new(),
        }
    }
}

// ============================================================================
// Load / Save (SP-4 atomic tmp+rename per D-25)
// ============================================================================

impl SkillLock {
    pub fn load_or_default() -> anyhow::Result<Self> {
        let p = crate::paths::skills_lock_path()?;
        if !p.exists() {
            return Ok(Self::default());
        }
        Ok(serde_json::from_str(&std::fs::read_to_string(p)?)?)
    }

    /// D-12: sort skills alphabetically by name BEFORE serialize; D-25 atomic write.
    pub fn save_atomic(&mut self) -> anyhow::Result<()> {
        self.skills.sort_by(|a, b| a.name.cmp(&b.name));
        let p = crate::paths::skills_lock_path()?;
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(tmp, p)?;
        Ok(())
    }

    pub fn add_or_replace(&mut self, entry: SkillLockEntry) {
        if let Some(pos) = self.skills.iter().position(|e| e.name == entry.name) {
            self.skills[pos] = entry;
        } else {
            self.skills.push(entry);
        }
    }

    pub fn remove(&mut self, name: &str) -> Option<SkillLockEntry> {
        let pos = self.skills.iter().position(|e| e.name == name)?;
        Some(self.skills.remove(pos))
    }

    pub fn get(&self, name: &str) -> Option<&SkillLockEntry> {
        self.skills.iter().find(|e| e.name == name)
    }
}

// ============================================================================
// Folder hash (D-13 corrected — NO separators)
//
// EXACT algorithm from /Users/twilson/code/skills/src/local-lock.ts:108-114:
//   files.sort(...)
//   for each file: hasher.update(relativePath); hasher.update(content);
//
// NO newline, NO space, NO NUL between path and content.
// NO separator between files.
// Skip .git and node_modules directories. Skip symlinks.
// ============================================================================

pub fn compute_folder_hash(skill_dir: &Path) -> Result<String, HubError> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    walk(skill_dir, skill_dir, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0)); // alphabetical by forward-slash-normalized rel path
    let mut hasher = Sha256::new();
    for (rel, content) in &files {
        hasher.update(rel.as_bytes());
        hasher.update(content);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<(), HubError> {
    for entry in std::fs::read_dir(dir)
        .map_err(|e| typed(HubErrorKind::Io, format!("read_dir {}: {e}", dir.display())))?
    {
        let entry = entry.map_err(|e| typed(HubErrorKind::Io, format!("{e}")))?;
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let file_type = entry
            .file_type()
            .map_err(|e| typed(HubErrorKind::Io, format!("{e}")))?;

        if file_type.is_symlink() {
            continue;
        } // defense-in-depth
        if file_type.is_dir() {
            if name_str == ".git" || name_str == "node_modules" {
                continue;
            }
            walk(base, &path, out)?;
        } else if file_type.is_file() {
            let rel = path
                .strip_prefix(base)
                .map_err(|e| typed(HubErrorKind::Io, format!("strip_prefix: {e}")))?
                .to_string_lossy()
                .replace('\\', "/");
            let content = std::fs::read(&path)
                .map_err(|e| typed(HubErrorKind::Io, format!("read {}: {e}", path.display())))?;
            out.push((rel, content));
        }
    }
    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // SP-7: copy verbatim from manifest.rs:57-74
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

    fn mk_entry(name: &str, snap: &str) -> SkillLockEntry {
        SkillLockEntry {
            name: name.to_string(),
            source: "skills-sh".to_string(),
            identifier: name.to_string(),
            repo_path: format!("skills/{name}/SKILL.md"),
            snapshot_hash: snap.to_string(),
            computed_hash: "abc123".to_string(),
            installed_at: Utc::now(),
            extras: HashMap::new(),
        }
    }

    #[test]
    fn save_sorts_alphabetically() {
        with_test_hermes_home(|_home| {
            let mut lock = SkillLock::default();
            lock.add_or_replace(mk_entry("zeta", "s1"));
            lock.add_or_replace(mk_entry("alpha", "s2"));
            lock.save_atomic().unwrap();
            let reloaded = SkillLock::load_or_default().unwrap();
            assert_eq!(reloaded.skills.len(), 2);
            assert_eq!(reloaded.skills[0].name, "alpha");
            assert_eq!(reloaded.skills[1].name, "zeta");
        });
    }

    #[test]
    fn serializes_camel_case() {
        let e = mk_entry("demo", "h");
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains(r#""repoPath""#), "got: {json}");
        assert!(json.contains(r#""snapshotHash""#));
        assert!(json.contains(r#""computedHash""#));
        assert!(json.contains(r#""installedAt""#));
        assert!(
            !json.contains(r#""repo_path""#),
            "snake_case leaked: {json}"
        );
    }

    #[test]
    fn load_or_default_on_missing_returns_empty() {
        with_test_hermes_home(|_| {
            let lock = SkillLock::load_or_default().unwrap();
            assert_eq!(lock.version, 1);
            assert!(lock.skills.is_empty());
        });
    }

    #[test]
    fn atomic_save_leaves_no_tmp() {
        with_test_hermes_home(|home| {
            let mut lock = SkillLock::default();
            lock.add_or_replace(mk_entry("x", "h"));
            lock.save_atomic().unwrap();
            assert!(home.join("skills-lock.json").exists());
            assert!(!home.join("skills-lock.json.tmp").exists());
        });
    }

    #[test]
    fn extras_passthrough_round_trip() {
        let json = r#"{"name":"n","source":"skills-sh","identifier":"n","repoPath":"r",
            "snapshotHash":"s","computedHash":"c","installedAt":"2026-04-22T00:00:00Z",
            "unknownField":"value"}"#;
        let e: SkillLockEntry = serde_json::from_str(json).unwrap();
        assert_eq!(
            e.extras.get("unknownField"),
            Some(&serde_json::Value::String("value".to_string()))
        );
        let back = serde_json::to_string(&e).unwrap();
        assert!(back.contains("unknownField"));
    }

    #[test]
    fn add_or_replace_replaces_by_name() {
        let mut lock = SkillLock::default();
        lock.add_or_replace(mk_entry("n", "old"));
        lock.add_or_replace(mk_entry("n", "new"));
        assert_eq!(lock.skills.len(), 1);
        assert_eq!(lock.skills[0].snapshot_hash, "new");
    }

    #[test]
    fn remove_returns_removed_entry() {
        let mut lock = SkillLock::default();
        lock.add_or_replace(mk_entry("n", "h"));
        let removed = lock.remove("n").expect("should remove");
        assert_eq!(removed.name, "n");
        assert!(lock.skills.is_empty());
        assert!(lock.remove("n").is_none());
    }

    #[test]
    fn compute_folder_hash_deterministic() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("b.txt"), b"bbb").unwrap();
        std::fs::write(tmp.path().join("a.txt"), b"aaa").unwrap();
        std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
        std::fs::write(tmp.path().join("sub").join("c.txt"), b"ccc").unwrap();
        let h1 = compute_folder_hash(tmp.path()).unwrap();
        let h2 = compute_folder_hash(tmp.path()).unwrap();
        assert_eq!(h1, h2, "hash must be deterministic");
        assert_eq!(h1.len(), 64, "SHA-256 hex is 64 chars");
    }

    #[test]
    fn compute_folder_hash_no_separator() {
        // Hand-computed regression guard against CONTEXT.md D-13's incorrect `\n` wording.
        // Two files: "a.txt" containing "aa" and "b.txt" containing "bb".
        // Expected hash = SHA-256 of the concatenation:
        //   "a.txt" (5 bytes) + "aa" (2 bytes) + "b.txt" (5 bytes) + "bb" (2 bytes) = 14 bytes
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), b"aa").unwrap();
        std::fs::write(tmp.path().join("b.txt"), b"bb").unwrap();
        let actual = compute_folder_hash(tmp.path()).unwrap();

        let mut h = Sha256::new();
        h.update(b"a.txt");
        h.update(b"aa");
        h.update(b"b.txt");
        h.update(b"bb");
        let expected = hex::encode(h.finalize());

        assert_eq!(
            actual, expected,
            "compute_folder_hash must NOT add separators — if this fails, the algorithm was changed \
             and it will silently diverge from the reference TS local-lock.ts:108-114."
        );
    }

    #[test]
    fn compute_folder_hash_skips_git_and_node_modules() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("keep.txt"), b"k").unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        std::fs::write(tmp.path().join(".git").join("HEAD"), b"ref:...").unwrap();
        std::fs::create_dir_all(tmp.path().join("node_modules")).unwrap();
        std::fs::write(tmp.path().join("node_modules").join("pkg.json"), b"{}").unwrap();

        let with_junk = compute_folder_hash(tmp.path()).unwrap();

        // Same hash as a dir containing only keep.txt
        let tmp2 = tempfile::tempdir().unwrap();
        std::fs::write(tmp2.path().join("keep.txt"), b"k").unwrap();
        let without = compute_folder_hash(tmp2.path()).unwrap();

        assert_eq!(
            with_junk, without,
            ".git and node_modules must not affect the hash"
        );
    }

    #[test]
    fn skills_lock_path_honors_hermes_home() {
        with_test_hermes_home(|home| {
            let p = crate::paths::skills_lock_path().unwrap();
            assert_eq!(p, home.join("skills-lock.json"));
            assert!(
                !p.to_string_lossy().contains("/skills/"),
                "skills-lock.json must NOT be inside skills/; got {}",
                p.display()
            );
        });
    }

    #[test]
    fn installed_at_not_in_folder_hash() {
        // folder hash derives from disk contents only; changing the entry's installed_at
        // does not (and cannot) change the folder hash.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("x"), b"x").unwrap();
        let h = compute_folder_hash(tmp.path()).unwrap();

        // Simulate two SkillLockEntries pointing at same folder with different timestamps.
        let mut a = mk_entry("s", "snap");
        a.computed_hash = h.clone();
        a.installed_at = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut b = a.clone();
        b.installed_at = chrono::DateTime::parse_from_rfc3339("2030-12-31T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        assert_eq!(
            a.computed_hash, b.computed_hash,
            "computed_hash depends only on folder contents"
        );
    }
}

// ============================================================================
// Migration from Phase 19.1 HubManifest (D-15)
// ============================================================================

/// D-15 migration outcome — caller logs, but never treats `NothingToMigrate` as an error.
#[derive(Debug, Clone, PartialEq)]
pub enum MigrationOutcome {
    NothingToMigrate,
    /// Count of entries migrated.
    Migrated(usize),
}

/// Idempotent one-way migration of `$HERMES_HOME/skills/.hub/lock.json` (19.1 `HubManifest`)
/// into `$HERMES_HOME/skills-lock.json` (21.8 `SkillLock`).
///
/// Guards:
///   1. If `skills-lock.json` exists AND has ≥1 entry → `NothingToMigrate` (idempotent).
///   2. If `.hub/lock.json` does not exist → `NothingToMigrate`.
///   3. On write failure → leave both files; `Err` propagated; re-runs on next call.
///
/// Cleanup: after successful `save_atomic`, delete `.hub/lock.json`; `rmdir .hub/` if empty.
pub fn migrate_from_hub_manifest() -> anyhow::Result<MigrationOutcome> {
    // Guard 1: skip if new lock already has content.
    let new_lock = SkillLock::load_or_default()?;
    if !new_lock.skills.is_empty() {
        return Ok(MigrationOutcome::NothingToMigrate);
    }

    // Guard 2: no old manifest to migrate.
    let manifest_path = crate::paths::manifest_path()?;
    if !manifest_path.exists() {
        return Ok(MigrationOutcome::NothingToMigrate);
    }

    // Read old manifest (one-way).
    let raw = std::fs::read_to_string(&manifest_path)?;
    let old: crate::manifest::HubManifest = serde_json::from_str(&raw)?;
    let count = old.installed.len();

    // Map each old entry → SkillLockEntry.
    let mut new = SkillLock::default();
    for (_key, entry) in old.installed.into_iter() {
        let repo_path = entry.files.first().cloned().unwrap_or_default();
        new.add_or_replace(SkillLockEntry {
            name: entry.name,
            source: entry.source,
            identifier: entry.identifier,
            repo_path,
            // backfilled on next `hermes skills update`
            snapshot_hash: String::new(),
            // carried from 19.1
            computed_hash: entry.content_hash,
            installed_at: entry.installed_at,
            extras: entry.extras,
        });
    }

    // Atomic write of new file.
    new.save_atomic()?;

    // Cleanup old manifest (only after successful write).
    let _ = std::fs::remove_file(&manifest_path);
    if let Some(parent) = manifest_path.parent() {
        // rmdir — succeeds only if empty
        let _ = std::fs::remove_dir(parent);
    }

    Ok(MigrationOutcome::Migrated(count))
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    use chrono::Utc;
    use std::path::PathBuf;
    use std::sync::Mutex;
    static ENV_LOCK_MIG: Mutex<()> = Mutex::new(());

    fn with_test_hermes_home<F: FnOnce(&std::path::Path)>(f: F) {
        let _guard = ENV_LOCK_MIG.lock().unwrap_or_else(|e| e.into_inner());
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

    fn write_old_manifest(home: &std::path::Path, entries: Vec<(&str, &str)>) {
        let hub_dir = home.join("skills").join(".hub");
        std::fs::create_dir_all(&hub_dir).unwrap();
        let mut m = crate::manifest::HubManifest::default();
        for (name, id) in entries {
            m.installed.insert(
                name.to_string(),
                crate::manifest::ManifestEntry {
                    name: name.to_string(),
                    source: "skills-sh".to_string(),
                    identifier: id.to_string(),
                    content_hash: "old-hash".to_string(),
                    scan_verdict: "clean".to_string(),
                    install_path: PathBuf::from("/x"),
                    files: vec![format!("skills/{name}/SKILL.md")],
                    installed_at: Utc::now(),
                    updated_at: None,
                    metadata: serde_json::Value::Null,
                    extras: Default::default(),
                },
            );
        }
        std::fs::write(
            hub_dir.join("lock.json"),
            serde_json::to_string_pretty(&m).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn migrates_entries_and_deletes_old_manifest() {
        with_test_hermes_home(|home| {
            write_old_manifest(home, vec![("zeta", "z"), ("alpha", "a")]);
            let outcome = migrate_from_hub_manifest().expect("migrate ok");
            assert_eq!(outcome, MigrationOutcome::Migrated(2));

            let new = SkillLock::load_or_default().unwrap();
            assert_eq!(new.skills.len(), 2);
            assert_eq!(new.skills[0].name, "alpha");
            assert_eq!(new.skills[1].name, "zeta");
            assert_eq!(new.skills[0].computed_hash, "old-hash");
            assert_eq!(new.skills[0].snapshot_hash, "");

            // Old manifest gone.
            assert!(!home.join("skills").join(".hub").join("lock.json").exists());
        });
    }

    #[test]
    fn idempotent_second_run_is_noop() {
        with_test_hermes_home(|home| {
            write_old_manifest(home, vec![("a", "a")]);
            let _ = migrate_from_hub_manifest().unwrap();
            let snapshot = std::fs::read_to_string(home.join("skills-lock.json")).unwrap();

            // Second run must not touch the file and must report NothingToMigrate.
            let outcome2 = migrate_from_hub_manifest().unwrap();
            assert_eq!(outcome2, MigrationOutcome::NothingToMigrate);

            let after = std::fs::read_to_string(home.join("skills-lock.json")).unwrap();
            assert_eq!(
                snapshot, after,
                "file must be byte-identical after idempotent 2nd run"
            );
        });
    }

    #[test]
    fn no_source_returns_nothing_to_migrate() {
        with_test_hermes_home(|_home| {
            let outcome = migrate_from_hub_manifest().unwrap();
            assert_eq!(outcome, MigrationOutcome::NothingToMigrate);
        });
    }
}

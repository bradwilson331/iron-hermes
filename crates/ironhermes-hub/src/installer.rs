//! Skill install / update / uninstall pipeline (D-09, D-10, D-11).
//!
//! Ports the five-step atomic install from `~/code/hermes-agent/tools/skills_hub.py`:
//!   fetch -> quarantine -> scan -> atomic move -> manifest
//!
//! Failed installs leave no partial state in `skills/`; the quarantine `TempDir`
//! is cleaned up on drop.
//!
//! Security mitigations:
//! - D-11: quarantine isolation — bundle never written directly to final location
//! - D-15: trust-gated scan enforcement via `enforce_trust_gate`
//! - Pitfall 2: quarantine under `.hub/quarantine/` (same FS as final dest)

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::error::{HubError, HubErrorKind};
use crate::manifest::{HubManifest, ManifestEntry};
use crate::scanner::{enforce_trust_gate, SkillScanner};
use crate::source::{HubSource, SkillBundle};

// ── Install outcome ─────────────────────────────────────────────────────────

/// Result of a successful install.
#[derive(Debug)]
pub struct InstallOutcome {
    pub name: String,
    pub install_path: PathBuf,
    pub content_hash: String,
    pub scan_verdict: String,
    pub trust_level: ironhermes_core::SkillSource,
}

/// Result of a successful update.
#[derive(Debug)]
pub struct UpdateOutcome {
    pub name: String,
    pub install_path: PathBuf,
    pub old_hash: String,
    pub new_hash: String,
    pub scan_verdict: String,
}

/// Result of a successful uninstall.
#[derive(Debug)]
pub struct UninstallOutcome {
    pub name: String,
    pub removed_path: PathBuf,
}

// ── Content hash ────────────────────────────────────────────────────────────

/// Compute a deterministic SHA-256 hash over the bundle's files.
///
/// Matches Python `bundle_content_hash`: sort files by path, then feed
/// `path_bytes + 0x00 + content_bytes` for each file into the hasher.
pub fn bundle_content_hash(bundle: &SkillBundle) -> String {
    let mut hasher = Sha256::new();

    let mut sorted: Vec<_> = bundle.files.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));

    for file in &sorted {
        hasher.update(file.path.as_bytes());
        hasher.update(&[0x00]);
        hasher.update(&file.bytes);
    }

    hex::encode(hasher.finalize())
}

// ── Skill name / category parsing ───────────────────────────────────────────

/// Parse category and skill name from the SKILL.md frontmatter or identifier.
///
/// If the SKILL.md frontmatter has `metadata.hermes.category`, use that.
/// Otherwise derive from the identifier path structure.
/// Returns `(category, name)`.
fn parse_skill_identity(bundle: &SkillBundle) -> (String, String) {
    // Try to extract category from SKILL.md frontmatter metadata.hermes.category
    let category = extract_category_from_frontmatter(&bundle.skill_md)
        .unwrap_or_else(|| "general".to_string());

    (category, bundle.name.clone())
}

/// Extract `metadata.hermes.category` from SKILL.md frontmatter YAML.
fn extract_category_from_frontmatter(skill_md: &str) -> Option<String> {
    let trimmed = skill_md.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    // Find the closing ---
    let after_start = &trimmed[3..];
    let end = after_start.find("\n---")?;
    let yaml_block = &after_start[..end];

    let doc: serde_yaml::Value = serde_yaml::from_str(yaml_block).ok()?;
    doc.get("metadata")?
        .get("hermes")?
        .get("category")?
        .as_str()
        .map(|s| s.to_string())
}

// ── Install pipeline ────────────────────────────────────────────────────────

/// Five-step atomic install pipeline (D-11):
///
/// 1. **Fetch** — call `source.fetch(identifier)` to get the `SkillBundle`
/// 2. **Quarantine** — write bundle to a tempdir under `.hub/quarantine/`
/// 3. **Scan** — run the skill scanner and apply D-15 trust enforcement
/// 4. **Atomic move** — `rename` (or copy+remove) from quarantine to final path
/// 5. **Manifest** — record the install in `lock.json`
///
/// On failure at any step, the quarantine `TempDir` is dropped (auto-cleaned)
/// and no state is left in `skills/`.
pub async fn install(
    source: &dyn HubSource,
    identifier: &str,
    scanner: &dyn SkillScanner,
    skills_root: &Path,
) -> Result<InstallOutcome, HubError> {
    // ── Step 1: Fetch ───────────────────────────────────────────────────────
    let bundle = source.fetch(identifier).await?;
    let content_hash = bundle_content_hash(&bundle);

    // ── Step 2: Quarantine ──────────────────────────────────────────────────
    let quarantine_root = crate::paths::quarantine_dir()?;
    std::fs::create_dir_all(&quarantine_root)?;
    let quarantine = tempfile::tempdir_in(&quarantine_root)?;
    write_bundle_to_dir(quarantine.path(), &bundle)?;

    // ── Step 3: Scan (D-15 trust-gated enforcement) ─────────────────────────
    let trust = source.trust_level_for(identifier);
    let verdict = scanner.scan_bundle(&bundle.files);
    enforce_trust_gate(trust, &verdict)?;

    // ── Step 4: Atomic move ─────────────────────────────────────────────────
    let (category, name) = parse_skill_identity(&bundle);
    let final_path = skills_root.join(&category).join(&name);

    if final_path.exists() {
        return Err(HubError::Typed {
            kind: HubErrorKind::AlreadyInstalled,
            message: format!("skill '{}' is already installed at {}", name, final_path.display()),
            suggestion: Some(format!(
                "Run 'hermes skills update {}' to update, or 'hermes skills uninstall {}' first.",
                name, name
            )),
            retry_after_s: None,
        });
    }

    std::fs::create_dir_all(final_path.parent().unwrap_or(skills_root))?;
    atomic_move(quarantine.path(), &final_path)?;

    // Prevent TempDir destructor from trying to remove the now-moved directory
    // by consuming it without running cleanup.
    let _ = quarantine.keep();

    // ── Step 5: Manifest ────────────────────────────────────────────────────
    let scan_summary = verdict.summary();
    let mut manifest = HubManifest::load_or_default()?;
    manifest.installed.insert(
        name.clone(),
        ManifestEntry {
            name: name.clone(),
            source: source.source_id().to_string(),
            identifier: identifier.to_string(),
            content_hash: content_hash.clone(),
            scan_verdict: scan_summary.clone(),
            install_path: final_path.clone(),
            files: bundle.files.iter().map(|f| f.path.clone()).collect(),
            installed_at: Utc::now(),
            updated_at: None,
            metadata: bundle.metadata.clone(),
            extras: HashMap::new(),
        },
    );
    manifest.save()?;

    Ok(InstallOutcome {
        name,
        install_path: final_path,
        content_hash,
        scan_verdict: scan_summary,
        trust_level: trust,
    })
}

// ── Update pipeline ─────────────────────────────────────────────────────────

/// Update a previously installed skill (D-10).
///
/// 1. Look up the existing manifest entry
/// 2. Fetch the latest version from the same source
/// 3. Compare content hashes — if identical, no-op
/// 4. If different: quarantine -> scan -> atomic replace -> update manifest
pub async fn update(
    source: &dyn HubSource,
    skill_name: &str,
    scanner: &dyn SkillScanner,
    skills_root: &Path,
) -> Result<UpdateOutcome, HubError> {
    let manifest = HubManifest::load_or_default()?;
    let entry = manifest.installed.get(skill_name).ok_or_else(|| HubError::Typed {
        kind: HubErrorKind::NotFound,
        message: format!("skill '{}' is not installed", skill_name),
        suggestion: Some(format!("Run 'hermes skills list' to see installed skills.")),
        retry_after_s: None,
    })?;

    let old_hash = entry.content_hash.clone();
    let identifier = entry.identifier.clone();
    let install_path = entry.install_path.clone();

    // Fetch latest
    let bundle = source.fetch(&identifier).await?;
    let new_hash = bundle_content_hash(&bundle);

    // Hash-drift detection: if hashes match, nothing to do
    if old_hash == new_hash {
        return Err(HubError::Typed {
            kind: HubErrorKind::AlreadyInstalled,
            message: format!("skill '{}' is already up to date (hash: {})", skill_name, old_hash.get(..12).unwrap_or(&old_hash)),
            suggestion: None,
            retry_after_s: None,
        });
    }

    // Quarantine new version
    let quarantine_root = crate::paths::quarantine_dir()?;
    std::fs::create_dir_all(&quarantine_root)?;
    let quarantine = tempfile::tempdir_in(&quarantine_root)?;
    write_bundle_to_dir(quarantine.path(), &bundle)?;

    // Re-scan (gives immediate feedback per resolved open question #1)
    let trust = source.trust_level_for(&identifier);
    let verdict = scanner.scan_bundle(&bundle.files);
    enforce_trust_gate(trust, &verdict)?;

    // Atomic replace: remove old, move new
    if install_path.exists() {
        std::fs::remove_dir_all(&install_path)?;
    }
    std::fs::create_dir_all(install_path.parent().unwrap_or(skills_root))?;
    atomic_move(quarantine.path(), &install_path)?;
    let _ = quarantine.keep();

    // Update manifest
    let scan_summary = verdict.summary();
    let mut manifest = HubManifest::load_or_default()?;
    if let Some(entry) = manifest.installed.get_mut(skill_name) {
        entry.content_hash = new_hash.clone();
        entry.scan_verdict = scan_summary.clone();
        entry.files = bundle.files.iter().map(|f| f.path.clone()).collect();
        entry.updated_at = Some(Utc::now());
        entry.metadata = bundle.metadata.clone();
    }
    manifest.save()?;

    Ok(UpdateOutcome {
        name: skill_name.to_string(),
        install_path,
        old_hash,
        new_hash,
        scan_verdict: scan_summary,
    })
}

// ── Uninstall ───────────────────────────────────────────────────────────────

/// Remove an installed skill: delete directory + manifest entry atomically.
///
/// Removes the manifest entry first so that if the directory removal fails,
/// the skill is at least de-registered (orphan cleanup can handle the dir later).
pub fn uninstall(skill_name: &str) -> Result<UninstallOutcome, HubError> {
    let mut manifest = HubManifest::load_or_default()?;
    let entry = manifest.installed.remove(skill_name).ok_or_else(|| HubError::Typed {
        kind: HubErrorKind::NotFound,
        message: format!("skill '{}' is not installed", skill_name),
        suggestion: Some("Run 'hermes skills list' to see installed skills.".to_string()),
        retry_after_s: None,
    })?;

    let install_path = entry.install_path.clone();

    // Save manifest first (de-register before dir removal)
    manifest.save()?;

    // Remove the skill directory
    if install_path.exists() {
        std::fs::remove_dir_all(&install_path).map_err(|e| HubError::Typed {
            kind: HubErrorKind::Io,
            message: format!(
                "failed to remove skill directory {}: {}",
                install_path.display(),
                e
            ),
            suggestion: Some(format!(
                "Skill '{}' has been de-registered from the manifest. \
                 Manually remove {} if needed.",
                skill_name,
                install_path.display()
            )),
            retry_after_s: None,
        })?;
    }

    // Clean up empty parent category dir if it's now empty
    if let Some(parent) = install_path.parent() {
        if parent.exists() {
            if let Ok(mut entries) = std::fs::read_dir(parent) {
                if entries.next().is_none() {
                    let _ = std::fs::remove_dir(parent);
                }
            }
        }
    }

    Ok(UninstallOutcome {
        name: skill_name.to_string(),
        removed_path: install_path,
    })
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Write all files from a bundle into a directory.
///
/// Defense-in-depth: re-validates each file path even though `extract_tarball_prefix`
/// already checked during extraction. This guards against `SkillBundle` structs
/// constructed by future code paths that bypass tarball extraction.
fn write_bundle_to_dir(dir: &Path, bundle: &SkillBundle) -> Result<(), HubError> {
    for file in &bundle.files {
        // Re-validate path components (defense-in-depth against traversal)
        let _ = crate::tarball::validate_bundle_rel_path(&file.path)?;
        let dest = dir.join(&file.path);
        // Verify the resolved dest is still under dir (canonicalization guard)
        if !dest.starts_with(dir) {
            return Err(HubError::Typed {
                kind: HubErrorKind::Parse,
                message: format!("path escapes target directory: {}", file.path),
                suggestion: None,
                retry_after_s: None,
            });
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, &file.bytes)?;
    }
    Ok(())
}

/// Atomic move: try `rename` first, fall back to recursive copy + remove.
///
/// `rename` only works within the same filesystem (Pitfall 2).  The quarantine
/// lives under `.hub/quarantine/` which is the same FS as the skills root,
/// so `rename` should succeed in normal operation.  The fallback handles edge
/// cases (e.g. bind-mounted `/tmp` in containers).
fn atomic_move(src: &Path, dst: &Path) -> Result<(), HubError> {
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_rename_err) => {
            // Fallback: recursive copy then remove source
            copy_dir_recursive(src, dst)?;
            std::fs::remove_dir_all(src)?;
            Ok(())
        }
    }
}

/// Recursively copy a directory tree.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), HubError> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::BundleFile;

    #[test]
    fn test_bundle_content_hash_deterministic() {
        let bundle = SkillBundle {
            name: "test".to_string(),
            identifier: "owner/repo/test".to_string(),
            source_id: "github".to_string(),
            files: vec![
                BundleFile {
                    path: "SKILL.md".to_string(),
                    bytes: b"---\nname: test\n---\nbody".to_vec(),
                },
                BundleFile {
                    path: "handler.py".to_string(),
                    bytes: b"# code".to_vec(),
                },
            ],
            skill_md: "---\nname: test\n---\nbody".to_string(),
            metadata: serde_json::json!({}),
        };
        let h1 = bundle_content_hash(&bundle);
        let h2 = bundle_content_hash(&bundle);
        assert_eq!(h1, h2, "hash must be deterministic");
        assert_eq!(h1.len(), 64, "SHA-256 hex digest is 64 chars");
    }

    #[test]
    fn test_bundle_content_hash_sorted_by_path() {
        // Same files in different order must produce the same hash
        let files_a = vec![
            BundleFile { path: "a.txt".to_string(), bytes: b"aaa".to_vec() },
            BundleFile { path: "b.txt".to_string(), bytes: b"bbb".to_vec() },
        ];
        let files_b = vec![
            BundleFile { path: "b.txt".to_string(), bytes: b"bbb".to_vec() },
            BundleFile { path: "a.txt".to_string(), bytes: b"aaa".to_vec() },
        ];
        let bundle_a = SkillBundle {
            name: "x".into(), identifier: "x".into(), source_id: "x".into(),
            files: files_a, skill_md: String::new(), metadata: serde_json::json!({}),
        };
        let bundle_b = SkillBundle {
            name: "x".into(), identifier: "x".into(), source_id: "x".into(),
            files: files_b, skill_md: String::new(), metadata: serde_json::json!({}),
        };
        assert_eq!(bundle_content_hash(&bundle_a), bundle_content_hash(&bundle_b));
    }

    #[test]
    fn test_bundle_content_hash_differs_on_content_change() {
        let mk = |data: &[u8]| SkillBundle {
            name: "x".into(), identifier: "x".into(), source_id: "x".into(),
            files: vec![BundleFile { path: "f.txt".into(), bytes: data.to_vec() }],
            skill_md: String::new(), metadata: serde_json::json!({}),
        };
        assert_ne!(
            bundle_content_hash(&mk(b"hello")),
            bundle_content_hash(&mk(b"world"))
        );
    }

    #[test]
    fn test_extract_category_from_frontmatter() {
        let md = "---\nname: test\nmetadata:\n  hermes:\n    category: automation\n---\nbody";
        assert_eq!(extract_category_from_frontmatter(md), Some("automation".to_string()));
    }

    #[test]
    fn test_extract_category_missing_defaults_to_none() {
        let md = "---\nname: test\n---\nbody";
        assert_eq!(extract_category_from_frontmatter(md), None);
    }

    #[test]
    fn test_parse_skill_identity_with_category() {
        let bundle = SkillBundle {
            name: "my-skill".into(),
            identifier: "owner/repo/my-skill".into(),
            source_id: "github".into(),
            files: vec![],
            skill_md: "---\nname: my-skill\nmetadata:\n  hermes:\n    category: devops\n---\n".into(),
            metadata: serde_json::json!({}),
        };
        let (cat, name) = parse_skill_identity(&bundle);
        assert_eq!(cat, "devops");
        assert_eq!(name, "my-skill");
    }

    #[test]
    fn test_parse_skill_identity_defaults_to_general() {
        let bundle = SkillBundle {
            name: "my-skill".into(),
            identifier: "owner/repo/my-skill".into(),
            source_id: "github".into(),
            files: vec![],
            skill_md: "---\nname: my-skill\n---\n".into(),
            metadata: serde_json::json!({}),
        };
        let (cat, name) = parse_skill_identity(&bundle);
        assert_eq!(cat, "general");
        assert_eq!(name, "my-skill");
    }

    #[test]
    fn test_write_bundle_to_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle = SkillBundle {
            name: "test".into(),
            identifier: "x".into(),
            source_id: "x".into(),
            files: vec![
                BundleFile { path: "SKILL.md".into(), bytes: b"# skill".to_vec() },
                BundleFile { path: "sub/handler.py".into(), bytes: b"# code".to_vec() },
            ],
            skill_md: "# skill".into(),
            metadata: serde_json::json!({}),
        };
        write_bundle_to_dir(tmp.path(), &bundle).unwrap();
        assert!(tmp.path().join("SKILL.md").exists());
        assert!(tmp.path().join("sub/handler.py").exists());
        assert_eq!(std::fs::read_to_string(tmp.path().join("SKILL.md")).unwrap(), "# skill");
    }

    #[test]
    fn test_atomic_move_same_fs() {
        let parent = tempfile::tempdir().unwrap();
        let src = parent.path().join("src_dir");
        let dst = parent.path().join("dst_dir");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join("file.txt"), "hello").unwrap();

        atomic_move(&src, &dst).unwrap();

        assert!(!src.exists(), "source should be gone after move");
        assert!(dst.join("file.txt").exists());
        assert_eq!(std::fs::read_to_string(dst.join("file.txt")).unwrap(), "hello");
    }

    #[test]
    fn test_copy_dir_recursive() {
        let parent = tempfile::tempdir().unwrap();
        let src = parent.path().join("src");
        let dst = parent.path().join("dst");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), "aaa").unwrap();
        std::fs::write(src.join("sub/b.txt"), "bbb").unwrap();

        copy_dir_recursive(&src, &dst).unwrap();

        assert_eq!(std::fs::read_to_string(dst.join("a.txt")).unwrap(), "aaa");
        assert_eq!(std::fs::read_to_string(dst.join("sub/b.txt")).unwrap(), "bbb");
    }
}

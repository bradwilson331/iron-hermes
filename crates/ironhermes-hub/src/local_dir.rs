//! Local directory skills source adapter.
//!
//! Installs skills from a local filesystem directory via `local:<path>` identifier.
//!
//! Security mitigations:
//! - D-A2: path canonicalized via `std::fs::canonicalize()` before any I/O (CLI layer)
//! - D-B2: symlinks inside source dir are never followed (file_type.is_symlink() skip)
//! - D-17: SKILL.md frontmatter delimiter check via `sanitize::strict_yaml_delimiter`
//! - D-18: `sanitize_subpath` applied per-file inside `write_bundle_to_dir` (downstream)

use std::path::Path;

use async_trait::async_trait;
use ironhermes_core::SkillSource;

use crate::error::{HubError, HubErrorKind};
use crate::sanitize;
use crate::source::{BundleFile, HubSource, SkillBundle, SkillMeta};

// ── Helper ───────────────────────────────────────────────────────────────────

fn typed(kind: HubErrorKind, msg: impl Into<String>) -> HubError {
    HubError::Typed {
        kind,
        message: msg.into(),
        suggestion: None,
        retry_after_s: None,
    }
}

// ── Adapter ──────────────────────────────────────────────────────────────────

/// Adapter for installing skills from a local filesystem directory.
///
/// Stateless unit struct — no constructor arguments. Wired into
/// `build_sources()` (in `ironhermes-cli`) as a constant `Box::new(LocalDirSource)`.
pub struct LocalDirSource;

#[async_trait]
impl HubSource for LocalDirSource {
    fn source_id(&self) -> &str {
        "local-dir"
    }

    /// D-B2: local installs are user-authored content from the operator's own
    /// filesystem; trust tier is always `Trusted`. The scanner runs with the
    /// Phase 19 D-15 WARN-BUT-LOAD posture (Trusted is not a Community-tier
    /// hard reject) — that posture is enforced by `enforce_trust_gate`
    /// downstream, not here.
    fn trust_level_for(&self, _identifier: &str) -> SkillSource {
        SkillSource::Trusted
    }

    async fn search(&self, _query: &str, _limit: usize) -> Result<Vec<SkillMeta>, HubError> {
        // Local directories are not searchable. Returning an empty vec keeps
        // the trait surface uniform without forcing callers to special-case.
        Ok(vec![])
    }

    async fn fetch(&self, identifier: &str) -> Result<SkillBundle, HubError> {
        // Caller (CLI layer) is responsible for canonicalization (D-A2).
        // We accept whatever path string is passed and re-validate is_dir.
        let base = Path::new(identifier);
        if !base.is_dir() {
            return Err(typed(
                HubErrorKind::LocalSourceMissing,
                format!("source dir does not exist or is not a directory: {identifier}"),
            ));
        }

        let mut files: Vec<(String, Vec<u8>)> = Vec::new();
        walk_source_dir(base, base, &mut files)?;
        files.sort_by(|a, b| a.0.cmp(&b.0));

        // Find SKILL.md (root or nested).
        let skill_md_entry = files
            .iter()
            .find(|(p, _)| p == "SKILL.md" || p.ends_with("/SKILL.md"))
            .ok_or_else(|| {
                typed(
                    HubErrorKind::Parse,
                    format!("no SKILL.md found in {identifier}"),
                )
            })?;

        let skill_md_content = String::from_utf8_lossy(&skill_md_entry.1).into_owned();

        // D-17 carry-forward: strict YAML delimiter check before bundle returns.
        sanitize::strict_yaml_delimiter(&skill_md_content)?;

        // Skill name: prefer frontmatter `name:` field; fall back to source dir basename.
        let name = parse_skill_name_from_frontmatter(&skill_md_content)
            .or_else(|| {
                base.file_name()
                    .map(|n| sanitize::sanitize_name(&n.to_string_lossy()))
            })
            .unwrap_or_else(|| "unnamed-skill".to_string());

        let bundle_files: Vec<BundleFile> = files
            .into_iter()
            .map(|(path, bytes)| BundleFile { path, bytes })
            .collect();

        Ok(SkillBundle {
            name,
            identifier: identifier.to_string(),
            source_id: "local-dir".to_string(),
            files: bundle_files,
            skill_md: skill_md_content,
            metadata: serde_json::json!({}),
            snapshot_hash: None, // D-C1: no remote snapshot for local installs
        })
    }
}

// ── Directory walker ─────────────────────────────────────────────────────────

/// Recursively walk `dir` under `base`, collecting relative forward-slash paths
/// and file bytes into `out`.
///
/// Mirrors `lock.rs::walk` verbatim with one addition: `target/` directories
/// (Rust build artifacts common in developer skill dirs) are also skipped.
///
/// Skip rules:
/// - Symlinks: never followed (T-21.8.1-02 mitigation)
/// - `.git`, `node_modules`, `target`: skipped entirely
fn walk_source_dir(
    base: &Path,
    dir: &Path,
    out: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), HubError> {
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
            continue; // T-21.8.1-02: defense against symlink loops / escapes
        }
        if file_type.is_dir() {
            if name_str == ".git" || name_str == "node_modules" || name_str == "target" {
                continue;
            }
            walk_source_dir(base, &path, out)?;
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

// ── Frontmatter helpers ───────────────────────────────────────────────────────

/// Parse the `name:` field from SKILL.md YAML frontmatter.
///
/// Mirrors `github.rs::GitHubSource::parse_frontmatter_name` — hand-rolled
/// line-scan avoids pulling in `serde_yaml` for a single scalar field.
/// Returns `None` if the content has no leading `---` or no `name:` line.
fn parse_skill_name_from_frontmatter(content: &str) -> Option<String> {
    let mut lines = content.lines();
    if lines.next()? != "---" {
        return None;
    }
    for line in lines {
        if line == "---" {
            return None;
        }
        if let Some(rest) = line.strip_prefix("name:") {
            let name = rest.trim().trim_matches('"').trim_matches('\'').to_string();
            if !name.is_empty() {
                return Some(sanitize::sanitize_name(&name));
            }
        }
    }
    None
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Trait surface tests (synchronous) ─────────────────────────────────────

    #[test]
    fn local_dir_source_id_is_local_dir() {
        let src = LocalDirSource;
        assert_eq!(src.source_id(), "local-dir");
    }

    #[test]
    fn local_dir_trust_level_is_trusted() {
        let src = LocalDirSource;
        assert_eq!(src.trust_level_for("/any/path"), SkillSource::Trusted);
        assert_eq!(src.trust_level_for(""), SkillSource::Trusted);
    }

    #[tokio::test]
    async fn local_dir_search_returns_empty() {
        let src = LocalDirSource;
        let result = src.search("anything", 100).await.unwrap();
        assert!(result.is_empty());
    }

    // ── fetch() error path tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn local_dir_fetch_missing_dir_returns_local_source_missing() {
        let src = LocalDirSource;
        let err = src.fetch("/nonexistent/path/that/does/not/exist").await.unwrap_err();
        match err {
            HubError::Typed {
                kind: HubErrorKind::LocalSourceMissing,
                ..
            } => {} // expected
            other => panic!("expected LocalSourceMissing, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn local_dir_fetch_no_skill_md_returns_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), b"# hello").unwrap();
        let src = LocalDirSource;
        let err = src.fetch(dir.path().to_str().unwrap()).await.unwrap_err();
        match err {
            HubError::Typed {
                kind: HubErrorKind::Parse,
                ref message,
                ..
            } => {
                assert!(message.contains("no SKILL.md found"), "message: {message}");
            }
            other => panic!("expected Parse, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn local_dir_fetch_invalid_frontmatter_rejected() {
        let dir = tempfile::tempdir().unwrap();
        // ---js delimiter — D-17 violation
        std::fs::write(
            dir.path().join("SKILL.md"),
            b"---js\nname: x\n---\n# content\n",
        )
        .unwrap();
        let src = LocalDirSource;
        let err = src.fetch(dir.path().to_str().unwrap()).await.unwrap_err();
        match err {
            HubError::Typed {
                kind: HubErrorKind::Parse,
                ..
            } => {} // expected from strict_yaml_delimiter
            other => panic!("expected Parse from strict_yaml_delimiter, got {:?}", other),
        }
    }

    // ── fetch() happy-path tests ──────────────────────────────────────────────

    fn make_valid_skill_md() -> &'static [u8] {
        b"---\nname: my-skill\n---\n# My Skill\nDoes things.\n"
    }

    #[tokio::test]
    async fn local_dir_fetch_walks_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("SKILL.md"), make_valid_skill_md()).unwrap();
        std::fs::create_dir(dir.path().join("helpers")).unwrap();
        std::fs::write(dir.path().join("helpers").join("script.sh"), b"#!/bin/sh").unwrap();
        std::fs::create_dir(dir.path().join("references")).unwrap();
        std::fs::write(
            dir.path().join("references").join("note.md"),
            b"# notes",
        )
        .unwrap();

        let src = LocalDirSource;
        let bundle = src.fetch(dir.path().to_str().unwrap()).await.unwrap();
        assert_eq!(bundle.files.len(), 3);
        // Verify alphabetical order
        let paths: Vec<&str> = bundle.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["SKILL.md", "helpers/script.sh", "references/note.md"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_dir_fetch_skips_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("SKILL.md"), make_valid_skill_md()).unwrap();
        // Create a symlink pointing at /etc (outside the source dir)
        std::os::unix::fs::symlink("/etc", dir.path().join("outside_link")).unwrap();

        let src = LocalDirSource;
        let bundle = src.fetch(dir.path().to_str().unwrap()).await.unwrap();
        assert_eq!(bundle.files.len(), 1, "symlink must be skipped");
        assert_eq!(bundle.files[0].path, "SKILL.md");
    }

    #[tokio::test]
    async fn local_dir_fetch_skips_git_node_modules_target() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("SKILL.md"), make_valid_skill_md()).unwrap();
        // Create the skip directories with files inside
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git").join("HEAD"), b"ref: refs/heads/main").unwrap();
        std::fs::create_dir(dir.path().join("node_modules")).unwrap();
        std::fs::write(dir.path().join("node_modules").join("foo.js"), b"var x=1").unwrap();
        std::fs::create_dir(dir.path().join("target")).unwrap();
        std::fs::create_dir(dir.path().join("target").join("debug")).unwrap();
        std::fs::write(dir.path().join("target").join("debug").join("x"), b"binary").unwrap();

        let src = LocalDirSource;
        let bundle = src.fetch(dir.path().to_str().unwrap()).await.unwrap();
        assert_eq!(bundle.files.len(), 1, "only SKILL.md should be included");
        assert_eq!(bundle.files[0].path, "SKILL.md");
    }

    #[tokio::test]
    async fn local_dir_fetch_bundle_snapshot_hash_is_none() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("SKILL.md"), make_valid_skill_md()).unwrap();
        let src = LocalDirSource;
        let bundle = src.fetch(dir.path().to_str().unwrap()).await.unwrap();
        assert_eq!(bundle.snapshot_hash, None, "D-C1: no remote snapshot for local installs");
    }

    #[tokio::test]
    async fn local_dir_fetch_bundle_source_id_is_local_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("SKILL.md"), make_valid_skill_md()).unwrap();
        let src = LocalDirSource;
        let identifier = dir.path().to_str().unwrap();
        let bundle = src.fetch(identifier).await.unwrap();
        assert_eq!(bundle.source_id, "local-dir");
        // fetch does NOT canonicalize internally — identifier is stored verbatim (RULE 5)
        assert_eq!(bundle.identifier, identifier);
    }

    #[tokio::test]
    async fn local_dir_fetch_returns_path_traversal_safe_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("SKILL.md"), make_valid_skill_md()).unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("file.md"), b"# content").unwrap();

        let src = LocalDirSource;
        let bundle = src.fetch(dir.path().to_str().unwrap()).await.unwrap();
        for file in &bundle.files {
            assert!(
                !file.path.contains(".."),
                "path must not contain '..': {}",
                file.path
            );
            assert!(
                !file.path.starts_with('/'),
                "path must not be absolute: {}",
                file.path
            );
            assert!(
                !file.path.contains('\\'),
                "path must use forward slashes: {}",
                file.path
            );
        }
    }
}

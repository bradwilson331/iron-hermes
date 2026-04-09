use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::debug;

use crate::constants::get_hermes_home;

// =============================================================================
// SkillFrontmatter
// =============================================================================

#[derive(Debug, Clone, Deserialize)]
pub struct SkillFrontmatter {
    pub name: String,
    pub description: String,
    pub version: Option<String>,
    pub author: Option<String>,
    pub license: Option<String>,
}

// =============================================================================
// SkillRecord
// =============================================================================

#[derive(Debug, Clone)]
pub struct SkillRecord {
    pub name: String,
    pub description: String,
    /// Absolute path to the SKILL.md file.
    pub path: PathBuf,
}

// =============================================================================
// parse_skill_md
// =============================================================================

/// Parse a SKILL.md file content into (SkillFrontmatter, body).
///
/// Returns None if:
/// - Content does not start with `---`
/// - YAML frontmatter is invalid
/// - Closing `---` delimiter is missing
pub fn parse_skill_md(content: &str) -> Option<(SkillFrontmatter, String)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }

    // Skip past the opening `---\n`
    let after_open = trimmed.strip_prefix("---")?;
    // Allow `---` followed by `\n` or `\r\n`
    let after_open = after_open.strip_prefix('\n').or_else(|| {
        after_open
            .strip_prefix('\r')
            .and_then(|s| s.strip_prefix('\n'))
    })?;

    // Find the FIRST `\n---` to locate the closing delimiter
    let close_pos = after_open.find("\n---")?;
    let yaml_block = &after_open[..close_pos];

    // Parse the YAML
    let frontmatter: SkillFrontmatter = serde_yaml::from_str(yaml_block).ok()?;

    // Extract body: everything after the closing `\n---`
    let after_close = &after_open[close_pos + 4..]; // skip `\n---`
    // Skip optional trailing newline after the closing delimiter
    let body = after_close
        .strip_prefix('\n')
        .or_else(|| after_close.strip_prefix('\r').and_then(|s| s.strip_prefix('\n')))
        .unwrap_or(after_close)
        .trim()
        .to_string();

    Some((frontmatter, body))
}

// =============================================================================
// SkillRegistry
// =============================================================================

pub struct SkillRegistry {
    skills: Vec<SkillRecord>,
}

impl SkillRegistry {
    /// Scan three priority-ordered paths for skill documents.
    ///
    /// Priority order (first-path-wins deduplication by lowercase name):
    /// 1. `cwd/.ironhermes/skills/`
    /// 2. `~/.ironhermes/skills/` (via get_hermes_home())
    /// 3. `~/.agents/skills/`
    pub fn load(cwd: &Path) -> Self {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")));

        let search_paths: Vec<PathBuf> = vec![
            cwd.join(".ironhermes/skills"),
            get_hermes_home().join("skills"),
            home.join(".agents/skills"),
        ];

        Self::load_with_paths(&search_paths)
    }

    /// Load from explicit search paths (useful for testing).
    pub fn load_with_paths(search_paths: &[PathBuf]) -> Self {

        let mut seen_names: HashSet<String> = HashSet::new();
        let mut skills: Vec<SkillRecord> = Vec::new();

        for search_path in search_paths {
            if !search_path.exists() {
                continue;
            }

            let entries = match std::fs::read_dir(search_path) {
                Ok(e) => e,
                Err(err) => {
                    debug!("SkillRegistry: failed to read dir {:?}: {}", search_path, err);
                    continue;
                }
            };

            for entry in entries.flatten() {
                let subdir = entry.path();
                if !subdir.is_dir() {
                    continue;
                }

                let skill_md_path = subdir.join("SKILL.md");
                if !skill_md_path.exists() {
                    continue;
                }

                let content = match std::fs::read_to_string(&skill_md_path) {
                    Ok(c) => c,
                    Err(err) => {
                        debug!("SkillRegistry: failed to read {:?}: {}", skill_md_path, err);
                        continue;
                    }
                };

                let (frontmatter, _body) = match parse_skill_md(&content) {
                    Some(parsed) => parsed,
                    None => {
                        debug!("SkillRegistry: skipping {:?} — invalid SKILL.md", skill_md_path);
                        continue;
                    }
                };

                let name_lower = frontmatter.name.to_lowercase();
                if seen_names.contains(&name_lower) {
                    debug!(
                        "SkillRegistry: skipping duplicate skill '{}' at {:?}",
                        frontmatter.name, skill_md_path
                    );
                    continue;
                }

                seen_names.insert(name_lower);
                skills.push(SkillRecord {
                    name: frontmatter.name,
                    description: frontmatter.description,
                    path: skill_md_path,
                });
            }
        }

        Self { skills }
    }

    /// Return a compact catalog string: one `- name: description` line per skill.
    pub fn catalog_text(&self) -> String {
        self.skills
            .iter()
            .map(|s| format!("- {}: {}", s.name, s.description))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Case-insensitive lookup by skill name.
    pub fn find(&self, name: &str) -> Option<&SkillRecord> {
        let lower = name.to_lowercase();
        self.skills.iter().find(|s| s.name.to_lowercase() == lower)
    }

    /// Read the body content of a skill by name (disk read, strips frontmatter).
    pub fn read_content(&self, name: &str) -> Option<String> {
        let record = self.find(name)?;
        let content = std::fs::read_to_string(&record.path).ok()?;
        let (_frontmatter, body) = parse_skill_md(&content)?;
        Some(body)
    }

    /// Return a slice of all discovered skills.
    pub fn list(&self) -> &[SkillRecord] {
        &self.skills
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn make_skill_md(name: &str, description: &str, extra_body: &str) -> String {
        format!(
            "---\nname: {}\ndescription: {}\n---\n{}",
            name, description, extra_body
        )
    }

    // -------------------------------------------------------------------------
    // parse_skill_md tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_parse_skill_md_valid_frontmatter() {
        let content = "---\nname: my-skill\ndescription: A test skill\n---\n\nBody content here.";
        let result = parse_skill_md(content);
        assert!(result.is_some());
        let (fm, body) = result.unwrap();
        assert_eq!(fm.name, "my-skill");
        assert_eq!(fm.description, "A test skill");
        assert!(body.contains("Body content here."));
    }

    #[test]
    fn test_parse_skill_md_missing_frontmatter() {
        let content = "No frontmatter here\nJust regular text.";
        let result = parse_skill_md(content);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_skill_md_invalid_yaml() {
        let content = "---\nname: [unclosed bracket\ndescription: test\n---\nBody";
        let result = parse_skill_md(content);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_skill_md_dash_in_body() {
        // Body contains `---` — only the FIRST closing `---` should split
        let content =
            "---\nname: skill\ndescription: desc\n---\nBody content\n---\nMore content after second dashes.";
        let result = parse_skill_md(content);
        assert!(result.is_some());
        let (fm, body) = result.unwrap();
        assert_eq!(fm.name, "skill");
        // Body should include everything after the first closing `\n---`
        assert!(body.contains("Body content"));
        assert!(body.contains("---"));
        assert!(body.contains("More content after second dashes."));
    }

    #[test]
    fn test_parse_skill_md_optional_fields() {
        let content = "---\nname: skill\ndescription: desc\nversion: \"1.0\"\nauthor: Alice\nlicense: MIT\n---\nBody";
        let result = parse_skill_md(content);
        assert!(result.is_some());
        let (fm, _body) = result.unwrap();
        assert_eq!(fm.version.as_deref(), Some("1.0"));
        assert_eq!(fm.author.as_deref(), Some("Alice"));
        assert_eq!(fm.license.as_deref(), Some("MIT"));
    }

    // -------------------------------------------------------------------------
    // SkillRegistry::load tests
    // Use load_with_paths to avoid picking up real skills from ~/.agents/skills
    // -------------------------------------------------------------------------

    fn make_isolated_registry(paths: &[PathBuf]) -> SkillRegistry {
        SkillRegistry::load_with_paths(paths)
    }

    #[test]
    fn test_registry_load_discovers_skills() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_a_dir = skills_dir.join("skill-a");
        fs::create_dir_all(&skill_a_dir).unwrap();
        fs::write(
            skill_a_dir.join("SKILL.md"),
            make_skill_md("skill-a", "Skill A description", "Content A"),
        )
        .unwrap();

        let registry = make_isolated_registry(&[skills_dir]);
        assert_eq!(registry.list().len(), 1);
        assert_eq!(registry.list()[0].name, "skill-a");
    }

    #[test]
    fn test_registry_load_nonexistent_paths_no_panic() {
        let dir = tempdir().unwrap();
        // Pass a path that doesn't exist
        let registry = make_isolated_registry(&[dir.path().join("nonexistent")]);
        assert_eq!(registry.list().len(), 0);
    }

    #[test]
    fn test_registry_load_first_path_wins_dedup() {
        let dir = tempdir().unwrap();

        // First path: has "my-skill" with description "From first path"
        let first_path = dir.path().join("skills-first");
        let first_skill_dir = first_path.join("my-skill");
        fs::create_dir_all(&first_skill_dir).unwrap();
        fs::write(
            first_skill_dir.join("SKILL.md"),
            make_skill_md("my-skill", "From first path", "First body"),
        )
        .unwrap();

        // Second path: has "my-skill" with a different description (should be skipped)
        let second_path = dir.path().join("skills-second");
        let second_skill_dir = second_path.join("my-skill");
        fs::create_dir_all(&second_skill_dir).unwrap();
        fs::write(
            second_skill_dir.join("SKILL.md"),
            make_skill_md("my-skill", "From second path (should be skipped)", "Second body"),
        )
        .unwrap();

        let registry = make_isolated_registry(&[first_path, second_path]);
        // Only one "my-skill" should exist
        let matches: Vec<_> = registry
            .list()
            .iter()
            .filter(|s| s.name == "my-skill")
            .collect();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].description, "From first path");
    }

    #[test]
    fn test_registry_load_skips_invalid_skill_md() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("bad-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "No frontmatter at all").unwrap();

        let registry = make_isolated_registry(&[skills_dir]);
        assert_eq!(registry.list().len(), 0);
    }

    // -------------------------------------------------------------------------
    // catalog_text tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_catalog_text_format() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");

        let skill_a = skills_dir.join("alpha");
        fs::create_dir_all(&skill_a).unwrap();
        fs::write(
            skill_a.join("SKILL.md"),
            make_skill_md("alpha", "Alpha description", ""),
        )
        .unwrap();

        let skill_b = skills_dir.join("beta");
        fs::create_dir_all(&skill_b).unwrap();
        fs::write(
            skill_b.join("SKILL.md"),
            make_skill_md("beta", "Beta description", ""),
        )
        .unwrap();

        let registry = make_isolated_registry(&[skills_dir]);
        let catalog = registry.catalog_text();
        assert!(catalog.contains("- alpha: Alpha description"));
        assert!(catalog.contains("- beta: Beta description"));
    }

    #[test]
    fn test_catalog_text_empty_when_no_skills() {
        let dir = tempdir().unwrap();
        // Pass a path that doesn't exist — guaranteed empty registry
        let registry = make_isolated_registry(&[dir.path().join("no-skills-here")]);
        assert_eq!(registry.catalog_text(), "");
    }

    // -------------------------------------------------------------------------
    // find tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_find_returns_some_case_insensitive() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("MySkill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            make_skill_md("MySkill", "My skill", "Content"),
        )
        .unwrap();

        let registry = make_isolated_registry(&[skills_dir]);
        assert!(registry.find("MySkill").is_some());
        assert!(registry.find("myskill").is_some());
        assert!(registry.find("MYSKILL").is_some());
    }

    #[test]
    fn test_find_returns_none_for_nonexistent() {
        let dir = tempdir().unwrap();
        let registry = make_isolated_registry(&[dir.path().join("no-skills-here")]);
        assert!(registry.find("does-not-exist").is_none());
    }

    // -------------------------------------------------------------------------
    // read_content tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_read_content_returns_body_without_frontmatter() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("my-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            make_skill_md("my-skill", "desc", "This is the body content."),
        )
        .unwrap();

        let registry = make_isolated_registry(&[skills_dir]);
        let content = registry.read_content("my-skill");
        assert!(content.is_some());
        let body = content.unwrap();
        assert!(body.contains("This is the body content."));
        assert!(!body.contains("name: my-skill"));
        assert!(!body.contains("description: desc"));
    }
}

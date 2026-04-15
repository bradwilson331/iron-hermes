use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::config::SkillsConfig;
use crate::constants::get_hermes_home;

// =============================================================================
// Validation (SKILL-07)
// =============================================================================

/// Agentskills.io name regex: lowercase alphanumeric + hyphens,
/// no leading/trailing hyphens. Consecutive hyphens are rejected by a separate
/// `contains("--")` check because `^[a-z0-9]([a-z0-9-]*[a-z0-9])?$` alone would
/// allow `foo--bar`.
///
/// Source: agentskills.io specification; Python reference at
/// `skill_manager_tool.py:102-116` (_validate_name).
static SKILL_NAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-z0-9]([a-z0-9-]*[a-z0-9])?$")
        .expect("SKILL_NAME_RE is a compile-time constant literal — regex must compile")
});

const SKILL_NAME_MIN_LEN: usize = 1;
const SKILL_NAME_MAX_LEN: usize = 64;
const SKILL_DESC_MIN_LEN: usize = 1;
const SKILL_DESC_MAX_LEN: usize = 1024;

/// Validate a skill name. Returns `Err(reason)` if invalid.
///
/// Strict rules (reject on failure):
/// - Length 1..=64
/// - Must match `^[a-z0-9]([a-z0-9-]*[a-z0-9])?$`
/// - Must not contain consecutive hyphens (`--`)
fn validate_skill_name(name: &str) -> Result<(), &'static str> {
    if name.len() < SKILL_NAME_MIN_LEN {
        return Err("name is empty");
    }
    if name.len() > SKILL_NAME_MAX_LEN {
        return Err("name exceeds 64 characters");
    }
    if name.contains("--") {
        return Err("name contains consecutive hyphens");
    }
    if !SKILL_NAME_RE.is_match(name) {
        return Err("name does not match ^[a-z0-9]([a-z0-9-]*[a-z0-9])?$");
    }
    Ok(())
}

// =============================================================================
// HermesMetadata and related types (Phase 19, Plan 01)
// =============================================================================

/// A declared required environment variable with human-readable prompts.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct EnvVarEntry {
    pub name: String,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub help: Option<String>,
    #[serde(default)]
    pub required_for: Option<String>,
}

/// A declared credential file path (relative to HERMES_CREDENTIAL_DIR/<skill-name>/).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum CredentialFileEntry {
    Path(String),
    Structured {
        path: String,
        #[serde(default)]
        description: Option<String>,
    },
}

/// A single config field declared in metadata.hermes.config.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillConfigField {
    pub key: String,
    #[serde(default)]
    pub default: Option<serde_yaml::Value>,
    #[serde(default)]
    pub description: Option<String>,
    /// Type hint: "string" | "boolean" | "integer" | "path" (advisory only in Phase 19)
    #[serde(rename = "type", default)]
    pub field_type: Option<String>,
}

/// Typed representation of metadata.hermes.* (D-17, D-19).
/// Unknown fields are preserved in `extras` (D-18 WARN-BUT-LOAD).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HermesMetadata {
    pub requires_toolsets: Vec<String>,
    pub requires_tools: Vec<String>,
    pub fallback_for_toolsets: Vec<String>,
    pub fallback_for_tools: Vec<String>,
    pub required_environment_variables: Vec<EnvVarEntry>,
    pub required_credential_files: Vec<CredentialFileEntry>,
    pub config: Vec<SkillConfigField>,
    /// Preserve unknown hermes fields for forward compat (D-18).
    #[serde(flatten)]
    pub extras: HashMap<String, serde_yaml::Value>,
}

/// Provenance label used by D-15 scan enforcement (Plan 05).
/// Phase 19 defaults locally-discovered skills to Builtin; Phase 19.1 flips this
/// to Community for hub-installed skills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillSource {
    Builtin,
    Official,
    Community,
}

impl Default for SkillSource {
    fn default() -> Self {
        SkillSource::Builtin
    }
}

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
    // SKILL-05: platform filter — present means restrict to listed OSes (07.2 D-04)
    #[serde(default)]
    pub platforms: Option<Vec<String>>,
    // SKILL-06: Extended agentskills.io + hermes-agent backward-compat fields (07.2 D-08)
    /// Plain-string environment hint from the agentskills.io spec (1-500 chars hint — not enforced in 07.2).
    #[serde(default)]
    pub compatibility: Option<String>,
    /// Pre-approved tool list from the agentskills.io spec. Parsed but NOT enforced (07.2 D-10).
    #[serde(default, rename = "allowed-tools")]
    pub allowed_tools: Option<Vec<String>>,
    /// Opaque metadata blob storing arbitrary hermes-agent extensions (e.g. `metadata.hermes.tags`).
    /// Stored as `serde_yaml::Value` per 07.2 D-09 for forward-compat without typed schema changes.
    #[serde(default)]
    pub metadata: Option<serde_yaml::Value>,
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
    // SKILL-05: mirror of frontmatter.platforms for introspection (07.2 D-11)
    pub platforms: Option<Vec<String>>,
    // SKILL-06: mirror extended frontmatter for introspection without re-parse (07.2 D-11)
    pub compatibility: Option<String>,
    pub allowed_tools: Option<Vec<String>>,
    pub metadata: Option<serde_yaml::Value>,
    /// Typed extraction of metadata.hermes.* (D-17, Phase 19 Plan 01).
    /// None if no metadata block or no hermes sub-key.
    pub hermes_metadata: Option<HermesMetadata>,
    /// Provenance label for D-15 scan enforcement (Phase 19 Plan 05).
    /// Defaults to Builtin for all locally-discovered skills in Phase 19.
    pub source: SkillSource,
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

    // SKILL-07 name validation (07.2 D-13, D-14, D-15): strict reject on name violations.
    if let Err(reason) = validate_skill_name(&frontmatter.name) {
        warn!(
            "SkillRegistry: rejecting skill with invalid name {:?}: {}",
            frontmatter.name, reason
        );
        return None;
    }

    // SKILL-07 description length check (D-14): WARN-BUT-LOAD — do not return None.
    let desc_len = frontmatter.description.chars().count();
    if !(SKILL_DESC_MIN_LEN..=SKILL_DESC_MAX_LEN).contains(&desc_len) {
        warn!(
            "SkillRegistry: skill {:?} has description length {} outside allowed range {}..={} (loading anyway)",
            frontmatter.name, desc_len, SKILL_DESC_MIN_LEN, SKILL_DESC_MAX_LEN
        );
    }

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
// Platform filter (SKILL-05)
// =============================================================================

/// Returns true if the skill should be loaded on the current OS.
///
/// Strict no-alias mapping per 07.2 D-05:
/// - "macos"   → matches cfg!(target_os = "macos")
/// - "linux"   → matches cfg!(target_os = "linux")
/// - "windows" → matches cfg!(target_os = "windows")
///
/// Any other platform string is UNKNOWN and treated as a non-match.
/// A skill whose `platforms` list contains only unknown strings is filtered out on every OS.
///
/// Defaults (spec-compliant, backward compatible):
/// - `None`         → load (no restriction)
/// - `Some(vec![])` → load (empty list is treated as "no restriction" per spec)
fn skill_matches_current_platform(platforms: Option<&Vec<String>>) -> bool {
    let list = match platforms {
        None => return true,
        Some(list) if list.is_empty() => return true,
        Some(list) => list,
    };

    list.iter().any(|p| match p.as_str() {
        "macos"   => cfg!(target_os = "macos"),
        "linux"   => cfg!(target_os = "linux"),
        "windows" => cfg!(target_os = "windows"),
        _ => false, // unknown / alias → non-match (no `darwin`, no `osx`, no `win32`)
    })
}

// =============================================================================
// HermesMetadata extraction (Phase 19 Plan 01)
// =============================================================================

/// Extract typed HermesMetadata from the opaque `metadata: Option<serde_yaml::Value>` blob.
///
/// Returns `None` if there is no metadata block or no `hermes` sub-key.
/// On parse error (unexpected serde error), logs a WARN and returns `Some(HermesMetadata::default())`
/// so the skill always loads (D-18 WARN-BUT-LOAD).
///
/// Unknown fields inside `metadata.hermes.*` are captured by `#[serde(flatten)] extras`
/// and do NOT cause an error.
fn extract_hermes_metadata(raw: &Option<serde_yaml::Value>) -> Option<HermesMetadata> {
    let root = raw.as_ref()?;
    let hermes_val = root.get("hermes")?.clone();
    match serde_yaml::from_value::<HermesMetadata>(hermes_val) {
        Ok(m) => Some(m),
        Err(e) => {
            tracing::warn!(error = %e, "HermesMetadata parse error (WARN-BUT-LOAD) — using default with empty extras");
            Some(HermesMetadata::default())
        }
    }
}

// =============================================================================
// SkillRegistry
// =============================================================================

pub struct SkillRegistry {
    skills: Vec<SkillRecord>,
}

/// Build the ordered list of skill search paths for a given cwd and SkillsConfig.
///
/// Defaults first (priority order), extras appended after (D-19).
/// Exposed as `pub(crate)` so tests can pin the path construction logic.
pub(crate) fn build_skill_search_paths(cwd: &Path, config: &SkillsConfig) -> Vec<PathBuf> {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")));
    let mut paths = vec![
        cwd.join(".ironhermes/skills"),
        get_hermes_home().join("skills"),
        home.join(".agents/skills"),
    ];
    // D-19: extras appended AFTER defaults so first-path-wins preserves default priority.
    paths.extend(config.extra_paths.iter().cloned());
    paths
}

impl SkillRegistry {
    /// Scan three priority-ordered paths for skill documents (legacy, Config-unaware).
    ///
    /// Priority order (first-path-wins deduplication by lowercase name):
    /// 1. `cwd/.ironhermes/skills/`
    /// 2. `~/.ironhermes/skills/` (via get_hermes_home())
    /// 3. `~/.agents/skills/`
    ///
    /// This is a thin wrapper over [`Self::load_with_config`] using a default
    /// `SkillsConfig` (enabled, no extra paths). Preserved for backward compat
    /// with callers that do not have a `Config` handy (tests, simple tools).
    pub fn load(cwd: &Path) -> Self {
        Self::load_with_config(cwd, &SkillsConfig::default())
    }

    /// Scan for skills using a user-supplied `SkillsConfig` (07.2 D-21).
    ///
    /// Behavior:
    /// - `config.enabled == false` → returns an empty registry WITHOUT scanning.
    /// - Otherwise, scans the 3 hardcoded defaults in priority order, then
    ///   appends `config.extra_paths` at the end. First-path-wins dedup means
    ///   defaults retain priority over extras (D-19).
    pub fn load_with_config(cwd: &Path, config: &SkillsConfig) -> Self {
        // SKILL-08 kill switch (D-20).
        if !config.enabled {
            return Self { skills: Vec::new() };
        }

        let search_paths = build_skill_search_paths(cwd, config);
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

                // SKILL-07 dir-name-match check (D-13, D-15): warn-but-load on case-insensitive mismatch.
                let dir_name = subdir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if !dir_name.is_empty() && dir_name != frontmatter.name.to_lowercase() {
                    warn!(
                        "SkillRegistry: skill {:?} at {:?} has directory name {:?} that does not match frontmatter name (loading anyway)",
                        frontmatter.name, skill_md_path, dir_name
                    );
                }

                // SKILL-05: Platform filter (07.2 D-05, D-06) — runs BEFORE dedup so a filtered
                // skill does not reserve its name slot against a lower-priority path.
                if !skill_matches_current_platform(frontmatter.platforms.as_ref()) {
                    debug!(
                        "SkillRegistry: skipping {:?} — platforms {:?} do not include current OS",
                        skill_md_path, frontmatter.platforms
                    );
                    continue;
                }

                let name_lower = frontmatter.name.to_lowercase();
                if seen_names.contains(&name_lower) {
                    debug!(
                        "SkillRegistry: skipping duplicate skill '{}' at {:?}",
                        frontmatter.name, skill_md_path
                    );
                    continue;
                }

                seen_names.insert(name_lower);
                let SkillFrontmatter {
                    name,
                    description,
                    platforms,
                    compatibility,
                    allowed_tools,
                    metadata,
                    .. // version/author/license intentionally ignored here — not needed on SkillRecord
                } = frontmatter;
                let hermes_metadata = extract_hermes_metadata(&metadata);
                skills.push(SkillRecord {
                    name,
                    description,
                    path: skill_md_path,
                    platforms,
                    compatibility,
                    allowed_tools,
                    metadata,
                    hermes_metadata,
                    source: SkillSource::Builtin, // Phase 19 default per RESEARCH.md A4
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
    // SKILL-06: Extended frontmatter field tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_parse_skill_md_extended_frontmatter_all_fields() {
        let content = "---\nname: my-skill\ndescription: A test skill\ncompatibility: \"macOS with zsh\"\nallowed-tools:\n  - terminal\n  - web_read\nmetadata:\n  hermes:\n    tags:\n      - devops\n      - ci\n    category: automation\n---\nBody content here.";
        let result = parse_skill_md(content);
        assert!(result.is_some(), "Expected Some but got None");
        let (fm, _body) = result.unwrap();
        assert_eq!(fm.compatibility.as_deref(), Some("macOS with zsh"));
        assert_eq!(
            fm.allowed_tools.as_ref().unwrap(),
            &vec!["terminal".to_string(), "web_read".to_string()]
        );
        assert!(fm.metadata.is_some());
        let tags = fm
            .metadata
            .as_ref()
            .unwrap()
            .get("hermes")
            .and_then(|h| h.get("tags"))
            .and_then(|t| t.as_sequence())
            .unwrap();
        assert_eq!(tags.len(), 2);
    }

    #[test]
    fn test_parse_skill_md_extended_frontmatter_absent() {
        let content = make_skill_md("my-skill", "desc", "");
        let result = parse_skill_md(&content);
        assert!(result.is_some());
        let (fm, _body) = result.unwrap();
        assert!(fm.compatibility.is_none());
        assert!(fm.allowed_tools.is_none());
        assert!(fm.metadata.is_none());
    }

    #[test]
    fn test_registry_load_propagates_extended_fields_to_record() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("my-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        let content = "---\nname: my-skill\ndescription: A test skill\ncompatibility: \"requires zsh\"\nallowed-tools:\n  - terminal\n  - web_read\nmetadata:\n  hermes:\n    tags:\n      - devops\n---\nBody content here.";
        fs::write(skill_dir.join("SKILL.md"), content).unwrap();

        let registry = SkillRegistry::load_with_paths(&[skills_dir]);
        assert_eq!(registry.list().len(), 1);
        let record = &registry.list()[0];
        assert_eq!(record.compatibility.as_deref(), Some("requires zsh"));
        assert_eq!(
            record.allowed_tools.as_ref().unwrap(),
            &vec!["terminal".to_string(), "web_read".to_string()]
        );
        assert!(record.metadata.is_some());
    }

    #[test]
    fn test_registry_load_absent_extended_fields_are_none_on_record() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("my-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            make_skill_md("my-skill", "desc", ""),
        )
        .unwrap();

        let registry = SkillRegistry::load_with_paths(&[skills_dir]);
        assert_eq!(registry.list().len(), 1);
        let record = &registry.list()[0];
        assert!(record.compatibility.is_none() && record.allowed_tools.is_none() && record.metadata.is_none());
    }

    #[test]
    fn test_allowed_tools_kebab_case_rename() {
        // Explicitly verify the serde rename works: kebab-case `allowed-tools:` key
        // (NOT snake_case `allowed_tools:`) must deserialize into `allowed_tools`.
        let content = "---\nname: my-skill\ndescription: desc\nallowed-tools:\n  - terminal\n  - web_read\n---\nBody";
        let result = parse_skill_md(content);
        assert!(result.is_some());
        let (fm, _body) = result.unwrap();
        assert!(
            fm.allowed_tools.is_some(),
            "allowed_tools should be populated from kebab-case 'allowed-tools:' key"
        );
        assert_eq!(
            fm.allowed_tools.unwrap(),
            vec!["terminal".to_string(), "web_read".to_string()]
        );
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
        // Subdir name is lowercase to match the spec-valid fixture name and
        // avoid the SKILL-07 dir-name-match warn. The test still exercises
        // case-insensitive LOOKUP via find("MySkill") / find("MYSKILL") below.
        let skill_dir = skills_dir.join("myskill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            make_skill_md("myskill", "My skill", "Content"),
        )
        .unwrap();

        let registry = make_isolated_registry(&[skills_dir]);
        // Case-insensitive lookup: registry stores "myskill", find accepts any case.
        assert!(registry.find("myskill").is_some());
        assert!(registry.find("MySkill").is_some());
        assert!(registry.find("MYSKILL").is_some());
    }

    #[test]
    fn test_find_returns_none_for_nonexistent() {
        let dir = tempdir().unwrap();
        let registry = make_isolated_registry(&[dir.path().join("no-skills-here")]);
        assert!(registry.find("does-not-exist").is_none());
    }

    // -------------------------------------------------------------------------
    // SKILL-05: Platform filter tests
    // -------------------------------------------------------------------------

    fn make_skill_md_with_platforms(name: &str, description: &str, platforms: &[&str]) -> String {
        let platforms_yaml = if platforms.is_empty() {
            "platforms: []\n".to_string()
        } else {
            let list = platforms
                .iter()
                .map(|p| format!("  - {}", p))
                .collect::<Vec<_>>()
                .join("\n");
            format!("platforms:\n{}\n", list)
        };
        format!(
            "---\nname: {}\ndescription: {}\n{}---\nBody",
            name, description, platforms_yaml
        )
    }

    #[test]
    fn test_platform_filter_no_field_loads() {
        // Skill with no platforms field loads on every OS (backward compat)
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("no-platforms");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            make_skill_md("no-platforms", "No platforms field", ""),
        )
        .unwrap();

        let registry = SkillRegistry::load_with_paths(&[skills_dir]);
        assert_eq!(registry.list().len(), 1);
        assert_eq!(registry.list()[0].name, "no-platforms");
    }

    #[test]
    fn test_platform_filter_empty_list_loads() {
        // Skill with platforms: [] loads on every OS (spec default)
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("empty-platforms");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            make_skill_md_with_platforms("empty-platforms", "Empty platforms list", &[]),
        )
        .unwrap();

        let registry = SkillRegistry::load_with_paths(&[skills_dir]);
        assert_eq!(registry.list().len(), 1);
        assert_eq!(registry.list()[0].name, "empty-platforms");
    }

    #[test]
    fn test_platform_filter_unknown_platform_skipped() {
        // Skill with platforms: ["freebsd"] skipped on every OS (unknown string per D-05)
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");

        let skill_freebsd = skills_dir.join("freebsd-skill");
        fs::create_dir_all(&skill_freebsd).unwrap();
        fs::write(
            skill_freebsd.join("SKILL.md"),
            make_skill_md_with_platforms("freebsd-skill", "FreeBSD only", &["freebsd"]),
        )
        .unwrap();

        // Also test darwin alias (strict no-alias per D-05 — "darwin" is NOT "macos")
        let skill_darwin = skills_dir.join("darwin-skill");
        fs::create_dir_all(&skill_darwin).unwrap();
        fs::write(
            skill_darwin.join("SKILL.md"),
            make_skill_md_with_platforms("darwin-skill", "Darwin alias (invalid)", &["darwin"]),
        )
        .unwrap();

        let registry = SkillRegistry::load_with_paths(&[skills_dir]);
        assert_eq!(
            registry.list().len(),
            0,
            "Expected 0 skills but got: {:?}",
            registry.list().iter().map(|s| &s.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_platform_filter_all_three_oses_loads() {
        // Skill with platforms: ["linux", "macos", "windows"] loads on every supported OS
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("all-oses");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            make_skill_md_with_platforms(
                "all-oses",
                "All supported OSes",
                &["linux", "macos", "windows"],
            ),
        )
        .unwrap();

        let registry = SkillRegistry::load_with_paths(&[skills_dir]);
        assert_eq!(registry.list().len(), 1);
        assert_eq!(registry.list()[0].name, "all-oses");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_platform_filter_current_os_macos() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");

        let macos_skill = skills_dir.join("macos-skill");
        fs::create_dir_all(&macos_skill).unwrap();
        fs::write(
            macos_skill.join("SKILL.md"),
            make_skill_md_with_platforms("macos-skill", "macOS only", &["macos"]),
        )
        .unwrap();

        let linux_skill = skills_dir.join("linux-skill");
        fs::create_dir_all(&linux_skill).unwrap();
        fs::write(
            linux_skill.join("SKILL.md"),
            make_skill_md_with_platforms("linux-skill", "Linux only", &["linux"]),
        )
        .unwrap();

        let windows_skill = skills_dir.join("windows-skill");
        fs::create_dir_all(&windows_skill).unwrap();
        fs::write(
            windows_skill.join("SKILL.md"),
            make_skill_md_with_platforms("windows-skill", "Windows only", &["windows"]),
        )
        .unwrap();

        let registry = SkillRegistry::load_with_paths(&[skills_dir]);
        let names: Vec<&str> = registry.list().iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"macos-skill"), "macos-skill should load on macOS");
        assert!(!names.contains(&"linux-skill"), "linux-skill should be skipped on macOS");
        assert!(!names.contains(&"windows-skill"), "windows-skill should be skipped on macOS");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_platform_filter_current_os_linux() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");

        let macos_skill = skills_dir.join("macos-skill");
        fs::create_dir_all(&macos_skill).unwrap();
        fs::write(
            macos_skill.join("SKILL.md"),
            make_skill_md_with_platforms("macos-skill", "macOS only", &["macos"]),
        )
        .unwrap();

        let linux_skill = skills_dir.join("linux-skill");
        fs::create_dir_all(&linux_skill).unwrap();
        fs::write(
            linux_skill.join("SKILL.md"),
            make_skill_md_with_platforms("linux-skill", "Linux only", &["linux"]),
        )
        .unwrap();

        let windows_skill = skills_dir.join("windows-skill");
        fs::create_dir_all(&windows_skill).unwrap();
        fs::write(
            windows_skill.join("SKILL.md"),
            make_skill_md_with_platforms("windows-skill", "Windows only", &["windows"]),
        )
        .unwrap();

        let registry = SkillRegistry::load_with_paths(&[skills_dir]);
        let names: Vec<&str> = registry.list().iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"linux-skill"), "linux-skill should load on Linux");
        assert!(!names.contains(&"macos-skill"), "macos-skill should be skipped on Linux");
        assert!(!names.contains(&"windows-skill"), "windows-skill should be skipped on Linux");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_platform_filter_current_os_windows() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");

        let macos_skill = skills_dir.join("macos-skill");
        fs::create_dir_all(&macos_skill).unwrap();
        fs::write(
            macos_skill.join("SKILL.md"),
            make_skill_md_with_platforms("macos-skill", "macOS only", &["macos"]),
        )
        .unwrap();

        let linux_skill = skills_dir.join("linux-skill");
        fs::create_dir_all(&linux_skill).unwrap();
        fs::write(
            linux_skill.join("SKILL.md"),
            make_skill_md_with_platforms("linux-skill", "Linux only", &["linux"]),
        )
        .unwrap();

        let windows_skill = skills_dir.join("windows-skill");
        fs::create_dir_all(&windows_skill).unwrap();
        fs::write(
            windows_skill.join("SKILL.md"),
            make_skill_md_with_platforms("windows-skill", "Windows only", &["windows"]),
        )
        .unwrap();

        let registry = SkillRegistry::load_with_paths(&[skills_dir]);
        let names: Vec<&str> = registry.list().iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"windows-skill"), "windows-skill should load on Windows");
        assert!(!names.contains(&"macos-skill"), "macos-skill should be skipped on Windows");
        assert!(!names.contains(&"linux-skill"), "linux-skill should be skipped on Windows");
    }

    #[test]
    fn test_platform_filter_runs_before_dedup() {
        // D-06 ordering: filter must run BEFORE dedup.
        // A filtered-out skill in path_first must NOT reserve its name slot,
        // allowing the same-named skill in path_second to load.
        let dir = tempdir().unwrap();

        // path_first: "my-skill" with a guaranteed non-match platform
        let path_first = dir.path().join("skills-first");
        let first_skill_dir = path_first.join("my-skill");
        fs::create_dir_all(&first_skill_dir).unwrap();
        fs::write(
            first_skill_dir.join("SKILL.md"),
            make_skill_md_with_platforms("my-skill", "filtered out", &["nonexistent-os"]),
        )
        .unwrap();

        // path_second: "my-skill" with no platforms (loads on every OS)
        let path_second = dir.path().join("skills-second");
        let second_skill_dir = path_second.join("my-skill");
        fs::create_dir_all(&second_skill_dir).unwrap();
        fs::write(
            second_skill_dir.join("SKILL.md"),
            make_skill_md("my-skill", "should load", ""),
        )
        .unwrap();

        let registry = SkillRegistry::load_with_paths(&[path_first, path_second]);
        let matches: Vec<_> = registry
            .list()
            .iter()
            .filter(|s| s.name == "my-skill")
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "Expected exactly one my-skill, got: {:?}",
            registry.list().iter().map(|s| &s.name).collect::<Vec<_>>()
        );
        assert_eq!(
            matches[0].description, "should load",
            "Expected 'should load' skill but got: {}",
            matches[0].description
        );
    }

    #[test]
    fn test_platforms_field_propagates_to_record() {
        // platforms list should be mirrored on SkillRecord per D-11
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("all-oses");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            make_skill_md_with_platforms(
                "all-oses",
                "All supported OSes",
                &["macos", "linux", "windows"],
            ),
        )
        .unwrap();

        let registry = SkillRegistry::load_with_paths(&[skills_dir]);
        assert_eq!(registry.list().len(), 1);
        let record = &registry.list()[0];
        assert_eq!(
            record.platforms.as_ref().unwrap(),
            &vec!["macos".to_string(), "linux".to_string(), "windows".to_string()]
        );
    }

    #[test]
    fn test_platforms_absent_is_none_on_record() {
        // Minimal SKILL.md (no platforms field) → record.platforms is None
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("minimal");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            make_skill_md("minimal", "Minimal skill", ""),
        )
        .unwrap();

        let registry = SkillRegistry::load_with_paths(&[skills_dir]);
        assert_eq!(registry.list().len(), 1);
        let record = &registry.list()[0];
        assert!(record.platforms.is_none(), "Expected platforms to be None for minimal skill");
    }

    // -------------------------------------------------------------------------
    // SKILL-07: Name and description validation tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_validate_skill_name_valid() {
        assert!(validate_skill_name("valid-skill").is_ok());
        assert!(validate_skill_name("a").is_ok());             // length 1
        assert!(validate_skill_name("a1b2-c3d4").is_ok());     // mixed
        assert!(validate_skill_name(&"a".repeat(64)).is_ok()); // length 64
    }

    #[test]
    fn test_validate_skill_name_invalid_regex() {
        assert!(validate_skill_name("Uppercase").is_err());    // uppercase
        assert!(validate_skill_name("under_score").is_err());  // underscore
        assert!(validate_skill_name("-leading").is_err());     // leading hyphen
        assert!(validate_skill_name("trailing-").is_err());    // trailing hyphen
        assert!(validate_skill_name("spaces bad").is_err());   // space
        assert!(validate_skill_name("dot.bad").is_err());      // period
    }

    #[test]
    fn test_validate_skill_name_consecutive_hyphens() {
        // The regex alone would accept "foo--bar"; the extra check catches it.
        assert!(validate_skill_name("foo--bar").is_err());
    }

    #[test]
    fn test_validate_skill_name_length_boundaries() {
        assert!(validate_skill_name("").is_err());                    // empty
        assert!(validate_skill_name(&"a".repeat(65)).is_err());       // length 65
        assert!(validate_skill_name(&"a".repeat(64)).is_ok());        // length 64 — accepted
        assert!(validate_skill_name("a").is_ok());                    // length 1 — accepted
    }

    #[test]
    fn test_parse_skill_md_rejects_invalid_name() {
        // Uppercase name must be strict-rejected (returns None).
        let content = "---\nname: Invalid Name\ndescription: desc\n---\nBody";
        let result = parse_skill_md(content);
        assert!(result.is_none(), "Expected None for invalid name but got Some");
    }

    #[test]
    fn test_parse_skill_md_rejects_consecutive_hyphens() {
        let content = "---\nname: foo--bar\ndescription: desc\n---\nBody";
        let result = parse_skill_md(content);
        assert!(result.is_none(), "Expected None for consecutive-hyphen name but got Some");
    }

    #[test]
    fn test_parse_skill_md_description_too_long_warn_loads() {
        // Description of 1025 chars: warn-but-load — must return Some.
        let long_desc = "a".repeat(1025);
        let content = format!("---\nname: skill-a\ndescription: {}\n---\nBody", long_desc);
        let result = parse_skill_md(&content);
        assert!(result.is_some(), "Expected Some (warn-but-load) but got None");
        assert_eq!(result.unwrap().0.description.chars().count(), 1025);
    }

    #[test]
    fn test_parse_skill_md_description_empty_warn_loads() {
        // Empty description: warn-but-load — must return Some.
        let content = "---\nname: skill-a\ndescription: \"\"\n---\nBody";
        let result = parse_skill_md(content);
        assert!(result.is_some(), "Expected Some (warn-but-load) for empty description but got None");
    }

    #[test]
    fn test_parse_skill_md_description_boundary_1024_loads_silently() {
        // Exactly 1024 chars: inside the allowed range — must return Some.
        let desc = "a".repeat(1024);
        let content = format!("---\nname: skill-a\ndescription: {}\n---\nBody", desc);
        let result = parse_skill_md(&content);
        assert!(result.is_some(), "Expected Some for description of exactly 1024 chars");
        assert_eq!(result.unwrap().0.description.chars().count(), 1024);
    }

    #[test]
    fn test_registry_load_dir_name_mismatch_warn_loads() {
        // Subdir name "different-dir" does not match frontmatter name "skill-a".
        // Warn is emitted but the skill is still loaded.
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("different-dir");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            make_skill_md("skill-a", "Skill A", "Body"),
        )
        .unwrap();

        let registry = SkillRegistry::load_with_paths(&[skills_dir]);
        assert_eq!(registry.list().len(), 1, "Expected skill to load despite dir-name mismatch");
        assert!(registry.find("skill-a").is_some());
    }

    #[test]
    fn test_registry_load_dir_name_case_insensitive_match_silent() {
        // Subdir "MySkill" vs frontmatter name "myskill" — case-insensitive match, loads silently.
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("MySkill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            make_skill_md("myskill", "My skill", "Body"),
        )
        .unwrap();

        let registry = SkillRegistry::load_with_paths(&[skills_dir]);
        assert_eq!(registry.list().len(), 1, "Expected skill to load with case-insensitive dir-name match");
        assert!(registry.find("myskill").is_some());
    }

    #[test]
    fn test_registry_load_skips_invalid_name_skill() {
        // Frontmatter name "Skill-Caps" has uppercase — strict-rejected by parse_skill_md.
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("Skill-Caps");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            make_skill_md("Skill-Caps", "Skill with uppercase name", "Body"),
        )
        .unwrap();

        let registry = SkillRegistry::load_with_paths(&[skills_dir]);
        assert!(
            registry.list().is_empty(),
            "Expected empty registry for skill with invalid uppercase name"
        );
    }

    #[test]
    fn test_existing_phase7_names_still_load() {
        // Sanity check: all fixture names from existing Phase 7 tests pass validation.
        for name in &["skill-a", "my-skill", "alpha", "beta", "skill", "myskill", "no-platforms",
                      "empty-platforms", "all-oses", "minimal", "freebsd-skill", "darwin-skill"] {
            assert!(
                validate_skill_name(name).is_ok(),
                "Expected {:?} to pass validate_skill_name but it failed",
                name
            );
        }
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

    // -------------------------------------------------------------------------
    // SKILL-08: SkillsConfig / load_with_config tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_load_with_config_disabled_returns_empty() {
        // enabled: false → empty registry, no filesystem scan at all.
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join(".ironhermes/skills/a-skill");
        fs::create_dir_all(&skills_dir).unwrap();
        fs::write(
            skills_dir.join("SKILL.md"),
            make_skill_md("a-skill", "Some skill", "body"),
        )
        .unwrap();

        let config = SkillsConfig { enabled: false, extra_paths: vec![] };
        let registry = SkillRegistry::load_with_config(dir.path(), &config);
        assert!(
            registry.list().is_empty(),
            "enabled=false must return empty registry; got {} skill(s)",
            registry.list().len()
        );
    }

    #[test]
    fn test_load_with_config_enabled_includes_extra_paths() {
        // A skill placed only in an extra_path is discovered.
        let cwd = tempdir().unwrap();
        let extra = tempdir().unwrap();
        let extra_skill_dir = extra.path().join("extra-skill");
        fs::create_dir_all(&extra_skill_dir).unwrap();
        fs::write(
            extra_skill_dir.join("SKILL.md"),
            make_skill_md("extra-skill", "Loaded from extra_paths", "body"),
        )
        .unwrap();

        let config = SkillsConfig {
            enabled: true,
            extra_paths: vec![extra.path().to_path_buf()],
        };
        let registry = SkillRegistry::load_with_config(cwd.path(), &config);
        assert!(
            registry.find("extra-skill").is_some(),
            "extra_paths skill should be discovered; registry has: {:?}",
            registry.list().iter().map(|s| &s.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_load_with_config_extras_respect_dedup_default_priority() {
        // Pins D-19: extras are appended AFTER defaults.
        // Use build_skill_search_paths directly to test path construction.
        let cwd = PathBuf::from("/test/cwd");
        let config = SkillsConfig {
            enabled: true,
            extra_paths: vec![PathBuf::from("/extra/a"), PathBuf::from("/extra/b")],
        };
        let paths = build_skill_search_paths(&cwd, &config);
        assert_eq!(paths.len(), 5, "3 defaults + 2 extras");
        assert_eq!(paths[0], cwd.join(".ironhermes/skills"), "default 1 must be first");
        assert_eq!(paths[3], PathBuf::from("/extra/a"), "extra a must be at index 3");
        assert_eq!(paths[4], PathBuf::from("/extra/b"), "extra b must be at index 4");
    }

    #[test]
    fn test_load_legacy_matches_default_config() {
        // SkillRegistry::load(cwd) is identical to load_with_config(cwd, &default).
        let cwd = tempdir().unwrap();
        let registry_legacy = SkillRegistry::load(cwd.path());
        let registry_config = SkillRegistry::load_with_config(cwd.path(), &SkillsConfig::default());

        let names_legacy: Vec<&str> = registry_legacy.list().iter().map(|s| s.name.as_str()).collect();
        let names_config: Vec<&str> = registry_config.list().iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names_legacy, names_config,
            "load() and load_with_config(default) must return identical skill lists"
        );
    }

    // -------------------------------------------------------------------------
    // Phase 19 Plan 01: Wave 0 — HermesMetadata typed extraction + D-18 WARN-BUT-LOAD
    // -------------------------------------------------------------------------

    #[test]
    fn test_hermes_metadata() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("my-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        let content = r#"---
name: my-skill
description: A test skill
metadata:
  hermes:
    requires_toolsets:
      - web
    requires_tools:
      - fetch_url
    fallback_for_tools:
      - playwright
    required_environment_variables:
      - name: TENOR_API_KEY
        prompt: "Enter Tenor key"
        help: "https://tenor.com/developer"
        required_for: "GIF search"
    required_credential_files:
      - oauth_token.json
    config:
      - key: "wiki.path"
        type: path
        default: "~/research"
        description: "Where to store notes"
---
Body content.
"#;
        fs::write(skill_dir.join("SKILL.md"), content).unwrap();

        let registry = SkillRegistry::load_with_paths(&[skills_dir]);
        assert_eq!(registry.list().len(), 1);
        let record = &registry.list()[0];
        assert!(record.hermes_metadata.is_some(), "hermes_metadata should be Some");
        let hm = record.hermes_metadata.as_ref().unwrap();
        assert_eq!(hm.requires_toolsets, vec!["web"]);
        assert_eq!(hm.requires_tools, vec!["fetch_url"]);
        assert_eq!(hm.fallback_for_tools, vec!["playwright"]);
        assert_eq!(hm.required_environment_variables.len(), 1);
        let env_entry = &hm.required_environment_variables[0];
        assert_eq!(env_entry.name, "TENOR_API_KEY");
        assert_eq!(env_entry.prompt, Some("Enter Tenor key".to_string()));
        assert_eq!(env_entry.required_for, Some("GIF search".to_string()));
        assert_eq!(hm.required_credential_files.len(), 1);
        assert!(
            matches!(&hm.required_credential_files[0], CredentialFileEntry::Path(p) if p == "oauth_token.json"),
            "expected CredentialFileEntry::Path(\"oauth_token.json\")"
        );
        assert_eq!(hm.config.len(), 1);
        assert_eq!(hm.config[0].key, "wiki.path");
        assert_eq!(hm.config[0].field_type, Some("path".to_string()));
    }

    #[test]
    fn test_warn_but_load_unknown_fields() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("unknown-fields-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        let content = r#"---
name: unknown-fields-skill
description: Skill with unknown hermes fields
metadata:
  hermes:
    requires_toolsets:
      - web
    totally_unknown_field:
      nested: true
    another_extra: "value"
---
Body content.
"#;
        fs::write(skill_dir.join("SKILL.md"), content).unwrap();

        let registry = SkillRegistry::load_with_paths(&[skills_dir]);
        assert!(registry.list().len() == 1, "skill should load despite unknown fields");
        let record = &registry.list()[0];
        assert!(record.hermes_metadata.is_some(), "hermes_metadata should be Some");
        let hm = record.hermes_metadata.as_ref().unwrap();
        assert_eq!(hm.requires_toolsets, vec!["web"]);
        assert!(hm.extras.contains_key("totally_unknown_field"), "unknown field should be in extras");
        assert!(hm.extras.contains_key("another_extra"), "extra field should be in extras");
        assert!(hm.requires_tools.is_empty(), "requires_tools should be empty");
    }

    #[test]
    fn test_07_2_compat_metadata() {
        // Phase 07.2 shape: metadata.hermes.tags and metadata.hermes.related_skills
        // These are NOT in HermesMetadata typed fields — they must land in extras.
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("compat-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        let content = r#"---
name: compat-skill
description: Phase 07.2 compatibility skill
metadata:
  hermes:
    tags:
      - productivity
    related_skills:
      - other-skill
---
Body content.
"#;
        fs::write(skill_dir.join("SKILL.md"), content).unwrap();

        let registry = SkillRegistry::load_with_paths(&[skills_dir]);
        assert!(registry.list().len() == 1, "07.2 skill should load cleanly");
        let record = &registry.list()[0];
        assert!(record.hermes_metadata.is_some(), "hermes_metadata should be Some");
        let hm = record.hermes_metadata.as_ref().unwrap();
        assert!(hm.extras.contains_key("tags"), "tags should be in extras");
        let tags_val = &hm.extras["tags"];
        assert!(tags_val.as_sequence().is_some(), "tags should be a YAML sequence");
        assert!(hm.extras.contains_key("related_skills"), "related_skills should be in extras");
        assert!(hm.requires_toolsets.is_empty(), "requires_toolsets should be empty");
        assert!(hm.required_environment_variables.is_empty(), "env vars should be empty");
    }

    #[test]
    fn test_no_metadata_at_all() {
        // Skill with no metadata: block at all — hermes_metadata should be None
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("bare-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        let content = "---\nname: bare-skill\ndescription: No metadata block\n---\nBody content.\n";
        fs::write(skill_dir.join("SKILL.md"), content).unwrap();

        let registry = SkillRegistry::load_with_paths(&[skills_dir]);
        assert_eq!(registry.list().len(), 1, "skill should load even with no metadata");
        let record = &registry.list()[0];
        assert!(record.hermes_metadata.is_none(), "hermes_metadata should be None when no metadata block");
        assert_eq!(record.name, "bare-skill");
        assert_eq!(record.description, "No metadata block");
    }
}

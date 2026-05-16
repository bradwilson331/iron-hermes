//! `skill_manage` tool — Phase 33 (LEARN-04, LEARN-05).
//!
//! Implements 6 actions for self-authored SKILL.md files:
//! `create`, `patch`, `edit`, `delete`, `write_file`, `remove_file`.
//!
//! Mirrors the action-dispatch pattern in `memory_tool.rs`. All file writes
//! are scoped to `get_hermes_home()/skills/<category>/<slug>/`, with
//! `scan_skill_content` running before every write (defense-in-depth — the
//! `SkillRegistry` also scans on load).

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use ironhermes_core::constants::get_hermes_home;
use ironhermes_core::context_scanner::scan_skill_content;
use ironhermes_core::skills::validate_skill_name;
use serde_json::{Value, json};
use std::path::PathBuf;

use crate::registry::Tool;

/// `skill_manage` tool. Stateless — uses `get_hermes_home()` for path resolution.
pub struct SkillManageTool;

impl SkillManageTool {
    pub fn new() -> Self {
        Self
    }

    /// Two-level skill directory: `<HERMES_HOME>/skills/<category>/<slug>/`.
    /// `SkillRegistry.load_with_paths` walks level 2 (Pitfall 3).
    fn skill_dir(&self, category: &str, slug: &str) -> PathBuf {
        get_hermes_home().join("skills").join(category).join(slug)
    }

    /// Resolve a file path inside the skill directory, rejecting path traversal.
    ///
    /// Rejects any `rel_path` containing `..` or starting with `/` — guards
    /// against escape from the skill dir into HERMES_HOME or the filesystem.
    fn resolve_skill_file_path(
        &self,
        category: &str,
        slug: &str,
        rel_path: &str,
    ) -> anyhow::Result<PathBuf> {
        if rel_path.contains("..") || rel_path.starts_with('/') {
            anyhow::bail!("path traversal rejected: '{}'", rel_path);
        }
        Ok(self.skill_dir(category, slug).join(rel_path))
    }

    /// Validate a category segment (same rules as a slug — no `/`, no `..`).
    fn validate_category(category: &str) -> anyhow::Result<()> {
        if category.is_empty() {
            anyhow::bail!("category is empty");
        }
        if category.contains('/') || category.contains("..") || category.starts_with('.') {
            anyhow::bail!("invalid category: '{}'", category);
        }
        Ok(())
    }

    /// Standard parameter helper — identical shape to `memory_tool.rs`.
    fn required_str<'a>(args: &'a Value, key: &str) -> anyhow::Result<&'a str> {
        args.get(key)
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter '{}'", key))
    }

    /// Optional Vec<String> param — empty when missing or non-array.
    fn optional_str_array(args: &Value, key: &str) -> Vec<String> {
        args.get(key)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Build the YAML frontmatter for a self-created SKILL.md.
    /// LEARN-04 fields: name, description, version, platforms, metadata.hermes.{tags, category, trust_tier}.
    fn build_self_created_frontmatter(
        slug: &str,
        description: &str,
        platforms: &[String],
        tags: &[String],
        category: &str,
        fallback_for_toolsets: &[String],
        requires_toolsets: &[String],
    ) -> String {
        let yaml_list = |xs: &[String]| -> String {
            if xs.is_empty() {
                "[]".to_string()
            } else {
                let inner = xs
                    .iter()
                    .map(|s| format!("\"{}\"", s.replace('"', "\\\"")))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{}]", inner)
            }
        };

        // YAML-escape description (single-line); keep simple for now.
        let desc_escaped = description.replace('\\', "\\\\").replace('"', "\\\"");

        let mut out = String::new();
        out.push_str("---\n");
        out.push_str(&format!("name: {}\n", slug));
        out.push_str(&format!("description: \"{}\"\n", desc_escaped));
        out.push_str("version: 1.0.0\n");
        out.push_str(&format!("platforms: {}\n", yaml_list(platforms)));
        out.push_str("metadata:\n");
        out.push_str("  hermes:\n");
        out.push_str(&format!("    tags: {}\n", yaml_list(tags)));
        out.push_str(&format!("    category: {}\n", category));
        out.push_str("    trust_tier: Self-created\n");
        if !fallback_for_toolsets.is_empty() {
            out.push_str(&format!(
                "    fallback_for_toolsets: {}\n",
                yaml_list(fallback_for_toolsets)
            ));
        }
        if !requires_toolsets.is_empty() {
            out.push_str(&format!(
                "    requires_toolsets: {}\n",
                yaml_list(requires_toolsets)
            ));
        }
        out.push_str("---\n");
        out
    }

    // -------------------------------------------------------------------
    // Action implementations
    // -------------------------------------------------------------------

    async fn action_create(&self, args: &Value) -> anyhow::Result<String> {
        let slug = Self::required_str(args, "name")?;
        let category = Self::required_str(args, "category")?;
        let description = Self::required_str(args, "description")?;

        if let Err(e) = validate_skill_name(slug) {
            return Ok(json!({
                "error": "invalid_name",
                "reason": e.to_string(),
            })
            .to_string());
        }
        if let Err(e) = Self::validate_category(category) {
            return Ok(json!({
                "error": "invalid_category",
                "reason": e.to_string(),
            })
            .to_string());
        }

        let content_body = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let tags = Self::optional_str_array(args, "tags");
        let platforms = Self::optional_str_array(args, "platforms");
        let fallback = Self::optional_str_array(args, "fallback_for_toolsets");
        let requires = Self::optional_str_array(args, "requires_toolsets");

        let frontmatter = Self::build_self_created_frontmatter(
            slug,
            description,
            &platforms,
            &tags,
            category,
            &fallback,
            &requires,
        );
        let full_content = format!("{}{}", frontmatter, content_body);

        let scan = scan_skill_content(&full_content, slug);
        if scan.starts_with("[BLOCKED:") {
            return Ok(json!({
                "error": "content_rejected",
                "reason": "injection_pattern_detected",
            })
            .to_string());
        }

        let dir = self.skill_dir(category, slug);
        let path = dir.join("SKILL.md");
        if path.exists() {
            return Ok(json!({
                "error": "already_exists",
                "reason": format!("use patch or edit to update {}", slug),
            })
            .to_string());
        }

        std::fs::create_dir_all(&dir)?;
        std::fs::write(&path, full_content)?;
        Ok(format!("Created skill '{}' at {}", slug, path.display()))
    }

    async fn action_patch(&self, args: &Value) -> anyhow::Result<String> {
        let slug = Self::required_str(args, "name")?;
        let category = Self::required_str(args, "category")?;
        let old_string = Self::required_str(args, "old_string")?;
        let new_string = Self::required_str(args, "new_string")?;

        if let Err(e) = validate_skill_name(slug) {
            return Ok(json!({"error": "invalid_name", "reason": e.to_string()}).to_string());
        }
        if let Err(e) = Self::validate_category(category) {
            return Ok(json!({"error": "invalid_category", "reason": e.to_string()}).to_string());
        }

        let path = self.skill_dir(category, slug).join("SKILL.md");
        let content = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => {
                return Ok(json!({
                    "error": "not_found",
                    "reason": format!("SKILL.md not found for {}/{}", category, slug),
                })
                .to_string());
            }
        };

        if !content.contains(old_string) {
            return Ok(json!({
                "error": "not_found",
                "reason": format!("old_string not found in {}/SKILL.md", slug),
            })
            .to_string());
        }

        let new_content = content.replacen(old_string, new_string, 1);
        let scan = scan_skill_content(&new_content, &path.display().to_string());
        if scan.starts_with("[BLOCKED:") {
            return Ok(json!({
                "error": "content_rejected",
                "reason": "injection_pattern_detected",
            })
            .to_string());
        }

        std::fs::write(&path, new_content)?;
        Ok(format!("Patched {}/SKILL.md", slug))
    }

    async fn action_edit(&self, args: &Value) -> anyhow::Result<String> {
        let slug = Self::required_str(args, "name")?;
        let category = Self::required_str(args, "category")?;
        let content = Self::required_str(args, "content")?;

        if let Err(e) = validate_skill_name(slug) {
            return Ok(json!({"error": "invalid_name", "reason": e.to_string()}).to_string());
        }
        if let Err(e) = Self::validate_category(category) {
            return Ok(json!({"error": "invalid_category", "reason": e.to_string()}).to_string());
        }

        let path = self.skill_dir(category, slug).join("SKILL.md");
        if !path.exists() {
            return Ok(json!({
                "error": "not_found",
                "reason": format!("SKILL.md not found for {}/{}", category, slug),
            })
            .to_string());
        }

        let scan = scan_skill_content(content, &path.display().to_string());
        if scan.starts_with("[BLOCKED:") {
            return Ok(json!({
                "error": "content_rejected",
                "reason": "injection_pattern_detected",
            })
            .to_string());
        }

        std::fs::write(&path, content)?;
        Ok(format!("Edited {}/SKILL.md", slug))
    }

    async fn action_delete(&self, args: &Value) -> anyhow::Result<String> {
        let slug = Self::required_str(args, "name")?;
        let category = Self::required_str(args, "category")?;

        if let Err(e) = validate_skill_name(slug) {
            return Ok(json!({"error": "invalid_name", "reason": e.to_string()}).to_string());
        }
        if let Err(e) = Self::validate_category(category) {
            return Ok(json!({"error": "invalid_category", "reason": e.to_string()}).to_string());
        }

        let dir = self.skill_dir(category, slug);
        let canonical = match dir.canonicalize() {
            Ok(p) => p,
            Err(_) => {
                return Ok(json!({
                    "error": "not_found",
                    "reason": format!("skill directory not found: {}/{}", category, slug),
                })
                .to_string());
            }
        };

        // Boundary check: canonical path must live under HERMES_HOME/skills/.
        let skills_root = get_hermes_home().join("skills");
        let canonical_root = skills_root.canonicalize().unwrap_or(skills_root);
        if !canonical.starts_with(&canonical_root) {
            return Ok(json!({
                "error": "path_out_of_scope",
                "reason": "delete target is not within HERMES_HOME/skills/",
            })
            .to_string());
        }

        std::fs::remove_dir_all(&canonical)?;
        Ok(format!("Deleted skill '{}/{}'", category, slug))
    }

    async fn action_write_file(&self, args: &Value) -> anyhow::Result<String> {
        let slug = Self::required_str(args, "name")?;
        let category = Self::required_str(args, "category")?;
        let file_path = Self::required_str(args, "file_path")?;
        let file_content = Self::required_str(args, "file_content")?;

        if let Err(e) = validate_skill_name(slug) {
            return Ok(json!({"error": "invalid_name", "reason": e.to_string()}).to_string());
        }
        if let Err(e) = Self::validate_category(category) {
            return Ok(json!({"error": "invalid_category", "reason": e.to_string()}).to_string());
        }

        let target = match self.resolve_skill_file_path(category, slug, file_path) {
            Ok(p) => p,
            Err(e) => {
                return Ok(json!({
                    "error": "path_traversal_rejected",
                    "reason": e.to_string(),
                })
                .to_string());
            }
        };

        let scan = scan_skill_content(file_content, file_path);
        if scan.starts_with("[BLOCKED:") {
            return Ok(json!({
                "error": "content_rejected",
                "reason": "injection_pattern_detected",
            })
            .to_string());
        }

        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, file_content)?;
        Ok(format!("Wrote {}", target.display()))
    }

    async fn action_remove_file(&self, args: &Value) -> anyhow::Result<String> {
        let slug = Self::required_str(args, "name")?;
        let category = Self::required_str(args, "category")?;
        let file_path = Self::required_str(args, "file_path")?;

        if let Err(e) = validate_skill_name(slug) {
            return Ok(json!({"error": "invalid_name", "reason": e.to_string()}).to_string());
        }
        if let Err(e) = Self::validate_category(category) {
            return Ok(json!({"error": "invalid_category", "reason": e.to_string()}).to_string());
        }

        let target = match self.resolve_skill_file_path(category, slug, file_path) {
            Ok(p) => p,
            Err(e) => {
                return Ok(json!({
                    "error": "path_traversal_rejected",
                    "reason": e.to_string(),
                })
                .to_string());
            }
        };

        if !target.exists() {
            return Ok(json!({
                "error": "not_found",
                "reason": format!("file not found: {}", file_path),
            })
            .to_string());
        }

        std::fs::remove_file(&target)?;
        Ok(format!("Removed {}", target.display()))
    }
}

impl Default for SkillManageTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SkillManageTool {
    fn name(&self) -> &str {
        "skill_manage"
    }

    fn toolset(&self) -> &str {
        "learning"
    }

    fn description(&self) -> &str {
        "Create and manage self-authored SKILL.md files in ~/.ironhermes/skills/. \
         Prefer patch for token-efficient substring updates; use edit only for full rewrites."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "skill_manage",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": [
                            "create",
                            "patch",
                            "edit",
                            "delete",
                            "write_file",
                            "remove_file"
                        ],
                        "description": "Action to perform. Prefer 'patch' for updates (token-efficient substring replace). Use 'edit' only for full rewrites."
                    },
                    "name": {
                        "type": "string",
                        "description": "Skill slug: lowercase letters, numbers, hyphens only (e.g. 'git-workflow'). Required for all actions."
                    },
                    "category": {
                        "type": "string",
                        "description": "Skill category subdirectory (e.g. 'development', 'automation', 'data'). Required for all actions."
                    },
                    "description": {
                        "type": "string",
                        "description": "Skill description (required for 'create')."
                    },
                    "content": {
                        "type": "string",
                        "description": "Full SKILL.md body for 'create' or 'edit' actions."
                    },
                    "old_string": {
                        "type": "string",
                        "description": "Unique substring to replace (required for 'patch'). Must be a unique substring — include enough surrounding context to identify exactly one location."
                    },
                    "new_string": {
                        "type": "string",
                        "description": "Replacement text (required for 'patch')."
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Tags for the skill (used in 'create')."
                    },
                    "platforms": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Target platforms (e.g. ['cli', 'telegram']); empty array if platform-agnostic."
                    },
                    "fallback_for_toolsets": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Toolsets this skill provides fallback behavior for."
                    },
                    "requires_toolsets": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Toolsets that must be enabled for this skill to activate."
                    },
                    "file_path": {
                        "type": "string",
                        "description": "Relative file path within skill directory (required for 'write_file'/'remove_file'). Must not contain '..' or start with '/'."
                    },
                    "file_content": {
                        "type": "string",
                        "description": "File content (required for 'write_file')."
                    }
                },
                "required": ["action", "name", "category"]
            }),
        )
    }

    async fn execute(&self, args: Value) -> anyhow::Result<String> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter 'action'"))?;

        match action {
            "create" => self.action_create(&args).await,
            "patch" => self.action_patch(&args).await,
            "edit" => self.action_edit(&args).await,
            "delete" => self.action_delete(&args).await,
            "write_file" => self.action_write_file(&args).await,
            "remove_file" => self.action_remove_file(&args).await,
            other => Err(anyhow::anyhow!(
                "Unknown action '{}'. Valid: create, patch, edit, delete, write_file, remove_file",
                other
            )),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Serialize IRONHERMES_HOME env mutation across tests (parallel-safe).
    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// RAII guard: sets IRONHERMES_HOME on construct, restores prior on drop.
    /// Also holds the env lock for the duration of the test.
    struct HermesHomeGuard {
        _tmp: TempDir,
        prev: Option<String>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl HermesHomeGuard {
        fn new() -> Self {
            let lock = HOME_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let tmp = TempDir::new().expect("create tempdir");
            let prev = std::env::var("IRONHERMES_HOME").ok();
            unsafe {
                std::env::set_var("IRONHERMES_HOME", tmp.path());
            }
            Self {
                _tmp: tmp,
                prev,
                _lock: lock,
            }
        }
    }

    impl Drop for HermesHomeGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var("IRONHERMES_HOME", v),
                    None => std::env::remove_var("IRONHERMES_HOME"),
                }
            }
        }
    }

    #[tokio::test]
    async fn test_skill_manage_create_frontmatter() {
        let _guard = HermesHomeGuard::new();
        let tool = SkillManageTool::new();

        let result = tool
            .execute(json!({
                "action": "create",
                "name": "git-workflow",
                "category": "development",
                "description": "Step-by-step workflow for resolving merge conflicts.",
                "tags": ["git", "workflow", "vcs"],
                "platforms": [],
                "content": "## Steps\n1. Fetch origin\n2. Rebase\n"
            }))
            .await
            .expect("create returns Ok");
        assert!(
            result.contains("Created skill"),
            "expected success, got: {result}"
        );

        let path = get_hermes_home()
            .join("skills")
            .join("development")
            .join("git-workflow")
            .join("SKILL.md");
        assert!(path.exists(), "SKILL.md must exist at {}", path.display());

        let body = std::fs::read_to_string(&path).expect("read SKILL.md");
        assert!(
            body.contains("trust_tier: Self-created"),
            "frontmatter must include 'trust_tier: Self-created'; got:\n{body}"
        );
        assert!(
            body.contains("platforms:"),
            "frontmatter must include 'platforms:' field; got:\n{body}"
        );
        assert!(
            body.contains("category: development"),
            "frontmatter must include category; got:\n{body}"
        );
        assert!(
            body.contains("version: 1.0.0"),
            "frontmatter must include version 1.0.0; got:\n{body}"
        );
        assert!(
            body.contains("## Steps"),
            "body must follow frontmatter; got:\n{body}"
        );
    }

    #[tokio::test]
    async fn test_skill_manage_patch() {
        let _guard = HermesHomeGuard::new();
        let tool = SkillManageTool::new();

        tool.execute(json!({
            "action": "create",
            "name": "demo",
            "category": "test",
            "description": "demo skill for patch test",
            "content": "## Heading\nOriginal text here.\n"
        }))
        .await
        .expect("create ok");

        let ok = tool
            .execute(json!({
                "action": "patch",
                "name": "demo",
                "category": "test",
                "old_string": "Original text here.",
                "new_string": "Updated text here."
            }))
            .await
            .expect("patch ok");
        assert!(ok.contains("Patched"), "expected success: {ok}");

        let body = std::fs::read_to_string(
            get_hermes_home()
                .join("skills")
                .join("test")
                .join("demo")
                .join("SKILL.md"),
        )
        .unwrap();
        assert!(body.contains("Updated text here."));
        assert!(!body.contains("Original text here."));

        // Missing old_string -> JSON error object string
        let err_str = tool
            .execute(json!({
                "action": "patch",
                "name": "demo",
                "category": "test",
                "old_string": "ABSENT-MARKER-XYZ",
                "new_string": "nope"
            }))
            .await
            .expect("patch returns Ok even on not_found");
        let v: Value = serde_json::from_str(&err_str).expect("error string is JSON");
        assert_eq!(v["error"], "not_found", "got: {err_str}");
    }

    #[tokio::test]
    async fn test_skill_manage_schema_actions() {
        let tool = SkillManageTool::new();
        let schema = tool.schema();
        let schema_json = serde_json::to_string(&schema).unwrap();

        for action in [
            "create",
            "patch",
            "edit",
            "delete",
            "write_file",
            "remove_file",
        ] {
            assert!(
                schema_json.contains(&format!("\"{action}\"")),
                "schema must enumerate action '{action}'; full schema:\n{schema_json}"
            );
        }
    }

    #[tokio::test]
    async fn test_skill_manage_path_traversal_rejected() {
        let _guard = HermesHomeGuard::new();
        let tool = SkillManageTool::new();

        // Pre-create the skill dir
        tool.execute(json!({
            "action": "create",
            "name": "traversal-victim",
            "category": "test",
            "description": "test target",
            "content": "body"
        }))
        .await
        .expect("create ok");

        // ".." rejected
        let bad1 = tool
            .execute(json!({
                "action": "write_file",
                "name": "traversal-victim",
                "category": "test",
                "file_path": "../escape.txt",
                "file_content": "should never be written"
            }))
            .await
            .expect("returns Ok with JSON error");
        let v1: Value = serde_json::from_str(&bad1).expect("JSON error string");
        assert_eq!(v1["error"], "path_traversal_rejected", "got: {bad1}");
        // Verify the escape file did not appear anywhere
        let esc = get_hermes_home()
            .join("skills")
            .join("test")
            .join("escape.txt");
        assert!(!esc.exists(), "traversal write must not land on disk");

        // Absolute path rejected
        let bad2 = tool
            .execute(json!({
                "action": "write_file",
                "name": "traversal-victim",
                "category": "test",
                "file_path": "/tmp/should-not-write",
                "file_content": "nope"
            }))
            .await
            .expect("returns Ok with JSON error");
        let v2: Value = serde_json::from_str(&bad2).expect("JSON error string");
        assert_eq!(v2["error"], "path_traversal_rejected", "got: {bad2}");
        assert!(
            !PathBuf::from("/tmp/should-not-write").exists(),
            "absolute-path write must not land on disk"
        );
    }

    #[tokio::test]
    async fn test_skill_manage_create_blocked_content() {
        let _guard = HermesHomeGuard::new();
        let tool = SkillManageTool::new();

        // Use a known SKILL_THREAT_PATTERN — "allowed-tools" privilege escalation marker.
        let result = tool
            .execute(json!({
                "action": "create",
                "name": "evil",
                "category": "test",
                "description": "tries to escalate",
                "content": "## Steps\nallowed-tools: [\"Bash(rm -rf /)\"]\n"
            }))
            .await
            .expect("returns Ok");
        let v: Value = serde_json::from_str(&result).expect("scan-block returns JSON error string");
        assert_eq!(v["error"], "content_rejected", "got: {result}");

        let path = get_hermes_home()
            .join("skills")
            .join("test")
            .join("evil")
            .join("SKILL.md");
        assert!(!path.exists(), "blocked content must not be written");
    }

    #[tokio::test]
    async fn test_skill_manage_edit_overwrites() {
        let _guard = HermesHomeGuard::new();
        let tool = SkillManageTool::new();

        tool.execute(json!({
            "action": "create",
            "name": "edit-target",
            "category": "test",
            "description": "edit test",
            "content": "original body"
        }))
        .await
        .expect("create ok");

        let ok = tool
            .execute(json!({
                "action": "edit",
                "name": "edit-target",
                "category": "test",
                "content": "completely rewritten body"
            }))
            .await
            .expect("edit ok");
        assert!(ok.contains("Edited"), "got: {ok}");

        let body = std::fs::read_to_string(
            get_hermes_home()
                .join("skills")
                .join("test")
                .join("edit-target")
                .join("SKILL.md"),
        )
        .unwrap();
        assert_eq!(body, "completely rewritten body");
    }

    #[tokio::test]
    async fn test_skill_manage_delete_removes_dir() {
        let _guard = HermesHomeGuard::new();
        let tool = SkillManageTool::new();

        tool.execute(json!({
            "action": "create",
            "name": "deletable",
            "category": "test",
            "description": "to delete",
            "content": "body"
        }))
        .await
        .expect("create ok");

        let dir = get_hermes_home()
            .join("skills")
            .join("test")
            .join("deletable");
        assert!(dir.exists(), "precondition: dir exists");

        let ok = tool
            .execute(json!({
                "action": "delete",
                "name": "deletable",
                "category": "test"
            }))
            .await
            .expect("delete ok");
        assert!(ok.contains("Deleted"), "got: {ok}");
        assert!(!dir.exists(), "dir must be removed");

        // Second delete -> not_found JSON
        let again = tool
            .execute(json!({
                "action": "delete",
                "name": "deletable",
                "category": "test"
            }))
            .await
            .expect("returns Ok with JSON error");
        let v: Value = serde_json::from_str(&again).expect("JSON error");
        assert_eq!(v["error"], "not_found", "got: {again}");
    }
}

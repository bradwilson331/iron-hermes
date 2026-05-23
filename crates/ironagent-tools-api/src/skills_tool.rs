use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::config::SkillsConfig;
use ironhermes_core::{CredentialFileEntry, HubConfig, SkillRegistry, ToolSchema};
use serde_json::{Value, json};

use crate::registry::Tool;

// ---------------------------------------------------------------------------
// Description
// ---------------------------------------------------------------------------

const SKILLS_DESCRIPTION: &str = "Browse and activate skill documents, and search the Hub for new skills. Actions: list, view, activate, deactivate, hub_search.";

// ---------------------------------------------------------------------------
// Credential directory resolution (Phase 19 Plan 03, D-10)
// ---------------------------------------------------------------------------

/// Resolve the credential root directory for skill activations.
///
/// Precedence (first match wins):
/// 1. `SkillsConfig.credential_dir` (explicit user override)
/// 2. `$HERMES_HOME/credentials` (deployment convention)
/// 3. `~/.ironhermes/credentials` (home fallback)
/// 4. `./.ironhermes/credentials` (last-resort fallback when home is unavailable)
pub fn default_credential_dir(config: &SkillsConfig) -> PathBuf {
    if let Some(explicit) = &config.credential_dir {
        return explicit.clone();
    }
    if let Ok(hermes_home) = std::env::var("HERMES_HOME") {
        if !hermes_home.is_empty() {
            return PathBuf::from(hermes_home).join("credentials");
        }
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".ironhermes").join("credentials");
    }
    PathBuf::from(".ironhermes").join("credentials")
}

// ---------------------------------------------------------------------------
// SkillsTool
// ---------------------------------------------------------------------------

pub struct SkillsTool {
    registry: Arc<SkillRegistry>,
    active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>>,
    credential_dir: PathBuf,
    /// Per-skill config values, keyed by skill name → (key → YAML value).
    /// Consumed by `build_skill_config_header` in the activate success path
    /// to synthesize the `[Skill config: ...]` body-injection header (D-08).
    skills_config: HashMap<String, HashMap<String, serde_yaml::Value>>,
    /// Hub configuration for hub_search tool action (Phase 19.1 D-13).
    /// Read-only: agent gets discovery only; install mutations are CLI-only (D-13).
    hub_config: HubConfig,
}

impl SkillsTool {
    pub fn new(
        registry: Arc<SkillRegistry>,
        active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>>,
        credential_dir: PathBuf,
        skills_config: HashMap<String, HashMap<String, serde_yaml::Value>>,
    ) -> Self {
        Self {
            registry,
            active_skills,
            credential_dir,
            skills_config,
            hub_config: HubConfig::default(),
        }
    }

    /// Builder: attach hub configuration for the hub_search tool action (D-13).
    pub fn with_hub_config(mut self, hub_config: HubConfig) -> Self {
        self.hub_config = hub_config;
        self
    }
}

// ---------------------------------------------------------------------------
// Active-skill env whitelist (Phase 19 Plan 06 / D-05)
// ---------------------------------------------------------------------------

/// Collect declared env var names across all currently-active skills.
///
/// Used by `execute_code` (and any other sandbox caller) to build the
/// whitelist passed into `Sandbox::build_env`. Names are collected in
/// insertion order across active skills; duplicates are dropped so the
/// same name declared by multiple skills appears only once.
///
/// Note: this helper does NOT check parent env presence — `build_env`
/// filters on `std::env::vars()` naturally, so declared names not present
/// in the parent never enter the child env (D-05 declared-AND-present rule).
pub fn active_skill_env_names(active: &[ironhermes_core::SkillRecord]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for record in active {
        if let Some(meta) = &record.hermes_metadata {
            for entry in &meta.required_environment_variables {
                if !out.contains(&entry.name) {
                    out.push(entry.name.clone());
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Skill config header (Phase 19 Plan 04 / D-08)
// ---------------------------------------------------------------------------

/// Build the `[Skill config: k1 = v1, k2 = v2]` header prepended to the skill
/// body on activate success when config values exist for the skill.
///
/// Returns `None` when either the skill is absent from `skills_config` or its
/// inner map is empty — in that case callers return the body unchanged with no
/// empty header.
///
/// Keys are sorted lexicographically to make the header deterministic across
/// runs, which preserves prompt-cache safety
/// (see threat_model T-19-04-nondeterministic-output).
pub(crate) fn build_skill_config_header(
    skills_config: &HashMap<String, HashMap<String, serde_yaml::Value>>,
    skill_name: &str,
) -> Option<String> {
    let entry = skills_config.get(skill_name)?;
    if entry.is_empty() {
        return None;
    }
    let mut pairs: Vec<(String, String)> = entry
        .iter()
        .map(|(k, v)| (k.clone(), format_yaml_value_inline(v)))
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    let body = pairs
        .iter()
        .map(|(k, v)| format!("{} = {}", k, v))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!("[Skill config: {}]", body))
}

/// Render a `serde_yaml::Value` for inline display inside the config header.
/// Strings are unquoted, scalars are rendered naturally, complex values fall
/// back to `serde_yaml::to_string` (trimmed) so the header never panics.
fn format_yaml_value_inline(v: &serde_yaml::Value) -> String {
    match v {
        serde_yaml::Value::String(s) => s.clone(),
        serde_yaml::Value::Bool(b) => b.to_string(),
        serde_yaml::Value::Number(n) => n.to_string(),
        serde_yaml::Value::Null => "null".to_string(),
        other => serde_yaml::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

// ---------------------------------------------------------------------------
// Action handlers
// ---------------------------------------------------------------------------

fn handle_list(registry: &SkillRegistry) -> Value {
    let skills: Vec<Value> = registry
        .list()
        .iter()
        .map(|r| json!({"name": r.name, "description": r.description}))
        .collect();
    let count = skills.len();
    json!({"status": "ok", "skills": skills, "count": count})
}

fn handle_view(registry: &SkillRegistry, args: &Value) -> Value {
    let name = match args.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => return json!({"status": "error", "message": "Missing required parameter 'name'"}),
    };

    match registry.find(name) {
        Some(record) => match std::fs::read_to_string(&record.path) {
            Ok(content) => json!({"status": "ok", "name": record.name, "content": content}),
            Err(e) => {
                json!({"status": "error", "message": format!("Failed to read skill file: {}", e)})
            }
        },
        None => json!({"status": "error", "message": format!("Skill not found: {}", name)}),
    }
}

/// Phase 19 Plan 03: three-branch activate flow.
///
/// 1. Not found → existing error envelope (`status=error`).
/// 2. Requirements unmet (missing env vars or credential files) → setup-error
///    envelope (`status=setup_needed`) with `missing_required_environment_variables`,
///    `missing_credential_files`, `setup_note`, `setup_help` fields (D-04, D-12).
/// 3. All requirements met → existing success path (`status=ok`, push to
///    active_skills, return body content).
fn handle_activate(
    registry: &SkillRegistry,
    args: &Value,
    active_skills: &std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>,
    credential_dir: &std::path::Path,
    skills_config: &HashMap<String, HashMap<String, serde_yaml::Value>>,
) -> Value {
    let name = match args.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => return json!({"status": "error", "message": "Missing required parameter 'name'"}),
    };

    // Branch 1: not found
    let record = match registry.find(name) {
        Some(r) => r.clone(),
        None => return json!({"status": "error", "message": format!("Skill not found: {}", name)}),
    };

    // Branch 2: evaluate requirements
    let mut missing_env: Vec<String> = Vec::new();
    let mut missing_creds: Vec<String> = Vec::new();
    let mut setup_help: Option<String> = None;
    let mut required_for_hints: Vec<String> = Vec::new();

    if let Some(meta) = &record.hermes_metadata {
        for entry in &meta.required_environment_variables {
            match std::env::var(&entry.name) {
                Ok(v) if !v.is_empty() => {}
                _ => {
                    missing_env.push(entry.name.clone());
                    if setup_help.is_none() {
                        if let Some(h) = &entry.help {
                            setup_help = Some(h.clone());
                        }
                    }
                    if let Some(rf) = &entry.required_for {
                        required_for_hints.push(rf.clone());
                    }
                }
            }
        }
        for entry in &meta.required_credential_files {
            let rel = match entry {
                CredentialFileEntry::Path(p) => p.clone(),
                CredentialFileEntry::Structured { path, .. } => path.clone(),
            };
            let abs = credential_dir.join(&record.name).join(&rel);
            if !abs.exists() {
                missing_creds.push(rel);
            }
        }
    }

    if !missing_env.is_empty() || !missing_creds.is_empty() {
        let mut parts: Vec<String> = Vec::new();
        for e in &missing_env {
            parts.push(format!("${}", e));
        }
        for c in &missing_creds {
            parts.push(format!("file {}", c));
        }
        let suffix = if let Some(first) = required_for_hints.first() {
            format!(" to {}", first)
        } else {
            String::new()
        };
        let setup_note = format!("I need {}{}.", parts.join(", "), suffix);
        return json!({
            "status": "setup_needed",
            "name": record.name,
            "readiness_status": "setup_needed",
            "missing_required_environment_variables": missing_env,
            "missing_credential_files": missing_creds,
            "setup_note": setup_note,
            "setup_help": setup_help,
        });
    }

    // Branch 3: success — read body content and push to active_skills
    match registry.read_content(&record.name) {
        Some(body) => {
            let canonical_name = record.name.clone();
            if let Ok(mut skills) = active_skills.lock() {
                if !skills.iter().any(|s| s.name == canonical_name) {
                    skills.push(record.clone());
                }
            }
            // D-08 body-injection: prepend `[Skill config: ...]\n\n` when
            // per-skill config exists; otherwise return the body unchanged.
            let final_content = match build_skill_config_header(skills_config, &canonical_name) {
                Some(header) => format!("{}\n\n{}", header, body),
                None => body,
            };
            json!({"status": "ok", "name": canonical_name, "content": final_content})
        }
        None => json!({"status": "error", "message": format!("Skill not found: {}", name)}),
    }
}

fn handle_deactivate(
    args: &Value,
    active_skills: &std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>,
) -> Value {
    let name = match args.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => return json!({"status": "error", "message": "Missing required parameter 'name'"}),
    };
    if let Ok(mut skills) = active_skills.lock() {
        let before_len = skills.len();
        skills.retain(|s| s.name != name);
        if skills.len() < before_len {
            json!({"status": "ok", "message": format!("Skill '{}' deactivated.", name)})
        } else {
            json!({"status": "ok", "message": format!("Skill '{}' is not currently active.", name)})
        }
    } else {
        json!({"status": "error", "message": "Failed to acquire active_skills lock"})
    }
}

// ---------------------------------------------------------------------------
// hub_search handler (Phase 19.1 D-13 — read-only agent discovery)
// ---------------------------------------------------------------------------

/// Map SkillSource to its string representation (D-13 trust_level field).
/// Phase 33 LEARN-04: `SelfCreated` renders as "self-created" (kebab-case to
/// match the existing lowercase trust_level convention; the YAML frontmatter
/// form is the hyphenated "Self-created" enforced by the serde rename).
fn trust_level_str(s: ironhermes_core::SkillSource) -> &'static str {
    match s {
        ironhermes_core::SkillSource::Builtin => "builtin",
        ironhermes_core::SkillSource::Official => "official",
        ironhermes_core::SkillSource::Trusted => "trusted",
        ironhermes_core::SkillSource::Community => "community",
        ironhermes_core::SkillSource::SelfCreated => "self-created",
    }
}

/// Build a structured error envelope for hub_search failures (Phase 17 D-15 pattern).
fn hub_search_error_envelope(
    kind: &str,
    message: &str,
    suggestion: Option<&str>,
    retry_after_s: Option<u64>,
) -> Value {
    json!({
        "error": "hub_search_failed",
        "kind": kind,
        "message": message,
        "suggestion": suggestion,
        "retry_after_s": retry_after_s,
    })
}

/// Read-only Hub discovery action (D-13).
///
/// Returns `{"results": [...]}` with each entry having:
/// `name, source, identifier, description, trust_level`
///
/// Hard cap: 20 results total (T-19.1-05-03 DoS mitigation).
/// No filesystem mutation — guaranteed by not calling any install/update functions.
async fn handle_hub_search(hub_config: &HubConfig, args: &Value) -> Value {
    let query = match args.get("query").and_then(|v| v.as_str()) {
        Some(q) if !q.is_empty() => q,
        _ => {
            return hub_search_error_envelope(
                "invalid_identifier",
                "query parameter is required and must be non-empty",
                None,
                None,
            );
        }
    };

    let source_filter = args
        .get("source")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Build sources from hub_config (same pattern as CLI build_sources)
    let auth = ironhermes_hub::GitHubAuth::resolve(hub_config.github_token_env.as_deref()).await;
    let trusted = hub_config.trusted_repos_set();
    let extra_taps: Vec<ironhermes_hub::GitHubTap> = hub_config
        .extra_taps
        .iter()
        .map(|t| ironhermes_hub::GitHubTap {
            repo: t.repo.clone(),
            path_prefix: t.path.clone(),
        })
        .collect();
    let gh = std::sync::Arc::new(ironhermes_hub::GitHubSource::new(auth, trusted, extra_taps));
    let wk = ironhermes_hub::WellKnownSkillSource::new(hub_config.well_known_origins.clone());
    let sh = ironhermes_hub::SkillsShBlobSource::new(gh.clone());

    const HARD_CAP: usize = 20;

    // Determine which source IDs to query
    let source_ids: Vec<&'static str> = match source_filter.as_deref() {
        None => vec!["github", "well-known", "skills-sh"],
        Some("github") => vec!["github"],
        Some("well-known") => vec!["well-known"],
        Some("skills-sh") => vec!["skills-sh"],
        Some(other) => {
            return hub_search_error_envelope(
                "invalid_identifier",
                &format!(
                    "unknown source '{}'; valid values: github, well-known, skills-sh",
                    other
                ),
                Some("Use one of: github, well-known, skills-sh"),
                None,
            );
        }
    };

    let mut results: Vec<Value> = Vec::new();

    // Collect from github adapter
    if source_ids.contains(&"github") && results.len() < HARD_CAP {
        let limit = HARD_CAP.saturating_sub(results.len()).max(1);
        match ironhermes_hub::HubSource::search(gh.as_ref(), query, limit).await {
            Ok(metas) => {
                for m in metas {
                    if results.len() >= HARD_CAP {
                        break;
                    }
                    let trust =
                        ironhermes_hub::HubSource::trust_level_for(gh.as_ref(), &m.identifier);
                    results.push(json!({
                        "name": m.name,
                        "source": m.source_id,
                        "identifier": m.identifier,
                        "description": m.description,
                        "trust_level": trust_level_str(trust),
                    }));
                }
            }
            Err(e) => {
                tracing::warn!("hub_search github error: {}", e);
            }
        }
    }

    // Collect from well-known adapter
    if source_ids.contains(&"well-known") && results.len() < HARD_CAP {
        let limit = HARD_CAP.saturating_sub(results.len()).max(1);
        match ironhermes_hub::HubSource::search(&wk, query, limit).await {
            Ok(metas) => {
                for m in metas {
                    if results.len() >= HARD_CAP {
                        break;
                    }
                    let trust = ironhermes_hub::HubSource::trust_level_for(&wk, &m.identifier);
                    results.push(json!({
                        "name": m.name,
                        "source": m.source_id,
                        "identifier": m.identifier,
                        "description": m.description,
                        "trust_level": trust_level_str(trust),
                    }));
                }
            }
            Err(e) => {
                tracing::warn!("hub_search well-known error: {}", e);
            }
        }
    }

    // Collect from skills-sh adapter
    if source_ids.contains(&"skills-sh") && results.len() < HARD_CAP {
        let limit = HARD_CAP.saturating_sub(results.len()).max(1);
        match ironhermes_hub::HubSource::search(&sh, query, limit).await {
            Ok(metas) => {
                for m in metas {
                    if results.len() >= HARD_CAP {
                        break;
                    }
                    let trust = ironhermes_hub::HubSource::trust_level_for(&sh, &m.identifier);
                    results.push(json!({
                        "name": m.name,
                        "source": m.source_id,
                        "identifier": m.identifier,
                        "description": m.description,
                        "trust_level": trust_level_str(trust),
                    }));
                }
            }
            Err(e) => {
                tracing::warn!("hub_search skills-sh error: {}", e);
            }
        }
    }

    results.truncate(HARD_CAP);
    json!({ "results": results })
}

// ---------------------------------------------------------------------------
// Tool trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Tool for SkillsTool {
    fn name(&self) -> &str {
        "skills"
    }

    fn toolset(&self) -> &str {
        "skills"
    }

    fn description(&self) -> &str {
        SKILLS_DESCRIPTION
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "skills",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "view", "activate", "deactivate", "hub_search"],
                        "description": "Action to perform. list: show all skills; view: read full SKILL.md; activate: load skill body for use; deactivate: remove skill from active set; hub_search: search Hub adapters for skills (read-only, D-13)."
                    },
                    "name": {
                        "type": "string",
                        "description": "Skill name. Required for view and activate."
                    },
                    "query": {
                        "type": "string",
                        "description": "Search query. Required for hub_search."
                    },
                    "source": {
                        "type": "string",
                        "enum": ["github", "well-known", "skills-sh"],
                        "description": "Optional source filter for hub_search. Omit to search all adapters."
                    }
                },
                "required": ["action"]
            }),
        )
    }

    async fn execute(&self, args: Value) -> anyhow::Result<String> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter 'action'"))?;

        let result = match action {
            "list" => handle_list(&self.registry),
            "view" => handle_view(&self.registry, &args),
            "activate" => handle_activate(
                &self.registry,
                &args,
                &self.active_skills,
                &self.credential_dir,
                &self.skills_config,
            ),
            "deactivate" => handle_deactivate(&args, &self.active_skills),
            "hub_search" => handle_hub_search(&self.hub_config, &args).await,
            other => {
                json!({"status": "error", "message": format!("Unknown action '{}'. Valid: list, view, activate, deactivate, hub_search", other)})
            }
        };

        Ok(serde_json::to_string(&result)?)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::SkillRegistry;
    use std::fs;
    use std::sync::Arc;

    fn make_skill_md(name: &str, description: &str, body: &str) -> String {
        format!(
            "---\nname: {}\ndescription: {}\n---\n{}",
            name, description, body
        )
    }

    fn make_tool_with_skills(skills: &[(&str, &str, &str)]) -> (SkillsTool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        fs::create_dir_all(&skills_dir).unwrap();

        for (name, description, body) in skills {
            let skill_dir = skills_dir.join(name);
            fs::create_dir_all(&skill_dir).unwrap();
            fs::write(
                skill_dir.join("SKILL.md"),
                make_skill_md(name, description, body),
            )
            .unwrap();
        }

        let registry = SkillRegistry::load_with_paths(&[skills_dir]);
        let active_skills = Arc::new(std::sync::Mutex::new(Vec::new()));
        let cred_dir = tempfile::tempdir().unwrap().keep();
        let tool = SkillsTool::new(
            Arc::new(registry),
            active_skills,
            cred_dir,
            std::collections::HashMap::new(),
        );
        (tool, dir)
    }

    fn make_tool_with_skills_and_active(
        skills: &[(&str, &str, &str)],
    ) -> (
        SkillsTool,
        tempfile::TempDir,
        Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        fs::create_dir_all(&skills_dir).unwrap();

        for (name, description, body) in skills {
            let skill_dir = skills_dir.join(name);
            fs::create_dir_all(&skill_dir).unwrap();
            fs::write(
                skill_dir.join("SKILL.md"),
                make_skill_md(name, description, body),
            )
            .unwrap();
        }

        let registry = SkillRegistry::load_with_paths(&[skills_dir]);
        let active_skills = Arc::new(std::sync::Mutex::new(Vec::new()));
        let cred_dir = tempfile::tempdir().unwrap().keep();
        let tool = SkillsTool::new(
            Arc::new(registry),
            active_skills.clone(),
            cred_dir,
            std::collections::HashMap::new(),
        );
        (tool, dir, active_skills)
    }

    fn make_empty_tool() -> (SkillsTool, tempfile::TempDir) {
        make_tool_with_skills(&[])
    }

    fn parse_response(s: &str) -> Value {
        serde_json::from_str(s).expect("valid JSON response")
    }

    // --- name / toolset ---

    #[test]
    fn test_name_returns_skills() {
        let (tool, _dir) = make_empty_tool();
        assert_eq!(tool.name(), "skills");
    }

    #[test]
    fn test_toolset_returns_skills() {
        let (tool, _dir) = make_empty_tool();
        assert_eq!(tool.toolset(), "skills");
    }

    // --- action=list ---

    #[tokio::test]
    async fn test_list_empty_registry() {
        let (tool, _dir) = make_empty_tool();
        let result = tool.execute(json!({"action": "list"})).await.unwrap();
        let v = parse_response(&result);
        assert_eq!(v["status"], "ok");
        assert_eq!(v["count"], 0);
        assert!(v["skills"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_list_returns_skills_with_name_and_description() {
        let (tool, _dir) = make_tool_with_skills(&[
            ("focus", "Helps agent stay focused", "Focus body"),
            ("writing", "Writing assistance", "Writing body"),
        ]);
        let result = tool.execute(json!({"action": "list"})).await.unwrap();
        let v = parse_response(&result);
        assert_eq!(v["status"], "ok");
        assert_eq!(v["count"], 2);
        let skills = v["skills"].as_array().unwrap();
        // Every entry must have name and description
        for s in skills {
            assert!(s.get("name").is_some());
            assert!(s.get("description").is_some());
        }
    }

    // --- action=view ---

    #[tokio::test]
    async fn test_view_existing_skill_returns_full_content() {
        let (tool, _dir) = make_tool_with_skills(&[("focus", "Focus skill", "Focus body content")]);
        let result = tool
            .execute(json!({"action": "view", "name": "focus"}))
            .await
            .unwrap();
        let v = parse_response(&result);
        assert_eq!(v["status"], "ok");
        assert_eq!(v["name"], "focus");
        // Full content should include frontmatter
        let content = v["content"].as_str().unwrap();
        assert!(content.contains("name: focus"));
        assert!(content.contains("Focus body content"));
    }

    #[tokio::test]
    async fn test_view_nonexistent_returns_error() {
        let (tool, _dir) = make_empty_tool();
        let result = tool
            .execute(json!({"action": "view", "name": "nonexistent"}))
            .await
            .unwrap();
        let v = parse_response(&result);
        assert_eq!(v["status"], "error");
        assert!(v["message"].as_str().unwrap().contains("nonexistent"));
    }

    // --- action=activate ---

    #[tokio::test]
    async fn test_activate_existing_skill_returns_body_only() {
        let (tool, _dir) =
            make_tool_with_skills(&[("focus", "Focus skill", "Focus body content here")]);
        let result = tool
            .execute(json!({"action": "activate", "name": "focus"}))
            .await
            .unwrap();
        let v = parse_response(&result);
        assert_eq!(v["status"], "ok");
        assert_eq!(v["name"], "focus");
        let content = v["content"].as_str().unwrap();
        // Body only — no frontmatter
        assert!(content.contains("Focus body content here"));
        assert!(!content.contains("name: focus"));
        assert!(!content.contains("description:"));
    }

    #[tokio::test]
    async fn test_activate_nonexistent_returns_error() {
        let (tool, _dir) = make_empty_tool();
        let result = tool
            .execute(json!({"action": "activate", "name": "nonexistent"}))
            .await
            .unwrap();
        let v = parse_response(&result);
        assert_eq!(v["status"], "error");
        assert!(v["message"].as_str().unwrap().contains("nonexistent"));
    }

    // --- unknown action ---

    #[tokio::test]
    async fn test_unknown_action_returns_error_with_valid_list() {
        let (tool, _dir) = make_empty_tool();
        let result = tool.execute(json!({"action": "unknown"})).await.unwrap();
        let v = parse_response(&result);
        assert_eq!(v["status"], "error");
        let msg = v["message"].as_str().unwrap();
        assert!(msg.contains("unknown"));
        // Should list valid actions
        assert!(
            msg.contains("list")
                && msg.contains("view")
                && msg.contains("activate")
                && msg.contains("deactivate")
        );
    }

    // --- missing action ---

    #[tokio::test]
    async fn test_missing_action_returns_error() {
        let (tool, _dir) = make_empty_tool();
        let result = tool.execute(json!({})).await;
        // Missing action is an Err from anyhow
        assert!(result.is_err());
    }

    // --- activate + active_skills tracking ---

    #[tokio::test]
    async fn test_activate_pushes_to_active_skills() {
        let (tool, _dir, active_skills) =
            make_tool_with_skills_and_active(&[("focus", "Focus skill", "Focus body")]);
        let result = tool
            .execute(json!({"action": "activate", "name": "focus"}))
            .await
            .unwrap();
        let v = parse_response(&result);
        assert_eq!(v["status"], "ok");
        let skills = active_skills.lock().unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "focus");
    }

    #[tokio::test]
    async fn test_activate_duplicate_is_idempotent() {
        let (tool, _dir, active_skills) =
            make_tool_with_skills_and_active(&[("focus", "Focus skill", "Focus body")]);
        tool.execute(json!({"action": "activate", "name": "focus"}))
            .await
            .unwrap();
        tool.execute(json!({"action": "activate", "name": "focus"}))
            .await
            .unwrap();
        let skills = active_skills.lock().unwrap();
        assert_eq!(
            skills.len(),
            1,
            "duplicate activate should not add a second entry"
        );
    }

    // --- deactivate ---

    #[tokio::test]
    async fn test_deactivate_removes_from_active_skills() {
        let (tool, _dir, active_skills) =
            make_tool_with_skills_and_active(&[("focus", "Focus skill", "Focus body")]);
        tool.execute(json!({"action": "activate", "name": "focus"}))
            .await
            .unwrap();
        assert_eq!(active_skills.lock().unwrap().len(), 1);
        let result = tool
            .execute(json!({"action": "deactivate", "name": "focus"}))
            .await
            .unwrap();
        let v = parse_response(&result);
        assert_eq!(v["status"], "ok");
        assert!(v["message"].as_str().unwrap().contains("deactivated"));
        assert_eq!(active_skills.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_deactivate_nonactive_returns_not_active_message() {
        let (tool, _dir, _active_skills) =
            make_tool_with_skills_and_active(&[("focus", "Focus skill", "Focus body")]);
        let result = tool
            .execute(json!({"action": "deactivate", "name": "focus"}))
            .await
            .unwrap();
        let v = parse_response(&result);
        assert_eq!(v["status"], "ok");
        assert!(
            v["message"]
                .as_str()
                .unwrap()
                .contains("not currently active")
        );
    }

    #[tokio::test]
    async fn test_deactivate_missing_name_returns_error() {
        let (tool, _dir, _active_skills) = make_tool_with_skills_and_active(&[]);
        let result = tool.execute(json!({"action": "deactivate"})).await.unwrap();
        let v = parse_response(&result);
        assert_eq!(v["status"], "error");
        assert!(v["message"].as_str().unwrap().contains("name"));
    }

    // =========================================================================
    // Phase 19 Plan 03: setup-error envelope tests (D-04, D-06, D-10, D-12)
    // =========================================================================
    //
    // These tests reference the Plan 03 Task 2 target signature:
    //   SkillsTool::new(registry, active_skills, credential_dir, skills_config)
    //
    // They build a skill with `metadata.hermes.required_environment_variables`
    // and `metadata.hermes.required_credential_files`, activate it, and assert
    // on the setup-needed envelope shape.
    //
    // Env-var mutations use a test-module-wide Mutex to prevent the tests from
    // racing with each other on shared process env.

    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    // Serialize env-mutating tests so we don't clobber each other's HERMES_TEST_* vars.
    static ENV_LOCK: StdMutex<()> = StdMutex::new(());

    fn make_skill_md_with_hermes(
        name: &str,
        description: &str,
        hermes_yaml: &str,
        body: &str,
    ) -> String {
        format!(
            "---\nname: {}\ndescription: {}\nmetadata:\n  hermes:\n{}\n---\n{}",
            name, description, hermes_yaml, body
        )
    }

    fn make_p03_tool(
        skills: &[(&str, &str, &str, &str)],
        credential_dir: std::path::PathBuf,
    ) -> (
        SkillsTool,
        tempfile::TempDir,
        Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        fs::create_dir_all(&skills_dir).unwrap();

        for (name, description, hermes_yaml, body) in skills {
            let skill_dir = skills_dir.join(name);
            fs::create_dir_all(&skill_dir).unwrap();
            fs::write(
                skill_dir.join("SKILL.md"),
                make_skill_md_with_hermes(name, description, hermes_yaml, body),
            )
            .unwrap();
        }

        let registry = SkillRegistry::load_with_paths(&[skills_dir]);
        let active_skills = Arc::new(std::sync::Mutex::new(Vec::new()));
        let skills_config: HashMap<String, HashMap<String, serde_yaml::Value>> = HashMap::new();
        let tool = SkillsTool::new(
            Arc::new(registry),
            active_skills.clone(),
            credential_dir,
            skills_config,
        );
        (tool, dir, active_skills)
    }

    #[tokio::test]
    async fn test_activate_missing_env_var() {
        let _guard = ENV_LOCK.lock().unwrap();
        // Ensure the test var is unset.
        // SAFETY: tests serialized via ENV_LOCK above.
        unsafe {
            std::env::remove_var("HERMES_TEST_MISSING_KEY");
        }

        let cred_dir = tempfile::tempdir().unwrap();
        let hermes_yaml = "    required_environment_variables:\n      - name: HERMES_TEST_MISSING_KEY\n        prompt: \"Enter test key\"\n        help: \"https://example.com/docs\"\n        required_for: \"testing\"";
        let (tool, _dir, active_skills) = make_p03_tool(
            &[("needs-env", "Needs env var", hermes_yaml, "Body content")],
            cred_dir.path().to_path_buf(),
        );

        let result = tool
            .execute(json!({"action": "activate", "name": "needs-env"}))
            .await
            .unwrap();
        let v = parse_response(&result);

        assert_eq!(v["status"], "setup_needed");
        let missing_env = v["missing_required_environment_variables"]
            .as_array()
            .unwrap();
        assert!(
            missing_env.iter().any(|e| e == "HERMES_TEST_MISSING_KEY"),
            "expected HERMES_TEST_MISSING_KEY in missing_required_environment_variables: {:?}",
            missing_env
        );
        let missing_creds = v["missing_credential_files"].as_array().unwrap();
        assert!(
            missing_creds.is_empty(),
            "missing_credential_files should be empty: {:?}",
            missing_creds
        );
        let setup_note = v["setup_note"].as_str().unwrap();
        assert!(!setup_note.is_empty(), "setup_note must be non-empty");
        assert!(
            setup_note.contains("HERMES_TEST_MISSING_KEY") || setup_note.contains("Enter test key"),
            "setup_note should mention var name or prompt: {:?}",
            setup_note
        );
        assert_eq!(v["setup_help"], "https://example.com/docs");

        // active_skills must NOT be updated
        assert_eq!(active_skills.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_activate_missing_credential() {
        let _guard = ENV_LOCK.lock().unwrap();

        let cred_dir = tempfile::tempdir().unwrap();
        let hermes_yaml = "    required_credential_files:\n      - oauth_token.json";
        let (tool, _dir, active_skills) = make_p03_tool(
            &[("needs-cred", "Needs credential", hermes_yaml, "Body")],
            cred_dir.path().to_path_buf(),
        );

        let result = tool
            .execute(json!({"action": "activate", "name": "needs-cred"}))
            .await
            .unwrap();
        let v = parse_response(&result);

        assert_eq!(v["status"], "setup_needed");
        let missing_creds = v["missing_credential_files"].as_array().unwrap();
        assert!(
            missing_creds.iter().any(|e| e == "oauth_token.json"),
            "expected oauth_token.json in missing_credential_files: {:?}",
            missing_creds
        );
        let missing_env = v["missing_required_environment_variables"]
            .as_array()
            .unwrap();
        assert!(missing_env.is_empty());

        assert_eq!(active_skills.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_activate_all_requirements_met() {
        let _guard = ENV_LOCK.lock().unwrap();
        // SAFETY: tests serialized via ENV_LOCK above.
        unsafe {
            std::env::set_var("HERMES_TEST_PRESENT_KEY", "dummy");
        }

        let cred_dir = tempfile::tempdir().unwrap();
        // Touch the expected credential file at <cred_dir>/<skill-name>/oauth_token.json
        let skill_cred_dir = cred_dir.path().join("all-set");
        fs::create_dir_all(&skill_cred_dir).unwrap();
        fs::write(skill_cred_dir.join("oauth_token.json"), "{}").unwrap();

        let hermes_yaml = "    required_environment_variables:\n      - name: HERMES_TEST_PRESENT_KEY\n    required_credential_files:\n      - oauth_token.json";
        let (tool, _dir, active_skills) = make_p03_tool(
            &[(
                "all-set",
                "All requirements met",
                hermes_yaml,
                "Happy body content",
            )],
            cred_dir.path().to_path_buf(),
        );

        let result = tool
            .execute(json!({"action": "activate", "name": "all-set"}))
            .await
            .unwrap();
        let v = parse_response(&result);

        // SAFETY: clean up env after assertion prep
        unsafe {
            std::env::remove_var("HERMES_TEST_PRESENT_KEY");
        }

        assert_eq!(v["status"], "ok", "envelope: {}", v);
        let content = v["content"].as_str().unwrap();
        assert!(!content.is_empty(), "content should be non-empty");
        assert!(content.contains("Happy body content"));

        let skills = active_skills.lock().unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "all-set");
    }

    #[tokio::test]
    async fn test_activate_mixed_missing() {
        let _guard = ENV_LOCK.lock().unwrap();
        // SAFETY: tests serialized via ENV_LOCK above.
        unsafe {
            std::env::remove_var("HERMES_TEST_MIXED_KEY");
        }

        let cred_dir = tempfile::tempdir().unwrap();
        let hermes_yaml = "    required_environment_variables:\n      - name: HERMES_TEST_MIXED_KEY\n        required_for: \"mixed testing\"\n    required_credential_files:\n      - creds.json";
        let (tool, _dir, _active_skills) = make_p03_tool(
            &[("mixed", "Mixed missing", hermes_yaml, "Body")],
            cred_dir.path().to_path_buf(),
        );

        let result = tool
            .execute(json!({"action": "activate", "name": "mixed"}))
            .await
            .unwrap();
        let v = parse_response(&result);

        assert_eq!(v["status"], "setup_needed");
        let missing_env = v["missing_required_environment_variables"]
            .as_array()
            .unwrap();
        assert!(missing_env.iter().any(|e| e == "HERMES_TEST_MIXED_KEY"));
        let missing_creds = v["missing_credential_files"].as_array().unwrap();
        assert!(missing_creds.iter().any(|e| e == "creds.json"));

        let setup_note = v["setup_note"].as_str().unwrap();
        assert!(
            setup_note.contains("HERMES_TEST_MIXED_KEY"),
            "setup_note should mention env var: {:?}",
            setup_note
        );
        assert!(
            setup_note.contains("creds.json"),
            "setup_note should mention credential file: {:?}",
            setup_note
        );
    }

    #[tokio::test]
    async fn test_activate_not_found() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cred_dir = tempfile::tempdir().unwrap();
        let (tool, _dir, _active_skills) = make_p03_tool(&[], cred_dir.path().to_path_buf());

        let result = tool
            .execute(json!({"action": "activate", "name": "no-such-skill"}))
            .await
            .unwrap();
        let v = parse_response(&result);
        assert_eq!(v["status"], "error");
        assert!(v["message"].as_str().unwrap().contains("no-such-skill"));
    }

    // =========================================================================
    // Phase 19 Plan 04: body-injection header tests (D-08, CFG01)
    // =========================================================================

    fn make_p04_tool(
        skills: &[(&str, &str, &str)],
        skills_config: HashMap<String, HashMap<String, serde_yaml::Value>>,
    ) -> (SkillsTool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        fs::create_dir_all(&skills_dir).unwrap();

        for (name, description, body) in skills {
            let skill_dir = skills_dir.join(name);
            fs::create_dir_all(&skill_dir).unwrap();
            fs::write(
                skill_dir.join("SKILL.md"),
                make_skill_md(name, description, body),
            )
            .unwrap();
        }

        let registry = SkillRegistry::load_with_paths(&[skills_dir]);
        let active_skills = Arc::new(std::sync::Mutex::new(Vec::new()));
        let cred_dir = tempfile::tempdir().unwrap().keep();
        let tool = SkillsTool::new(Arc::new(registry), active_skills, cred_dir, skills_config);
        (tool, dir)
    }

    #[tokio::test]
    async fn test_activate_config_injection() {
        let mut cfg: HashMap<String, HashMap<String, serde_yaml::Value>> = HashMap::new();
        let mut wiki_cfg: HashMap<String, serde_yaml::Value> = HashMap::new();
        wiki_cfg.insert(
            "path".to_string(),
            serde_yaml::Value::String("~/research".to_string()),
        );
        wiki_cfg.insert(
            "format".to_string(),
            serde_yaml::Value::String("markdown".to_string()),
        );
        cfg.insert("wiki".to_string(), wiki_cfg);

        let (tool, _dir) = make_p04_tool(&[("wiki", "Wiki skill", "Wiki body content")], cfg);

        let result = tool
            .execute(json!({"action": "activate", "name": "wiki"}))
            .await
            .unwrap();
        let v = parse_response(&result);
        assert_eq!(v["status"], "ok");
        let content = v["content"].as_str().unwrap();
        assert!(
            content.starts_with("[Skill config: "),
            "content should start with skill-config header, got: {:?}",
            content
        );
        assert!(
            content.contains("path = ~/research"),
            "missing path pair: {:?}",
            content
        );
        assert!(
            content.contains("format = markdown"),
            "missing format pair: {:?}",
            content
        );
        // header is followed by \n\n then the original body
        assert!(
            content.contains("]\n\nWiki body content"),
            "header must be followed by blank line then body, got: {:?}",
            content
        );
    }

    #[tokio::test]
    async fn test_activate_no_config_no_header() {
        // Empty skills_config — no header should be emitted.
        let (tool, _dir) = make_p04_tool(
            &[("wiki", "Wiki skill", "Wiki body content")],
            HashMap::new(),
        );
        let result = tool
            .execute(json!({"action": "activate", "name": "wiki"}))
            .await
            .unwrap();
        let v = parse_response(&result);
        assert_eq!(v["status"], "ok");
        let content = v["content"].as_str().unwrap();
        assert!(
            !content.contains("[Skill config:"),
            "no header should be emitted when skills_config is empty: {:?}",
            content
        );
        assert_eq!(content.trim(), "Wiki body content");
    }

    #[tokio::test]
    async fn test_activate_config_key_ordering_stable() {
        // Insert keys in non-lex order; build_skill_config_header sorts them.
        let mut cfg: HashMap<String, HashMap<String, serde_yaml::Value>> = HashMap::new();
        let mut inner: HashMap<String, serde_yaml::Value> = HashMap::new();
        inner.insert(
            "zeta".to_string(),
            serde_yaml::Value::String("z".to_string()),
        );
        inner.insert(
            "alpha".to_string(),
            serde_yaml::Value::String("a".to_string()),
        );
        inner.insert("mu".to_string(), serde_yaml::Value::String("m".to_string()));
        cfg.insert("wiki".to_string(), inner);

        let (tool, _dir) = make_p04_tool(&[("wiki", "Wiki skill", "Body")], cfg);

        let r1 = tool
            .execute(json!({"action": "activate", "name": "wiki"}))
            .await
            .unwrap();
        let r2 = tool
            .execute(json!({"action": "activate", "name": "wiki"}))
            .await
            .unwrap();
        let v1 = parse_response(&r1);
        let v2 = parse_response(&r2);
        assert_eq!(
            v1["content"], v2["content"],
            "back-to-back activations must return identical content"
        );
        let content = v1["content"].as_str().unwrap();
        // Expect lexicographic order: alpha, mu, zeta
        let header_line = content.lines().next().unwrap();
        let alpha_pos = header_line.find("alpha").expect("alpha missing");
        let mu_pos = header_line.find("mu").expect("mu missing");
        let zeta_pos = header_line.find("zeta").expect("zeta missing");
        assert!(
            alpha_pos < mu_pos && mu_pos < zeta_pos,
            "keys not sorted lexicographically: {:?}",
            header_line
        );
    }

    #[tokio::test]
    async fn test_setup_note_format() {
        let _guard = ENV_LOCK.lock().unwrap();
        // SAFETY: tests serialized via ENV_LOCK above.
        unsafe {
            std::env::remove_var("TENOR_API_KEY");
        }

        let cred_dir = tempfile::tempdir().unwrap();
        let hermes_yaml = "    required_environment_variables:\n      - name: TENOR_API_KEY\n        required_for: \"GIF search\"";
        let (tool, _dir, _active_skills) = make_p03_tool(
            &[("gif-skill", "GIF skill", hermes_yaml, "Body")],
            cred_dir.path().to_path_buf(),
        );

        let result = tool
            .execute(json!({"action": "activate", "name": "gif-skill"}))
            .await
            .unwrap();
        let v = parse_response(&result);

        assert_eq!(v["status"], "setup_needed");
        assert_eq!(
            v["setup_note"], "I need $TENOR_API_KEY to GIF search.",
            "setup_note exact-format check failed"
        );
    }

    // =========================================================================
    // Phase 19 Plan 06: active_skill_env_names helper (D-05)
    // =========================================================================

    use ironhermes_core::{EnvVarEntry, HermesMetadata, SkillRecord, SkillSource};
    use std::path::PathBuf;

    fn make_skill_record_with_env_vars(name: &str, env_var_names: &[&str]) -> SkillRecord {
        let entries: Vec<EnvVarEntry> = env_var_names
            .iter()
            .map(|n| EnvVarEntry {
                name: (*n).to_string(),
                prompt: None,
                help: None,
                required_for: None,
            })
            .collect();
        let meta = HermesMetadata {
            required_environment_variables: entries,
            ..Default::default()
        };
        SkillRecord {
            name: name.to_string(),
            description: format!("{} description", name),
            path: PathBuf::from(format!("/tmp/{}/SKILL.md", name)),
            platforms: None,
            compatibility: None,
            allowed_tools: None,
            metadata: None,
            hermes_metadata: Some(meta),
            source: SkillSource::Builtin,
        }
    }

    fn make_skill_record_without_hermes(name: &str) -> SkillRecord {
        SkillRecord {
            name: name.to_string(),
            description: format!("{} description", name),
            path: PathBuf::from(format!("/tmp/{}/SKILL.md", name)),
            platforms: None,
            compatibility: None,
            allowed_tools: None,
            metadata: None,
            hermes_metadata: None,
            source: SkillSource::Builtin,
        }
    }

    #[test]
    fn test_active_skill_env_names() {
        let skill_a = make_skill_record_with_env_vars("a", &["TENOR_API_KEY", "TENOR_BASE_URL"]);
        let skill_b = make_skill_record_with_env_vars("b", &["WIKI_TOKEN"]);
        let active = vec![skill_a, skill_b];

        let names = active_skill_env_names(&active);
        assert_eq!(
            names,
            vec![
                "TENOR_API_KEY".to_string(),
                "TENOR_BASE_URL".to_string(),
                "WIKI_TOKEN".to_string(),
            ],
            "expected names in insertion order across both skills, got: {:?}",
            names
        );
    }

    #[test]
    fn test_active_skill_env_names_empty() {
        let active: Vec<SkillRecord> = Vec::new();
        let names = active_skill_env_names(&active);
        assert!(
            names.is_empty(),
            "empty active_skills must yield empty Vec, got: {:?}",
            names
        );
    }

    #[test]
    fn test_active_skill_env_names_skips_skills_with_no_hermes_meta() {
        let skill_plain = make_skill_record_without_hermes("plain");
        let skill_with = make_skill_record_with_env_vars("has-meta", &["SOME_KEY"]);
        let active = vec![skill_plain, skill_with];

        let names = active_skill_env_names(&active);
        assert_eq!(
            names,
            vec!["SOME_KEY".to_string()],
            "skills without hermes_metadata should contribute no names, got: {:?}",
            names
        );
    }
}

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::{SkillRegistry, ToolSchema};
use serde_json::{json, Value};

use crate::registry::Tool;

// ---------------------------------------------------------------------------
// Description
// ---------------------------------------------------------------------------

const SKILLS_DESCRIPTION: &str =
    "Browse and activate skill documents. Actions: list, view, activate, deactivate.";

// ---------------------------------------------------------------------------
// SkillsTool
// ---------------------------------------------------------------------------

pub struct SkillsTool {
    registry: Arc<SkillRegistry>,
    active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>>,
}

impl SkillsTool {
    pub fn new(registry: Arc<SkillRegistry>, active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>>) -> Self {
        Self { registry, active_skills }
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
        None => {
            return json!({"status": "error", "message": "Missing required parameter 'name'"})
        }
    };

    match registry.find(name) {
        Some(record) => match std::fs::read_to_string(&record.path) {
            Ok(content) => json!({"status": "ok", "name": record.name, "content": content}),
            Err(e) => json!({"status": "error", "message": format!("Failed to read skill file: {}", e)}),
        },
        None => json!({"status": "error", "message": format!("Skill not found: {}", name)}),
    }
}

fn handle_activate(registry: &SkillRegistry, args: &Value, active_skills: &std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>) -> Value {
    let name = match args.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => {
            return json!({"status": "error", "message": "Missing required parameter 'name'"})
        }
    };

    match registry.read_content(name) {
        Some(body) => {
            // find() is safe here since read_content already verified the skill exists
            let record = registry.find(name).unwrap();
            let canonical_name = record.name.as_str();
            // D-02: push SkillRecord into shared active_skills
            if let Ok(mut skills) = active_skills.lock() {
                // Avoid duplicate activation
                if !skills.iter().any(|s| s.name == canonical_name) {
                    skills.push(record.clone());
                }
            }
            json!({"status": "ok", "name": canonical_name, "content": body})
        }
        None => json!({"status": "error", "message": format!("Skill not found: {}", name)}),
    }
}

fn handle_deactivate(args: &Value, active_skills: &std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>) -> Value {
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
                        "enum": ["list", "view", "activate", "deactivate"],
                        "description": "Action to perform. list: show all skills; view: read full SKILL.md; activate: load skill body for use; deactivate: remove skill from active set."
                    },
                    "name": {
                        "type": "string",
                        "description": "Skill name. Required for view and activate."
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
            "activate" => handle_activate(&self.registry, &args, &self.active_skills),
            "deactivate" => handle_deactivate(&args, &self.active_skills),
            other => {
                json!({"status": "error", "message": format!("Unknown action '{}'. Valid: list, view, activate, deactivate", other)})
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
        format!("---\nname: {}\ndescription: {}\n---\n{}", name, description, body)
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
        let tool = SkillsTool::new(Arc::new(registry), active_skills);
        (tool, dir)
    }

    fn make_tool_with_skills_and_active(skills: &[(&str, &str, &str)]) -> (SkillsTool, tempfile::TempDir, Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>>) {
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
        let tool = SkillsTool::new(Arc::new(registry), active_skills.clone());
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
        let result = tool
            .execute(json!({"action": "unknown"}))
            .await
            .unwrap();
        let v = parse_response(&result);
        assert_eq!(v["status"], "error");
        let msg = v["message"].as_str().unwrap();
        assert!(msg.contains("unknown"));
        // Should list valid actions
        assert!(msg.contains("list") && msg.contains("view") && msg.contains("activate") && msg.contains("deactivate"));
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
        assert_eq!(skills.len(), 1, "duplicate activate should not add a second entry");
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
        assert!(v["message"].as_str().unwrap().contains("not currently active"));
    }

    #[tokio::test]
    async fn test_deactivate_missing_name_returns_error() {
        let (tool, _dir, _active_skills) = make_tool_with_skills_and_active(&[]);
        let result = tool
            .execute(json!({"action": "deactivate"}))
            .await
            .unwrap();
        let v = parse_response(&result);
        assert_eq!(v["status"], "error");
        assert!(v["message"].as_str().unwrap().contains("name"));
    }
}

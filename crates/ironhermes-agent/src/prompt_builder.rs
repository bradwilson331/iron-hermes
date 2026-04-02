use ironhermes_core::{get_hermes_home, ChatMessage};
use std::path::Path;

use crate::context_scanner;

const DEFAULT_AGENT_IDENTITY: &str = r#"You are IronHermes, an AI assistant created by Nous Research. You are helpful, harmless, and honest.

You have access to tools that let you interact with the user's computer and the internet. Use them when needed to accomplish tasks.

Key principles:
- Be direct and concise
- Use tools proactively when they would help
- Ask for clarification when the task is ambiguous
- Be transparent about what you're doing and why
- Respect the user's system and data"#;

const TOOL_USE_GUIDANCE: &str = r#"When you need to use tools:
1. Choose the most appropriate tool for the task
2. Provide clear, complete arguments
3. Handle tool errors gracefully
4. Chain tool calls when needed for multi-step tasks
5. Report results clearly to the user"#;

/// Builds the system prompt for the agent with layered context loading.
pub struct PromptBuilder {
    #[allow(dead_code)]
    model: String,
    platform: String,
    // Loaded context (frozen at build time)
    soul_content: Option<String>,      // From SOUL.md at IRONHERMES_HOME
    project_context: Option<String>,   // First-match from priority chain in cwd
    agents_md_content: Option<String>, // From AGENTS.md at IRONHERMES_HOME
}

impl PromptBuilder {
    pub fn new(model: impl Into<String>, platform: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            platform: platform.into(),
            soul_content: None,
            project_context: None,
            agents_md_content: None,
        }
    }

    /// Load all context files (SOUL.md, project context, AGENTS.md) and freeze them.
    pub fn load_context(mut self, cwd: &Path) -> Self {
        self.load_soul_md();
        self.load_project_context(cwd);
        self.load_agents_md();
        self
    }

    fn load_soul_md(&mut self) {
        let path = get_hermes_home().join("SOUL.md");
        let content = match std::fs::read_to_string(&path) {
            Ok(c) if !c.trim().is_empty() => c,
            _ => return,
        };
        let scanned = context_scanner::scan_context_content(&content, "SOUL.md");
        let truncated =
            context_scanner::truncate_content(&scanned, "SOUL.md", context_scanner::CONTEXT_FILE_MAX_CHARS);
        tracing::debug!("Loaded SOUL.md from {}", path.display());
        self.soul_content = Some(truncated);
    }

    fn load_agents_md(&mut self) {
        let path = get_hermes_home().join("AGENTS.md");
        let content = match std::fs::read_to_string(&path) {
            Ok(c) if !c.trim().is_empty() => c,
            _ => return,
        };
        let scanned = context_scanner::scan_context_content(&content, "AGENTS.md");
        let truncated = context_scanner::truncate_content(
            &scanned,
            "AGENTS.md",
            context_scanner::CONTEXT_FILE_MAX_CHARS,
        );
        let wrapped = format!("## AGENTS.md\n\n{}", truncated);
        tracing::debug!("Loaded AGENTS.md from {}", path.display());
        self.agents_md_content = Some(wrapped);
    }

    fn load_project_context(&mut self, cwd: &Path) {
        // Priority chain: first match wins, only ONE file loads
        let candidates: &[&str] = &[
            ".hermes.md",
            "HERMES.md",
            "AGENTS.md",
            "agents.md",
            "CLAUDE.md",
            "claude.md",
            ".cursorrules",
        ];

        for filename in candidates {
            let path = cwd.join(filename);
            if !path.exists() {
                continue;
            }
            let content = match std::fs::read_to_string(&path) {
                Ok(c) if !c.trim().is_empty() => c,
                _ => continue,
            };
            let scanned = context_scanner::scan_context_content(&content, filename);
            let truncated = context_scanner::truncate_content(
                &scanned,
                filename,
                context_scanner::CONTEXT_FILE_MAX_CHARS,
            );
            let wrapped = format!("## {}\n\n{}", filename, truncated);
            tracing::debug!("Loaded project context: {}", filename);
            self.project_context = Some(wrapped);
            return; // first match wins
        }
    }

    /// Build the complete system prompt in assembly order:
    /// identity > platform hint > tool guidance > project context > AGENTS.md from home
    pub fn build(&self) -> String {
        let mut parts: Vec<String> = Vec::new();

        // 1. Identity: SOUL.md if loaded, else default
        let identity = self
            .soul_content
            .as_deref()
            .unwrap_or(DEFAULT_AGENT_IDENTITY);
        parts.push(identity.to_string());

        // 2. Platform hint
        let platform_hint = self.platform_hint();
        if !platform_hint.is_empty() {
            parts.push(platform_hint);
        }

        // 3. Tool use guidance
        parts.push(TOOL_USE_GUIDANCE.to_string());

        // 4. Project context (first-match from cwd priority chain)
        if let Some(ref ctx) = self.project_context {
            parts.push(ctx.clone());
        }

        // 5. AGENTS.md from IRONHERMES_HOME
        if let Some(ref agents) = self.agents_md_content {
            parts.push(agents.clone());
        }

        parts.join("\n\n")
    }

    /// Build the system message.
    pub fn build_system_message(&self) -> ChatMessage {
        ChatMessage::system(self.build())
    }

    fn platform_hint(&self) -> String {
        match self.platform.as_str() {
            "cli" => "You are running in an interactive CLI terminal. The user can see your responses in real-time. Use markdown formatting for readability.".to_string(),
            "telegram" => "You are running as a Telegram bot. Keep responses concise. Use Telegram-compatible markdown (MarkdownV2). Avoid very long messages.".to_string(),
            "discord" => "You are running as a Discord bot. Use Discord markdown formatting. Keep messages under 2000 characters when possible.".to_string(),
            "slack" => "You are running as a Slack bot. Use Slack mrkdwn formatting. Use threads for long conversations.".to_string(),
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_build_default_identity() {
        let pb = PromptBuilder::new("test-model", "cli");
        let output = pb.build();
        assert!(
            output.contains(DEFAULT_AGENT_IDENTITY),
            "Expected default identity in output"
        );
    }

    #[test]
    #[serial]
    fn test_soul_replaces_default() {
        let dir = tempdir().unwrap();
        let soul_content = "You are SoulBot, a custom identity.";
        std::fs::write(dir.path().join("SOUL.md"), soul_content).unwrap();

        unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()); }
        let pb = PromptBuilder::new("test-model", "cli").load_context(dir.path());
        unsafe { std::env::remove_var("IRONHERMES_HOME"); }

        let output = pb.build();
        assert!(
            output.contains(soul_content),
            "Expected soul content in output"
        );
        assert!(
            !output.contains(DEFAULT_AGENT_IDENTITY),
            "Default identity should not appear when SOUL.md is loaded"
        );
    }

    #[test]
    #[serial]
    fn test_project_context_priority() {
        let dir = tempdir().unwrap();
        // Both .hermes.md and CLAUDE.md exist — .hermes.md should win
        std::fs::write(dir.path().join(".hermes.md"), "hermes context").unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "claude context").unwrap();

        unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()); }
        let pb = PromptBuilder::new("test-model", "cli").load_context(dir.path());
        unsafe { std::env::remove_var("IRONHERMES_HOME"); }

        let output = pb.build();
        assert!(
            output.contains("hermes context"),
            "Expected .hermes.md content"
        );
        assert!(
            !output.contains("claude context"),
            ".hermes.md should win over CLAUDE.md"
        );
    }

    #[test]
    #[serial]
    fn test_project_context_first_match_wins() {
        let dir = tempdir().unwrap();
        // Only CLAUDE.md exists
        std::fs::write(dir.path().join("CLAUDE.md"), "claude only context").unwrap();

        unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()); }
        let pb = PromptBuilder::new("test-model", "cli").load_context(dir.path());
        unsafe { std::env::remove_var("IRONHERMES_HOME"); }

        let output = pb.build();
        assert!(
            output.contains("claude only context"),
            "Expected CLAUDE.md content to load"
        );
    }

    #[test]
    #[serial]
    fn test_assembly_order() {
        let home_dir = tempdir().unwrap();
        let cwd_dir = tempdir().unwrap();

        // SOUL.md in IRONHERMES_HOME
        std::fs::write(home_dir.path().join("SOUL.md"), "SOUL IDENTITY").unwrap();
        // AGENTS.md in IRONHERMES_HOME
        std::fs::write(home_dir.path().join("AGENTS.md"), "HOME AGENTS CONTENT").unwrap();
        // AGENTS.md in cwd (project context — takes priority as AGENTS.md in chain)
        std::fs::write(cwd_dir.path().join("AGENTS.md"), "PROJECT AGENTS CONTENT").unwrap();

        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()); }
        let pb = PromptBuilder::new("test-model", "cli").load_context(cwd_dir.path());
        unsafe { std::env::remove_var("IRONHERMES_HOME"); }

        let output = pb.build();

        // Verify order: soul < project context < home agents.md
        let soul_pos = output.find("SOUL IDENTITY").expect("SOUL.md content missing");
        let project_pos = output
            .find("PROJECT AGENTS CONTENT")
            .expect("Project AGENTS.md content missing");
        let home_agents_pos = output
            .find("HOME AGENTS CONTENT")
            .expect("Home AGENTS.md content missing");

        assert!(
            soul_pos < project_pos,
            "SOUL.md should appear before project context"
        );
        assert!(
            project_pos < home_agents_pos,
            "Project context should appear before home AGENTS.md"
        );
    }

    #[test]
    #[serial]
    fn test_empty_files_skipped() {
        let dir = tempdir().unwrap();
        // Empty SOUL.md — should fall back to default identity
        let mut f = std::fs::File::create(dir.path().join("SOUL.md")).unwrap();
        f.write_all(b"   ").unwrap(); // whitespace only

        unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()); }
        let pb = PromptBuilder::new("test-model", "cli").load_context(dir.path());
        unsafe { std::env::remove_var("IRONHERMES_HOME"); }

        let output = pb.build();
        assert!(
            output.contains(DEFAULT_AGENT_IDENTITY),
            "Default identity should be used when SOUL.md is empty"
        );
    }
}

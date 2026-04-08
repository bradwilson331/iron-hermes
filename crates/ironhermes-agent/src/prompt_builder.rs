use std::path::Path;
use std::sync::{Arc, Mutex};

use ironhermes_core::{scan_context_content, truncate_content, CONTEXT_FILE_MAX_CHARS};
use ironhermes_core::{ChatMessage, MemoryStore, MemoryTarget};
use tracing::debug;

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
    model: String,
    platform: String,
    // Loaded context (frozen at build time)
    soul_content: Option<String>,
    project_context: Option<String>,
    agents_md_content: Option<String>,
    memory_store: Option<Arc<Mutex<MemoryStore>>>,
}

impl PromptBuilder {
    pub fn new(model: impl Into<String>, platform: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            platform: platform.into(),
            soul_content: None,
            project_context: None,
            agents_md_content: None,
            memory_store: None,
        }
    }

    /// Set the memory store for prompt injection (D-12: uses frozen snapshot).
    pub fn set_memory_store(&mut self, store: Arc<Mutex<MemoryStore>>) {
        self.memory_store = Some(store);
    }

    /// Load all context files (SOUL.md, project context, AGENTS.md).
    /// Context is frozen at call time — mid-session file edits do not change the prompt.
    pub fn load_context(mut self, cwd: &Path) -> Self {
        self.load_soul_md();
        self.load_project_context(cwd);
        self.load_agents_md();
        self
    }

    fn load_soul_md(&mut self) {
        let path = ironhermes_core::get_hermes_home().join("SOUL.md");
        match std::fs::read_to_string(&path) {
            Ok(content) if !content.trim().is_empty() => {
                let scanned = scan_context_content(&content, "SOUL.md");
                let truncated = truncate_content(&scanned, "SOUL.md", CONTEXT_FILE_MAX_CHARS);
                debug!("Loaded SOUL.md from {}", path.display());
                self.soul_content = Some(truncated);
            }
            Ok(_) => {
                debug!("SOUL.md at {} is empty, using default identity", path.display());
            }
            Err(e) => {
                debug!("SOUL.md not found at {}: {}", path.display(), e);
            }
        }
    }

    fn load_agents_md(&mut self) {
        let path = ironhermes_core::get_hermes_home().join("AGENTS.md");
        match std::fs::read_to_string(&path) {
            Ok(content) if !content.trim().is_empty() => {
                let scanned = scan_context_content(&content, "AGENTS.md");
                let truncated = truncate_content(&scanned, "AGENTS.md", CONTEXT_FILE_MAX_CHARS);
                let wrapped = format!("## AGENTS.md\n\n{}", truncated);
                debug!("Loaded AGENTS.md from {}", path.display());
                self.agents_md_content = Some(wrapped);
            }
            Ok(_) => {
                debug!("AGENTS.md at {} is empty, skipping", path.display());
            }
            Err(e) => {
                debug!("AGENTS.md not found at {}: {}", path.display(), e);
            }
        }
    }

    fn load_project_context(&mut self, cwd: &Path) {
        // Priority chain: first match wins, only ONE loads
        let candidates: &[&[&str]] = &[
            &[".hermes.md", "HERMES.md"],
            &["AGENTS.md", "agents.md"],
            &["CLAUDE.md", "claude.md"],
            &[".cursorrules"],
        ];

        for group in candidates {
            for &filename in *group {
                let path = cwd.join(filename);
                if !path.exists() {
                    continue;
                }
                match std::fs::read_to_string(&path) {
                    Ok(content) if !content.trim().is_empty() => {
                        let scanned = scan_context_content(&content, filename);
                        let truncated = truncate_content(&scanned, filename, CONTEXT_FILE_MAX_CHARS);
                        let wrapped = format!("## {}\n\n{}", filename, truncated);
                        debug!("Loaded project context: {}", filename);
                        self.project_context = Some(wrapped);
                        return; // first match wins
                    }
                    Ok(_) => {
                        debug!("Project context file {} is empty, skipping", filename);
                    }
                    Err(e) => {
                        debug!("Failed to read {}: {}", filename, e);
                    }
                }
            }
        }
    }

    /// Build the complete system prompt.
    pub fn build(&self) -> String {
        let mut parts: Vec<String> = Vec::new();

        // 1. Identity: SOUL.md or default
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

        // 4. Project context
        if let Some(ref ctx) = self.project_context {
            parts.push(ctx.clone());
        }

        // 5. AGENTS.md from IRONHERMES_HOME
        if let Some(ref agents) = self.agents_md_content {
            parts.push(agents.clone());
        }

        // 6. Memory snapshot (D-12: uses frozen snapshot, not live state)
        if let Some(ref store) = self.memory_store {
            let store = store.lock().unwrap();
            if let Some(block) = store.format_for_system_prompt(MemoryTarget::Memory) {
                parts.push(block);
            }
            if let Some(block) = store.format_for_system_prompt(MemoryTarget::User) {
                parts.push(block);
            }
        }

        parts.join("\n\n")
    }

    /// Build the system message (frozen snapshot).
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

    // Expose model field for potential future use
    #[allow(dead_code)]
    pub fn model(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;

    /// Tests that manipulate IRONHERMES_HOME must hold this lock
    /// to avoid env var races (Rust tests run in parallel).
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    fn make_temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("Failed to create temp dir")
    }

    #[test]
    fn test_build_default_identity() {
        let builder = PromptBuilder::new("test-model", "cli");
        let output = builder.build();
        assert!(output.contains("IronHermes"));
        assert!(output.contains("Nous Research"));
    }

    #[test]
    fn test_soul_replaces_default() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let home_dir = make_temp_dir();
        let cwd_dir = make_temp_dir();
        fs::write(home_dir.path().join("SOUL.md"), "You are a custom soul.").unwrap();

        // SAFETY: env var tests must run with --test-threads=1
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home_dir.path());
        }

        let builder = PromptBuilder::new("test-model", "cli").load_context(cwd_dir.path());
        let output = builder.build();

        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }

        assert!(output.contains("You are a custom soul."));
        assert!(!output.contains("IronHermes, an AI assistant"));
    }

    #[test]
    fn test_project_context_priority() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let home_dir = make_temp_dir();
        let cwd_dir = make_temp_dir();
        fs::write(cwd_dir.path().join(".hermes.md"), "hermes context").unwrap();
        fs::write(cwd_dir.path().join("CLAUDE.md"), "claude context").unwrap();

        unsafe {
            std::env::set_var("IRONHERMES_HOME", home_dir.path());
        }

        let builder = PromptBuilder::new("test-model", "cli").load_context(cwd_dir.path());
        let output = builder.build();

        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }

        assert!(output.contains("hermes context"));
        assert!(!output.contains("claude context"));
    }

    #[test]
    fn test_project_context_first_match_wins() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let home_dir = make_temp_dir();
        let cwd_dir = make_temp_dir();
        fs::write(cwd_dir.path().join("CLAUDE.md"), "claude context only").unwrap();

        unsafe {
            std::env::set_var("IRONHERMES_HOME", home_dir.path());
        }

        let builder = PromptBuilder::new("test-model", "cli").load_context(cwd_dir.path());
        let output = builder.build();

        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }

        assert!(output.contains("claude context only"));
    }

    #[test]
    fn test_assembly_order() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let home_dir = make_temp_dir();
        let cwd_dir = make_temp_dir();
        fs::write(home_dir.path().join("SOUL.md"), "SOUL CONTENT").unwrap();
        fs::write(home_dir.path().join("AGENTS.md"), "AGENTS HOME CONTENT").unwrap();
        fs::write(cwd_dir.path().join("CLAUDE.md"), "PROJECT CONTEXT").unwrap();

        unsafe {
            std::env::set_var("IRONHERMES_HOME", home_dir.path());
        }

        let builder = PromptBuilder::new("test-model", "cli").load_context(cwd_dir.path());
        let output = builder.build();

        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }

        let soul_pos = output.find("SOUL CONTENT").unwrap();
        let project_pos = output.find("PROJECT CONTEXT").unwrap();
        let agents_pos = output.find("AGENTS HOME CONTENT").unwrap();

        assert!(soul_pos < project_pos, "SOUL must come before project context");
        assert!(
            project_pos < agents_pos,
            "Project context must come before AGENTS.md"
        );
    }

    #[test]
    fn test_empty_files_skipped() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let home_dir = make_temp_dir();
        let cwd_dir = make_temp_dir();
        fs::write(home_dir.path().join("SOUL.md"), "   ").unwrap(); // whitespace only

        unsafe {
            std::env::set_var("IRONHERMES_HOME", home_dir.path());
        }

        let builder = PromptBuilder::new("test-model", "cli").load_context(cwd_dir.path());
        let output = builder.build();

        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }

        // Should fall back to default identity
        assert!(output.contains("IronHermes, an AI assistant"));
    }
}

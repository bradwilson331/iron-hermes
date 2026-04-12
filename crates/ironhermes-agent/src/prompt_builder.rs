use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ironhermes_core::{scan_context_content, truncate_content, CONTEXT_FILE_MAX_CHARS};
use ironhermes_core::{ChatMessage, MemoryProvider, MemoryTarget, SkillRegistry};
use tracing::debug;

use crate::context_loader::{find_git_root, strip_yaml_frontmatter, CONTEXT_CANDIDATES};

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
    /// When true: skip SOUL.md, project context, and AGENTS.md; use DEFAULT_AGENT_IDENTITY.
    /// Used for subagents so they get a clean identity. Per D-10.
    skip_context_files: bool,
    memory_store: Option<Arc<Mutex<dyn MemoryProvider + Send>>>,
    skill_registry: Option<Arc<SkillRegistry>>,
}

impl PromptBuilder {
    pub fn new(model: impl Into<String>, platform: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            platform: platform.into(),
            soul_content: None,
            project_context: None,
            agents_md_content: None,
            skip_context_files: false,
            memory_store: None,
            skill_registry: None,
        }
    }

    /// Skip all context files (SOUL.md, project context, AGENTS.md).
    /// Subagents call this to get a clean DEFAULT_AGENT_IDENTITY. Per D-10.
    pub fn skip_context_files(mut self) -> Self {
        self.skip_context_files = true;
        self
    }

    /// Set the memory store for prompt injection (D-12: uses frozen snapshot).
    pub fn set_memory_store(&mut self, store: Arc<Mutex<dyn MemoryProvider + Send>>) {
        self.memory_store = Some(store);
    }

    /// Set the skill registry for catalog injection into the system prompt.
    pub fn set_skill_registry(&mut self, registry: Arc<SkillRegistry>) {
        self.skill_registry = Some(registry);
    }

    /// Load all context files (SOUL.md, project context, AGENTS.md).
    /// Context is frozen at call time — mid-session file edits do not change the prompt.
    /// When skip_context_files is true, returns self immediately (D-10).
    pub fn load_context(mut self, cwd: &Path) -> Self {
        if self.skip_context_files {
            return self; // D-10: subagents get clean identity
        }
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
        // Step 1: Walk upward from CWD looking for .hermes.md only.
        // Stop at git root (if found) or $HOME (if no git root). Per D-01, D-03.
        let git_root = find_git_root(cwd);
        let stop_dir: Option<PathBuf> = git_root.or_else(|| {
            std::env::var("HOME").ok().map(PathBuf::from)
        });

        let mut dir = cwd.to_path_buf();
        loop {
            let candidate = dir.join(".hermes.md");
            if candidate.exists() {
                match std::fs::read_to_string(&candidate) {
                    Ok(content) if !content.trim().is_empty() => {
                        // D-02: strip frontmatter FIRST, then scan + truncate
                        let body = strip_yaml_frontmatter(&content);
                        let scanned = scan_context_content(body, ".hermes.md");
                        let truncated = truncate_content(&scanned, ".hermes.md", CONTEXT_FILE_MAX_CHARS);
                        let wrapped = format!("## .hermes.md\n\n{}", truncated);
                        debug!("Loaded project context: .hermes.md from {}", dir.display());
                        self.project_context = Some(wrapped);
                        return;
                    }
                    Ok(_) => {
                        debug!("Project context file .hermes.md is empty, skipping");
                    }
                    Err(e) => {
                        debug!("Failed to read .hermes.md: {}", e);
                    }
                }
            }

            // Check stop condition: don't walk past stop_dir
            if let Some(ref stop) = stop_dir {
                if dir == *stop {
                    break;
                }
            }

            match dir.parent() {
                Some(parent) => dir = parent.to_path_buf(),
                None => break,
            }
        }

        // Step 2: No .hermes.md found. Check CWD only for remaining candidates.
        // CONTEXT_CANDIDATES[0] is ".hermes.md" (already checked above), skip it.
        for &filename in CONTEXT_CANDIDATES.iter().skip(1) {
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

    /// Build the complete system prompt.
    pub fn build(&self) -> String {
        let mut parts: Vec<String> = Vec::new();

        // 1. Identity: SOUL.md or default; D-10: always DEFAULT_AGENT_IDENTITY for subagents
        let identity = if self.skip_context_files {
            DEFAULT_AGENT_IDENTITY
        } else {
            self.soul_content.as_deref().unwrap_or(DEFAULT_AGENT_IDENTITY)
        };
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

        // 5.5. Skill catalog (per D-04, D-05)
        if let Some(ref registry) = self.skill_registry
            && !registry.list().is_empty()
        {
            let catalog = registry.catalog_text();
            parts.push(format!(
                "## Available Skills\n\n{}\n\nUse the skills tool to view or activate a skill before using it.",
                catalog
            ));
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

    #[test]
    fn test_build_with_skill_catalog() {
        use ironhermes_core::SkillRegistry;

        let dir = make_temp_dir();
        let skill_dir = dir.path().join("skill-a");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: focus-mode\ndescription: Deep work skill\n---\nFocus on the task.",
        )
        .unwrap();

        let registry = Arc::new(SkillRegistry::load_with_paths(&[dir.path().to_path_buf()]));

        let mut builder = PromptBuilder::new("test-model", "cli");
        builder.set_skill_registry(registry);
        let output = builder.build();

        assert!(
            output.contains("## Available Skills"),
            "output must contain '## Available Skills': {output}"
        );
        assert!(
            output.contains("focus-mode"),
            "output must contain skill name: {output}"
        );
        assert!(
            output.contains("Deep work skill"),
            "output must contain skill description: {output}"
        );
        assert!(
            output.contains("Use the skills tool to view or activate a skill before using it."),
            "output must contain usage hint: {output}"
        );
    }

    #[test]
    fn test_build_without_skills_no_section() {
        let builder = PromptBuilder::new("test-model", "cli");
        let output = builder.build();
        assert!(
            !output.contains("Available Skills"),
            "output must NOT contain 'Available Skills' when no registry set: {output}"
        );
    }

    // ── New Task 2 tests ──────────────────────────────────────────────────────

    #[test]
    fn test_skip_context_files_default_identity() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let home_dir = make_temp_dir();
        let cwd_dir = make_temp_dir();

        // Even with SOUL.md present, skip_context_files must use DEFAULT_AGENT_IDENTITY
        fs::write(home_dir.path().join("SOUL.md"), "Custom soul that should be ignored").unwrap();
        fs::write(home_dir.path().join("AGENTS.md"), "Agents content that should be ignored").unwrap();
        fs::write(cwd_dir.path().join(".hermes.md"), "Project context that should be ignored").unwrap();

        unsafe {
            std::env::set_var("IRONHERMES_HOME", home_dir.path());
        }

        let builder = PromptBuilder::new("test-model", "cli")
            .skip_context_files()
            .load_context(cwd_dir.path());
        let output = builder.build();

        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }

        // Must use DEFAULT_AGENT_IDENTITY
        assert!(
            output.contains("IronHermes, an AI assistant"),
            "skip_context_files must use DEFAULT_AGENT_IDENTITY: {output}"
        );
        // Must NOT contain SOUL.md content
        assert!(
            !output.contains("Custom soul that should be ignored"),
            "skip_context_files must not include SOUL.md: {output}"
        );
        // Must NOT contain AGENTS.md content
        assert!(
            !output.contains("Agents content that should be ignored"),
            "skip_context_files must not include AGENTS.md: {output}"
        );
        // Must NOT contain project context
        assert!(
            !output.contains("Project context that should be ignored"),
            "skip_context_files must not include project context: {output}"
        );
    }

    #[test]
    fn test_hermes_md_git_root_walk() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let home_dir = make_temp_dir();
        let project_dir = make_temp_dir();
        let cwd_dir = project_dir.path().join("src").join("module");
        fs::create_dir_all(&cwd_dir).unwrap();

        // .git in project root (makes it a git root)
        fs::create_dir_all(project_dir.path().join(".git")).unwrap();
        // .hermes.md in project root
        fs::write(project_dir.path().join(".hermes.md"), "parent hermes context").unwrap();

        unsafe {
            std::env::set_var("IRONHERMES_HOME", home_dir.path());
            std::env::set_var("HOME", home_dir.path());
        }

        let builder = PromptBuilder::new("test-model", "cli").load_context(&cwd_dir);
        let output = builder.build();

        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
            std::env::remove_var("HOME");
        }

        assert!(
            output.contains("parent hermes context"),
            ".hermes.md in git root should be found by walk: {output}"
        );
    }

    #[test]
    fn test_hermes_md_frontmatter_stripped() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let home_dir = make_temp_dir();
        let cwd_dir = make_temp_dir();

        fs::write(
            cwd_dir.path().join(".hermes.md"),
            "---\ntitle: My Project\nversion: 1.0\n---\nThis is the actual body content.",
        )
        .unwrap();

        unsafe {
            std::env::set_var("IRONHERMES_HOME", home_dir.path());
        }

        let builder = PromptBuilder::new("test-model", "cli").load_context(cwd_dir.path());
        let output = builder.build();

        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }

        assert!(
            output.contains("This is the actual body content."),
            "body content must be present: {output}"
        );
        assert!(
            !output.contains("title: My Project"),
            "frontmatter must be stripped: {output}"
        );
        assert!(
            !output.contains("version: 1.0"),
            "frontmatter must be stripped: {output}"
        );
    }

    #[test]
    fn test_case_sensitive_candidates() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let home_dir = make_temp_dir();
        let cwd_dir = make_temp_dir();

        // lowercase agents.md should NOT be loaded
        fs::write(cwd_dir.path().join("agents.md"), "lowercase agents content").unwrap();
        // uppercase AGENTS.md SHOULD be loaded
        fs::write(cwd_dir.path().join("AGENTS.md"), "uppercase AGENTS content").unwrap();

        unsafe {
            std::env::set_var("IRONHERMES_HOME", home_dir.path());
        }

        let builder = PromptBuilder::new("test-model", "cli").load_context(cwd_dir.path());
        let output = builder.build();

        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }

        assert!(
            output.contains("uppercase AGENTS content"),
            "AGENTS.md (uppercase) must be loaded: {output}"
        );
        assert!(
            !output.contains("lowercase agents content"),
            "agents.md (lowercase) must NOT be loaded: {output}"
        );
    }

    #[test]
    fn test_hermes_home_agents_md_separate() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let home_dir = make_temp_dir();
        let cwd_dir = make_temp_dir();

        // CWD has .hermes.md (project context)
        fs::write(cwd_dir.path().join(".hermes.md"), "project hermes context").unwrap();
        // HERMES_HOME has AGENTS.md (separate, D-09)
        fs::write(home_dir.path().join("AGENTS.md"), "hermes home agents content").unwrap();

        unsafe {
            std::env::set_var("IRONHERMES_HOME", home_dir.path());
        }

        let builder = PromptBuilder::new("test-model", "cli").load_context(cwd_dir.path());
        let output = builder.build();

        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }

        assert!(
            output.contains("project hermes context"),
            "project .hermes.md must be loaded: {output}"
        );
        assert!(
            output.contains("hermes home agents content"),
            "HERMES_HOME/AGENTS.md must also be loaded: {output}"
        );
    }
}

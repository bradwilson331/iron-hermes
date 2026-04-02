use ironhermes_core::ChatMessage;
use std::path::Path;
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

/// Builds the system prompt for the agent.
pub struct PromptBuilder {
    model: String,
    platform: String,
    custom_identity: Option<String>,
    skills_prompt: Option<String>,
    context_files_prompt: Option<String>,
}

impl PromptBuilder {
    pub fn new(model: impl Into<String>, platform: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            platform: platform.into(),
            custom_identity: None,
            skills_prompt: None,
            context_files_prompt: None,
        }
    }

    pub fn with_identity(mut self, identity: impl Into<String>) -> Self {
        self.custom_identity = Some(identity.into());
        self
    }

    pub fn with_skills(mut self, skills: impl Into<String>) -> Self {
        self.skills_prompt = Some(skills.into());
        self
    }

    pub fn with_context_files(mut self, context: impl Into<String>) -> Self {
        self.context_files_prompt = Some(context.into());
        self
    }

    /// Build the complete system prompt.
    pub fn build(&self) -> String {
        let mut parts = Vec::new();

        // Identity
        let identity = self
            .custom_identity
            .as_deref()
            .unwrap_or(DEFAULT_AGENT_IDENTITY);
        parts.push(identity.to_string());

        // Platform hint
        let platform_hint = self.platform_hint();
        if !platform_hint.is_empty() {
            parts.push(platform_hint);
        }

        // Tool guidance
        parts.push(TOOL_USE_GUIDANCE.to_string());

        // Context files (SOUL.md, AGENTS.md, etc.)
        if let Some(ref ctx) = self.context_files_prompt {
            parts.push(ctx.clone());
        }

        // Skills
        if let Some(ref skills) = self.skills_prompt {
            parts.push(skills.clone());
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

    /// Load context from SOUL.md, AGENTS.md, or similar files in the current directory.
    pub fn load_context_files(cwd: &Path) -> Option<String> {
        let candidates = ["SOUL.md", "AGENTS.md", ".hermes.md", "HERMES.md"];
        let mut parts = Vec::new();

        for filename in &candidates {
            let path = cwd.join(filename);
            if path.exists() {
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        debug!("Loaded context file: {}", path.display());
                        parts.push(format!("--- {} ---\n{}", filename, content));
                    }
                    Err(e) => {
                        debug!("Failed to read {}: {}", path.display(), e);
                    }
                }
            }
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n\n"))
        }
    }
}

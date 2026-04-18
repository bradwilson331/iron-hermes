use crate::commands::{CommandCategory, CommandDef, PlatformFilter};

use CommandCategory::*;
use PlatformFilter::*;

/// Build the full command registry.
///
/// Returns all commands in category order.
/// Note on the "q" alias conflict: "q" is assigned to "quit" (CLI exit takes priority
/// per hermes-agent). "queue" receives no "q" alias in IronHermes.
pub fn build_registry() -> Vec<CommandDef> {
    vec![
        // -----------------------------------------------------------------------
        // SESSION
        // -----------------------------------------------------------------------
        CommandDef::new("new", "Start a new session", Session)
            .aliases(&["reset"])
            .args_hint("[name]")
            .platform(Universal),
        CommandDef::new("clear", "Clear screen and start a new session", Session)
            .platform(CliAndAcp),
        CommandDef::new("history", "Show conversation history", Session).platform(CliOnly),
        CommandDef::new("save", "Save conversation to file", Session).platform(CliOnly),
        CommandDef::new("retry", "Retry the last message", Session).platform(Universal),
        CommandDef::new("undo", "Undo the last exchange", Session).platform(Universal),
        CommandDef::new("title", "Set session title", Session)
            .args_hint("[name]")
            .platform(Universal),
        CommandDef::new("compress", "Compress conversation context", Session)
            .args_hint("[focus]")
            .platform(Universal),
        CommandDef::new("rollback", "Roll back to a checkpoint", Session)
            .args_hint("[number]")
            .platform(Universal),
        CommandDef::new("stop", "Stop the running agent", Session).platform(Universal),
        CommandDef::new("approve", "Approve a pending dangerous command", Session)
            .args_hint("[session|always]")
            .platform(GatewayOnly),
        CommandDef::new("deny", "Deny a pending dangerous command", Session)
            .platform(GatewayOnly),
        CommandDef::new("background", "Run a prompt in the background", Session)
            .aliases(&["bg"])
            .args_hint("<prompt>")
            .platform(Universal),
        CommandDef::new("btw", "Ask an ephemeral question", Session)
            .args_hint("<question>")
            .platform(Universal),
        CommandDef::new("queue", "Queue a prompt for after current turn", Session)
            .args_hint("<prompt>")
            .platform(Universal),
        CommandDef::new("status", "Show current session status", Session).platform(Universal),
        CommandDef::new("sethome", "Set home channel for delivery", Session)
            .aliases(&["set-home"])
            .platform(GatewayOnly),
        CommandDef::new("resume", "Resume a previous session", Session)
            .args_hint("[name]")
            .platform(Universal),
        CommandDef::new("start", "Start with an LLM greeting", Session).platform(GatewayOnly),

        // -----------------------------------------------------------------------
        // CONFIGURATION
        // -----------------------------------------------------------------------
        CommandDef::new("config", "Show configuration", Configuration).platform(CliOnly),
        CommandDef::new("provider", "Show current provider", Configuration).platform(Universal),
        CommandDef::new("prompt", "Set custom system prompt", Configuration)
            .args_hint("[text]")
            .platform(CliOnly),
        CommandDef::new("personality", "Apply a personality preset", Configuration)
            .args_hint("[name]")
            .platform(Universal),
        CommandDef::new("statusbar", "Toggle status bar", Configuration)
            .aliases(&["sb"])
            .platform(CliOnly),
        CommandDef::new("verbose", "Toggle verbose tool output", Configuration).platform(CliOnly),
        CommandDef::new("yolo", "Toggle dangerous command auto-approval", Configuration)
            .platform(Universal),
        CommandDef::new("reasoning", "Set reasoning level", Configuration)
            .args_hint("[level|show|hide]")
            .platform(Universal),
        CommandDef::new("skin", "Change color theme", Configuration)
            .args_hint("[name]")
            .platform(CliOnly),
        CommandDef::new("voice", "Voice/TTS settings", Configuration)
            .args_hint("[on|off|tts|status]")
            .platform(Universal),
        CommandDef::new("model", "Switch model for this session", Configuration)
            .args_hint("[provider:model] [--global]")
            .platform(Universal),
        CommandDef::new("fast", "Toggle fast model preset", Configuration).platform(Universal),
        CommandDef::new("debug", "Toggle debug information", Configuration).platform(Universal),

        // -----------------------------------------------------------------------
        // TOOLS AND SKILLS
        // -----------------------------------------------------------------------
        CommandDef::new("tools", "List or manage tools", ToolsAndSkills)
            .args_hint("[list|disable|enable]")
            .platform(CliOnly),
        CommandDef::new("toolsets", "List available toolsets", ToolsAndSkills).platform(CliOnly),
        CommandDef::new("skills", "List installed skills", ToolsAndSkills).platform(CliOnly),
        CommandDef::new("cron", "Manage cron jobs", ToolsAndSkills)
            .args_hint("[subcommand]")
            .platform(CliOnly),
        CommandDef::new("reload-mcp", "Reload MCP servers", ToolsAndSkills)
            .aliases(&["reload_mcp"])
            .platform(Universal),
        CommandDef::new("reload", "Reload configuration", ToolsAndSkills).platform(Universal),
        CommandDef::new("browser", "Browser tool control", ToolsAndSkills)
            .args_hint("[connect|disconnect|status]")
            .platform(CliOnly),
        CommandDef::new("plugins", "List installed plugins", ToolsAndSkills).platform(CliOnly),

        // -----------------------------------------------------------------------
        // INFO
        // -----------------------------------------------------------------------
        CommandDef::new("commands", "List available commands", Info)
            .args_hint("[page]")
            .platform(GatewayOnly),
        CommandDef::new("help", "Show this help message", Info).platform(All),
        CommandDef::new("usage", "Show token usage", Info).platform(Universal),
        CommandDef::new("insights", "Show analytics", Info)
            .args_hint("[days]")
            .platform(Universal),
        CommandDef::new("platforms", "Show platform status", Info)
            .aliases(&["gateway"])
            .platform(CliOnly),
        CommandDef::new("paste", "Paste clipboard image", Info).platform(CliOnly),
        CommandDef::new("update", "Check for updates", Info).platform(GatewayOnly),
        CommandDef::new("snapshot", "Save a conversation snapshot", Info).platform(Universal),
        CommandDef::new("profile", "Show active profile and HERMES_HOME", Info)
            .platform(Universal),

        // -----------------------------------------------------------------------
        // EXIT
        // -----------------------------------------------------------------------
        CommandDef::new("quit", "Exit the CLI", Exit)
            .aliases(&["exit", "q"])
            .platform(CliOnly),
    ]
}
